//! Lazy DFA: subset construction done on the fly while searching, with
//! transitions cached so each (state, char) pair is computed at most once.
//!
//! A DFA state is the set of NFA states reachable after the input so far
//! (only char-consuming states, `Match`, and pending `$` assertions are kept).
//! The search is unanchored: the NFA start is re-seeded at every position.
//! Used for `is_match`, which needs no capture positions.

use crate::nfa::{Look, Nfa, State, StateId};
use std::collections::HashMap;

const UNKNOWN: u32 = u32::MAX;
/// Clear the cache when it grows past this many states (bounded memory).
pub const MAX_CACHED_STATES: usize = 4096;

struct DState {
    nfa_states: Box<[StateId]>,
    is_match: bool,
    ascii: Box<[u32; 128]>,
    other: HashMap<char, u32>,
}

pub struct LazyDfa {
    states: Vec<DState>,
    index: HashMap<Box<[StateId]>, u32>,
    start: Option<u32>,
    /// Number of cache flushes so far (exposed for tests/benchmarks).
    pub flushes: usize,
    stack: Vec<StateId>,
    seen: Vec<bool>,
    touched: Vec<StateId>,
}

impl LazyDfa {
    pub fn new(nfa: &Nfa) -> LazyDfa {
        LazyDfa {
            states: Vec::new(),
            index: HashMap::new(),
            start: None,
            flushes: 0,
            stack: Vec::new(),
            seen: vec![false; nfa.states.len()],
            touched: Vec::new(),
        }
    }

    pub fn cached_states(&self) -> usize {
        self.states.len()
    }

    /// Epsilon closure of `seeds`. `at_start`/`at_end` decide whether `^`/`$` pass.
    fn closure(&mut self, nfa: &Nfa, seeds: &[StateId], at_start: bool, at_end: bool) -> Vec<StateId> {
        let mut out = Vec::new();
        self.stack.clear();
        self.stack.extend(seeds.iter().rev());
        while let Some(s) = self.stack.pop() {
            if std::mem::replace(&mut self.seen[s], true) {
                continue;
            }
            self.touched.push(s);
            match &nfa.states[s] {
                State::Goto(n) | State::Save { next: n, .. } => self.stack.push(*n),
                State::Split { first, second } => {
                    self.stack.push(*second);
                    self.stack.push(*first);
                }
                State::Look { look: Look::StartText, next } => {
                    if at_start {
                        self.stack.push(*next);
                    }
                }
                State::Look { look: Look::EndText, next } => {
                    if at_end {
                        self.stack.push(*next);
                    } else {
                        out.push(s); // pending until we know we're at the end
                    }
                }
                State::Class { .. } | State::Match => out.push(s),
            }
        }
        for s in self.touched.drain(..) {
            self.seen[s] = false;
        }
        out.sort_unstable();
        out
    }

    fn intern(&mut self, nfa: &Nfa, set: Vec<StateId>) -> u32 {
        if let Some(&id) = self.index.get(set.as_slice()) {
            return id;
        }
        let is_match = set.iter().any(|&s| nfa.states[s] == State::Match);
        let key: Box<[StateId]> = set.into_boxed_slice();
        let id = self.states.len() as u32;
        self.states.push(DState { nfa_states: key.clone(), is_match, ascii: Box::new([UNKNOWN; 128]), other: HashMap::new() });
        self.index.insert(key, id);
        id
    }

    fn flush(&mut self) {
        self.states.clear();
        self.index.clear();
        self.start = None;
        self.flushes += 1;
    }

    fn start_state(&mut self, nfa: &Nfa) -> u32 {
        if let Some(s) = self.start {
            return s;
        }
        let set = self.closure(nfa, &[nfa.start], true, false);
        let s = self.intern(nfa, set);
        self.start = Some(s);
        s
    }

    fn next(&mut self, nfa: &Nfa, from: u32, c: char) -> u32 {
        let cached = {
            let st = &self.states[from as usize];
            if (c as u32) < 128 {
                st.ascii[c as usize]
            } else {
                st.other.get(&c).copied().unwrap_or(UNKNOWN)
            }
        };
        if cached != UNKNOWN {
            return cached;
        }
        let mut seeds: Vec<StateId> = self.states[from as usize]
            .nfa_states
            .iter()
            .filter_map(|&s| match &nfa.states[s] {
                State::Class { ranges, next } if State::class_matches(ranges, c) => Some(*next),
                _ => None,
            })
            .collect();
        seeds.push(nfa.start); // unanchored: a match may also start after this char
        let set = self.closure(nfa, &seeds, false, false);
        let mut from = from;
        if self.states.len() >= MAX_CACHED_STATES {
            // Keep memory bounded: drop everything and re-create the source state.
            let src = self.states[from as usize].nfa_states.to_vec();
            self.flush();
            from = self.intern(nfa, src);
        }
        let to = self.intern(nfa, set);
        let st = &mut self.states[from as usize];
        if (c as u32) < 128 {
            st.ascii[c as usize] = to;
        } else {
            st.other.insert(c, to);
        }
        to
    }

    /// Does `nfa` match anywhere in `text`?
    pub fn is_match(&mut self, nfa: &Nfa, text: &str) -> bool {
        let mut s = self.start_state(nfa);
        if text.is_empty() {
            let set = self.states[s as usize].nfa_states.to_vec();
            let fin = self.closure(nfa, &set, true, true);
            return fin.iter().any(|&x| nfa.states[x] == State::Match);
        }
        for c in text.chars() {
            if self.states[s as usize].is_match {
                return true;
            }
            s = self.next(nfa, s, c);
        }
        if self.states[s as usize].is_match {
            return true;
        }
        // End of input: let pending `$` assertions through.
        let set = self.states[s as usize].nfa_states.to_vec();
        let fin = self.closure(nfa, &set, false, true);
        fin.iter().any(|&x| nfa.states[x] == State::Match)
    }
}
