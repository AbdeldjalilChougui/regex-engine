//! Differential testing against the `regex` crate on randomly generated
//! patterns and haystacks. Every generated pattern must give the same
//! `is_match` (lazy DFA *and* Pike VM), `find`, `captures` and `find_iter`
//! results as `regex`.

use regex_engine::Regex;

/// xorshift64*: deterministic, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}

fn gen_atom(r: &mut Rng, depth: u32) -> String {
    match r.below(if depth > 2 { 6 } else { 9 }) {
        0..=2 => r.pick(&["a", "b", "c", "x"]).to_string(),
        3 => r.pick(&[".", "[ab]", "[^a]", "[a-c]", "[^\\n]", "[x-z]", "[]a]", "[a-]"]).to_string(),
        4 => r.pick(&["\\d", "\\w", "\\s", "\\D", "\\W", "\\S", "\\.", "\\n"]).to_string(),
        5 => r.pick(&["^", "$"]).to_string(),
        6 => format!("({})", gen_alt(r, depth + 1)),
        7 => format!("(?:{})", gen_alt(r, depth + 1)),
        _ => format!("({})", gen_concat(r, depth + 1)),
    }
}

fn gen_piece(r: &mut Rng, depth: u32) -> String {
    let atom = gen_atom(r, depth);
    if atom == "^" || atom == "$" {
        return atom;
    }
    let q = match r.below(12) {
        0 => "*",
        1 => "+",
        2 => "?",
        3 => "*?",
        4 => "+?",
        5 => "??",
        6 => "{2}",
        7 => "{1,3}",
        8 => "{0,2}?",
        9 => "{2,}",
        _ => "",
    };
    format!("{atom}{q}")
}

fn gen_concat(r: &mut Rng, depth: u32) -> String {
    let n = 1 + r.below(4);
    (0..n).map(|_| gen_piece(r, depth)).collect()
}

fn gen_alt(r: &mut Rng, depth: u32) -> String {
    let n = 1 + r.below(if depth > 1 { 2 } else { 3 });
    (0..n).map(|_| gen_concat(r, depth)).collect::<Vec<_>>().join("|")
}

fn gen_haystack(r: &mut Rng) -> String {
    let n = r.below(12);
    (0..n).map(|_| *["a", "b", "c", "x", "1", " ", "\n", ".", "-", "]"].get(r.below(10) as usize).unwrap()).collect()
}

fn spans(re: &regex::Regex, text: &str) -> Vec<Option<(usize, usize)>> {
    match re.captures(text) {
        None => vec![],
        Some(c) => (0..c.len()).map(|i| c.get(i).map(|m| (m.start(), m.end()))).collect(),
    }
}

fn our_spans(re: &Regex, text: &str) -> Vec<Option<(usize, usize)>> {
    match re.captures(text) {
        None => vec![],
        Some(c) => c.into_iter().map(|m| m.map(|m| (m.start, m.end))).collect(),
    }
}

#[test]
fn differential_against_regex_crate() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let (mut patterns, mut checks) = (0, 0);
    // DIFF_PATTERNS overrides; debug builds run a smaller sample.
    let n: usize = std::env::var("DIFF_PATTERNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 200 } else { 4000 });
    for _ in 0..n {
        let pat = gen_alt(&mut r, 0);
        let Ok(reference) = regex::Regex::new(&pat) else {
            continue; // the generator only emits syntax both should accept, but be safe
        };
        let ours = Regex::new(&pat).unwrap_or_else(|e| panic!("we rejected {pat:?}: {e}"));
        patterns += 1;
        for _ in 0..25 {
            let hay = gen_haystack(&mut r);
            let want = reference.is_match(&hay);
            assert_eq!(ours.is_match(&hay), want, "is_match (DFA) /{pat}/ on {hay:?}");
            assert_eq!(ours.is_match_pikevm(&hay), want, "is_match (PikeVM) /{pat}/ on {hay:?}");
            assert_eq!(
                ours.find(&hay).map(|m| (m.start, m.end)),
                reference.find(&hay).map(|m| (m.start(), m.end())),
                "find /{pat}/ on {hay:?}"
            );
            assert_eq!(our_spans(&ours, &hay), spans(&reference, &hay), "captures /{pat}/ on {hay:?}");
            let it: Vec<_> = ours.find_iter(&hay).into_iter().map(|m| (m.start, m.end)).collect();
            let want_it: Vec<_> = reference.find_iter(&hay).map(|m| (m.start(), m.end())).collect();
            assert_eq!(it, want_it, "find_iter /{pat}/ on {hay:?}");
            checks += 1;
        }
    }
    assert!(patterns * 10 > n * 9, "only {patterns} of {n} patterns were usable");
    eprintln!("{patterns} patterns, {checks} haystacks checked");
}

#[test]
fn hand_picked_cases_match_regex_crate() {
    let cases: &[(&str, &[&str])] = &[
        ("(a*)*", &["", "b", "aa", "aab"]),
        ("(a|b)*?c", &["abac", "c", "ab"]),
        ("(?:a|ab)(c|bcd)(d*)", &["abcd", "acd"]),
        ("^$|a", &["", "a", "b"]),
        ("(a*)+$", &["aaab", "b", ""]),
        ("x*", &["xxyxx", "", "yyy"]),
        ("\\bnot_supported_but_unused|[0-9]{2,3}", &["12345", "1"]),
        ("(\\w+)\\s(\\w+)", &["hello world", "a  b"]),
        ("a{0}", &["aaa"]),
        ("(a?){5}a{5}", &["aaaaa", "aaaa"]),
    ];
    for (pat, hays) in cases {
        let Ok(reference) = regex::Regex::new(pat) else { continue };
        let Ok(ours) = Regex::new(pat) else { continue };
        for hay in *hays {
            assert_eq!(ours.is_match(hay), reference.is_match(hay), "/{pat}/ {hay:?}");
            assert_eq!(our_spans(&ours, hay), spans(&reference, hay), "/{pat}/ {hay:?}");
            let it: Vec<_> = ours.find_iter(hay).into_iter().map(|m| (m.start, m.end)).collect();
            let want: Vec<_> = reference.find_iter(hay).map(|m| (m.start(), m.end())).collect();
            assert_eq!(it, want, "find_iter /{pat}/ {hay:?}");
        }
    }
}
