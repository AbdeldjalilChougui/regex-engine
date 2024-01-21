//! A regex engine built from scratch:
//!
//! pattern ─[`parser`]→ AST ─[`nfa`]→ Thompson NFA ─┬─[`dfa`]→ lazy DFA   (`is_match`)
//!                                                  └─[`pikevm`]→ Pike VM (`find`, `captures`)
//!
//! Match semantics are leftmost-first, like Perl and the `regex` crate.
//! Offsets are byte offsets into the haystack.

pub mod dfa;
pub mod nfa;
pub mod parser;
pub mod pikevm;

use std::cell::RefCell;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Parse(parser::Error),
    TooBig,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => e.fmt(f),
            Error::TooBig => f.write_str("compiled regex exceeds the size limit"),
        }
    }
}

impl std::error::Error for Error {}

/// A compiled regular expression. Not `Sync`: the lazy DFA cache lives in a `RefCell`.
pub struct Regex {
    pattern: String,
    nfa: nfa::Nfa,
    dfa: RefCell<dfa::LazyDfa>,
}

/// Byte span of a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

impl Match {
    pub fn as_str<'t>(&self, text: &'t str) -> &'t str {
        &text[self.start..self.end]
    }
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Regex, Error> {
        let parsed = parser::parse(pattern).map_err(Error::Parse)?;
        let nfa = nfa::Nfa::compile(&parsed.ast, parsed.groups).map_err(|_| Error::TooBig)?;
        let dfa = RefCell::new(dfa::LazyDfa::new(&nfa));
        Ok(Regex { pattern: pattern.to_string(), nfa, dfa })
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// Number of capture groups, including group 0 (the whole match).
    pub fn captures_len(&self) -> usize {
        self.nfa.groups
    }

    pub fn nfa(&self) -> &nfa::Nfa {
        &self.nfa
    }

    /// Fast yes/no answer using the lazy DFA.
    pub fn is_match(&self, text: &str) -> bool {
        self.dfa.borrow_mut().is_match(&self.nfa, text)
    }

    /// Same answer computed with the Pike VM (slower; used for cross-checking).
    pub fn is_match_pikevm(&self, text: &str) -> bool {
        pikevm::search(&self.nfa, text, 0).is_some()
    }

    fn find_at(&self, text: &str, start: usize) -> Option<Match> {
        let slots = pikevm::search(&self.nfa, text, start)?;
        Some(Match { start: slots[0]?, end: slots[1]? })
    }

    /// Leftmost-first match.
    pub fn find(&self, text: &str) -> Option<Match> {
        if !self.is_match(text) {
            return None; // DFA as a prefilter
        }
        self.find_at(text, 0)
    }

    /// Capture group spans of the leftmost-first match (`None` for groups that didn't participate).
    pub fn captures(&self, text: &str) -> Option<Vec<Option<Match>>> {
        if !self.is_match(text) {
            return None;
        }
        let slots = pikevm::search(&self.nfa, text, 0)?;
        Some(
            slots
                .chunks(2)
                .map(|p| match (p[0], p[1]) {
                    (Some(s), Some(e)) => Some(Match { start: s, end: e }),
                    _ => None,
                })
                .collect(),
        )
    }

    /// All successive non-overlapping matches, with the same empty-match
    /// rule as the `regex` crate: an empty match right at the end of the
    /// previous match is skipped.
    pub fn find_iter(&self, text: &str) -> Vec<Match> {
        let mut out = Vec::new();
        if !self.is_match(text) {
            return out;
        }
        let mut pos = 0;
        let mut last_end = None;
        while pos <= text.len() {
            let Some(mut m) = self.find_at(text, pos) else { break };
            if m.start == m.end && Some(m.end) == last_end {
                let Some(c) = text[pos..].chars().next() else { break };
                match self.find_at(text, pos + c.len_utf8()) {
                    Some(m2) => m = m2,
                    None => break,
                }
            }
            out.push(m);
            pos = m.end;
            last_end = Some(m.end);
        }
        out
    }

    /// How many states the lazy DFA has built so far.
    pub fn dfa_cached_states(&self) -> usize {
        self.dfa.borrow().cached_states()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(re: &str, text: &str) -> Option<(usize, usize)> {
        Regex::new(re).unwrap().find(text).map(|m| (m.start, m.end))
    }

    #[test]
    fn literals_and_classes() {
        assert_eq!(m("abc", "xxabcxx"), Some((2, 5)));
        assert_eq!(m("[0-9]+", "ab 123 cd"), Some((3, 6)));
        assert_eq!(m(r"\d+\.\d+", "pi is 3.14!"), Some((6, 10)));
        assert_eq!(m("[^a-z]", "abcD"), Some((3, 4)));
        assert_eq!(m("a.c", "a\nc abc"), Some((4, 7)));
    }

    #[test]
    fn anchors() {
        assert_eq!(m("^ab", "ab ab"), Some((0, 2)));
        assert_eq!(m("^ab", "xab"), None);
        assert_eq!(m("ab$", "ab ab"), Some((3, 5)));
        assert_eq!(m("^$", ""), Some((0, 0)));
        assert_eq!(m("$", "abc"), Some((3, 3)));
    }

    #[test]
    fn leftmost_first_semantics() {
        assert_eq!(m("a|ab", "ab"), Some((0, 1)));
        assert_eq!(m("ab|a", "ab"), Some((0, 2)));
        assert_eq!(m("a+?", "aaa"), Some((0, 1)));
        assert_eq!(m("a{2,3}", "aaaa"), Some((0, 3)));
        assert_eq!(m("a{2,3}?", "aaaa"), Some((0, 2)));
    }

    #[test]
    fn captures_work() {
        let re = Regex::new(r"(\w+)@(\w+)\.(com|org)(x)?").unwrap();
        let caps = re.captures("mail bob@example.org now").unwrap();
        let text = "mail bob@example.org now";
        assert_eq!(caps[1].unwrap().as_str(text), "bob");
        assert_eq!(caps[2].unwrap().as_str(text), "example");
        assert_eq!(caps[3].unwrap().as_str(text), "org");
        assert_eq!(caps[4], None);
    }

    #[test]
    fn unicode_haystack() {
        assert_eq!(m("é+", "caféé!"), Some((3, 7)));
        assert_eq!(m(".$", "日本"), Some((3, 6)));
    }

    #[test]
    fn pathological_pattern_is_linear() {
        // (a?){n}a{n} against a^n: exponential for backtrackers.
        let n = 25;
        let re = Regex::new(&format!("{}{}", "a?".repeat(n), "a".repeat(n))).unwrap();
        let text = "a".repeat(n);
        assert!(re.is_match(&text));
        assert_eq!(re.find(&text).map(|m| m.end), Some(n));
    }

    #[test]
    fn dfa_cache_is_bounded() {
        let re = Regex::new("[ab]*a[ab]{12}").unwrap();
        let mut x: u32 = 12345;
        let text: String = (0..200_000)
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                if (x >> 16) & 1 == 0 { 'a' } else { 'b' }
            })
            .collect();
        assert!(re.is_match(&text));
        assert!(re.dfa_cached_states() <= dfa::MAX_CACHED_STATES);
    }
}
