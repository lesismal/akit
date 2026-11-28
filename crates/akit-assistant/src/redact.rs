//! What the assistant hands a model, redacted (I162, fit's `assistant/redact.go`): the
//! providers' keys it knows, secrets found in text by pattern (private keys, password hashes,
//! tokens, the keys of well-known APIs, `name=value` secrets, signed playback URLs), the
//! settings' own patterns (`data.redact`), and, in JSON, every string under a member whose name
//! says it holds a secret, with the members no tool reads left out ([`HIDDEN`]).

use std::sync::OnceLock;

use akio_regex::Regex;
use akit_config::value::Value;

/// What a secret is replaced with, fit's.
pub const REDACTED: &str = "[redacted]";
/// What a signed playback URL is replaced with, fit's.
pub const REDACTED_URL: &str = "[redacted playback URL]";

/// The members of the config the assistant never hands a model, whatever is redacted in them:
/// its own settings, the console's passwords, the store and its key's source, and the Admin
/// API's own address and token. fit's `hiddenMembers`.
pub const HIDDEN: &[&str] = &[
    "admin.assistant",
    "admin.console.users",
    "admin.store",
    "admin.address",
    "admin.token_file",
];

/// fit's `secretPatterns`, compiled once; the media grant's cookie is akit's
/// (`akit_media_grant`, decision 037).
fn patterns() -> &'static [(Regex, &'static str)] {
    static P: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        [
            (
                r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?s:.*?)(-----END [A-Z0-9 ]*PRIVATE KEY-----|$)",
                "[redacted private key]",
            ),
            (r"\$2[abxy]?\$\d{2}\$[./A-Za-z0-9]{53}", REDACTED),
            (r#"\$argon2(?:id|i|d)\$[^\s"'\\]+"#, REDACTED),
            (r#"\$[56]\$[^\s"'\\:]+"#, REDACTED),
            (
                r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
                REDACTED,
            ),
            (
                r"(?i)\b(bearer|basic)(\s+)[A-Za-z0-9._~+/=-]{8,}",
                "${1}${2}[redacted]",
            ),
            (r"\b(?:sk|pk|rk)-[A-Za-z0-9_-]{16,}", REDACTED),
            (r"\bAKIA[0-9A-Z]{16}\b", REDACTED),
            (r"\bgh[pousr]_[A-Za-z0-9]{20,}", REDACTED),
            (r"\bxox[abprs]-[A-Za-z0-9-]{10,}", REDACTED),
            (r"\bAIza[0-9A-Za-z_-]{30,}", REDACTED),
            (
                r#"(?i)\b((?:[a-z]+[_-])?(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|authorization))(\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s,;&"']+)"#,
                "${1}${2}[redacted]",
            ),
            (
                r#"(?:\b[A-Za-z][A-Za-z0-9+.-]*://[^\s"'<>\\?]*|/[^\s"'<>\\?]*|\b[A-Za-z0-9._~%-]+)\?(?:[^\s"'<>\\]|\\u0026)*?(?:\b|\\u0026)sig=(?:[^\s"'<>\\]|\\u0026)*"#,
                REDACTED_URL,
            ),
            (
                r#"(\b|\\u0026)(sig|akit_media_grant)=[^\s"'<>\\&;,]+"#,
                "${1}${2}=[redacted]",
            ),
        ]
        .into_iter()
        .filter_map(|(re, with)| Regex::new(re).ok().map(|r| (r, with)))
        .collect()
    })
}

/// Replaces the secrets of what a tool hands a model.
#[derive(Debug, Default)]
pub struct Redactor {
    literals: Vec<String>,
    extra: Vec<Regex>,
}

impl Redactor {
    /// A redactor of the keys `literals` (those of 8 bytes or more) and the patterns `extra`
    /// (`data.redact`, checked when the settings loaded).
    #[must_use]
    pub fn new(literals: Vec<String>, extra: &[String]) -> Redactor {
        Redactor {
            literals: literals.into_iter().filter(|l| l.len() >= 8).collect(),
            extra: extra
                .iter()
                .filter_map(|e| akit_router::regexp(e.as_bytes()).ok())
                .collect(),
        }
    }

    /// `s` with its secrets replaced.
    #[must_use]
    pub fn text(&self, s: &str) -> String {
        let mut s = s.to_owned();
        for l in &self.literals {
            if s.contains(l.as_str()) {
                s = s.replace(l.as_str(), REDACTED);
            }
        }
        for (re, with) in patterns() {
            s = re.replace_all(&s, with).into_owned();
        }
        for re in &self.extra {
            s = re.replace_all(&s, REDACTED).into_owned();
        }
        s
    }

    /// `v`, JSON, compacted, with the members `hide` names (dotted paths from its root) left
    /// out, every string under a member that holds a secret replaced, and the text of what is
    /// left redacted. fit's `redactJSON`.
    #[must_use]
    pub fn json(&self, v: &Value, hide: &[&str]) -> String {
        let mut b = String::new();
        write(&mut b, v, "", false, hide);
        self.text(&b)
    }

    /// `text`, JSON, as [`json`](Self::json) writes it; `None` when it is not JSON.
    #[must_use]
    pub fn json_text(&self, text: &str, hide: &[&str]) -> Option<String> {
        akit_config::value::parse(text)
            .ok()
            .map(|v| self.json(&v, hide))
    }
}

