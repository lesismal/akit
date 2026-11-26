//! `akit hash-password` (I89), as fit's `fit hash-password`: the bcrypt hash of a password, for
//! `admin.console.users` and `basic_auth`. The password is read from standard input, never from
//! the arguments, which other users of the machine could see: its first line, or, from a
//! terminal, asked for twice, the terminal in raw mode (akio-sys's `terminal::raw`, akio's A233)
//! and a `*` shown for each character typed, as fit's.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::fd::AsFd;
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
        // Raw mode for as long as the password is typed: the guard puts the terminal back.
        match akio_sys::terminal::raw(stdin.as_fd()) {
            Ok(_raw) => read_twice(&mut stdin.lock(), &mut std::io::stderr()),
            Err(e) => Err(e.to_string()),
        }
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

/// The password, asked for twice on `prompt` and read from `input`, a terminal in raw mode, each
/// masked ([`read_masked`]); an error when the two differ.
fn read_twice(input: &mut impl BufRead, prompt: &mut impl Write) -> Result<String, String> {
    let _ = write!(prompt, "Password: ");
    let _ = prompt.flush();
    let password = read_masked(input, prompt)?;
    if password.is_empty() {
        return Ok(password);
    }
    let _ = write!(prompt, "Confirm password: ");
    let _ = prompt.flush();
    if read_masked(input, prompt)? != password {
        return Err("the passwords do not match".into());
    }
    Ok(password)
}

/// The most bytes of a password typed at a terminal: far past bcrypt's 72, kept from growing
/// without end, as fit's.
const MAX_TYPED: usize = 1024;

/// A line read from a terminal in raw mode, as fit's `readMasked`: a `*` written to `echo` for
/// each character typed, the last one taken back on Backspace and all of them on Ctrl-U; escape
/// sequences, as arrow keys send, and other control characters dropped; Ctrl-C, Ctrl-D or the end
/// of `input` an error ("interrupted"). A new line (`\r\n`, which raw mode does not make of `\n`)
/// is written when it returns.
fn read_masked(input: &mut impl BufRead, echo: &mut impl Write) -> Result<String, String> {
    let r = masked(input, echo);
    let _ = write!(echo, "\r\n");
    let _ = echo.flush();
    r
}

fn masked(input: &mut impl BufRead, echo: &mut impl Write) -> Result<String, String> {
    let mut b: Vec<u8> = Vec::new();
    let mut byte = || -> Result<u8, String> {
        let mut c = [0u8; 1];
        match input.read(&mut c) {
            Ok(1) => Ok(c[0]),
            Ok(_) => Err("interrupted".into()),
            Err(e) => Err(e.to_string()),
        }
    };
    loop {
        let c = byte()?;
        match c {
            b'\r' | b'\n' => {
                return String::from_utf8(b).map_err(|_| "the password is not UTF-8".into());
            }
            3 | 4 => return Err("interrupted".into()), // Ctrl-C, Ctrl-D
            8 | 127 => {
                // Backspace: the last character, all its bytes.
                if let Some(start) = b.iter().rposition(|&x| !is_continuation(x)) {
                    b.truncate(start);
                    let _ = write!(echo, "\x08 \x08");
                }
            }
            21 => {
                // Ctrl-U: every character.
                let n = b.iter().filter(|&&x| !is_continuation(x)).count();
                let _ = write!(echo, "{}", "\x08 \x08".repeat(n));
                b.clear();
            }
            27 => {
                // ESC: the sequence it starts dropped.
                if matches!(byte(), Ok(b'[' | b'O')) {
                    while !matches!(byte(), Err(_) | Ok(0x40..=0x7e)) {}
                }
            }
            c if c < 32 => {}
            _ if b.len() >= MAX_TYPED => {}
            c => {
                b.push(c);
                // A character's later bytes: its `*` is written.
                if !is_continuation(c) {
                    let _ = write!(echo, "*");
                }
            }
        }
        let _ = echo.flush();
    }
}

/// Whether `b` is a later byte of a UTF-8 character.
fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
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
    fn reads_a_line() {
        assert_eq!(read_line(&mut &b"pw\r\nmore\n"[..]).unwrap(), "pw");
        assert_eq!(read_line(&mut &b"pw"[..]).unwrap(), "pw");
    }

    /// fit's `TestHashPasswordTerminal`: typed "s3x", Backspace, "cré", Enter; then a Ctrl-U'd
    /// attempt, an arrow key and "s3cré", Enter.
    #[test]
    fn asks_twice_masked() {
        let mut out = Vec::new();
        let typed = "s3x\x7fcré\rjunk\x15\x1b[As3cré\r";
        assert_eq!(
            read_twice(&mut typed.as_bytes(), &mut out).unwrap(),
            "s3cré"
        );
        let want = format!(
            "Password: ***\x08 \x08***\r\nConfirm password: ****{}*****\r\n",
            "\x08 \x08".repeat(4)
        );
        assert_eq!(String::from_utf8(out).unwrap(), want);
        for (typed, err) in [
            ("a\rb\r", "the passwords do not match"),
            ("a\x03", "interrupted"),
            ("a\rb", "interrupted"),
        ] {
            assert_eq!(
                read_twice(&mut typed.as_bytes(), &mut Vec::new()).unwrap_err(),
                err,
                "{typed:?}"
            );
        }
        // An empty one is returned for the caller to refuse.
        assert_eq!(read_twice(&mut &b"\r"[..], &mut Vec::new()).unwrap(), "");
        // Past the bound, the rest is dropped.
        let long = format!("{}\r", "x".repeat(MAX_TYPED + 10));
        assert_eq!(
            read_masked(&mut long.as_bytes(), &mut Vec::new())
                .unwrap()
                .len(),
            MAX_TYPED
        );
    }
}
