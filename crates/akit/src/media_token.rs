//! `akit media-token` (I121), as fit's `fit media-token`: prints a token a `media_auth` policy's
//! secret signs, for a publisher or player to send as a bearer token or the `token` query
//! parameter, and when it expires, on standard error. The token is fit's format (decision 040):
//! fit takes it given the same policy name and secret, and akit takes fit's.

use std::process::ExitCode;
use std::time::Duration;

use akit_media::auth::{Grant, Policy};

use crate::flags::{self, Spec};

const FLAGS: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: "a config file, or a directory of them, to read the policy from; give -c again for more (akit.json when none is given)",
    },
    Spec {
        name: "policy",
        bool: false,
        usage: "the media_auth policy whose secret signs the token (required)",
    },
    Spec {
        name: "publish",
        bool: false,
        usage: "a stream pattern the token may publish, as live/* or room1-*; give it again, or separate them with commas, for more",
    },
    Spec {
        name: "play",
        bool: false,
        usage: "a stream pattern the token may play, as -publish has them",
    },
    Spec {
        name: "ttl",
        bool: false,
        usage: "how long the token is good for; the policy's token_ttl when left out",
    },
    Spec {
        name: "key",
        bool: false,
        usage: "the key of the policy's keyring to sign with; the first when left out, and an error for a policy with a single secret",
    },
    Spec {
        name: "subject",
        bool: false,
        usage: "who the token is for, as logs name it (default \"media-token\")",
    },
];

/// The patterns of every `-name` flag, each split at commas, blanks left out.
fn patterns(flags: &[(&str, String)], name: &str) -> Vec<String> {
    flags
        .iter()
        .filter(|(n, _)| *n == name)
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `akit media-token -policy p [-publish ...] [-play ...] [-ttl d] [-subject s] [-c ...]`:
/// prints the token, or says why not and exits 1 (2 for a usage error).
pub(crate) fn media_token(args: &[String]) -> ExitCode {
    let flags = match flags::parse("media-token", args, FLAGS) {
        Ok(f) => f,
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    let last = |name: &str| {
        flags
            .iter()
            .rev()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.clone())
    };
    let policy = last("policy").unwrap_or_default();
    let subject = last("subject").unwrap_or_else(|| "media-token".to_owned());
    let (publish, play) = (patterns(&flags, "publish"), patterns(&flags, "play"));
    let mut ttl = Duration::ZERO;
    if let Some(v) = last("ttl") {
        match flags::parse_duration(&v) {
            Some(d) => ttl = d,
            None => {
                eprintln!(
                    "invalid value {v:?} for flag -ttl: parse error\n{}",
                    flags::usage("media-token", FLAGS)
                );
                return ExitCode::from(2);
            }
        }
    }
    if policy.is_empty() {
        eprintln!("akit media-token: -policy is required");
        return ExitCode::from(2);
    }
    if publish.is_empty() && play.is_empty() {
        eprintln!(
            "akit media-token: give -publish, -play or both: a token that may do nothing is no use"
        );
        return ExitCode::from(2);
    }
    let paths = crate::config_paths(&flags);
    let name = paths.join(", ");
    let config = match akit_config::read_config_files(&paths)
        .and_then(|files| akit_config::load_config_files(&files))
    {
        Ok((c, _)) => c,
        Err(errs) => {
            for e in &errs.0 {
                eprintln!("akit media-token: {name}: {e}");
            }
            return ExitCode::from(1);
        }
    };
    let Some(cfg) = config.media_auth.iter().find(|p| p.name == policy) else {
        eprintln!("akit media-token: {name} has no media_auth policy named {policy:?}");
        return ExitCode::from(1);
    };
    if cfg.secret.is_empty() && cfg.secret_file.is_empty() && cfg.keys.is_empty() {
        eprintln!(
            "akit media-token: media_auth {policy:?} has neither secret, secret_file nor keys: \
             tokens it would sign could not be checked by a running akit, which signs with a key \
             of its own without one"
        );
        return ExitCode::from(1);
    }
    let key = last("key").unwrap_or_default();
    if !key.is_empty() && cfg.keys.is_empty() {
        eprintln!("akit media-token: media_auth {policy:?} has no keyring, so no key {key:?}");
        return ExitCode::from(1);
    }
    let grant = Grant { publish, play };
    let signed = Policy::from_config(cfg)
        .map_err(|e| format!("media_auth {policy:?}: {e}"))
        .and_then(|p| {
            if key.is_empty() {
                p.issue(&subject, &grant, ttl)
            } else {
                p.issue_key(&key, &subject, &grant, ttl)
            }
        });
    match signed {
        Ok((token, expiry)) => {
            println!("{token}");
            eprintln!("akit media-token: good until {}", rfc3339(expiry));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("akit media-token: {e}");
            ExitCode::from(1)
        }
    }
}

/// A time in seconds since 1970 as RFC 3339 in UTC, as Go's `time.RFC3339` writes it.
// No step overflows for any `i64`: the days are at most 1.1e14, the era at most 7.3e8.
#[allow(clippy::arithmetic_side_effects)]
fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-from-civil, inverted.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(4_949_344_838), "2126-11-03T02:00:38Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn pattern_flags() {
        let f = vec![
            ("publish", "live/*, room-*".to_owned()),
            ("play", "*".to_owned()),
            ("publish", "x".to_owned()),
        ];
        assert_eq!(patterns(&f, "publish"), ["live/*", "room-*", "x"]);
        assert_eq!(patterns(&f, "play"), ["*"]);
    }
}
