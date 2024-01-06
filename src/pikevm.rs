//! Pike VM: NFA simulation that tracks capture positions per thread and
//! gives leftmost-first (Perl/`regex` crate) match semantics.

use crate::nfa::{Look, Nfa, State, StateId};

pub type Slots = Vec<Option<usize>>;

struct Thread {
    state: StateId,
    slots: Slots,
}

/// Sparse set membership by generation counter, reset in O(1).
struct Visited {
    gen: Vec<u32>,
    cur: u32,
}

impl Visited {
    fn new(n: usize) -> Visited {
        Visited { gen: vec![0; n], cur: 1 }
    }

    fn clear(&mut self) {
        self.cur += 1;
    }

    /// Returns true if newly inserted.
    fn insert(&mut self, s: StateId) -> bool {
        if self.gen[s] == self.cur {
            false
        } else {
            self.gen[s] = self.cur;
            true
        }
    }
}

/// Follow epsilon edges from `start` in priority order, appending the
/// resulting char-consuming / match threads to `list`.
fn add_thread(
    nfa: &Nfa,
    list: &mut Vec<Thread>,
    visited: &mut Visited,
    start: StateId,
    slots: Slots,
    pos: usize,
    text_len: usize,
) {
    let mut stack = vec![(start, slots)];
    while let Some((s, mut slots)) = stack.pop() {
        if !visited.insert(s) {
            continue;
        }
        match &nfa.states[s] {
            State::Goto(n) => stack.push((*n, slots)),
            State::Split { first, second } => {
                stack.push((*second, slots.clone()));
                stack.push((*first, slots));
            }
            State::Save { slot, next } => {
                slots[*slot] = Some(pos);
                stack.push((*next, slots));
            }
            State::Look { look, next } => {
                let ok = match look {
                    Look::StartText => pos == 0,
                    Look::EndText => pos == text_len,
                };
                if ok {
                    stack.push((*next, slots));
                }
            }
            State::Class { .. } | State::Match => list.push(Thread { state: s, slots }),
        }
    }
}

/// Search `text` starting at byte offset `start`. Returns the capture slots
/// (byte offsets, `2 * groups` entries) of the leftmost-first match.
pub fn search(nfa: &Nfa, text: &str, start: usize) -> Option<Slots> {
    let nslots = nfa.groups * 2;
    let mut clist: Vec<Thread> = Vec::new();
    let mut nlist: Vec<Thread> = Vec::new();
    let mut visited = Visited::new(nfa.states.len());
    let mut matched: Option<Slots> = None;
    let mut pos = start;
    loop {
        let c = text[pos..].chars().next();
        if matched.is_none() {
            // Lowest priority: a new attempt starting here.
            add_thread(nfa, &mut clist, &mut visited, nfa.start, vec![None; nslots], pos, text.len());
        }
        if clist.is_empty() && matched.is_some() {
            break;
        }
        visited.clear();
        let next_pos = pos + c.map_or(0, char::len_utf8);
        for t in clist.drain(..) {
            match &nfa.states[t.state] {
                State::Match => {
                    matched = Some(t.slots);
                    // Lower-priority threads can't win anymore.
                    break;
                }
                State::Class { ranges, next } => {
                    if let Some(c) = c {
                        if State::class_matches(ranges, c) {
                            add_thread(nfa, &mut nlist, &mut visited, *next, t.slots, next_pos, text.len());
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        std::mem::swap(&mut clist, &mut nlist);
        if c.is_none() {
            break;
        }
        pos = next_pos;
        if matched.is_some() && clist.is_empty() {
            break;
        }
    }
    matched
}
