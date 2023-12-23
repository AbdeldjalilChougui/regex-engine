//! Regex syntax → AST.
//!
//! Supported: literals, `.`, classes `[a-z]`, `[^...]`, escapes `\d \w \s \D \W \S`
//! (ASCII definitions) and `\n \t \r \f \v`, escaped metacharacters, anchors `^ $`,
//! capturing `( )` and non-capturing `(?: )` groups, alternation `|`, and the
//! quantifiers `* + ? {n} {n,} {n,m}`, each with a lazy `?` variant.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ast {
    Empty,
    /// A set of inclusive char ranges (literals, `.`, classes and escapes all become this).
    Class(Vec<(char, char)>),
    StartText,
    EndText,
    Group { index: Option<usize>, inner: Box<Ast> },
    Concat(Vec<Ast>),
    Alt(Vec<Ast>),
    Repeat { inner: Box<Ast>, min: u32, max: Option<u32>, greedy: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "regex parse error at {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for Error {}

pub const MAX_REPEAT: u32 = 1000;

/// Result of parsing: the AST and the number of capture groups (group 0 = whole match).
pub struct Parsed {
    pub ast: Ast,
    pub groups: usize,
}

pub fn parse(pattern: &str) -> Result<Parsed, Error> {
    let mut p = Parser { chars: pattern.chars().collect(), pos: 0, groups: 1, depth: 0 };
    let ast = p.alternation()?;
    if p.pos < p.chars.len() {
        return Err(p.err("unmatched ')'"));
    }
    Ok(Parsed { ast, groups: p.groups })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
    depth: usize,
}

const MAX_CHAR: char = char::MAX;

pub fn ascii_digit() -> Vec<(char, char)> {
    vec![('0', '9')]
}

pub fn ascii_word() -> Vec<(char, char)> {
    vec![('0', '9'), ('A', 'Z'), ('_', '_'), ('a', 'z')]
}

pub fn ascii_space() -> Vec<(char, char)> {
    vec![('\t', '\r'), (' ', ' ')]
}

fn prev_char(c: char) -> char {
    match c {
        '\u{E000}' => '\u{D7FF}',
        _ => char::from_u32(c as u32 - 1).unwrap(),
    }
}

fn next_char(c: char) -> char {
    match c {
        '\u{D7FF}' => '\u{E000}',
        _ => char::from_u32(c as u32 + 1).unwrap(),
    }
}

/// Sort and merge overlapping/adjacent ranges.
pub fn canonicalize(mut r: Vec<(char, char)>) -> Vec<(char, char)> {
    r.sort();
    let mut out: Vec<(char, char)> = Vec::with_capacity(r.len());
    for (lo, hi) in r {
        if let Some(last) = out.last_mut() {
            if last.1 == MAX_CHAR || lo <= next_char(last.1) {
                if hi > last.1 {
                    last.1 = hi;
                }
                continue;
            }
        }
        out.push((lo, hi));
    }
    out
}

pub fn negate(r: &[(char, char)]) -> Vec<(char, char)> {
    let r = canonicalize(r.to_vec());
    let mut out = Vec::new();
    let mut next = '\0';
    let mut done = false;
    for &(lo, hi) in &r {
        if lo > next {
            out.push((next, prev_char(lo)));
        }
        if hi == MAX_CHAR {
            done = true;
            break;
        }
        next = next_char(hi);
    }
    if !done {
        out.push((next, MAX_CHAR));
    }
    out
}

impl Parser {
    fn err(&self, msg: &str) -> Error {
        Error { pos: self.pos, msg: msg.into() }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn alternation(&mut self) -> Result<Ast, Error> {
        let mut alts = vec![self.concat()?];
        while self.eat('|') {
            alts.push(self.concat()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Ast::Alt(alts) })
    }

    fn concat(&mut self) -> Result<Ast, Error> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            let atom = self.quantifiers(atom)?;
            items.push(atom);
        }
        Ok(match items.len() {
            0 => Ast::Empty,
            1 => items.pop().unwrap(),
            _ => Ast::Concat(items),
        })
    }

    fn quantifiers(&mut self, mut atom: Ast) -> Result<Ast, Error> {
        loop {
            let start = self.pos;
            let (min, max) = match self.peek() {
                Some('*') => (0, None),
                Some('+') => (1, None),
                Some('?') => (0, Some(1)),
                Some('{') => match self.counted()? {
                    Some(mm) => mm,
                    None => return Ok(atom),
                },
                _ => return Ok(atom),
            };
            if self.pos == start {
                self.pos += 1;
            }
            if matches!(atom, Ast::Repeat { .. }) {
                return Err(Error { pos: start, msg: "nested repetition operator".into() });
            }
            if matches!(atom, Ast::Empty) {
                return Err(Error { pos: start, msg: "repetition operator missing expression".into() });
            }
            let greedy = !self.eat('?');
            atom = Ast::Repeat { inner: Box::new(atom), min, max, greedy };
        }
    }

    /// Parse `{n}`, `{n,}` or `{n,m}` at the cursor (consuming it).
    fn counted(&mut self) -> Result<Option<(u32, Option<u32>)>, Error> {
        let start = self.pos;
        self.pos += 1; // '{'
        let num = |p: &mut Parser| -> Result<Option<u32>, Error> {
            let s = p.pos;
            while p.peek().is_some_and(|c| c.is_ascii_digit()) {
                p.pos += 1;
            }
            if s == p.pos {
                return Ok(None);
            }
            let text: String = p.chars[s..p.pos].iter().collect();
            text.parse::<u32>().ok().filter(|&n| n <= MAX_REPEAT).map(Some).ok_or_else(|| p.err("repetition count too large"))
        };
        let Some(min) = num(self)? else {
            self.pos = start;
            return Err(self.err("invalid repetition: expected a number after '{'"));
        };
        let max = if self.eat(',') { num(self)? } else { Some(min) };
        if !self.eat('}') {
            return Err(self.err("unclosed counted repetition"));
        }
        if let Some(m) = max {
            if m < min {
                return Err(self.err("invalid repetition range (max < min)"));
            }
        }
        Ok(Some((min, max)))
    }

    fn atom(&mut self) -> Result<Ast, Error> {
        let c = self.peek().unwrap();
        self.pos += 1;
        Ok(match c {
            '(' => {
                self.depth += 1;
                if self.depth > 200 {
                    return Err(self.err("nesting too deep"));
                }
                let index = if self.eat('?') {
                    if !self.eat(':') {
                        return Err(self.err("unsupported group flag (only (?:...) is supported)"));
                    }
                    None
                } else {
                    self.groups += 1;
                    Some(self.groups - 1)
                };
                let inner = self.alternation()?;
                if !self.eat(')') {
                    return Err(self.err("unclosed group"));
                }
                self.depth -= 1;
                Ast::Group { index, inner: Box::new(inner) }
            }
            '[' => Ast::Class(self.class()?),
            '.' => Ast::Class(negate(&[('\n', '\n')])),
            '^' => Ast::StartText,
            '$' => Ast::EndText,
            '\\' => Ast::Class(self.escape()?),
            '*' | '+' | '?' => {
                self.pos -= 1;
                return Err(self.err("repetition operator missing expression"));
            }
            '{' => {
                self.pos -= 1;
                return Err(self.err("repetition operator missing expression"));
            }
            c => Ast::Class(vec![(c, c)]),
        })
    }

    /// After a backslash. Returns the char set it denotes.
    fn escape(&mut self) -> Result<Vec<(char, char)>, Error> {
        let Some(c) = self.peek() else {
            return Err(self.err("trailing backslash"));
        };
        self.pos += 1;
        let one = |c: char| vec![(c, c)];
        Ok(match c {
            'd' => ascii_digit(),
            'D' => negate(&ascii_digit()),
            'w' => ascii_word(),
            'W' => negate(&ascii_word()),
            's' => ascii_space(),
            'S' => negate(&ascii_space()),
            'n' => one('\n'),
            't' => one('\t'),
            'r' => one('\r'),
            'f' => one('\x0C'),
            'v' => one('\x0B'),
            c if c.is_ascii_punctuation() => one(c),
            _ => {
                self.pos -= 1;
                return Err(self.err("unsupported escape"));
            }
        })
    }

    fn class(&mut self) -> Result<Vec<(char, char)>, Error> {
        let negated = self.eat('^');
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err("unclosed character class"));
            };
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;
            if c == '[' {
                return Err(self.err("nested classes are not supported"));
            }
            let lo = self.class_atom()?;
            let lo = match lo {
                ClassAtom::Set(set) => {
                    ranges.extend(set);
                    continue;
                }
                ClassAtom::Char(c) => c,
            };
            // A range `a-z`, unless '-' is the last char before ']'.
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&n| n != ']') {
                self.pos += 1;
                let hi = match self.class_atom()? {
                    ClassAtom::Char(c) => c,
                    ClassAtom::Set(_) => return Err(self.err("invalid range end")),
                };
                if hi < lo {
                    return Err(self.err("invalid class range (end < start)"));
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        let ranges = canonicalize(ranges);
        Ok(if negated { negate(&ranges) } else { ranges })
    }

    fn class_atom(&mut self) -> Result<ClassAtom, Error> {
        let c = self.peek().unwrap();
        self.pos += 1;
        if c != '\\' {
            return Ok(ClassAtom::Char(c));
        }
        let set = self.escape()?;
        Ok(if set.len() == 1 && set[0].0 == set[0].1 { ClassAtom::Char(set[0].0) } else { ClassAtom::Set(set) })
    }
}

