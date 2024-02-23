//! rgrep: print lines matching a pattern, using this crate's engine.

use regex_engine::Regex;
use std::io::{BufRead, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (only, args) = match args.first().map(String::as_str) {
        Some("-o") => (true, &args[1..]),
        _ => (false, &args[..]),
    };
    let Some(pattern) = args.first() else {
        eprintln!("usage: rgrep [-o] <pattern> [file...]");
        return ExitCode::from(2);
    };
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rgrep: {e}");
            return ExitCode::from(2);
        }
    };
    let mut found = false;
    let mut out = std::io::stdout().lock();
    let mut scan = |reader: &mut dyn BufRead, name: Option<&str>| {
        for line in reader.lines().map_while(Result::ok) {
            if only {
                for m in re.find_iter(&line) {
                    found = true;
                    let _ = writeln!(out, "{}", m.as_str(&line));
                }
            } else if re.is_match(&line) {
                found = true;
                let _ = match name {
                    Some(n) => writeln!(out, "{n}:{line}"),
                    None => writeln!(out, "{line}"),
                };
            }
        }
    };
    let files = &args[1..];
    if files.is_empty() {
        scan(&mut std::io::stdin().lock(), None);
    } else {
        for f in files {
            match std::fs::File::open(f) {
                Ok(file) => scan(&mut std::io::BufReader::new(file), (files.len() > 1).then_some(f.as_str())),
                Err(e) => eprintln!("rgrep: {f}: {e}"),
            }
        }
    }
    if found {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
