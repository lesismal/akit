//! Secrets written inline in a config, redacted in what the API answers (I80b, the threat
//! model's L2; [decision 035](../../../docs/decisions/035-admin-api-redacts-secrets.md)), and put
//! back where a change sends them redacted.
//!
//! A secret is found as fit's console and assistant find one (fit's `secretName`,
//! `assistant/redact.go`, `cluster/templates.go`): every string under a member whose name says it
//! holds a password, a key, a token or a hash (but not a file or a variable naming one, nor a
//! limit), but for `rate_limit`'s `key` selectors, and a push target's `url`. Each is answered as
//! [`REDACTED`], fit's word. A change whose body holds [`REDACTED`] gets the secret the running
//! config keeps at the same place (matched by member, by `name` in a list of named objects, or by
//! index), as fit's console puts redacted values back; one with no secret there is refused.

use akit_config::value::Value;

/// What a secret is answered as, fit's `[redacted]`.
pub(crate) const REDACTED: &str = "[redacted]";

/// Whether a member named `name` holds a secret, as fit's `secretName`: a password, a key, a
/// token or a hash, but not a file or a variable naming one, nor a limit.
pub(crate) fn secret_name(name: &str) -> bool {
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

/// Whether `s`, under a member named `name` that [`secret_name`] takes for a secret's, is a
/// selector rather than one: `rate_limit`'s `key` (`ip`, `route` or `header:Name`), as fit's
/// `selectorValue`.
fn selector(name: &str, s: &str) -> bool {
    if name != "key" {
        return false;
    }
    match s.strip_prefix("header:") {
        Some(h) => {
            !h.is_empty()
                && h.len() <= 64
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_|~-".contains(&b))
        }
        None => s == "ip" || s == "route",
    }
}

/// Where a value sits: the name of the member that holds it (arrays add nothing), and whether it
/// is under a secret.
#[derive(Clone, Copy)]
struct At<'a> {
    name: &'a str,
    secret: bool,
}

impl<'a> At<'a> {
    /// The member `key` of the object this one names.
    fn member(self, key: &'a str) -> At<'a> {
        At {
            name: key,
            secret: self.secret || secret_name(key) || (key == "url" && self.name == "push"),
        }
    }
}

/// Replaces every secret in `v`; whether it replaced any.
fn walk(v: &mut Value, at: At<'_>) -> bool {
    match v {
        Value::String(s) => {
            if at.secret && !s.is_empty() && s != REDACTED && !selector(at.name, s) {
                *s = REDACTED.to_owned();
                return true;
            }
            false
        }
        Value::Array(a) => a.iter_mut().fold(false, |any, x| walk(x, at) | any),
        Value::Object(ms) => ms
            .iter_mut()
            .fold(false, |any, (k, x)| walk(x, at.member(k)) | any),
        _ => false,
    }
}

const ROOT: At<'static> = At {
    name: "",
    secret: false,
};

/// A whole config, standard JSON, with its secrets redacted: as it is when it holds none, or
/// else written out as fit writes an edited config. `None` when it is not JSON.
pub(crate) fn config(json: &str) -> Option<String> {
    let mut v = akit_config::value::parse(json).ok()?;
    Some(if walk(&mut v, ROOT) {
        v.to_json()
    } else {
        json.to_owned()
    })
}

/// Whether a pointer's token is an array's index, as fit's `secretPath` takes it.
fn index(t: &str) -> bool {
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
}

/// `v`, the value at the pointer `tokens`, with its secrets redacted.
pub(crate) fn at_pointer(v: &mut Value, tokens: &[String]) {
    let mut at = ROOT;
    for t in tokens.iter().filter(|t| !index(t)) {
        at = at.member(t);
    }
    walk(v, at);
}

/// Puts back, in `new`, the secrets `old` keeps where `new` has [`REDACTED`]: matched by member,
/// by `name` in a list of named objects, or else by index, as fit's `restoreSecrets`. `at` names
/// the place for messages. Whether it put any back; an error for a [`REDACTED`] with no secret
/// kept at its place.
pub(crate) fn restore(new: &mut Value, old: Option<&Value>, at: &str) -> Result<bool, String> {
    match new {
        Value::String(s) if s == REDACTED => match old {
            Some(Value::String(o)) if o != REDACTED => {
                s.clone_from(o);
                Ok(true)
            }
            _ => Err(format!(
                "{} is {REDACTED}, but no secret is kept there: write the secret itself",
                if at.is_empty() { "/" } else { at }
            )),
        },
        Value::Object(ms) => {
            let mut any = false;
            for (k, x) in ms.iter_mut() {
                let o = old.and_then(|o| o.get(k));
                any |= restore(x, o, &format!("{at}/{}", escape(k)))?;
            }
            Ok(any)
        }
        Value::Array(items) => {
            let olds = match old {
                Some(Value::Array(a)) => a.as_slice(),
                _ => &[],
            };
            let mut any = false;
            for (i, x) in items.iter_mut().enumerate() {
                let o = match name_of(x) {
                    Some(n) => olds.iter().find(|o| name_of(o) == Some(n)),
                    None => olds.get(i),
                };
                any |= restore(x, o, &format!("{at}/{i}"))?;
            }
            Ok(any)
        }
        _ => Ok(false),
    }
}

