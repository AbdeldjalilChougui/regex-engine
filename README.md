# regex-engine

A regular expression engine written from scratch in Rust, following Russ Cox's
[articles](https://swtch.com/~rsc/regexp/): parser → Thompson NFA → Pike VM and lazy DFA.
It runs in time linear in the input (no backtracking), and its results are checked
against the [`regex`](https://crates.io/crates/regex) crate by differential testing.

## Quick start

```rust
use regex_engine::Regex;

let re = Regex::new(r"(\w+)@(\w+)\.(com|org)")?;
assert!(re.is_match("mail bob@example.org"));          // lazy DFA
let caps = re.captures("mail bob@example.org").unwrap(); // Pike VM
let m = re.find("x 12 345").map(|m| (m.start, m.end));
let all = re.find_iter("a@b.com c@d.org");
```

There is also a tiny grep: `cargo run --release --bin rgrep -- [-o] '<pattern>' file...`

## Syntax

Literals, `.` (any char but `\n`), classes `[a-z]`, `[^...]`, `[]a]`, `[a-]`,
`\d \w \s \D \W \S` (ASCII definitions), `\n \t \r \f \v`, escaped punctuation,
anchors `^` and `$` (start/end of text), groups `( )` and `(?: )`, alternation `|`,
quantifiers `* + ? {n} {n,} {n,m}` and their lazy forms `*? +? ?? {n,m}?`.

## How it works

1. **Parser** (`parser.rs`): recursive descent to an AST. Every literal, class and
   escape becomes a sorted list of `char` ranges, so later stages see one kind of leaf.
2. **Thompson NFA** (`nfa.rs`): `Class`, `Split` (ordered: the first branch has
   priority, which is how greedy and lazy differ), `Save` (capture slots), `Look`
   (`^`/`$`) and `Match`. Counted repetition is expanded. `e*` is compiled as a single
   loop, or as `(e+)?` when `e` can match the empty string. This is the same choice the
   `regex` crate makes, and the shape changes which captures a Pike VM reports.
3. **Pike VM** (`pikevm.rs`) simulates all threads in lockstep, in priority order,
   copying capture slots per thread. When a thread matches, lower-priority threads are
   cut, which gives leftmost-first semantics. Used for `find`, `captures` and `find_iter`.
4. **Lazy DFA** (`dfa.rs`) builds DFA states (sets of NFA states) on demand while
   scanning and caches transitions: 128-entry tables for ASCII plus a map for other
   chars. The search is unanchored (the NFA start is re-added after each char), and `$`
   assertions stay pending until the end of the input. The cache is flushed when it
   passes 4096 states, so memory stays bounded. Used for `is_match`, and as a quick
   reject before running the Pike VM.

## Tests

```sh
cargo test                                  # unit tests + differential (200 patterns in debug)
cargo test --release --test differential    # 4000 random patterns x 25 haystacks
DIFF_PATTERNS=20000 cargo test --release --test differential
```

`tests/differential.rs` generates random patterns (nested groups, alternations, every
quantifier form, classes, escapes, anchors) and haystacks. For each pair it checks
`is_match` (both DFA and Pike VM), `find`, `captures` and `find_iter` against the
`regex` crate. `examples/minimize.rs` shrinks a failing case to a minimal reproduction.
It found the `(e+)?` capture-priority issue described above.

## Status / limitations

Learning project.
- No flags (`(?i)`, `(?m)`, `(?s)`), no `\b`, no Unicode classes (`\p{..}`), no
  named groups, no nested classes or class set operations.
- `\d \w \s` are ASCII-only, while `regex` defaults to Unicode. The differential tests
  use ASCII haystacks, where the two agree.
- The DFA only answers `is_match`. Match positions come from the Pike VM, which is much
  slower than a real DFA-based search.
- `Regex` is not `Sync` (the DFA cache sits in a `RefCell`).

## License

MIT, see [LICENSE](LICENSE).
