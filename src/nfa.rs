//! Thompson construction: AST → NFA.

use crate::parser::Ast;
use std::fmt;

pub type StateId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    StartText,
    EndText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Consume one char in one of the ranges.
    Class { ranges: Vec<(char, char)>, next: StateId },
    /// Epsilon fork; `first` has priority (greedy vs lazy is just the order).
    Split { first: StateId, second: StateId },
    /// Epsilon edge.
    Goto(StateId),
    /// Record the current position in capture slot `slot`.
    Save { slot: usize, next: StateId },
    /// Zero-width assertion.
    Look { look: Look, next: StateId },
    Match,
}

#[derive(Debug, Clone)]
pub struct Nfa {
    pub states: Vec<State>,
    pub start: StateId,
    /// Number of capture groups including group 0.
    pub groups: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooBig;

impl fmt::Display for TooBig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("compiled regex exceeds the size limit")
    }
}

pub const MAX_STATES: usize = 100_000;

impl State {
    pub fn class_matches(ranges: &[(char, char)], c: char) -> bool {
        // Ranges are sorted and disjoint: binary search.
        ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }
}

/// Can this expression match the empty string?
pub fn can_be_empty(ast: &Ast) -> bool {
    match ast {
        Ast::Empty | Ast::StartText | Ast::EndText => true,
        Ast::Class(_) => false,
        Ast::Group { inner, .. } => can_be_empty(inner),
        Ast::Concat(items) => items.iter().all(can_be_empty),
        Ast::Alt(alts) => alts.iter().any(can_be_empty),
        Ast::Repeat { inner, min, .. } => *min == 0 || can_be_empty(inner),
    }
}

struct Builder {
    states: Vec<State>,
}

const HOLE: StateId = usize::MAX;

impl Builder {
    fn push(&mut self, s: State) -> Result<StateId, TooBig> {
        if self.states.len() >= MAX_STATES {
            return Err(TooBig);
        }
        self.states.push(s);
        Ok(self.states.len() - 1)
    }

    /// Point every dangling `HOLE` edge of the fragment's exits at `target`.
    fn patch(&mut self, exits: &[StateId], target: StateId) {
        for &e in exits {
            match &mut self.states[e] {
                State::Class { next, .. } | State::Save { next, .. } | State::Look { next, .. } | State::Goto(next) => {
                    *next = target
                }
                State::Split { first, second } => {
                    if *first == HOLE {
                        *first = target;
                    } else {
                        *second = target;
                    }
                }
                State::Match => {}
            }
        }
    }

    /// Compile `ast`; returns `(entry, exits)` where exits are states with a HOLE edge.
    fn compile(&mut self, ast: &Ast) -> Result<(StateId, Vec<StateId>), TooBig> {
        match ast {
            Ast::Empty => {
                let s = self.push(State::Goto(HOLE))?;
                Ok((s, vec![s]))
            }
            Ast::Class(ranges) => {
                let s = self.push(State::Class { ranges: ranges.clone(), next: HOLE })?;
                Ok((s, vec![s]))
            }
            Ast::StartText | Ast::EndText => {
                let look = if *ast == Ast::StartText { Look::StartText } else { Look::EndText };
                let s = self.push(State::Look { look, next: HOLE })?;
                Ok((s, vec![s]))
            }
            Ast::Group { index: None, inner } => self.compile(inner),
            Ast::Group { index: Some(i), inner } => {
                let open = self.push(State::Save { slot: 2 * i, next: HOLE })?;
                let (entry, exits) = self.compile(inner)?;
                self.patch(&[open], entry);
                let close = self.push(State::Save { slot: 2 * i + 1, next: HOLE })?;
                self.patch(&exits, close);
                Ok((open, vec![close]))
            }
            Ast::Concat(items) => {
                let (entry, mut exits) = self.compile(&items[0])?;
                for item in &items[1..] {
                    let (e, x) = self.compile(item)?;
                    self.patch(&exits, e);
                    exits = x;
                }
                Ok((entry, exits))
            }
            Ast::Alt(alts) => {
                // Chain of splits: alt0 | (alt1 | (alt2 ...)), preserving priority order.
                let mut exits = Vec::new();
                let (mut entry, x) = self.compile(alts.last().unwrap())?;
                exits.extend(x);
                for alt in alts[..alts.len() - 1].iter().rev() {
                    let (e, x) = self.compile(alt)?;
                    exits.extend(x);
                    entry = self.push(State::Split { first: e, second: entry })?;
                }
                Ok((entry, exits))
            }
            Ast::Repeat { inner, min, max, greedy } => self.repeat(inner, *min, *max, *greedy),
        }
    }

