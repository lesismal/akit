//! Command-line flags as Go's `flag` package reads them, so that fit's command lines work
//! unchanged: `-name value`, `-name=value`, `--name` the same, a bool flag as `-name` or
//! `-name=false`, flags before the arguments, `--` ending them.

use std::time::Duration;

/// A flag a command takes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Spec {
    /// Its name, without the dash.
    pub(crate) name: &'static str,
    /// Whether it is a bool flag (no separate value).
    pub(crate) bool: bool,
    /// Its usage line.
    pub(crate) usage: &'static str,
}

/// Why the flags could not be read; the message is printed and the command exits 2.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Usage(pub String);

/// The flags `args` give, in order, as (name, value); a bool flag given as `-name` is `"true"`.
/// Arguments after the flags are refused: no command of fit's takes any.
pub(crate) fn parse(
    cmd: &str,
    args: &[String],
    specs: &[Spec],
) -> Result<Vec<(&'static str, String)>, Usage> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            break;
        }
        let Some(body) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
            return Err(Usage(format!("akit {cmd}: unexpected argument {a:?}")));
        };
        if body.is_empty() || body.starts_with('-') || body.starts_with('=') {
            return Err(Usage(format!("akit {cmd}: bad flag syntax: {a}")));
        }
        let (name, value) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_owned())),
            None => (body, None),
        };
        if name == "h" || name == "help" {
            return Err(Usage(usage(cmd, specs)));
        }
        let Some(spec) = specs.iter().find(|s| s.name == name) else {
            return Err(Usage(format!(
                "flag provided but not defined: -{name}\n{}",
                usage(cmd, specs)
            )));
        };
        let value = match (value, spec.bool) {
            (Some(v), true) => match v.as_str() {
                "1" | "t" | "T" | "true" | "TRUE" | "True" => "true".to_owned(),
                "0" | "f" | "F" | "false" | "FALSE" | "False" => "false".to_owned(),
                _ => {
                    return Err(Usage(format!(
                        "invalid boolean value {v:?} for -{name}: parse error\n{}",
                        usage(cmd, specs)
                    )));
                }
            },
            (None, true) => "true".to_owned(),
            (Some(v), false) => v,
            (None, false) => match it.next() {
                Some(v) => v.clone(),
                None => {
                    return Err(Usage(format!(
                        "flag needs an argument: -{name}\n{}",
                        usage(cmd, specs)
                    )));
                }
            },
        };
        out.push((spec.name, value));
    }
    if let Some(a) = it.next() {
        return Err(Usage(format!("akit {cmd}: unexpected argument {a:?}")));
    }
    Ok(out)
}

/// The usage of `cmd`'s flags.
pub(crate) fn usage(cmd: &str, specs: &[Spec]) -> String {
    let mut s = format!("Usage of akit {cmd}:");
    for f in specs {
        s.push_str(&format!("\n  -{}", f.name));
        if !f.bool {
            s.push_str(" value");
        }
        s.push_str(&format!("\n    \t{}", f.usage));
    }
    s
}

/// A duration as Go's `time.ParseDuration` reads it: `300ms`, `1.5h`, `2h45m`, `0`. A negative
/// one is `None` here: no flag of akit's takes one.
pub(crate) fn parse_duration(s: &str) -> Option<Duration> {
    if s == "0" {
        return Some(Duration::ZERO);
    }
    let mut rest = s.strip_prefix('+').unwrap_or(s);
    if rest.is_empty() {
        return None;
    }
    let mut total = 0f64;
    while !rest.is_empty() {
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let (num, after) = rest.split_at(num_len);
        if num.is_empty() || num == "." {
            return None;
        }
        let n: f64 = num.parse().ok()?;
        let unit_len = after
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(after.len());
        let (unit, next) = after.split_at(unit_len);
        let scale = match unit {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return None,
        };
        total += n * scale;
        rest = next;
    }
    Duration::try_from_secs_f64(total).ok()
}

/// A duration as Go prints one: `30s`, `1m30s`, `1h0m0s`, `500ms`.
pub(crate) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    if secs == 0 {
        return match nanos {
            0 => "0s".to_owned(),
            n if n % 1_000_000 == 0 => format!("{}ms", n / 1_000_000),
            n if n % 1_000 == 0 => format!("{}µs", n / 1_000),
            n => format!("{n}ns"),
        };
    }
    let mut s_part = format!("{}", secs % 60);
    if nanos != 0 {
        let frac = format!("{nanos:09}");
        s_part.push('.');
        s_part.push_str(frac.trim_end_matches('0'));
    }
    let (h, m) = (secs / 3600, (secs / 60) % 60);
    if h > 0 {
        format!("{h}h{m}m{s_part}s")
    } else if m > 0 {
        format!("{m}m{s_part}s")
    } else {
        format!("{s_part}s")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Spec, format_duration, parse, parse_duration};

    const SPECS: &[Spec] = &[
        Spec {
            name: "c",
            bool: false,
            usage: "config",
        },
        Spec {
            name: "watch",
            bool: true,
            usage: "watch",
        },
    ];

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn go_flag_forms() {
        let got = parse(
            "run",
            &args(&["-c", "a", "--c=b", "-watch", "-watch=false"]),
            SPECS,
        )
        .unwrap();
        assert_eq!(
            got,
            [
                ("c", "a".into()),
                ("c", "b".into()),
                ("watch", "true".into()),
                ("watch", "false".into())
            ]
        );
        assert_eq!(
            parse("run", &args(&["--", "-c"]), SPECS).unwrap_err().0,
            "akit run: unexpected argument \"-c\""
        );
        assert!(
            parse("run", &args(&["-c"]), SPECS)
                .unwrap_err()
                .0
                .starts_with("flag needs an argument: -c")
        );
        assert!(
            parse("run", &args(&["-x"]), SPECS)
                .unwrap_err()
                .0
                .starts_with("flag provided but not defined: -x")
        );
        assert!(parse("run", &args(&["-watch=maybe"]), SPECS).is_err());
        assert_eq!(
            parse("run", &args(&["x"]), SPECS).unwrap_err().0,
            "akit run: unexpected argument \"x\""
        );
        assert!(
            parse("run", &args(&["-h"]), SPECS)
                .unwrap_err()
                .0
                .starts_with("Usage of akit run:")
        );
    }

    #[test]
    fn durations() {
        for (s, d) in [
            ("30s", Duration::from_secs(30)),
            ("0", Duration::ZERO),
            ("1m30s", Duration::from_secs(90)),
            ("1.5h", Duration::from_secs(5400)),
            ("250ms", Duration::from_millis(250)),
            ("2h45m", Duration::from_secs(9900)),
        ] {
            assert_eq!(parse_duration(s), Some(d), "{s}");
        }
        for bad in ["", "30", "s", "1x", "-1s", "1..5s"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
        for (d, s) in [
            (Duration::from_secs(30), "30s"),
            (Duration::from_secs(90), "1m30s"),
            (Duration::from_secs(3600), "1h0m0s"),
            (Duration::from_millis(500), "500ms"),
            (Duration::from_millis(1500), "1.5s"),
            (Duration::ZERO, "0s"),
        ] {
            assert_eq!(format_duration(d), s);
        }
    }
}
