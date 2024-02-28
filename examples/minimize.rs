//! Debugging aid: shrink a (pattern, haystack) pair while our captures still
//! differ from the `regex` crate's.
//!
//! cargo run --release --example minimize -- '<pattern>' '<haystack>'
use regex_engine::Regex;
fn differs(p: &str, h: &str) -> bool {
    let (Ok(a), Ok(b)) = (Regex::new(p), regex::Regex::new(p)) else { return false };
    let ours: Vec<_> = a.captures(h).map(|c| c.into_iter().map(|m| m.map(|m| (m.start, m.end))).collect()).unwrap_or_default();
    let theirs: Vec<_> = b.captures(h).map(|c| (0..c.len()).map(|i| c.get(i).map(|m| (m.start(), m.end()))).collect()).unwrap_or_default();
    ours != theirs
}
fn main() {
    let mut p: Vec<char> = std::env::args().nth(1).unwrap().chars().collect();
    let mut h: Vec<char> = std::env::args().nth(2).unwrap().chars().collect();
    let s = |v: &Vec<char>| v.iter().collect::<String>();
    assert!(differs(&s(&p), &s(&h)));
    loop {
        let mut changed = false;
        for len in (1..=p.len()).rev() {
            let mut i = 0;
            while i + len <= p.len() {
                let mut q = p.clone();
                q.drain(i..i + len);
                if differs(&s(&q), &s(&h)) { p = q; changed = true; } else { i += 1; }
            }
        }
        for i in (0..h.len()).rev() {
            let mut g = h.clone(); g.remove(i);
            if differs(&s(&p), &s(&g)) { h = g; changed = true; }
        }
        if !changed { break; }
    }
    println!("{:?} on {:?}", s(&p), s(&h));
    let a = Regex::new(&s(&p)).unwrap();
    println!("ours  {:?}", a.captures(&s(&h)));
    let b = regex::Regex::new(&s(&p)).unwrap();
    println!("regex {:?}", b.captures(&s(&h)));
}