fn write(b: &mut String, v: &Value, path: &str, secret: bool, hide: &[&str]) {
    match v {
        Value::Object(ms) => {
            b.push('{');
            let mut first = true;
            for (k, x) in ms {
                let sub = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if hide.contains(&sub.as_str()) {
                    continue;
                }
                if !first {
                    b.push(',');
                }
                first = false;
                akit_config::value::write_string(b, k);
                b.push(':');
                let s = secret || secret_name(k) || push_url(path, k);
                write(b, x, &sub, s, hide);
            }
            b.push('}');
        }
        Value::Array(a) => {
            b.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write(b, x, path, secret, hide);
            }
            b.push(']');
        }
        Value::String(s) => {
            let name = path.rsplit('.').next().unwrap_or("");
            if secret && !s.is_empty() && !selector_value(name, s) {
                akit_config::value::write_string(b, REDACTED);
            } else {
                akit_config::value::write_string(b, s);
            }
        }
        other => crate::json::write(b, other),
    }
}

/// Whether a member named `name` holds a secret: a password, a key, a token or a hash, but not
/// a file or a variable naming one, nor a limit. fit's `secretName`.
#[must_use]
pub fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.ends_with("_file")
        || n.ends_with("_env")
        || n.ends_with("_ttl")
        || n.starts_with("max_")
        || n.starts_with("min_")
    {
        return false;
    }
    if n.contains("password") || n.contains("secret") || n.contains("passphrase") {
        return true;
    }
    if matches!(
        n.as_str(),
        "passwd"
            | "pass"
            | "token"
            | "tokens"
            | "key"
            | "keys"
            | "apikey"
            | "hash"
            | "credential"
            | "credentials"
            | "authorization"
            | "cookie"
            | "signature"
            | "psk"
            | "private_key"
    ) {
        return true;
    }
    [
        "_token",
        "_tokens",
        "_key",
        "_keys",
        "_hash",
        "_sha256",
        "_signature",
    ]
    .iter()
    .any(|s| n.ends_with(s))
}

/// Whether member `key` of the object at `path` is a push target's `url`, which carries its
/// stream key and is a secret whole.
fn push_url(path: &str, key: &str) -> bool {
    key == "url" && (path == "push" || path.ends_with(".push"))
}

/// Whether `s`, under a member named `name` taken for a secret's, is a selector rather than
/// one: `rate_limit`'s `key`, `ip`, `route` or `header:Name`.
fn selector_value(name: &str, s: &str) -> bool {
    if name != "key" {
        return false;
    }
    if s == "ip" || s == "route" {
        return true;
    }
    s.strip_prefix("header:").is_some_and(|h| {
        !h.is_empty()
            && h.len() <= 64
            && h.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+.^_|~-".contains(&c))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// fit's `TestRedact`: the patterns, the keys known, the settings' own, and JSON by member.
    #[test]
    fn redacts() {
        let r = Redactor::new(vec!["my-own-key-123".into()], &["secret-[0-9]+".into()]);
        let text = "key my-own-key-123, Authorization: Bearer abcdefgh12345, sk-abcdefghijklmnop1234, \
                    password=hunter2 x, $2b$10$abcdefghijklmnopqrstuuG2rmOVKzOv2Hd2t1Gc9i1dK1Szy/7Ri, \
                    secret-42, https://v.example/m.m3u8?exp=1&sig=abc and akit_media_grant=xyz";
        let out = r.text(text);
        for gone in [
            "my-own-key-123",
            "abcdefgh12345",
            "sk-abcdefghijklmnop1234",
            "hunter2",
            "$2b$10$",
            "secret-42",
            "sig=abc",
            "=xyz",
        ] {
            assert!(!out.contains(gone), "{gone} in {out}");
        }
        assert!(out.contains(REDACTED_URL), "{out}");
        let pem = "-----BEGIN PRIVATE KEY-----\nMIIabc\n-----END PRIVATE KEY-----";
        assert_eq!(r.text(pem), "[redacted private key]");
        let v = akit_config::value::parse(
            r#"{"admin": {"assistant": {"x": 1}, "console": {"users": [1], "address": "a"}},
                "routes": [{"middleware": [{"module": "rate_limit", "params": {"key": "header:X-User"}},
                {"params": {"password": "p", "users": [{"password_hash": "h"}], "token_file": "/t", "max_tokens": 3}}]}],
                "streaming": {"push": [{"url": "rtmp://h/live/streamkey"}]}}"#,
        )
        .unwrap();
        let out = r.json(&v, HIDDEN);
        assert!(
            !out.contains("assistant") && !out.contains("users\":[1]"),
            "{out}"
        );
        assert!(out.contains(r#""key":"header:X-User""#), "{out}");
        assert!(
            out.contains(r#""password":"[redacted]""#)
                && out.contains(r#""password_hash":"[redacted]""#)
        );
        assert!(out.contains(r#""token_file":"/t""#) && out.contains(r#""max_tokens":3"#));
        assert!(!out.contains("streamkey"), "{out}");
    }

    #[test]
    fn names() {
        for s in [
            "password",
            "api_key",
            "token",
            "hmac_secret",
            "pin_sha256",
            "key",
        ] {
            assert!(secret_name(s), "{s}");
        }
        for s in [
            "password_file",
            "key_env",
            "max_keys",
            "session_ttl",
            "name",
        ] {
            assert!(!secret_name(s), "{s}");
        }
        assert!(selector_value("key", "ip") && !selector_value("key", "abc"));
    }
}
