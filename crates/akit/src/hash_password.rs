//! `akit hash-password` (I89), as fit's `fit hash-password`: the bcrypt hash of a password, for
//! `admin.console.users` and `basic_auth`. The password is read from standard input, never from
//! the arguments, which other users of the machine could see: its first line, or, from a
//! terminal, asked for twice. fit turns the terminal's echo off and shows a `*` for each
//! character; akio cannot yet (A233), so from a terminal the password is seen as it is typed,
//! and the prompt says so.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::process::ExitCode;

use akio_crypto::bcrypt;

use crate::flags::{self, Spec};

const FLAGS: &[Spec] = &[Spec {
    name: "cost",
    bool: false,
    usage: "the bcrypt cost: each one more doubles the time a check takes (default 10)",
}];

/// The most bytes of standard input read for the password, as fit's.
const MAX_LINE: u64 = 1024;

/// `akit hash-password [-cost n]`: prints the hash, or says why not and exits 1 (2 for a usage
/// error).
pub(crate) fn hash_password(args: &[String]) -> ExitCode {
    let flags = match flags::parse("hash-password", args, FLAGS) {
        Ok(f) => f,
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    let mut cost = bcrypt::DEFAULT_COST;
    if let Some((_, v)) = flags.iter().rev().find(|(n, _)| *n == "cost") {
        match v.parse::<u32>() {
            Ok(c) => cost = c,
            Err(_) => {
                eprintln!(
                    "invalid value {v:?} for flag -cost: parse error\n{}",
                    flags::usage("hash-password", FLAGS)
                );
                return ExitCode::from(2);
            }
        }
    }
    let stdin = std::io::stdin();
    let password = if stdin.is_terminal() {
        read_twice(&mut stdin.lock(), &mut std::io::stderr())
    } else {
        read_line(&mut stdin.lock())
    };
    match password.and_then(|p| hash(&p, cost)) {
        Ok(h) => {
            println!("{h}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("akit hash-password: {msg}");
            ExitCode::from(1)
        }
    }
}

/// The hash of `password` at `cost`, with fit's checks: not empty, at most 72 bytes.
pub(crate) fn hash(password: &str, cost: u32) -> Result<String, String> {
    if password.is_empty() {
        return Err("the password is empty".into());
    }
    if password.len() > bcrypt::MAX_PASSWORD_LEN {
        return Err("bcrypt takes passwords of at most 72 bytes".into());
    }
    if !(bcrypt::MIN_COST..=bcrypt::MAX_COST).contains(&cost) {
        return Err(format!(
            "the cost is {} to {}, not {cost}",
            bcrypt::MIN_COST,
            bcrypt::MAX_COST
        ));
    }
    bcrypt::hash(password.as_bytes(), cost).map_err(|e| e.to_string())
}

/// The first line of `input`, without its line ending.
fn read_line(input: &mut impl BufRead) -> Result<String, String> {
    let mut line = String::new();
    Read::take(&mut *input, MAX_LINE)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// The password, asked for twice on `prompt` and read from `input`; an error when the two differ.
fn read_twice(input: &mut impl BufRead, prompt: &mut impl Write) -> Result<String, String> {
    let _ = writeln!(
        prompt,
        "(the password is shown as it is typed; to hide it: read -rs p; printf '%s\\n' \"$p\" | akit hash-password)"
    );
    let _ = write!(prompt, "Password: ");
    let _ = prompt.flush();
    let password = read_line(input)?;
    if password.is_empty() {
        return Ok(password);
    }
    let _ = write!(prompt, "Confirm password: ");
    let _ = prompt.flush();
    if read_line(input)? != password {
        return Err("the passwords do not match".into());
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn hashes_that_verify() {
        let h = hash("correct horse battery staple", bcrypt::MIN_COST).unwrap();
        assert!(h.starts_with("$2b$04$"));
        assert!(
            bcrypt::Hash::parse(&h)
                .unwrap()
                .verify(b"correct horse battery staple")
        );
        assert_eq!(hash("", 10).unwrap_err(), "the password is empty");
        assert!(hash(&"x".repeat(73), 10).unwrap_err().contains("72 bytes"));
        assert!(hash("x", 3).unwrap_err().contains("not 3"));
    }

    #[test]
    fn reads_a_line_or_asks_twice() {
        let mut out = Vec::new();
        assert_eq!(read_line(&mut &b"pw\r\nmore\n"[..]).unwrap(), "pw");
        assert_eq!(read_line(&mut &b"pw"[..]).unwrap(), "pw");
        assert_eq!(read_twice(&mut &b"pw\npw\n"[..], &mut out).unwrap(), "pw");
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Confirm password: ")
        );
        assert_eq!(
            read_twice(&mut &b"pw\nother\n"[..], &mut Vec::new()).unwrap_err(),
            "the passwords do not match"
        );
    }
}
