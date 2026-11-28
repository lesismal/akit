//! The `media_tokens` handler (I143) through akit's listener behind `basic_auth`, with fit's
//! `handler/mediatokens` tests ported: batches, their checks and bounds, keys, revokes of the
//! single secret, one key and all keys, with the files rewritten and the policy changed in place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use akio::Engine;
use akit_media::auth::{Credentials, registry};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

/// `pw`, bcrypt at cost 4.
const HASH: &str = "$2y$04$n.ddWQ.hfkNRsx7o3Y1w3eongqIP2oVF8Ka9OEQQwsoVnWY6O.jlW";
/// `me:pw`.
const BASIC: &str = "Basic bWU6cHc=";
const OLD: &str = "the media tokens' first secret, 32 bytes or more";

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn post(addr: SocketAddr, path: &str, body: &str, auth: bool) -> Resp {
    request(addr, "POST", path, body, auth)
}

fn request(addr: SocketAddr, method: &str, path: &str, body: &str, auth: bool) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let a = if auth {
        format!("Authorization: {BASIC}\r\n")
    } else {
        String::new()
    };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{a}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    (&s).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut len = 0usize;
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((n, v)) = l.split_once(':') {
            if n.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap();
            }
            headers.push((n.to_owned(), v.trim().to_owned()));
        }
    }
    let mut b = vec![0; len];
    r.read_exact(&mut b).unwrap();
    Resp {
        status,
        headers,
        body: String::from_utf8(b).unwrap(),
    }
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn dir() -> Dir {
    let p = std::env::temp_dir().join(format!(
        "akit-tokens-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    Dir(p)
}

struct Akit {
    engine: Arc<Engine>,
    roles: Manager,
    http: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
    }
}

fn start(json: &str) -> Akit {
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_media::MODULES, akit_middleware::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine =
        Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(Arc::clone(&engine)));
    let http = engine.local_addrs()[0];
    let mut m = Manager::new(&[akit_media::role::MEDIA_AUTH], Arc::clone(&engine), None);
    let errs = m.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    Akit {
        engine,
        roles: m,
        http,
        _cell: cell,
    }
}

/// A config of `media_tokens` behind `basic_auth`, its policy `live` as `policy` writes it.
fn config(policy: &str, params: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "live", {policy}}}],
          "routes": [{{"match": {{"path": ["/media-tokens", "/media-tokens/*"]}},
            "middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "me", "password_hash": "{HASH}"}}]}}}}],
            "handler": {{"module": "media_tokens", "params": {{"auth": "live"{params}}}}}}}]}}"#
    )
}

/// The value of `key` in a flat JSON answer, as a string.
fn field(body: &str, key: &str) -> String {
    let at = body
        .find(&format!("\"{key}\":"))
        .unwrap_or_else(|| panic!("{key} in {body}"))
        + key.len()
        + 3;
    let rest = &body[at..];
    if let Some(r) = rest.strip_prefix('"') {
        r[..r.find('"').unwrap()].to_owned()
    } else {
        rest[..rest.find([',', '}']).unwrap()].to_owned()
    }
}

fn allowed(a: &Akit, token: &str, stream: &str, play: bool) -> bool {
    let p = registry::lookup(registry::key(&a.engine), "live").unwrap();
    let action = if play {
        akit_config::media::MediaAction::Play
    } else {
        akit_config::media::MediaAction::Publish
    };
    p.allow(
        action,
        stream,
        &Credentials {
            token: token.into(),
            ..Credentials::default()
        },
    )
    .is_ok()
}

