//! Where the config akit runs comes from when it is kept in the encrypted store (I93b, I93c), as
//! fit's `cli/store.go`: the config files, a `.store` among them aside; the store that goes with
//! them, the one `admin.store` names, a `.store` file `-c` gives, or the one beside the files
//! (`site.json` gives `site.store`) once it exists or `-admin` asks for one; its password, from
//! `admin.store`, or else `$AKIT_STORE_PASSWORD` or the key file beside it (`<store>.key`, made
//! 0600 with a random key when neither it nor the store exists). With a store, the config runs
//! from what it holds merged onto the files', its values winning; the files' `admin` is put in
//! and never kept. Opening a store derives its key: on the control thread, at start.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use akit_admin::bootstrap::{merge_config, split_admin, with_admin, with_console_users};
use akit_admin::store::{Store, User};
use akit_config::value::{Value, parse};

/// The environment variable holding the password of a store `admin.store` does not describe:
/// fit's `FIT_STORE_PASSWORD`, renamed (decision 037). Without it, the password is the key file
/// beside the store, the store's path with `.key` added.
pub(crate) const STORE_PASSWORD_ENV: &str = "AKIT_STORE_PASSWORD";

/// The store `-c` names none of and `admin.store` says nothing of, with no config file: fit's
/// `fit.store`, renamed as `akit.json` is.
const DEFAULT_STORE: &str = "akit.store";

/// Where the store is when `admin.store` does not say: a `.store` file given with `-c`, which is
/// used (`explicit`), or the store beside the config files, used when it exists or `create` is
/// set (`akit run -admin`, `akit store import`).
#[derive(Debug, Clone)]
pub(crate) struct Spec {
    pub(crate) path: String,
    pub(crate) explicit: bool,
    pub(crate) create: bool,
}

/// An open store, shared by the control thread (the config written), the console's pool (the
/// users) and SIGHUP (read again).
pub(crate) type Shared = Arc<Mutex<Store>>;

pub(crate) fn lock(s: &Shared) -> MutexGuard<'_, Store> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A config kept in a store: the store, the bootstrap file's `admin` (with the store's users'
/// passwords), and the base config, without `admin`, that akit runs from.
pub(crate) struct Boot {
    pub(crate) store: Shared,
    /// The store's file.
    pub(crate) path: String,
    pub(crate) admin: Option<String>,
    pub(crate) base: String,
    /// The store did not exist: its base config is the files', not written yet.
    pub(crate) seeded: bool,
    /// The store's config, merged onto the files', differs from what it held.
    pub(crate) merged: bool,
    /// What the store held, before the merge.
    pub(crate) stored: Option<String>,
    /// The console users the store keeps beside the config.
    pub(crate) users: Vec<User>,
}

impl Boot {
    /// The config akit runs: the base config with `admin`.
    pub(crate) fn full(&self) -> String {
        match &self.admin {
            Some(a) => with_admin(&self.base, a),
            None => self.base.clone(),
        }
    }
}

fn file_exists(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.is_dir())
}

/// The config paths without the `.store` file among them, and that file; more than one is an
/// error.
pub(crate) fn split_store_paths(paths: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let mut files = Vec::new();
    let mut store: Option<String> = None;
    for p in paths {
        let is_store = Path::new(p)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("store"));
        if !is_store {
            files.push(p.clone());
            continue;
        }
        if let Some(first) = &store {
            return Err(format!("-c names two stores, {first} and {p}; give one"));
        }
        store = Some(p.clone());
    }
    Ok((files, store))
}

/// The store kept beside the config files when `-c` names none and `admin.store` says nothing:
/// the first's path with `.store` for its extension (a directory's with `.store` added), or
/// `akit.store` with no file.
pub(crate) fn default_store_path(files: &[String]) -> String {
    let Some(first) = files.first().filter(|f| f.as_str() != "-") else {
        return DEFAULT_STORE.to_owned();
    };
    let p = Path::new(first);
    if p.is_dir() {
        return format!("{}.store", first.trim_end_matches('/'));
    }
    p.with_extension("store").to_string_lossy().into_owned()
}

