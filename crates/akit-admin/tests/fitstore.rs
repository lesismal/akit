//! The encrypted store against fit's own code (I93): the stores in `tests/fitstore` were written
//! by fit's `admin/store` package, of every version, one with fit's own Argon2id settings (time
//! 3, 64 MiB, 4 lanes) and an export, by `tests/fitstore/akit_vectors_test.go` run inside fit's
//! package at the compatibility gate's fit pin (`TestAkitVectors`), `vectors.json` saying what
//! each holds. akit opens each and finds it; akit writes one back and keeps what it does not read
//! (the sections). With `AKIT_FITSTORE_OUT=<dir>`, `akit_writes_for_fit` writes akit's stores and
//! a `manifest.json` there for the same Go file's `TestAkitWrote` to open with fit's code
//! (`AKIT_WROTE_DIR=<dir> go test -run TestAkitWrote ./admin/store`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

use akit_admin::store::{self, DEFAULT_PARAMS, Params, Store, User};
use akit_config::value::{Value, parse, write_string};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fitstore")
}

struct Vector {
    file: String,
    password: String,
    export: bool,
    config: String,
    users: Vec<User>,
    sections: Vec<(String, Value)>,
}

fn string(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn vectors() -> Vec<Vector> {
    let text = std::fs::read_to_string(fixtures().join("vectors.json")).unwrap();
    let Value::Array(items) = parse(&text).unwrap() else {
        panic!("vectors.json is not a list")
    };
    items
        .iter()
        .map(|v| Vector {
            file: string(v, "file"),
            password: string(v, "password"),
            export: matches!(v.get("export"), Some(Value::Bool(true))),
            config: string(v, "config"),
            users: match v.get("users") {
                Some(Value::Array(us)) => us
                    .iter()
                    .map(|u| User {
                        username: string(u, "username"),
                        password_hash: string(u, "password_hash"),
                    })
                    .collect(),
                _ => Vec::new(),
            },
            sections: match v.get("sections") {
                Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => Vec::new(),
            },
        })
        .collect()
}

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("fitstore-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn json(raw: &[u8]) -> Value {
    parse(std::str::from_utf8(raw).unwrap()).unwrap()
}

/// Every store fit wrote opens in akit, with the config byte for byte, the users and the
/// sections; a wrong password does not open it.
#[test]
fn opens_what_fit_wrote() {
    let vs = vectors();
    assert_eq!(vs.len(), 5, "the vectors");
    for v in &vs {
        let path = fixtures().join(&v.file);
        if v.export {
            let file = std::fs::read(&path).unwrap();
            let (data, users) = store::unseal(&file, &v.password).unwrap();
            assert_eq!(String::from_utf8(data).unwrap(), v.config, "{}", v.file);
            assert_eq!(users, v.users, "{}", v.file);
            assert!(matches!(
                store::unseal(&file, "wrong"),
                Err(store::Error::Open(_))
            ));
            continue;
        }
        // Opened from a copy: opening never writes, but a test must not touch its fixtures.
        let d = tmp(&v.file);
        let copy = d.join(&v.file);
        std::fs::copy(&path, &copy).unwrap();
        let (s, data) = Store::open(&copy, &v.password).unwrap();
        assert_eq!(
            String::from_utf8(data.unwrap()).unwrap(),
            v.config,
            "{}",
            v.file
        );
        assert_eq!(s.users(), v.users, "{}", v.file);
        let names: Vec<&str> = s.section_names().collect();
        let want: Vec<&str> = v.sections.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, want, "{}", v.file);
        for (k, want) in &v.sections {
            assert_eq!(&json(s.section(k).unwrap()), want, "{} {k}", v.file);
        }
        assert!(
            matches!(Store::open(&copy, "wrong"), Err(store::Error::Open(_))),
            "{}",
            v.file
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// A store fit wrote with sections, written by akit, keeps them as they were, and its users.
#[test]
fn keeps_what_it_does_not_read() {
    let v = vectors()
        .into_iter()
        .find(|v| v.file == "v3.store")
        .unwrap();
    let d = tmp("keeps");
    let path = d.join("v3.store");
    std::fs::copy(fixtures().join("v3.store"), &path).unwrap();
    let (mut s, _) = Store::open(&path, &v.password).unwrap();
    s.write(br#"{"routes":[]}"#.to_vec()).unwrap();
    let (o, data) = Store::open(&path, &v.password).unwrap();
    assert_eq!(data.unwrap(), br#"{"routes":[]}"#);
    assert_eq!(o.users(), v.users);
    for (k, want) in &v.sections {
        assert_eq!(&json(o.section(k).unwrap()), want, "{k}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// Writes akit's stores, and what each holds, for fit's `TestAkitWrote`; nothing without
/// `AKIT_FITSTORE_OUT`. Each is opened by akit too.
#[test]
fn akit_writes_for_fit() {
    let Some(out) = std::env::var_os("AKIT_FITSTORE_OUT").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let cheap = Params {
        time: 1,
        memory_kib: 64,
        threads: 1,
    };
    let config = "{\"listeners\": [{\"name\": \"web\", \"address\": \":8080\"}], \"routes\": [{\"handler\": {\"module\": \"respond\", \"params\": {\"body\": \"h\\u00e9 \u{e9} <&>\"}}}]}";
    let users = vec![
        User {
            username: "ada".into(),
            password_hash: "$2a$04$ada".into(),
        },
        User {
            username: "b\u{f6}b".into(),
            password_hash: "$2a$04$bob\"q\\".into(),
        },
    ];
    let mut manifest = String::from("[");
    let mut entry =
        |file: &str, pw: &str, export: bool, users: &[User], sections: &[(&str, &str)]| {
            if manifest.len() > 1 {
                manifest.push(',');
            }
            manifest.push_str("\n{\"file\":");
            write_string(&mut manifest, file);
            manifest.push_str(",\"password\":");
            write_string(&mut manifest, pw);
            manifest.push_str(&format!(",\"export\":{export},\"config\":"));
            write_string(&mut manifest, config);
            manifest.push_str(",\"users\":[");
            for (i, u) in users.iter().enumerate() {
                if i > 0 {
                    manifest.push(',');
                }
                manifest.push_str("{\"username\":");
                write_string(&mut manifest, &u.username);
                manifest.push_str(",\"password_hash\":");
                write_string(&mut manifest, &u.password_hash);
                manifest.push('}');
            }
            manifest.push_str("],\"sections\":{");
            for (i, (k, v)) in sections.iter().enumerate() {
                if i > 0 {
                    manifest.push(',');
                }
                write_string(&mut manifest, k);
                manifest.push(':');
                manifest.push_str(v);
            }
            manifest.push_str("}}");
        };
    for (file, pw, params, sections) in [
        ("akit-v2.store", "akit-v2", cheap, &[][..]),
        (
            "akit-v3.store",
            "akit-v3",
            cheap,
            &[
                ("assistant", r#"{"model": "m"}"#),
                ("cluster", r#"{"version":2}"#),
            ][..],
        ),
        (
            "akit-default.store",
            "akit-default",
            DEFAULT_PARAMS,
            &[][..],
        ),
    ] {
        let path = out.join(file);
        let _ = std::fs::remove_file(&path);
        let mut s = Store::create_with(&path, pw, params).unwrap();
        s.write(config.as_bytes().to_vec()).unwrap();
        s.set_users(users.clone()).unwrap();
        for (k, v) in sections {
            s.set_section(k, v.as_bytes()).unwrap();
        }
        let (o, data) = Store::open(&path, pw).unwrap();
        assert_eq!(data.unwrap(), config.as_bytes());
        assert_eq!(o.users(), users);
        entry(file, pw, false, &users, sections);
    }
    let sealed = store::seal_with("akit-export", config.as_bytes(), &users, cheap).unwrap();
    std::fs::write(out.join("akit-export.store"), sealed).unwrap();
    entry("akit-export.store", "akit-export", true, &users, &[]);
    manifest.push_str("\n]\n");
    std::fs::write(out.join("manifest.json"), manifest).unwrap();
}