/// The `name` of an object in a list, which matches it to the old list's.
fn name_of(v: &Value) -> Option<&str> {
    match v.get("name") {
        Some(Value::String(n)) if !n.is_empty() => Some(n),
        _ => None,
    }
}

/// A pointer's token, escaped (RFC 6901).
fn escape(t: &str) -> String {
    t.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(s: &str) -> Value {
        akit_config::value::parse(s).unwrap()
    }

    #[test]
    fn names_as_fit() {
        for n in [
            "password",
            "password_hash",
            "hmac_secret",
            "Authorization",
            "token",
            "api_key",
            "client_secret",
            "key",
            "psk",
            "body_sha256",
        ] {
            assert!(secret_name(n), "{n}");
        }
        for n in [
            "token_file",
            "password_file",
            "hmac_secret_file",
            "key_file",
            "max_keys",
            "lock_ttl",
            "secret_env",
            "name",
            "keyring",
            "host",
        ] {
            assert!(!secret_name(n), "{n}");
        }
    }

    #[test]
    fn redacts_secrets_only() {
        let src = r#"{"routes": [
  {"middleware": [
    {"module": "basic_auth", "params": {"users": [{"username": "a", "password_hash": "$2a$10$x"}]}},
    {"module": "rate_limit", "params": {"key": "header:X-User", "rate": 5}},
    {"module": "rate_limit", "params": {"key": "s3cret"}},
    {"module": "headers", "params": {"request": {"set": {"Authorization": "Bearer abc", "X-Seen": "1"}}}}
  ],
  "handler": {"module": "proxy", "params": {"push": [{"url": "rtmp://h/live/streamkey"}], "url": "http://u"}}}
], "admin": {"token_file": "/etc/akit/token"}}"#;
        let out = config(src).unwrap();
        assert!(!out.contains("$2a$10$x"), "{out}");
        assert!(!out.contains("abc"), "{out}");
        assert!(!out.contains("s3cret"), "{out}");
        assert!(!out.contains("streamkey"), "{out}");
        for kept in [
            "header:X-User",
            "\"X-Seen\": \"1\"",
            "http://u",
            "/etc/akit/token",
            "\"username\": \"a\"",
        ] {
            assert!(out.contains(kept), "{kept}: {out}");
        }
        assert_eq!(out.matches(REDACTED).count(), 4, "{out}");
        // Nothing secret: as written.
        let plain = "{ \"listeners\" : [] }";
        assert_eq!(config(plain).unwrap(), plain);
        assert!(config("not json").is_none());
    }

    #[test]
    fn redacts_under_a_pointer() {
        let mut v = parse(r#"{"password_hash": "h", "username": "u"}"#);
        at_pointer(
            &mut v,
            &["routes", "0", "middleware", "0", "params", "users", "1"].map(String::from),
        );
        assert_eq!(
            v.get("password_hash"),
            Some(&Value::String(REDACTED.into()))
        );
        assert_eq!(v.get("username"), Some(&Value::String("u".into())));
        let mut v = Value::String("h".into());
        at_pointer(&mut v, &["users", "0", "password_hash"].map(String::from));
        assert_eq!(v, Value::String(REDACTED.into()));
        let mut v = Value::String("ip".into());
        at_pointer(&mut v, &["params", "key"].map(String::from));
        assert_eq!(v, Value::String("ip".into()));
    }

    #[test]
    fn restores_by_member_name_and_index() {
        let old = parse(
            r#"{"users": [{"name": "a", "password_hash": "ha"}, {"name": "b", "password_hash": "hb"}],
                "list": [{"secret": "s0"}, {"secret": "s1"}], "token": "t"}"#,
        );
        let mut new = parse(
            r#"{"users": [{"name": "b", "password_hash": "[redacted]"}, {"name": "c", "password_hash": "new"}],
                "list": [{"secret": "[redacted]"}, {"secret": "[redacted]"}], "token": "[redacted]"}"#,
        );
        assert_eq!(restore(&mut new, Some(&old), ""), Ok(true));
        let want = parse(
            r#"{"users": [{"name": "b", "password_hash": "hb"}, {"name": "c", "password_hash": "new"}],
                "list": [{"secret": "s0"}, {"secret": "s1"}], "token": "t"}"#,
        );
        assert_eq!(new, want);
        let mut plain = parse(r#"{"token": "x"}"#);
        assert_eq!(restore(&mut plain, Some(&old), ""), Ok(false));
        let mut none = parse(r#"{"users": [{"name": "z", "password_hash": "[redacted]"}]}"#);
        let e = restore(&mut none, Some(&old), "").unwrap_err();
        assert!(e.starts_with("/users/0/password_hash is [redacted]"), "{e}");
        let mut root = Value::String(REDACTED.into());
        assert!(restore(&mut root, None, "/a~1b").is_err());
    }
}