    fn split(&mut self, greedy: bool, body: StateId) -> Result<StateId, TooBig> {
        // Returns a split whose "continue" edge is a HOLE.
        if greedy {
            self.push(State::Split { first: body, second: HOLE })
        } else {
            self.push(State::Split { first: HOLE, second: body })
        }
    }

    fn repeat(&mut self, inner: &Ast, min: u32, max: Option<u32>, greedy: bool) -> Result<(StateId, Vec<StateId>), TooBig> {
        // Mandatory copies (for an unbounded repeat, the last one becomes `e+`).
        let mandatory = match max {
            None => min.saturating_sub(1),
            Some(_) => min,
        };
        let mut entry = None;
        let mut exits: Vec<StateId> = Vec::new();
        let chain = |b: &mut Builder, e: StateId, x: Vec<StateId>, entry: &mut Option<StateId>, exits: &mut Vec<StateId>| {
            match entry {
                None => *entry = Some(e),
                Some(_) => b.patch(exits, e),
            }
            *exits = x;
        };
        for _ in 0..mandatory {
            let (e, x) = self.compile(inner)?;
            chain(self, e, x, &mut entry, &mut exits);
        }
        match max {
            None => {
                // `e+`: body, then a split that loops back to the body or leaves.
                // `e*` is compiled as `(e+)?` (as the regex crate does), so an
                // empty-matching body still gets one iteration that sets captures.
                let (e, x) = self.compile(inner)?;
                if min == 0 && !can_be_empty(inner) {
                    // Simple loop: one split that is both entry and exit.
                    let l = self.split(greedy, e)?;
                    self.patch(&x, l);
                    chain(self, l, vec![l], &mut entry, &mut exits);
                    return Ok((entry.unwrap(), exits));
                }
                let back = self.split(greedy, e)?;
                self.patch(&x, back);
                if min == 0 {
                    let opt = self.split(greedy, e)?;
                    chain(self, opt, vec![opt, back], &mut entry, &mut exits);
                } else {
                    chain(self, e, vec![back], &mut entry, &mut exits);
                }
            }
            Some(max) => {
                // Optional copies: each is split(body, out), nested.
                let mut outs = Vec::new();
                for _ in min..max {
                    let (e, x) = self.compile(inner)?;
                    let s = self.split(greedy, e)?;
                    chain(self, s, x, &mut entry, &mut exits);
                    outs.push(s);
                }
                exits.extend(outs);
            }
        }
        match entry {
            Some(e) => Ok((e, exits)),
            None => {
                // {0} or {0,0}: matches empty.
                let s = self.push(State::Goto(HOLE))?;
                Ok((s, vec![s]))
            }
        }
    }
}

impl Nfa {
    pub fn compile(ast: &Ast, groups: usize) -> Result<Nfa, TooBig> {
        let mut b = Builder { states: Vec::new() };
        let open = b.push(State::Save { slot: 0, next: HOLE })?;
        let (entry, exits) = b.compile(ast)?;
        b.patch(&[open], entry);
        let close = b.push(State::Save { slot: 1, next: HOLE })?;
        b.patch(&exits, close);
        let m = b.push(State::Match)?;
        b.patch(&[close], m);
        Ok(Nfa { states: b.states, start: open, groups })
    }
}
