//! The config kept in the encrypted store (I93b, I93c), as fit's `admin/bootstrap.go`: the
//! bootstrap file's `admin`, which never goes into the store, split from the rest and put back
//! into every config as it loads; the store's config merged onto the config files' at a start;
//! the console users whose passwords changed, kept in the store, put into the bootstrap file's
//! `admin`; and `akit run -admin`'s defaults.

use akit_config::value::{Value, parse};
use akit_config::{Error, Errors};

use crate::store::User;

/// The config read from the store, as SIGHUP reads it again: fit's `SourceStore`.
pub const SOURCE_STORE: &str = "store";

/// The console user `akit run -admin` adds when the config names none, with a temporary
/// password, [`DEFAULT_CONSOLE_PASSWORD`]: fit's.
pub const DEFAULT_CONSOLE_USER: &str = "admin";
/// The temporary password of [`DEFAULT_CONSOLE_USER`]: fit's.
pub const DEFAULT_CONSOLE_PASSWORD: &str = "admin";

/// What a config with `admin` of its own is told while `admin` comes from the bootstrap file:
/// fit's `errAdminInStore`, in akit's words.
pub const ADMIN_IN_STORE: &str = "admin is set in the config file akit started with, while the \
     rest of the config is kept in the store; it changes when akit restarts";

/// [`ADMIN_IN_STORE`] as a config's errors.
#[must_use]
pub fn admin_in_store() -> Errors {
    Errors(vec![Error {
        file: None,
        path: "admin".to_owned(),
        line: 0,
        col: 0,
        msg: ADMIN_IN_STORE.to_owned(),
    }])
}

/// `data` as a JSON object's members: "a config is a JSON object" when it is not, as fit's
/// `decodeObject`.
fn object(data: &str) -> Result<Vec<(String, Value)>, String> {
    match parse(data) {
        Ok(Value::Object(ms)) => Ok(ms),
        Ok(_) => Err("a config is a JSON object".to_owned()),
        Err(e) => Err(e
            .as_slice()
            .first()
            .map_or_else(|| "the config is not JSON".to_owned(), ToString::to_string)),
    }
}

/// A value written out, without the newline [`Value::to_json`] ends with.
fn encoded(v: &Value) -> String {
    let mut s = v.to_json();
    s.truncate(s.trim_end().len());
    s
}

/// Splits `data`, a config as standard JSON, into its `admin`, as JSON (`None` when it has
/// none), and the rest, written out again without it; `data` itself, as it was, when it has no
/// `admin`.
///
/// # Errors
///
/// A config that is not a JSON object.
pub fn split_admin(data: &str) -> Result<(Option<String>, String), String> {
    let mut ms = match parse(data) {
        Ok(Value::Object(ms)) => ms,
        Ok(_) => return Err("the config is not a JSON object".to_owned()),
        Err(_) => return Err("the config is not JSON".to_owned()),
    };
    let Some(i) = ms.iter().position(|(k, _)| k == "admin") else {
        return Ok((None, data.to_owned()));
    };
    let (_, admin) = ms.remove(i);
    Ok((Some(encoded(&admin)), Value::Object(ms).to_json()))
}

/// `data`, a config as a JSON object, with `admin` added as its last member, after a line break,
/// so that every position in `data` stays where it was and its errors are reported where they
/// were written; `data` as it is when it is not an object (loading it says so).
#[must_use]
pub fn with_admin(data: &str, admin: &str) -> String {
    let (Some(start), Some(end)) = (data.find('{'), data.rfind('}')) else {
        return data.to_owned();
    };
    if end < start {
        return data.to_owned();
    }
    let inner = data.get(start.saturating_add(1)..end).unwrap_or("");
    let sep = if inner.trim().is_empty() { "\n" } else { ",\n" };
    let mut out = String::with_capacity(data.len().saturating_add(admin.len()).saturating_add(16));
    out.push_str(data.get(..end).unwrap_or(""));
    out.push_str(sep);
    out.push_str("\"admin\": ");
    out.push_str(admin);
    out.push_str("\n}");
    out
}

/// Whether `data`, a config as standard JSON, sets `admin`.
#[must_use]
pub fn has_admin(data: &str) -> bool {
    matches!(parse(data), Ok(v @ Value::Object(_)) if v.get("admin").is_some())
}

/// `admin`, the bootstrap file's `admin` as JSON, with the `password_hash` of each user of
/// `admin.console.users` replaced by the one `users` gives for its name; a user `admin` does not
/// list is left out. `admin` as it was when nothing is replaced, or it does not parse or sets no
/// console, as fit's `WithConsoleUsers`.
#[must_use]
pub fn with_console_users(admin: &str, users: &[User]) -> String {
    if users.is_empty() {
        return admin.to_owned();
    }
    let Ok(mut root) = parse(admin) else {
        return admin.to_owned();
    };
    let mut changed = false;
    if let Value::Object(a) = &mut root {
        if let Some((_, Value::Object(c))) = a.iter_mut().find(|(k, _)| k == "console") {
            if let Some((_, Value::Array(list))) = c.iter_mut().find(|(k, _)| k == "users") {
                for u in list.iter_mut() {
                    let Value::Object(fields) = u else { continue };
                    let Some(name) = fields.iter().find_map(|(k, v)| match (k.as_str(), v) {
                        ("username", Value::String(s)) => Some(s.clone()),
                        _ => None,
                    }) else {
                        continue;
                    };
                    let Some(user) = users.iter().find(|u| u.username == name) else {
                        continue;
                    };
                    let hash = Value::String(user.password_hash.clone());
                    match fields.iter_mut().find(|(k, _)| k == "password_hash") {
                        Some(slot) => slot.1 = hash,
                        None => fields.push(("password_hash".to_owned(), hash)),
                    }
                    changed = true;
                }
            }
        }
    }
    if changed {
        encoded(&root)
    } else {
        admin.to_owned()
    }
}