/// The config files `paths` name, a `.store` among them aside, and which store goes with them.
/// `given` is whether `-c` was given at all: without it, when there is no `akit.json` but there is
/// an `akit.store`, or `create` is set, akit runs from the store alone (no files).
pub(crate) fn load_boot(
    paths: &[String],
    given: bool,
    create: bool,
) -> Result<(Vec<String>, Spec), String> {
    let (mut files, store) = split_store_paths(paths)?;
    if !given
        && files.first().is_some_and(|f| !file_exists(f))
        && (create || file_exists(DEFAULT_STORE))
    {
        files.clear();
    }
    let spec = Spec {
        path: store.clone().unwrap_or_else(|| default_store_path(&files)),
        explicit: store.is_some(),
        create,
    };
    Ok((files, spec))
}

/// A string member of `v`, `None` when it has none (or it is not a string).
fn string(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The path and password of the store that goes with `admin` (the config's, as JSON; `None` when
/// it has none) and `spec`, or `None` when there is none.
///
/// # Errors
///
/// A password that cannot be read, as fit's.
pub(crate) fn resolve_store(
    admin: Option<&str>,
    spec: &Spec,
) -> Result<Option<(String, String)>, String> {
    let described = admin
        .and_then(|a| parse(a).ok())
        .and_then(|a| a.get("store").cloned())
        .filter(|s| string(s, "path").is_some_and(|p| !p.is_empty()));
    if let Some(s) = described {
        let path = if spec.explicit {
            spec.path.clone()
        } else {
            string(&s, "path").unwrap_or_default()
        };
        return Ok(Some((path, store_password(&s)?)));
    }
    if !spec.path.is_empty() && (spec.explicit || spec.create || file_exists(&spec.path)) {
        let pw = implicit_password(&spec.path)?;
        return Ok(Some((spec.path.clone(), pw)));
    }
    Ok(None)
}

/// The password of the store `admin.store`, `s`, describes, from where it says, as fit's
/// `storePassword`: exactly one of `password`, `password_file` and `password_env`.
fn store_password(s: &Value) -> Result<String, String> {
    let set = |k: &str| string(s, k).filter(|v| !v.is_empty());
    match (set("password"), set("password_file"), set("password_env")) {
        (Some(p), None, None) => Ok(p),
        (None, Some(f), None) => {
            let data = std::fs::read_to_string(&f)
                .map_err(|e| format!("admin.store.password_file: {f}: {e}"))?;
            let p = data.trim();
            if p.is_empty() {
                return Err(format!("admin.store.password_file {f} is empty"));
            }
            Ok(p.to_owned())
        }
        (None, None, Some(v)) => match std::env::var(&v) {
            Ok(p) if !p.is_empty() => Ok(p),
            _ => Err(format!(
                "admin.store.password_env: ${v} is empty or not set"
            )),
        },
        _ => Err(
            "admin.store: set exactly one of password, password_file and password_env".to_owned(),
        ),
    }
}

/// The password of the store at `path`, which `admin.store` does not describe:
/// `$AKIT_STORE_PASSWORD`, or the key file beside it, made 0600 with a random key when neither it
/// nor the store exists yet. The key is never printed.
fn implicit_password(path: &str) -> Result<String, String> {
    if let Ok(p) = std::env::var(STORE_PASSWORD_ENV) {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    let key_path = format!("{path}.key");
    match std::fs::read_to_string(&key_path) {
        Ok(data) => {
            let p = data.trim();
            if p.is_empty() {
                return Err(format!("the store's key file {key_path} is empty"));
            }
            return Ok(p.to_owned());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("the store's key file: {e}")),
    }
    if file_exists(path) {
        return Err(format!(
            "the store {path} has no key file {key_path} beside it: copy it there, or set \
             ${STORE_PASSWORD_ENV} to the store's password"
        ));
    }
    let key = akit_admin::store::new_key().map_err(|e| format!("the store's key file: {e}"))?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key_path)
        .map_err(|e| format!("the store's key file: {e}"))?;
    let written = f
        .write_all(format!("{key}\n").as_bytes())
        .and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&key_path);
        return Err(format!("the store's key file: {e}"));
    }
    Ok(key)
}

