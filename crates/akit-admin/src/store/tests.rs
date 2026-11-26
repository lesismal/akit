//! fit's `admin/store` tests (`store_test.go`, `seal_test.go`, `sections_test.go`), ported. Like
//! fit's, they make Argon2id cheap; the format records the settings.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;

const CHEAP: Params = Params {
    time: 1,
    memory_kib: 64,
    threads: 1,
};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("akit-store-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(path: &Path, pw: &str) -> Result<(Store, Option<Vec<u8>>), Error> {
    Store::open_with(path, pw, CHEAP)
}

fn users(list: &[(&str, &str)]) -> Vec<User> {
    list.iter()
        .map(|(u, h)| User {
            username: (*u).into(),
            password_hash: (*h).into(),
        })
        .collect()
}

#[test]
fn write_and_open() {
    let d = dir("write");
    let path = d.join("akit.store");
    let (mut s, data) = open(&path, "secret").unwrap();
    assert!(data.is_none(), "a new store holds a config");
    let want = br#"{"listeners":[]}"#.to_vec();
    s.write(want.clone()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the store's mode");
    let file = std::fs::read(&path).unwrap();
    let want_file = br#"{"config":{"listeners":[]}}"#;
    assert_eq!(&file[..8], MAGIC);
    assert_eq!(file[8], VERSION);
    assert_eq!(file.len(), HEADER_LEN + want_file.len() + 16);
    let (_, got) = open(&path, "secret").unwrap();
    assert_eq!(got.unwrap(), want);
    // Each write takes a new nonce; the salt stays.
    s.write(want).unwrap();
    let again = std::fs::read(&path).unwrap();
    assert_eq!(
        again[SALT_OFFSET..NONCE_OFFSET],
        file[SALT_OFFSET..NONCE_OFFSET],
        "the salt changed"
    );
    assert_ne!(
        again[NONCE_OFFSET..HEADER_LEN],
        file[NONCE_OFFSET..HEADER_LEN],
        "the nonce did not change"
    );
    assert_eq!(
        std::fs::read_dir(&d).unwrap().count(),
        1,
        "files left beside the store"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn wrong_password_or_changed_byte() {
    let d = dir("wrong");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    s.write(b"{}".to_vec()).unwrap();
    assert!(matches!(open(&path, "wrong"), Err(Error::Open(_))));
    let file = std::fs::read(&path).unwrap();
    for i in [
        9 + 1 + 4 + 4 + 1,
        SALT_OFFSET,
        HEADER_LEN - 1,
        HEADER_LEN,
        file.len() - 1,
    ] {
        let mut changed = file.clone();
        changed[i] ^= 1;
        std::fs::write(&path, &changed).unwrap();
        assert!(open(&path, "secret").is_err(), "byte {i} changed: opened");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn not_a_store() {
    let d = dir("not");
    for (name, content) in [
        (
            "json",
            r#"{"listeners":[{"name":"web","address":":8080"}],"routes":[]}"#,
        ),
        ("short", "FITSTORE"),
    ] {
        let path = d.join(name);
        std::fs::write(&path, content).unwrap();
        assert!(
            matches!(open(&path, "secret"), Err(Error::NotStore(_))),
            "{name}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn header_bounds() {
    let d = dir("bounds");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    s.params = Params {
        time: 1,
        memory_kib: MAX_MEMORY_KIB + 1,
        threads: 1,
    };
    let mut file = s.seal_payload(VERSION, b"{}".to_vec()).unwrap();
    std::fs::write(&path, &file).unwrap();
    let e = open(&path, "secret").unwrap_err();
    assert!(
        matches!(&e, Error::Other(m) if m.contains("past what akit takes")),
        "{e}"
    );
    file[MAGIC.len()] = VERSION_SECTIONS + 1;
    std::fs::write(&path, &file).unwrap();
    assert!(open(&path, "secret").is_err(), "an unknown version opened");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn read_after_another_write() {
    let d = dir("read");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    s.write(br#"{"a":1}"#.to_vec()).unwrap();
    // Another process imports a store: a new salt, the same password.
    let mut other = Store::create_with(&path, "secret", CHEAP).unwrap();
    other.write(br#"{"b":2}"#.to_vec()).unwrap();
    assert_eq!(s.read().unwrap(), br#"{"b":2}"#);
    s.write(br#"{"c":3}"#.to_vec()).unwrap();
    assert_eq!(other.read().unwrap(), br#"{"c":3}"#);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn empty_password() {
    assert!(open(&std::env::temp_dir().join("x"), "").is_err());
    assert!(Store::create_with("x", "", CHEAP).is_err());
}

#[test]
fn users_round_trip() {
    let d = dir("users");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    let cfg = br#"{"listeners":[]}"#.to_vec();
    s.write(cfg.clone()).unwrap();
    assert!(s.users().is_empty());
    let us = users(&[("ada", "$2a$04$ada"), ("bob", "$2a$04$bob")]);
    s.set_users(us.clone()).unwrap();
    assert_eq!(s.users(), us);
    // A fresh open returns the same config and users.
    let (mut s2, data) = open(&path, "secret").unwrap();
    assert_eq!(data.unwrap(), cfg);
    assert_eq!(s2.users(), us);
    // A write keeps them.
    let next = br#"{"listeners":[{"name":"web","address":":8080"}]}"#.to_vec();
    s2.write(next.clone()).unwrap();
    let (mut s3, data) = open(&path, "secret").unwrap();
    assert_eq!(data.unwrap(), next);
    assert_eq!(s3.users(), us);
    // set_users changes them, keeping the config.
    let one = users(&[("ada", "$2a$04$new")]);
    s3.set_users(one.clone()).unwrap();
    let (s4, data) = open(&path, "secret").unwrap();
    assert_eq!(data.unwrap(), next);
    assert_eq!(s4.users(), one);
    let _ = std::fs::remove_dir_all(&d);
}

/// A store keeps users only beside a config.
#[test]
fn set_users_without_config() {
    let d = dir("nousers");
    let (mut s, _) = open(&d.join("akit.store"), "secret").unwrap();
    assert!(s.set_users(users(&[("ada", "")])).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

/// A version 1 store, whose payload is the config alone, still opens, and its next write is
/// version 2.
#[test]
fn old_version_readable() {
    let d = dir("v1");
    let path = d.join("akit.store");
    let (s, _) = open(&path, "secret").unwrap();
    let want = br#"{"listeners":[]}"#.to_vec();
    std::fs::write(&path, s.seal_payload(VERSION_CONFIG, want.clone()).unwrap()).unwrap();
    let (mut old, data) = open(&path, "secret").unwrap();
    assert_eq!(data.unwrap(), want);
    assert!(old.users().is_empty());
    let us = users(&[("ada", "$2a$04$ada")]);
    old.set_users(us.clone()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap()[MAGIC.len()], VERSION);
    let (again, data) = open(&path, "secret").unwrap();
    assert_eq!(data.unwrap(), want);
    assert_eq!(again.users(), us);
    let _ = std::fs::remove_dir_all(&d);
}

/// The store embeds the config in a JSON envelope, so it refuses a config that is not JSON
/// rather than making a file it cannot read.
#[test]
fn write_not_json() {
    let d = dir("notjson");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    for bad in [
        "listeners: []",
        "{\"a\":1,}",
        "{} {}",
        "{\"a\":01}",
        "\"\u{1}\"",
    ] {
        let e = s.write(bad.as_bytes().to_vec()).unwrap_err();
        assert!(e.to_string().contains("not JSON"), "{bad:?}: {e}");
    }
    assert!(!path.exists(), "the store was written");
    let _ = std::fs::remove_dir_all(&d);
}

/// A file sealed with a password opens with it, with the users beside the config, and not with
/// another; one sealed with a store's key opens with the store's password.
#[test]
fn seal_unseal() {
    let us = users(&[("ada", "$2a$04$ada")]);
    let file = seal_with("export-pw", br#"{"routes": []}"#, &us, CHEAP).unwrap();
    let (data, got) = unseal(&file, "export-pw").unwrap();
    assert_eq!(data, br#"{"routes": []}"#);
    assert_eq!(got, us);
    assert!(matches!(unseal(&file, "other"), Err(Error::Open(_))));
    let d = dir("seal");
    let (s, _) = open(&d.join("akit.store"), "secret").unwrap();
    let file = s.seal(b"{}", &[]).unwrap();
    let (data, got) = s.unseal(&file).unwrap();
    assert_eq!(data, b"{}");
    assert!(got.is_empty());
    assert!(unseal(&file, "secret").is_ok());
    let _ = std::fs::remove_dir_all(&d);
}

/// Nothing is sealed or opened without a password, a config that is not JSON is not sealed, and
/// a file larger than a store may be is not read.
#[test]
fn seal_unseal_refused() {
    let e = seal_with("", b"{}", &[], CHEAP).unwrap_err();
    assert!(e.to_string().contains("password is empty"), "{e}");
    let e = seal_with("pw", b"listeners: []", &[], CHEAP).unwrap_err();
    assert!(e.to_string().contains("not JSON"), "{e}");
    let e = unseal(b"FITSTORE", "").unwrap_err();
    assert!(e.to_string().contains("password is empty"), "{e}");
    let big = vec![0u8; MAX_BYTES + HEADER_LEN + 100];
    let e = unseal(&big, "pw").unwrap_err();
    assert!(e.to_string().contains("larger than"), "{e}");
    assert!(matches!(
        unseal(
            b"not a store at all, but long enough to have a header and a tag",
            "pw"
        ),
        Err(Error::NotStore(_))
    ));
}

/// A version 2 file whose payload holds no config, or is not the payload's JSON, does not open,
/// from a file or as an export.
#[test]
fn payload_without_config() {
    let d = dir("noconfig");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    for payload in [r#"{"users": []}"#, "[1]", "not JSON", r#"{"config": null}"#] {
        let file = s
            .seal_payload(VERSION, payload.as_bytes().to_vec())
            .unwrap();
        let e = unseal(&file, "secret").unwrap_err();
        assert!(e.to_string().contains("holds no config"), "{payload}: {e}");
        let e = s.open_file(&file).unwrap_err();
        assert!(e.to_string().contains("holds no config"), "{payload}: {e}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A config as large as a store holds, with users beside it, is more than a store holds, and is
/// not written.
#[test]
fn users_past_the_bound() {
    let d = dir("bound");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    s.write(b"{}".to_vec()).unwrap();
    s.set_users(users(&[("ada", "$2a$04$ada")])).unwrap();
    let mut full = vec![b'a'; MAX_BYTES];
    full[0] = b'"';
    full[MAX_BYTES - 1] = b'"';
    let e = s.write(full).unwrap_err();
    assert!(
        e.to_string()
            .contains("the config, its users and its sections are larger than"),
        "{e}"
    );
    assert_eq!(s.read().unwrap(), b"{}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A file whose header asks for more Argon2id time, memory or threads than the factor times what
/// akit writes is refused before a key is derived.
#[test]
fn unseal_cost_bounded() {
    let file = seal_with("pw", b"{}", &[], CHEAP).unwrap();
    let d = DEFAULT_PARAMS;
    let f = MAX_UNSEAL_FACTOR;
    for p in [
        Params {
            time: f * d.time + 1,
            ..d
        },
        Params {
            memory_kib: f * d.memory_kib + 8,
            ..d
        },
        Params {
            memory_kib: 64 * d.memory_kib.min(MAX_MEMORY_KIB / 64),
            threads: u8::try_from(f * u32::from(d.threads) + 1).unwrap(),
            ..d
        },
    ] {
        let mut b = file.clone();
        b[10..14].copy_from_slice(&p.time.to_be_bytes());
        b[14..18].copy_from_slice(&p.memory_kib.to_be_bytes());
        b[18] = p.threads;
        let e = unseal(&b, "pw").unwrap_err();
        assert!(e.to_string().contains("more than"), "{p:?}: {e}");
    }
    // Up to the factor, it opens as far as the key.
    let mut b = file;
    b[10..14].copy_from_slice(&(f * d.time).to_be_bytes());
    assert!(matches!(unseal(&b, "pw"), Err(Error::Open(_))));
}

/// fit's `TestSections`: sections kept beside the config, through writes, users and a fresh open;
/// removed with empty data; their names and data checked.
#[test]
fn sections() {
    let d = dir("sections");
    let path = d.join("akit.store");
    let (mut s, _) = open(&path, "secret").unwrap();
    assert!(
        s.set_section("assistant", b"{}").is_err(),
        "a section kept before a config"
    );
    s.write(br#"{"routes":[]}"#.to_vec()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap()[MAGIC.len()], VERSION);
    s.set_section("assistant", br#"{"model": "m"}"#).unwrap();
    s.set_section("cluster", br#"{"version":2}"#).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap()[MAGIC.len()],
        VERSION_SECTIONS,
        "a store with sections is version 3"
    );
    s.set_users(users(&[("ada", "h")])).unwrap();
    s.write(br#"{"routes":[1]}"#.to_vec()).unwrap();
    let (o, data) = open(&path, "secret").unwrap();
    assert_eq!(data.unwrap(), br#"{"routes":[1]}"#);
    assert_eq!(o.section("assistant").unwrap(), br#"{"model": "m"}"#);
    assert_eq!(o.section("cluster").unwrap(), br#"{"version":2}"#);
    assert_eq!(o.users(), users(&[("ada", "h")]));
    assert_eq!(
        o.section_names().collect::<Vec<_>>(),
        ["assistant", "cluster"]
    );
    // An export holds no sections.
    let (_, got) = unseal(&o.seal(b"{}", &[]).unwrap(), "secret").unwrap();
    assert!(got.is_empty());
    // Removed, the last one makes it version 2 again.
    s.set_section("assistant", b"").unwrap();
    s.set_section("cluster", b"").unwrap();
    assert_eq!(std::fs::read(&path).unwrap()[MAGIC.len()], VERSION);
    let long = "x".repeat(MAX_SECTION_NAME + 1);
    assert!(s.set_section("", b"{}").is_err());
    assert!(s.set_section(&long, b"{}").is_err());
    assert!(s.set_section("x", b"{").is_err());
    let _ = std::fs::remove_dir_all(&d);
}

/// fit's `TestSectionsOnlyInVersion3`: a version 2 file's `sections` are not read.
#[test]
fn sections_only_in_version_3() {
    let d = dir("v2sections");
    let (s, _) = open(&d.join("akit.store"), "secret").unwrap();
    let file = s
        .seal_payload(VERSION, br#"{"config":{},"sections":{"a":1}}"#.to_vec())
        .unwrap();
    let mut t = Store::empty(Path::new("x"), "secret", CHEAP);
    t.open_file(&file).unwrap();
    assert_eq!(t.section_names().count(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

/// The strict JSON reader, as Go's `json.Valid`, and member spans.
#[test]
fn strict_json() {
    for ok in [
        "{}",
        " [1, -2.5e+3, true, false, null, \"\\u00e9\\n\"] ",
        "{\"a\":{\"b\":[{}]}}",
        "0",
        "\"é\"",
    ] {
        assert!(json::valid(ok.as_bytes()), "{ok}");
    }
    for bad in [
        "",
        "{",
        "[1,]",
        "{\"a\":1,}",
        "// c\n{}",
        "01",
        "1.",
        "-",
        "\"\\x\"",
        "{a:1}",
        "[1] [2]",
        "tru",
    ] {
        assert!(!json::valid(bad.as_bytes()), "{bad}");
    }
    let deep = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
    assert!(!json::valid(deep.as_bytes()));
    let b = br#"{ "config" : {"x": [1]}, "u\u0073ers":null }"#;
    let m = json::members(b).unwrap();
    assert_eq!(m[0].0, "config");
    assert_eq!(&b[m[0].1.clone()], br#"{"x": [1]}"#);
    assert_eq!(m[1].0, "users");
    assert!(json::members(b"[1]").is_none());
}