/// `over`, a config as a JSON object, merged onto `base`, another: an object member both set is
/// merged the same way, member by member, and every other value `over` sets replaces `base`'s,
/// lists whole; `base`'s members first, in their order, then those only `over` sets. fit's
/// `MergeConfig`.
///
/// # Errors
///
/// Either is not a JSON object.
pub fn merge_config(base: &str, over: &str) -> Result<String, String> {
    let b = object(base)?;
    let o = object(over)?;
    Ok(Value::Object(merge_objects(b, o)).to_json())
}

fn merge_objects(
    mut base: Vec<(String, Value)>,
    over: Vec<(String, Value)>,
) -> Vec<(String, Value)> {
    for (k, ov) in over {
        match base.iter_mut().find(|(bk, _)| *bk == k) {
            Some(slot) => {
                let old = std::mem::replace(&mut slot.1, Value::Null);
                slot.1 = match (old, ov) {
                    (Value::Object(bo), Value::Object(oo)) => Value::Object(merge_objects(bo, oo)),
                    (_, ov) => ov,
                };
            }
            None => base.push((k, ov)),
        }
    }
    base
}

/// The member `k` of `o`, an object at `path`, added empty when `o` has none (or it is `null`).
fn member_object<'a>(
    o: &'a mut Vec<(String, Value)>,
    k: &str,
    path: &str,
) -> Result<&'a mut Vec<(String, Value)>, Errors> {
    let i = match o.iter().position(|(m, _)| m == k) {
        Some(i) => i,
        None => {
            o.push((k.to_owned(), Value::Object(Vec::new())));
            o.len().saturating_sub(1)
        }
    };
    let Some(slot) = o.get_mut(i) else {
        return Err(not_object(path));
    };
    if slot.1 == Value::Null {
        slot.1 = Value::Object(Vec::new());
    }
    match &mut slot.1 {
        Value::Object(m) => Ok(m),
        _ => Err(not_object(path)),
    }
}

fn not_object(path: &str) -> Errors {
    Errors(vec![Error {
        file: None,
        path: path.to_owned(),
        line: 0,
        col: 0,
        msg: "must be an object".to_owned(),
    }])
}