/// Whether `a` and `b` are the same JSON value.
fn json_equal(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Opens the store of the config `data`, standard JSON: the one `admin.store` names (at `spec`'s
/// path when it is explicit), or the one `spec` describes; `None` when there is none. When the
/// store holds a config, the config's members beside `admin` are its seed, and what the store
/// holds is merged onto them, the store's winning. Derives the store's key: control plane only.
///
/// # Errors
///
/// The store does not open (a wrong password, a changed byte), its password cannot be read, or
/// what it holds does not merge.
pub(crate) fn open_store(data: &str, spec: &Spec) -> Result<Option<Boot>, String> {
    // A config that is not an object: loading it says what is wrong.
    let Ok((admin, rest)) = split_admin(data) else {
        return Ok(None);
    };
    let Some((path, password)) = resolve_store(admin.as_deref(), spec)? else {
        return Ok(None);
    };
    let (st, held) = Store::open(&path, &password).map_err(|e| e.to_string())?;
    let users = st.users().to_vec();
    let stored = match held {
        None => None,
        Some(h) => Some(
            String::from_utf8(h)
                .map_err(|_| format!("the store {path} holds a config that is not UTF-8"))?,
        ),
    };
    // The store keeps the password hashes of the console users changed since the bootstrap file
    // was written: they replace its own, so that a restart logs in with them.
    let admin = admin.map(|a| with_console_users(&a, &users));
    let (base, seeded, merged) = match &stored {
        None => (rest, true, false),
        Some(held) => {
            let m = merge_config(&rest, held)
                .map_err(|e| format!("merging the store {path} onto the config files: {e}"))?;
            if json_equal(&m, held) {
                (held.clone(), false, false)
            } else {
                (m, false, true)
            }
        }
    };
    Ok(Some(Boot {
        store: Arc::new(Mutex::new(st)),
        path,
        admin,
        base,
        seeded,
        merged,
        stored,
        users,
    }))
}

/// A store written from the files at start, put back as it was when the start fails after it:
/// a config that does not compile, or a port another process holds, must not stay sealed in the
/// store, whose values would then win over the fixed files at the next start. fit's deferred
/// restore in `serve`.
pub(crate) struct Restore {
    path: String,
    /// What the file held, `None` when there was none; not restored when it could not be read.
    old: Option<Option<Vec<u8>>>,
    armed: bool,
}

impl Restore {
    /// Remembers the store's file at `path` as it is now.
    pub(crate) fn of(path: &str) -> Restore {
        let old = match std::fs::read(path) {
            Ok(b) => Some(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
            Err(_) => None,
        };
        Restore {
            path: path.to_owned(),
            old,
            armed: true,
        }
    }

    /// The start succeeded: the file stays as it is now.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match &self.old {
            Some(Some(b)) => {
                let _ = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .create(true)
                    .mode(0o600)
                    .open(&self.path)
                    .and_then(|mut f| f.write_all(b).and_then(|()| f.sync_all()));
            }
            Some(None) => {
                let _ = std::fs::remove_file(&self.path);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn store_paths() {
        let p = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let (files, st) = split_store_paths(&p(&["a.json", "b.STORE", "c.yaml"])).unwrap();
        assert_eq!(files, p(&["a.json", "c.yaml"]));
        assert_eq!(st.as_deref(), Some("b.STORE"));
        assert!(split_store_paths(&p(&["a.store", "b.store"])).is_err());
        assert_eq!(
            default_store_path(&p(&["/etc/akit/site.json"])),
            "/etc/akit/site.store"
        );
        assert_eq!(
            default_store_path(&p(&["site.conf", "x.yaml"])),
            "site.store"
        );
        assert_eq!(default_store_path(&[]), "akit.store");
        assert_eq!(default_store_path(&p(&["-"])), "akit.store");
        let d = std::env::temp_dir();
        let dir = d.to_string_lossy().trim_end_matches('/').to_owned();
        assert_eq!(
            default_store_path(std::slice::from_ref(&dir)),
            format!("{dir}.store")
        );
    }

    #[test]
    fn passwords_from_admin_store() {
        let s = parse(r#"{"path": "x.store", "password": "pw"}"#).unwrap();
        assert_eq!(store_password(&s).unwrap(), "pw");
        let s = parse(r#"{"path": "x.store", "password": "pw", "password_env": "X"}"#).unwrap();
        assert!(store_password(&s).unwrap_err().contains("exactly one"));
        let s = parse(r#"{"path": "x.store", "password_env": "AKIT_TEST_NOT_SET_EVER"}"#).unwrap();
        assert!(store_password(&s).unwrap_err().contains("empty or not set"));
    }
}