enum ClassAtom {
    Char(char),
    Set(Vec<(char, char)>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_structure() {
        let p = parse("a(b|c)*?(?:d)$").unwrap();
        assert_eq!(p.groups, 2);
        let Ast::Concat(items) = p.ast else { panic!() };
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[1], Ast::Repeat { min: 0, max: None, greedy: false, .. }));
        assert!(matches!(&items[2], Ast::Group { index: None, .. }));
        assert_eq!(items[3], Ast::EndText);
    }

    #[test]
    fn classes() {
        assert_eq!(parse("[a-cx]").unwrap().ast, Ast::Class(vec![('a', 'c'), ('x', 'x')]));
        assert_eq!(parse("[]a]").unwrap().ast, Ast::Class(vec![(']', ']'), ('a', 'a')]));
        assert_eq!(parse("[a-]").unwrap().ast, Ast::Class(vec![('-', '-'), ('a', 'a')]));
        let Ast::Class(neg) = parse("[^b]").unwrap().ast else { panic!() };
        assert_eq!(neg[0], ('\0', 'a'));
        assert_eq!(neg[1].0, 'c');
    }

    #[test]
    fn negate_roundtrip() {
        let r = vec![('a', 'f'), ('x', 'z'), ('\u{D7FF}', '\u{E001}')];
        assert_eq!(negate(&negate(&r)), canonicalize(r));
        assert_eq!(negate(&[('\0', char::MAX)]), vec![]);
    }

    #[test]
    fn counted_repetition() {
        assert!(matches!(parse("a{2,5}").unwrap().ast, Ast::Repeat { min: 2, max: Some(5), .. }));
        assert!(matches!(parse("a{3,}").unwrap().ast, Ast::Repeat { min: 3, max: None, .. }));
        assert!(matches!(parse("a{4}?").unwrap().ast, Ast::Repeat { min: 4, max: Some(4), greedy: false, .. }));
    }

    #[test]
    fn errors() {
        for bad in ["(", ")", "a**", "*a", "[a", "a{2,1}", "a{x}", "\\", "[z-a]", "(?i)a", "\\q", "a{5000}"] {
            assert!(parse(bad).is_err(), "{bad:?} should fail");
        }
    }
}