/// `data`, a config as standard JSON, with `admin` and `admin.console` set, empty where the
/// config leaves them out (their defaults), as `akit run -admin` asks; when `admin.console`
/// names no user, it gets [`DEFAULT_CONSOLE_USER`] with `hash`, the bcrypt hash of
/// [`DEFAULT_CONSOLE_PASSWORD`], and that user is returned too, its password to be changed in
/// time. fit's `WithDefaultConsole`.
///
/// # Errors
///
/// A config that is not a JSON object, or whose `admin` or `admin.console` is not one.
pub fn with_default_console(data: &str, hash: &str) -> Result<(String, Option<String>), Errors> {
    let mut root = object(data).map_err(|msg| {
        Errors(vec![Error {
            file: None,
            path: String::new(),
            line: 0,
            col: 0,
            msg,
        }])
    })?;
    let a = member_object(&mut root, "admin", "admin")?;
    let c = member_object(a, "console", "admin.console")?;
    let mut temp = None;
    let users = c.iter().position(|(k, _)| k == "users");
    let empty = users.and_then(|i| c.get(i)).is_none_or(|(_, v)| {
        matches!(v, Value::Null) || matches!(v, Value::Array(l) if l.is_empty())
    });
    if empty {
        let user = Value::Object(vec![
            (
                "username".to_owned(),
                Value::String(DEFAULT_CONSOLE_USER.to_owned()),
            ),
            ("password_hash".to_owned(), Value::String(hash.to_owned())),
        ]);
        match users.and_then(|i| c.get_mut(i)) {
            Some(slot) => slot.1 = Value::Array(vec![user]),
            None => c.push(("users".to_owned(), Value::Array(vec![user]))),
        }
        temp = Some(DEFAULT_CONSOLE_USER.to_owned());
    }
    Ok((Value::Object(root).to_json(), temp))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn v(s: &str) -> Value {
        parse(s).unwrap()
    }

    /// fit's `TestMergeConfig`.
    #[test]
    fn merges_onto_the_files() {
        let got = merge_config(
            r#"{"a": {"x": 1, "y": [1, 2]}, "b": 1, "c": "file"}"#,
            r#"{"a": {"y": [3], "z": true}, "c": "store", "d": null}"#,
        )
        .unwrap();
        assert_eq!(
            v(&got),
            v(r#"{"a": {"x": 1, "y": [3], "z": true}, "b": 1, "c": "store", "d": null}"#)
        );
        assert!(merge_config("[1]", "{}").is_err());
        assert!(merge_config("{}", "x").is_err());
    }

    /// fit's `TestWithDefaultConsole`.
    #[test]
    fn default_console() {
        let (out, temp) = with_default_console(r#"{"listeners": []}"#, "HASH").unwrap();
        assert_eq!(temp.as_deref(), Some(DEFAULT_CONSOLE_USER));
        let users = v(&out);
        let u = users
            .get("admin")
            .and_then(|a| a.get("console"))
            .and_then(|c| c.get("users"))
            .unwrap();
        assert_eq!(u, &v(r#"[{"username": "admin", "password_hash": "HASH"}]"#));
        // A config with users of its own keeps them, and no password is temporary.
        let input = r#"{"admin": {"address": "127.0.0.1:0", "console": {"users": [{"username": "ada", "password_hash": "H"}]}}}"#;
        let (out, temp) = with_default_console(input, "HASH").unwrap();
        assert!(temp.is_none() && out.contains("\"ada\"") && !out.contains("HASH"));
        // An empty list, or null, is no user.
        let (out, temp) =
            with_default_console(r#"{"admin": {"console": {"users": []}}}"#, "HASH").unwrap();
        assert!(temp.is_some() && out.contains("HASH"));
        let (_, temp) = with_default_console(r#"{"admin": null}"#, "HASH").unwrap();
        assert!(temp.is_some());
        let e = with_default_console(r#"{"admin": 1}"#, "HASH").unwrap_err();
        assert_eq!(e.0[0].path, "admin");
        let e = with_default_console(r#"{"admin": {"console": []}}"#, "HASH").unwrap_err();
        assert_eq!(e.0[0].path, "admin.console");
    }

    /// fit's `TestWithConsoleUsers`.
    #[test]
    fn console_users_merged() {
        let admin = r#"{"address": "127.0.0.1:0",
  "console": {"address": "127.0.0.1:0", "users": [
    {"username": "ada", "password_hash": "old-ada"},
    {"username": "bob", "password_hash": "old-bob"}]}}"#;
        let users = [
            User {
                username: "bob".into(),
                password_hash: "new-bob".into(),
            },
            User {
                username: "nobody".into(),
                password_hash: "new-nobody".into(),
            },
        ];
        let got = with_console_users(admin, &users);
        assert_eq!(
            v(&got),
            v(
                r#"{"address": "127.0.0.1:0", "console": {"address": "127.0.0.1:0", "users": [
              {"username": "ada", "password_hash": "old-ada"},
              {"username": "bob", "password_hash": "new-bob"}]}}"#
            )
        );
        // The members keep their order.
        assert!(got.find("\"address\"").unwrap() < got.find("\"console\"").unwrap());
        assert!(got.find("\"ada\"").unwrap() < got.find("\"bob\"").unwrap());
        for (name, a, us) in [
            ("no users", admin, &[][..]),
            (
                "nothing to replace",
                admin,
                &[User {
                    username: "nobody".into(),
                    password_hash: "x".into(),
                }][..],
            ),
            ("no console", r#"{"address": "127.0.0.1:0"}"#, &users[..]),
            ("not JSON", "address: :8080", &users[..]),
            ("not an object", "[1]", &users[..]),
            ("console not an object", r#"{"console": true}"#, &users[..]),
            (
                "users not an array",
                r#"{"console": {"users": {}}}"#,
                &users[..],
            ),
            (
                "a user not an object",
                r#"{"console": {"users": [1]}}"#,
                &users[..],
            ),
            (
                "a name not a string",
                r#"{"console": {"users": [{"username": 1}]}}"#,
                &users[..],
            ),
        ] {
            assert_eq!(with_console_users(a, us), a, "{name}");
        }
    }

    #[test]
    fn admin_split_and_put_back() {
        let data = "{\"listeners\": [],\n \"admin\": {\"address\": \"127.0.0.1:0\"}}";
        let (admin, rest) = split_admin(data).unwrap();
        let admin = admin.unwrap();
        assert!(!has_admin(&rest) && has_admin(data));
        assert_eq!(v(&admin), v(r#"{"address": "127.0.0.1:0"}"#));
        let full = with_admin(&rest, &admin);
        // Every position of the rest stays where it was.
        assert!(full.starts_with(rest.trim_end().trim_end_matches('}')));
        assert_eq!(v(&full), v(data));
        // No admin: the data as it was.
        let (none, same) = split_admin("{\"a\": 1}").unwrap();
        assert!(none.is_none() && same == "{\"a\": 1}");
        assert_eq!(v(&with_admin("{}", "{}")), v(r#"{"admin": {}}"#));
        assert_eq!(v(&with_admin("{ \n}", "1")), v(r#"{"admin": 1}"#));
        assert_eq!(with_admin("[1]", "{}"), "[1]");
        assert!(split_admin("[1]").is_err() && split_admin("x").is_err());
    }
}