/// fit's `TestIssue`, `TestIssueValidation`, `TestIssueLimits`, `TestIssuePrefixes` and
/// `TestMethodsAndPaths`.
#[test]
fn batches() {
    let d = dir();
    let file = d.0.join("secret");
    std::fs::write(&file, OLD).unwrap();
    let a = start(&config(
        &format!(r#""secret_file": "{}""#, file.display()),
        r#", "prefixes": ["mtg-"], "max_tokens": 2, "max_ttl": "2h", "max_body_bytes": 512"#,
    ));
    let h = a.http;
    let body = r#"{"tokens": [{"subject": "alice", "publish": ["mtg-1-*"], "play": ["mtg-1-*"], "ttl": "1h"}, {"subject": "bob", "play": ["mtg-1-*"]}]}"#;
    assert_eq!(post(h, "/media-tokens", body, false).status, 401);
    let r = post(h, "/media-tokens", body, true);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(field(&r.body, "subject"), "alice");
    let tok = field(&r.body, "token");
    assert!(tok.starts_with("fm1."), "{tok}");
    assert!(!r.body.contains("\"key\""), "{}", r.body);
    assert!(allowed(&a, &tok, "mtg-1-cam", false));
    assert!(!allowed(&a, &tok, "mtg-2-cam", false));
    for (body, want) in [
        ("not json", "the body must be JSON"),
        (r#"{"tokens": []}"#, "the batch is empty"),
        (
            r#"{"tokens": [{"subject": "a", "play": ["mtg-1"]}, {"subject": "b", "play": ["mtg-1"]}, {"subject": "c", "play": ["mtg-1"]}]}"#,
            "at most 2 a request",
        ),
        (
            r#"{"tokens": [{"subject": "", "play": ["mtg-1"]}]}"#,
            "is not a subject",
        ),
        (
            r#"{"tokens": [{"subject": "a"}]}"#,
            "at least one publish or play",
        ),
        (
            r#"{"tokens": [{"subject": "a", "play": ["a b"]}]}"#,
            "is not a stream pattern",
        ),
        (
            r#"{"tokens": [{"subject": "a", "play": ["other-1"]}]}"#,
            "starts with none of the prefixes",
        ),
        (
            r#"{"tokens": [{"subject": "a", "play": ["mtg-1"], "ttl": "3h"}]}"#,
            "longer than max_ttl",
        ),
        (
            r#"{"tokens": [{"subject": "a", "play": ["mtg-1"], "ttl": "-1s"}]}"#,
            "not a positive duration",
        ),
        (
            r#"{"key": "k1", "tokens": [{"subject": "a", "play": ["mtg-1"]}]}"#,
            "leave key out",
        ),
    ] {
        let r = post(h, "/media-tokens", body, true);
        assert!(
            r.status == 400 && r.body.contains(want),
            "{body}: {} {}",
            r.status,
            r.body
        );
    }
    let big = format!(
        r#"{{"tokens": [{{"subject": "{}", "play": ["mtg-1"]}}]}}"#,
        "a".repeat(600)
    );
    assert_eq!(post(h, "/media-tokens", &big, true).status, 413);
    assert_eq!(request(h, "GET", "/media-tokens", "", true).status, 405);
    assert_eq!(
        request(h, "GET", "/media-tokens/revoke", "", true).status,
        405
    );
    assert_eq!(post(h, "/media-tokens/other", "", true).status, 404);
}

/// fit's `TestRevoke`: the single secret rotated, its file rewritten, the old tokens refused and
/// new ones taken.
#[test]
fn revoke_single() {
    let d = dir();
    let file = d.0.join("secret");
    std::fs::write(&file, OLD).unwrap();
    let a = start(&config(
        &format!(r#""secret_file": "{}""#, file.display()),
        "",
    ));
    let h = a.http;
    let issue = || {
        let r = post(
            h,
            "/media-tokens",
            r#"{"tokens": [{"subject": "s", "play": ["live/*"]}]}"#,
            true,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        field(&r.body, "token")
    };
    let old = issue();
    assert!(allowed(&a, &old, "live/a", true));
    assert_eq!(
        post(h, "/media-tokens/revoke", r#"{"key": "x"}"#, true).status,
        400
    );
    assert_eq!(post(h, "/media-tokens/revoke", "[1]", true).status, 400);
    let r = post(h, "/media-tokens/revoke", "", true);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(field(&r.body, "revoked"), "true");
    let now = std::fs::read_to_string(&file).unwrap();
    assert!(now != OLD && now.len() >= 16, "{now}");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
    assert!(!allowed(&a, &old, "live/a", true));
    let fresh = issue();
    assert!(allowed(&a, &fresh, "live/a", true));
    // A policy made from the file, as a restart reads it, agrees.
    let cfg = akit_config::media::MediaAuth {
        name: "live".into(),
        secret_file: file.to_string_lossy().into_owned(),
        ..akit_config::media::MediaAuth::default()
    };
    let again = akit_media::auth::Policy::from_config(&cfg).unwrap();
    let c = |t: &str| Credentials {
        token: t.into(),
        ..Credentials::default()
    };
    assert!(
        again
            .allow(akit_config::media::MediaAction::Play, "live/a", &c(&fresh))
            .is_ok()
    );
    assert!(
        again
            .allow(akit_config::media::MediaAction::Play, "live/a", &c(&old))
            .is_err()
    );
    // No file to write: 500, and nothing changes.
    std::fs::remove_dir_all(&d.0).unwrap();
    let r = post(h, "/media-tokens/revoke", "", true);
    assert_eq!(r.status, 500, "{}", r.body);
    assert!(allowed(&a, &fresh, "live/a", true));
}

/// fit's `TestIssueWithKey`, `TestIssueKeyErrors`, `TestRevokeOneKey`, `TestRevokeAllKeys` and
/// `TestRevokeRingChoices`.
#[test]
fn keyring() {
    let d = dir();
    let f1 = d.0.join("k1");
    let f2 = d.0.join("k2");
    std::fs::write(&f1, "the first key's secret, 16+").unwrap();
    std::fs::write(&f2, "the second key's secret, 16+").unwrap();
    let a = start(&config(
        &format!(
            r#""keys": [{{"key": "k1", "secret_file": "{}"}}, {{"key": "k2", "secret_file": "{}"}}]"#,
            f1.display(),
            f2.display()
        ),
        "",
    ));
    let h = a.http;
    let issue = |key: &str| {
        let k = if key.is_empty() {
            String::new()
        } else {
            format!(r#""key": "{key}", "#)
        };
        let r = post(
            h,
            "/media-tokens",
            &format!(r#"{{{k}"tokens": [{{"subject": "s", "play": ["live/*"]}}]}}"#),
            true,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        (field(&r.body, "token"), field(&r.body, "key"))
    };
    let (t1, k) = issue("");
    assert_eq!(k, "k1");
    let (t2, k) = issue("k2");
    assert_eq!(k, "k2");
    let r = post(
        h,
        "/media-tokens",
        r#"{"key": "nope", "tokens": [{"subject": "s", "play": ["live/*"]}]}"#,
        true,
    );
    assert!(
        r.status == 400 && r.body.contains("no key named"),
        "{}",
        r.body
    );
    // A keyring's revoke names a key, or all.
    for (body, want) in [("", 400), (r#"{"key": "nope"}"#, 400)] {
        assert_eq!(
            post(h, "/media-tokens/revoke", body, true).status,
            want,
            "{body}"
        );
    }
    let before2 = std::fs::read_to_string(&f2).unwrap();
    let r = post(h, "/media-tokens/revoke", r#"{"key": "k1"}"#, true);
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(r#""keys":["k1"]"#), "{}", r.body);
    assert!(!allowed(&a, &t1, "live/a", true));
    assert!(allowed(&a, &t2, "live/a", true));
    assert_eq!(std::fs::read_to_string(&f2).unwrap(), before2);
    let (t1b, _) = issue("k1");
    assert!(allowed(&a, &t1b, "live/a", true));
    let r = post(h, "/media-tokens/revoke", r#"{"all": true}"#, true);
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains(r#""keys":["k1","k2"]"#), "{}", r.body);
    for t in [&t1b, &t2] {
        assert!(!allowed(&a, t, "live/a", true));
    }
    assert_ne!(std::fs::read_to_string(&f2).unwrap(), before2);
}

/// fit's `TestRouteMustAuthenticate` and `TestProvisionNeedsSecretFile`, as akit-config checks
/// them.
#[test]
fn route_and_policy_checks() {
    let no_mw = r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
      "media_auth": [{"name": "live", "secret_file": "/tmp/x"}],
      "routes": [{"handler": {"module": "media_tokens", "params": {"auth": "live"}}}]}"#;
    let e = akit_config::from_json(no_mw, None).unwrap_err().to_string();
    assert!(
        e.contains("serves only callers its route authenticates"),
        "{e}"
    );
    let inline = config(r#""secret": "0123456789abcdef-inline""#, "");
    let e = akit_config::from_json(&inline, None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("has neither"), "{e}");
    let ring = config(
        r#""keys": [{"key": "a", "secret": "0123456789abcdef-inline"}]"#,
        "",
    );
    let e = akit_config::from_json(&ring, None).unwrap_err().to_string();
    assert!(e.contains(r#"key "a" has none"#), "{e}");
    // fit's `TestURLsProvision`.
    for params in [
        r#", "url_base": "ftp://x""#,
        r#", "url_base": "https://""#,
        r#", "url_base": "https://x/?a""#,
        r#", "url_base": "https://u@x""#,
        r#", "url_base": "::""#,
        r#", "url_prefixes": ["vod/"]"#,
        r#", "url_prefixes": ["/a/../b"]"#,
        r#", "url_prefixes": ["/a b"]"#,
    ] {
        let e = akit_config::from_json(
            &config(r#""secret_file": "/tmp/x", "url_signing": {}"#, params),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("url_"), "{params}: {e}");
    }
}

/// A keyring k1, k2 in files of `d`.
fn ring(d: &Dir) -> String {
    let f1 = d.0.join("k1");
    let f2 = d.0.join("k2");
    std::fs::write(&f1, "the first key's secret, 16+").unwrap();
    std::fs::write(&f2, "the second key's secret, 16+").unwrap();
    format!(
        r#""keys": [{{"key": "k1", "secret_file": "{}"}}, {{"key": "k2", "secret_file": "{}"}}]"#,
        f1.display(),
        f2.display()
    )
}

/// A single secret in a file of `d`.
fn single(d: &Dir) -> String {
    let f = d.0.join("secret");
    std::fs::write(&f, OLD).unwrap();
    format!(r#""secret_file": "{}""#, f.display())
}

/// The URLs of a `/urls` answer.
fn urls_of(body: &str) -> Vec<String> {
    let list = &body[body.find("\"urls\":[").unwrap() + 8..];
    let list = &list[..list.find(']').unwrap()];
    list.split(',')
        .map(|u| u.trim_matches('"').to_owned())
        .collect()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// fit's `TestURLs`: URLs signed whole for a viewer, each for its own resource, bound to the
/// address and session asked for, with the key asked for; each is the query the policy signs
/// for the same viewer and expiry.
#[test]
fn urls() {
    let d = dir();
    let a = start(&config(
        &format!(
            r#"{}, "url_signing": {{"ttl": "5m", "bind_address": "subnet"}}"#,
            ring(&d)
        ),
        r#", "url_base": "https://media.example.com/", "url_prefixes": ["/vod/", "/live/"], "max_ttl": "1h""#,
    ));
    let h = a.http;
    let before = now();
    let r = post(
        h,
        "/media-tokens/urls",
        r#"{"subject": "user-1", "address": "203.0.113.7", "session": "tab-1", "key": "k2",
            "paths": ["/vod/films/films/bbb/index.m3u8", "/live/cam.flv"]}"#,
        true,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        (field(&r.body, "subject"), field(&r.body, "session")),
        ("user-1".to_owned(), "tab-1".to_owned())
    );
    assert_eq!(field(&r.body, "key"), "k2");
    let exp: i64 = field(&r.body, "expires").parse().unwrap();
    assert!((exp - (before + 300)).abs() <= 5, "{exp}");
    let list = urls_of(&r.body);
    let p = registry::lookup(registry::key(&a.engine), "live").unwrap();
    let viewer = akit_media::auth::Viewer {
        subject: "user-1".into(),
        address: Some("203.0.113.7".parse().unwrap()),
        session: "tab-1".into(),
    };
    for (u, (path, res)) in list.iter().zip([
        (
            "https://media.example.com/vod/films/films/bbb/index.m3u8",
            "/vod/films/films/bbb/",
        ),
        ("https://media.example.com/live/cam.flv", "/live/cam/"),
    ]) {
        let (base, q) = u.split_once('?').unwrap();
        assert_eq!(base, path);
        assert!(
            q.contains("kid=k2") && q.contains("ip=203.0.113.0/24") && q.contains("sid=tab-1"),
            "{u}"
        );
        assert_eq!(q, p.sign_url(res, &viewer, exp, "k2").unwrap());
    }
    // No session asked for: a new one each time; the ttl asked for; the ring's first key; no
    // address.
    let one = |body: &str| {
        let r = post(h, "/media-tokens/urls", body, true);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.header("cache-control"), Some("no-store"));
        r.body
    };
    let x = one(r#"{"subject": "user-1", "ttl": "10m", "paths": ["/vod/films/a/index.m3u8"]}"#);
    let y = one(r#"{"subject": "user-1", "paths": ["/vod/films/a/index.m3u8"]}"#);
    assert_ne!(field(&x, "session"), field(&y, "session"));
    assert_eq!(field(&x, "session").len(), 16);
    assert_eq!(field(&x, "key"), "k1");
    assert!(!urls_of(&x)[0].contains("ip="));
    let exp: i64 = field(&x, "expires").parse().unwrap();
    assert!((exp - (now() + 600)).abs() <= 5, "{exp}");
}

/// fit's `TestURLsValidation`.
#[test]
fn urls_validation() {
    let d = dir();
    let a = start(&config(
        &format!(r#"{}, "url_signing": {{}}"#, ring(&d)),
        r#", "url_prefixes": ["/vod/"], "max_ttl": "1h", "max_tokens": 2"#,
    ));
    let h = a.http;
    for (body, want) in [
        ("nonsense", "must be JSON"),
        (r#"{"subject": "u", "paths": []}"#, "paths is empty"),
        (
            r#"{"subject": "u", "paths": ["/vod/a/x", "/vod/a/y", "/vod/a/z"]}"#,
            "at most 2",
        ),
        (r#"{"paths": ["/vod/a/x"]}"#, "not a subject"),
        (
            r#"{"subject": "a\u0001", "paths": ["/vod/a/x"]}"#,
            "not a subject",
        ),
        (
            r#"{"subject": "u", "session": "a b", "paths": ["/vod/a/x"]}"#,
            "not a session",
        ),
        (
            r#"{"subject": "u", "address": "nope", "paths": ["/vod/a/x"]}"#,
            "not an IP address",
        ),
        (
            r#"{"subject": "u", "ttl": "nope", "paths": ["/vod/a/x"]}"#,
            "not a duration",
        ),
        (
            r#"{"subject": "u", "ttl": "2h", "paths": ["/vod/a/x"]}"#,
            "longer than max_ttl",
        ),
        (
            r#"{"subject": "u", "key": "k9", "paths": ["/vod/a/x"]}"#,
            "no key named",
        ),
        (
            r#"{"subject": "u", "paths": ["/vod/../x"]}"#,
            "not a path a URL is signed for",
        ),
        (
            r#"{"subject": "u", "paths": ["/live/a/index.m3u8"]}"#,
            "none of the url_prefixes",
        ),
    ] {
        let r = post(h, "/media-tokens/urls", body, true);
        assert!(
            r.status == 400 && r.body.contains(want),
            "{body}: {} {}",
            r.status,
            r.body
        );
    }
    let r = request(h, "GET", "/media-tokens/urls", "", true);
    assert_eq!((r.status, r.header("allow")), (405, Some("POST")));
    assert_eq!(post(h, "/media-tokens/urls", "{}", false).status, 401);
    drop(a);

    // A key asked of a policy with one secret; its URLs name no key.
    let g = start(&config(
        &format!(r#"{}, "url_signing": {{}}"#, single(&d)),
        "",
    ));
    let r = post(
        g.http,
        "/media-tokens/urls",
        r#"{"subject": "u", "key": "k1", "paths": ["/vod/a/x"]}"#,
        true,
    );
    assert!(r.status == 400 && r.body.contains("one key"), "{}", r.body);
    let r = post(
        g.http,
        "/media-tokens/urls",
        r#"{"subject": "u", "paths": ["/vod/a/x"]}"#,
        true,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains("\"key\""), "{}", r.body);
    let u = &urls_of(&r.body)[0];
    assert!(u.starts_with("/vod/a/x?exp=") && !u.contains("kid="), "{u}");
    drop(g);

    // A policy without url_signing.
    let n = start(&config(&single(&d), ""));
    let r = post(
        n.http,
        "/media-tokens/urls",
        r#"{"subject": "u", "paths": ["/vod/a/x"]}"#,
        true,
    );
    assert!(
        r.status == 404 && r.body.contains("no url_signing"),
        "{}",
        r.body
    );
}
