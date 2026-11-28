//! fit's `mediaauth/urlsign_test.go`, ported (I144): the checks on the policy's state, at times
//! the tests name rather than the clock's; the handlers' part is in `tests/vod.rs`,
//! `tests/handlers.rs` and `tests/tokens.rs`.
#![allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use std::net::IpAddr;
use std::sync::Arc;

use akit_config::media::{
    DEFAULT_PLAY_GRACE, DEFAULT_SESSION_IDLE, DEFAULT_URL_TTL, MediaAction, MediaAuth, MediaKey,
    MediaToken,
};

use super::*;
use crate::auth::{Policy, Viewer};
use crate::http::Access;

/// The tests' clock's start, in seconds.
const T0: i64 = 1_800_000_000;
const NS: i64 = 1_000_000_000;
const RES: &str = "/vod/films/a/";
const STATIC_TOKEN: &str = "0123456789abcdef-static";
const PEER: &str = "203.0.113.7";

fn secs(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap()
}

/// A policy with a keyring k1, k2 and `url_signing` as `m` changes it (fit's `urlPolicy`).
fn policy(m: impl FnOnce(&mut MediaAuth)) -> Arc<Policy> {
    let mut cfg = MediaAuth {
        name: "site".into(),
        keys: vec![
            MediaKey {
                key: "k1".into(),
                secret: "the first key, 16 bytes+".into(),
                ..MediaKey::default()
            },
            MediaKey {
                key: "k2".into(),
                secret: "the second key, 16 bytes+".into(),
                ..MediaKey::default()
            },
        ],
        tokens: vec![MediaToken {
            token: STATIC_TOKEN.into(),
            play: vec!["vod/*".into()],
            ..MediaToken::default()
        }],
        url_signing: Some(UrlSigning::default()),
        ..MediaAuth::default()
    };
    m(&mut cfg);
    Policy::from_config(&cfg).unwrap()
}

fn single(m: impl FnOnce(&mut MediaAuth)) -> Arc<Policy> {
    policy(|c| {
        c.keys.clear();
        c.secret = "a single secret of 16+".into();
        m(c);
    })
}

fn viewer(sub: &str, sid: &str) -> Viewer {
    Viewer {
        subject: sub.into(),
        address: None,
        session: sid.into(),
    }
}

fn signed(p: &Policy, res: &str, v: &Viewer, exp: i64, key: &str) -> String {
    p.sign_url(res, v, exp, key).unwrap()
}

/// What a request carries besides its query.
#[derive(Default)]
struct From {
    peer: Option<&'static str>,
    xff: Vec<&'static str>,
    referer: Option<String>,
    origin: Option<&'static str>,
}

fn peer(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// A request of `resource` with the grant `query` at `at` seconds, as the guard checks it.
fn play_from(
    p: &Arc<Policy>,
    query: &str,
    resource: &str,
    at: i64,
    f: &From,
) -> Result<Access, TouchError> {
    let Some(g) = Grant::parse(query) else {
        p.count_url_refusal(UrlReason::Signature);
        return Err(TouchError::Url(UrlReason::Signature));
    };
    let addr = peer(f.peer.unwrap_or(PEER));
    let seen = Seen {
        peer: Some(addr),
        xff: f.xff.clone(),
        referer: f.referer.as_deref(),
        origin: f.origin,
    };
    let key = p.let_in(&seen, resource, &g, &format!("{addr}:4000"), at * NS)?;
    Ok(Access::of_url(p, resource, g, key, false))
}

fn play(p: &Arc<Policy>, query: &str, resource: &str, at: i64) -> Result<Access, TouchError> {
    play_from(p, query, resource, at, &From::default())
}

fn refused(r: Result<Access, TouchError>, why: UrlReason, what: &str) {
    match r {
        Err(TouchError::Url(got)) if got == why => {}
        Err(e) => panic!("{what}: refused {e:?}, want {why:?}"),
        Ok(_) => panic!("{what}: let in, want {why:?}"),
    }
}

fn refusals(p: &Policy, why: &str) -> u64 {
    p.url_refusals()
        .into_iter()
        .find(|(r, _)| *r == why)
        .map_or(0, |(_, n)| n)
}

/// The query as pairs, decoded.
fn pairs(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_owned(), crate::auth::unescape(v).unwrap())
        })
        .collect()
}

fn encode(p: &[(String, String)]) -> String {
    p.iter()
        .map(|(k, v)| format!("{k}={}", crate::handlers::query_escape(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn get<'a>(p: &'a [(String, String)], k: &str) -> Option<&'a str> {
    p.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str())
}

fn set(p: &mut Vec<(String, String)>, k: &str, v: &str) {
    match p.iter_mut().find(|(x, _)| x == k) {
        Some(e) => v.clone_into(&mut e.1),
        None => p.push((k.to_owned(), v.to_owned())),
    }
}

fn del(p: &mut Vec<(String, String)>, k: &str) {
    p.retain(|(x, _)| x != k);
}

type Change = Box<dyn Fn(&mut Vec<(String, String)>)>;

/// A URL is good for its resource alone, until its expiry, and for no other resource or with
/// any field changed (fit's `TestSignedURL`).
#[test]
fn signed_url() {
    let p = policy(|_| {});
    let q = signed(&p, RES, &viewer("user 42", "s1"), T0 + 60, "");
    assert!(
        q.contains("kid=k1") && q.contains("sub=user+42") && q.contains("sid=s1"),
        "{q}"
    );
    assert!(!q.contains("&g="), "{q}");
    let a = play(&p, &q, RES, T0).unwrap();
    assert!(a.by_url());
    assert_eq!(a.subject, "user 42");
    assert_eq!(a.expiry(), Some(T0 + 60));
    refused(
        play(&p, &q, "/vod/films/b/", T0),
        UrlReason::Signature,
        "another item",
    );
    for i in 1..RES.len() - 1 {
        let mut b = RES.as_bytes().to_vec();
        b[i] ^= 0x01;
        let r = String::from_utf8(b).unwrap();
        refused(play(&p, &q, &r, T0), UrlReason::Signature, &r);
    }
    let base = pairs(&q);
    let changes: Vec<Change> = vec![
        Box::new(|v| set(v, "exp", "1800000061")),
        Box::new(|v| set(v, "sub", "user 43")),
        Box::new(|v| set(v, "sid", "s2")),
        Box::new(|v| del(v, "sid")),
        Box::new(|v| set(v, "kid", "k2")),
        Box::new(|v| set(v, "kid", "k3")),
        Box::new(|v| del(v, "kid")),
        Box::new(|v| set(v, "ip", "203.0.113.7")),
        Box::new(|v| set(v, "g", "1")),
        Box::new(|v| set(v, "g", "2")),
        Box::new(|v| set(v, "sig", &"A".repeat(22))),
        Box::new(|v| set(v, "sig", "short")),
        Box::new(|v| del(v, "sub")),
        Box::new(|v| set(v, "exp", "-1")),
        Box::new(|v| v.push(("exp".into(), "1800000001".into()))),
        Box::new(|v| set(v, "sub", "a\nb")),
    ];
    for change in &changes {
        let mut v = base.clone();
        change(&mut v);
        let q = encode(&v);
        refused(play(&p, &q, RES, T0), UrlReason::Signature, &q);
    }
    assert!(refusals(&p, "signature") >= 20, "{:?}", p.url_refusals());
}

/// Once a URL expires it is refused, while the grant its playlist handed on lives play_grace
/// past the later of the URL's expiry and the item's end, and is handed on unchanged (fit's
/// `TestGrantOutlivesTheURL`).
#[test]
fn grant_outlives_the_url() {
    let p = policy(|_| {});
    let q = signed(&p, RES, &viewer("u", "s1"), T0 + 60, "k2");
    let a = play(&p, &q, RES, T0).unwrap();
    let (child, cookie) = a.pass_on_at(Duration::from_secs(7200), T0);
    assert!(cookie.is_empty());
    assert!(
        child.contains("&g=1&") && child.contains("kid=k2"),
        "{child}"
    );
    let want = T0 + 7200 + secs(DEFAULT_PLAY_GRACE);
    assert_eq!(get(&pairs(&child), "exp"), Some(want.to_string().as_str()));
    let (short, _) = a.pass_on_at(Duration::from_secs(10), T0);
    let want = T0 + 60 + secs(DEFAULT_PLAY_GRACE);
    assert_eq!(get(&pairs(&short), "exp"), Some(want.to_string().as_str()));
    refused(
        play(&p, &q, RES, T0 + 60),
        UrlReason::Expired,
        "the URL expired",
    );
    play(&p, &child, RES, T0 + 60).unwrap();
    let again = play(&p, &child, RES, T0 + 60).unwrap();
    assert_eq!(
        again.pass_on_at(Duration::from_secs(5 * 3600), T0 + 60).0,
        child
    );
    refused(
        play(&p, &child, "/vod/films/b/", T0 + 60),
        UrlReason::Signature,
        "the grant for another item",
    );
    let end = T0 + 7200 + secs(DEFAULT_PLAY_GRACE);
    refused(
        play(&p, &child, RES, end),
        UrlReason::Expired,
        "the grant expired",
    );
    assert_eq!(refusals(&p, "expired"), 2);
}

/// A URL bound to an address is refused from another, to its network with `bind_address`
/// `subnet`, and the viewer behind trusted proxies is the last untrusted address of
/// `X-Forwarded-For` (fit's `TestAddressBound`).
#[test]
fn address_bound() {
    let p = policy(|_| {});
    let v = Viewer {
        address: Some(peer("::ffff:203.0.113.7")),
        ..viewer("u", "")
    };
    let q = signed(&p, RES, &v, T0 + 60, "");
    assert!(q.contains("ip=203.0.113.7&"), "{q}");
    play(&p, &q, RES, T0).unwrap();
    let other = From {
        peer: Some("203.0.113.8"),
        ..From::default()
    };
    refused(
        play_from(&p, &q, RES, T0, &other),
        UrlReason::Address,
        "another address",
    );
    assert_eq!(refusals(&p, "address"), 1);

    let sub = policy(|c| {
        let u = c.url_signing.as_mut().unwrap();
        u.subnet = true;
        u.trusted_proxies = vec!["10.0.0.0/8".into()];
    });
    let v = Viewer {
        address: Some(peer("203.0.113.7")),
        ..viewer("u", "")
    };
    let q = signed(&sub, RES, &v, T0 + 60, "");
    assert!(q.contains("ip=203.0.113.0/24&"), "{q}");
    let at = |a: &'static str| From {
        peer: Some(a),
        ..From::default()
    };
    play_from(&sub, &q, RES, T0, &at("203.0.113.200")).unwrap();
    refused(
        play_from(&sub, &q, RES, T0, &at("203.0.114.7")),
        UrlReason::Address,
        "another network",
    );
    let behind = |xff: &'static str| From {
        peer: Some("10.1.2.3"),
        xff: vec![xff],
        ..From::default()
    };
    play_from(
        &sub,
        &q,
        RES,
        T0,
        &behind("198.51.100.1, 203.0.113.9, 10.0.0.2"),
    )
    .unwrap();
    for (xff, what) in [
        ("203.0.113.9, 198.51.100.1", "another behind a proxy"),
        ("nonsense", "a forwarded list that does not parse"),
        ("10.9.9.9", "all trusted"),
    ] {
        refused(
            play_from(&sub, &q, RES, T0, &behind(xff)),
            UrlReason::Address,
            what,
        );
    }
    let v6 = Viewer {
        address: Some(peer("2001:db8:1:2:3::4")),
        ..viewer("u", "")
    };
    let q6 = signed(&sub, RES, &v6, T0 + 60, "");
    assert!(q6.contains("ip=2001:db8:1:2::/64&"), "{q6}");
    play_from(&sub, &q6, RES, T0, &at("2001:db8:1:2:ffff::1")).unwrap();
    refused(
        play_from(&sub, &q6, RES, T0, &at("2001:db8:1:3::1")),
        UrlReason::Address,
        "another IPv6 network",
    );
    // An address escaped in the query is read as it is meant.
    let esc = q6.replace("ip=2001:db8:1:2::/64", "ip=2001%3Adb8%3A1%3A2%3A%3A%2F64");
    play_from(&sub, &esc, RES, T0, &at("2001:db8:1:2::9")).unwrap();
    assert_eq!(
        sub.viewer_addr(peer("10.1.2.3"), &["198.51.100.1, 203.0.113.9"]),
        Some(peer("203.0.113.9"))
    );
    assert_eq!(
        p.viewer_addr(peer("::ffff:10.1.2.3"), &["198.51.100.1"]),
        Some(peer("10.1.2.3"))
    );
    assert!(bound_to("203.0.113.7", Some(peer("203.0.113.7"))));
    assert!(!bound_to("nonsense", Some(peer("203.0.113.7"))));
    assert!(!bound_to("x/1", None));
}

/// `max_sessions` refuses a third session, or ends the oldest; sessions idle past
/// `session_idle` are over; the policy lists them and ends one (fit's `TestSessions`).
#[test]
fn sessions() {
    let exp = T0 + 3600;
    for end_oldest in [false, true] {
        let p = policy(|c| {
            let u = c.url_signing.as_mut().unwrap();
            u.max_sessions = 2;
            u.end_oldest = end_oldest;
        });
        let go = |sub: &str, sid: &str, at: i64| {
            let q = signed(&p, RES, &viewer(sub, sid), exp, "");
            play(&p, &q, RES, at)
        };
        go("u", "a", T0).unwrap();
        go("u", "b", T0 + 1).unwrap();
        go("v", "a", T0 + 1).unwrap();
        go("u", "a", T0 + 1).unwrap();
        if end_oldest {
            go("u", "c", T0 + 1).unwrap();
            refused(go("u", "a", T0 + 1), UrlReason::SessionEnded, "the oldest");
            go("u", "b", T0 + 1).unwrap();
            assert_eq!(refusals(&p, "session_ended"), 1);
        } else {
            refused(go("u", "c", T0 + 1), UrlReason::SessionLimit, "a third");
            go("u", "a", T0 + 1).unwrap();
            assert_eq!(refusals(&p, "session_limit"), 1);
        }
        let ss = p.sessions_at((T0 + 1) * NS);
        let want = if end_oldest { 4 } else { 3 };
        assert_eq!(ss.len(), want, "{ss:?}");
        let first = &ss[0];
        assert_eq!(
            (
                first.subject.as_str(),
                first.id.as_str(),
                first.resource.as_str(),
                first.address.as_str()
            ),
            ("u", "a", RES, "203.0.113.7:4000")
        );
        assert_eq!(ss[want - 1].subject, "v");
        assert_eq!(first.ended, end_oldest);
        // Past session_idle the sessions are over, and new ones begin.
        let later = T0 + 1 + secs(DEFAULT_SESSION_IDLE) + 1;
        go("u", "d", later).unwrap();
        go("u", "e", later).unwrap();
        assert!(p.end_session("u", "d"));
        assert!(!p.end_session("u", "d"));
        assert!(!p.end_session("u", "zz"));
        assert!(!p.end_session("w", "d"));
        refused(
            go("u", "d", later),
            UrlReason::SessionEnded,
            "an ended session",
        );
        go("u", "f", later).unwrap();
        // An ended session is remembered until its grants expire.
        assert!(p.sessions_at(exp * NS).is_empty());
    }
}

/// A grant handed on keeps an ended session refused as long as the grant lives, past the URL's
/// expiry (fit's `TestEndedSessionGrant`).
#[test]
fn ended_session_grant() {
    let p = policy(|c| {
        let u = c.url_signing.as_mut().unwrap();
        u.max_sessions = 1;
        u.end_oldest = true;
    });
    let a = play(
        &p,
        &signed(&p, RES, &viewer("u", "a"), T0 + 60, ""),
        RES,
        T0,
    )
    .unwrap();
    let (child, _) = a.pass_on_at(Duration::from_secs(3600), T0);
    play(
        &p,
        &signed(&p, RES, &viewer("u", "b"), T0 + 60, ""),
        RES,
        T0,
    )
    .unwrap();
    refused(
        play(&p, &child, RES, T0 + 1800),
        UrlReason::SessionEnded,
        "a's grant",
    );
}

/// `max_tracked_sessions` bounds the table: a new session past it is refused 503, until a sweep
/// finds sessions over (fit's `TestTrackedSessions`).
#[test]
fn tracked_sessions() {
    let p = policy(|c| c.url_signing.as_mut().unwrap().max_tracked_sessions = 2);
    let go = |sub: &str, at: i64| {
        let q = signed(&p, RES, &viewer(sub, "s"), T0 + 3600, "");
        play(&p, &q, RES, at)
    };
    go("a", T0).unwrap();
    go("b", T0).unwrap();
    assert!(matches!(go("c", T0), Err(TouchError::Full)));
    go("a", T0).unwrap();
    let later = T0 + secs(DEFAULT_SESSION_IDLE) + 60;
    go("c", later).unwrap();
    assert_eq!(p.tracked_sessions(), 1);
    // A URL without a session is not tracked.
    let q = signed(&p, RES, &viewer("z", ""), T0 + 7200, "");
    play(&p, &q, RES, later).unwrap();
    assert_eq!(p.tracked_sessions(), 1);
}

/// Rotating a key refuses the URLs and grants it signed, and only those; rotating a single
/// secret refuses every one (fit's `TestRotateRefusesURLs`).
#[test]
fn rotate_refuses_urls() {
    let p = policy(|_| {});
    let q1 = signed(&p, RES, &viewer("u", ""), T0 + 60, "k1");
    let q2 = signed(&p, RES, &viewer("u", ""), T0 + 60, "k2");
    let a = play(&p, &q1, RES, T0).unwrap();
    let (child, _) = a.pass_on_at(Duration::from_secs(3600), T0);
    p.rotate_key("k1", "a fresh secret, 16 bytes+").unwrap();
    refused(play(&p, &q1, RES, T0), UrlReason::Signature, "k1's URL");
    refused(
        play(&p, &child, RES, T0),
        UrlReason::Signature,
        "k1's grant",
    );
    play(&p, &q2, RES, T0).unwrap();

    let one = single(|_| {});
    let q = signed(&one, RES, &viewer("u", ""), T0 + 60, "");
    assert!(!q.contains("kid="), "{q}");
    play(&one, &q, RES, T0).unwrap();
    one.rotate("another single secret 16+").unwrap();
    refused(
        play(&one, &q, RES, T0),
        UrlReason::Signature,
        "after rotate",
    );
    refused(
        play(&one, &q2, RES, T0),
        UrlReason::Signature,
        "a key the policy has not",
    );
}

/// `referers` and `origins` refuse requests from other pages and scripts, and let in those that
/// send neither (fit's `TestReferersAndOrigins`).
#[test]
fn referers_and_origins() {
    let p = policy(|c| {
        let u = c.url_signing.as_mut().unwrap();
        u.referers = vec!["https://*.example.com".into()];
        u.origins = vec![
            "https://www.example.com".into(),
            "http://localhost:*".into(),
        ];
    });
    let q = signed(&p, RES, &viewer("u", ""), T0 + 60, "");
    let referer = |r: String| From {
        referer: Some(r),
        ..From::default()
    };
    play(&p, &q, RES, T0).unwrap();
    play_from(
        &p,
        &q,
        RES,
        T0,
        &referer("https://WWW.example.com/watch/1?x=y".into()),
    )
    .unwrap();
    play_from(&p, &q, RES, T0, &referer("https://a.example.com".into())).unwrap();
    for (r, what) in [
        (
            "https://evil.test/example.com".to_owned(),
            "another referer",
        ),
        ("nonsense".to_owned(), "a referer that is no URL"),
        (
            format!("https://{}.example.com/", "a".repeat(300)),
            "a long referer",
        ),
    ] {
        refused(
            play_from(&p, &q, RES, T0, &referer(r)),
            UrlReason::Referer,
            what,
        );
    }
    let origin = |o: &'static str| From {
        origin: Some(o),
        ..From::default()
    };
    play_from(&p, &q, RES, T0, &origin("http://localhost:8080")).unwrap();
    refused(
        play_from(&p, &q, RES, T0, &origin("https://example.com")),
        UrlReason::Origin,
        "another origin",
    );
}

/// With `cookie`, a playlist hands its grant on as a cookie on the resource's path; a playlist
/// fetched with the cookie hands nothing on (fit's `TestGrantCookie`, the policy's part).
#[test]
fn grant_cookie() {
    let p = policy(|c| c.url_signing.as_mut().unwrap().cookie = true);
    let q = signed(&p, RES, &viewer("u", "s"), T0 + 60, "");
    let a = play(&p, &q, RES, T0).unwrap();
    let (query, cookie) = a.pass_on_at(Duration::from_secs(3600), T0);
    assert!(query.is_empty());
    let age = 3600 + secs(DEFAULT_PLAY_GRACE);
    let prefix = format!("{GRANT_COOKIE}=exp=");
    assert!(cookie.starts_with(&prefix), "{cookie}");
    assert!(
        cookie.ends_with(&format!(
            "; Path={RES}; Max-Age={age}; HttpOnly; SameSite=Lax"
        )),
        "{cookie}"
    );
    let value = cookie
        .strip_prefix(&format!("{GRANT_COOKIE}="))
        .and_then(|c| c.split(';').next())
        .unwrap();
    let a = play(&p, value, RES, T0 + 120).unwrap();
    assert_eq!(
        a.pass_on_at(Duration::from_secs(3600), T0 + 120),
        (String::new(), String::new())
    );
    refused(
        play(&p, value, "/vod/films/b/", T0 + 120),
        UrlReason::Signature,
        "the cookie on another item",
    );
    // Over TLS the cookie is Secure, for players on another site.
    let g = Grant::parse(&q).unwrap();
    let c = super::cookie(&g, RES, true, T0);
    assert!(c.ends_with("; HttpOnly; Secure; SameSite=None"), "{c}");
}

/// An empty access hands nothing on; one let in by a token in its query hands that token on
/// (fit's `TestPassOnToken`).
#[test]
fn pass_on_token() {
    let none = Access::default();
    assert_eq!(
        none.pass_on_at(Duration::ZERO, T0),
        (String::new(), String::new())
    );
    assert!(!none.by_url() && none.expiry().is_none());
    let mut token = Access::default();
    token.query_token = "a+b".into();
    assert_eq!(token.pass_on_at(Duration::ZERO, T0).0, "token=a%2Bb");
}

/// fit's `TestResourceOf`.
#[test]
fn resources() {
    let long = format!("/a/{}", "b".repeat(1100));
    for (path, want) in [
        ("/vod/films/films/bbb/index.m3u8", "/vod/films/films/bbb/"),
        ("/vod/films/a/v0.12.m4s", "/vod/films/a/"),
        ("/live/cam.flv", "/live/cam/"),
        ("/flv/live/cam.flv", "/flv/live/cam/"),
        ("/hls/live/cam/index.m3u8", "/hls/live/cam/"),
        ("/index.m3u8", ""),
        ("/.flv", ""),
        ("/a/.flv", "/a/"),
        ("/a/", ""),
        ("vod/a/index.m3u8", ""),
        ("/a/../b/index.m3u8", ""),
        ("/a//index.m3u8", ""),
        ("/a/./index.m3u8", ""),
        ("/a b/index.m3u8", ""),
        ("/a/index.m3u8?x", ""),
        (long.as_str(), ""),
    ] {
        assert_eq!(resource_of(path).unwrap_or_default(), want, "{path}");
    }
}

/// fit's `TestSignURLErrors`.
#[test]
fn sign_url_errors() {
    let p = policy(|_| {});
    let exp = T0 + 60;
    let u = viewer("u", "");
    assert!(p.sign_url("/a", &u, exp, "").is_err());
    assert!(p.sign_url(RES, &viewer("", ""), exp, "").is_err());
    assert!(p.sign_url(RES, &viewer("a\x01", ""), exp, "").is_err());
    assert!(p.sign_url(RES, &viewer("u", "a b"), exp, "").is_err());
    assert!(p.sign_url(RES, &u, exp, "k9").is_err());
    assert!(p.sign_url(RES, &u, 0, "").is_err());
    assert!(single(|_| {}).sign_url(RES, &u, exp, "k1").is_err());
    let plain = single(|c| c.url_signing = None);
    assert_eq!(
        plain.sign_url(RES, &u, exp, ""),
        Err("the policy has no url_signing".to_owned())
    );
    assert!(!plain.url_signing());
    assert_eq!(plain.url_ttl(), DEFAULT_URL_TTL);
    assert!(p.url_signing());
    assert_eq!(p.url_ttl(), DEFAULT_URL_TTL);
}

/// fit's `TestURLSigningCompileErrors`.
#[test]
fn compile_errors() {
    let no_secret = MediaAuth {
        name: "x".into(),
        url_signing: Some(UrlSigning::default()),
        public: vec![MediaAction::Publish],
        ..MediaAuth::default()
    };
    assert!(Policy::from_config(&no_secret).is_err());
    let proxy = MediaAuth {
        name: "x".into(),
        secret: "a single secret of 16+".into(),
        url_signing: Some(UrlSigning {
            trusted_proxies: vec!["nonsense".into()],
            ..UrlSigning::default()
        }),
        ..MediaAuth::default()
    };
    assert!(Policy::from_config(&proxy).is_err());
}

/// The small parts: a bad escape, names, the origins' patterns and the sweep's pacing (fit's
/// `TestURLParts`).
#[test]
fn parts() {
    assert!(Grant::parse(&format!("exp=1&sub=%zz&sig={}", "a".repeat(22))).is_none());
    assert!(Grant::parse(&format!("exp=1&sub=u&sig={}", "a".repeat(22))).is_some());
    assert!(!valid_session("a b") && !valid_session(&"a".repeat(65)));
    assert!(valid_session("a.b_c~d-e"));
    let pats = |p: &str| vec![p.to_owned()];
    assert!(match_origin(&pats("a*b*c"), "aXbYbZc"));
    assert!(!match_origin(&pats("a*b"), "aXc"));
    assert!(match_origin(&pats("a**"), "a"));
    assert_eq!(origin_of("https://a.b/c/d"), "https://a.b");
    assert_eq!(origin_of("https://a.b"), "https://a.b");
    let t = Sessions::default();
    let hour = 3600 * NS;
    t.sweep(hour, Duration::from_secs(60));
    let first = t.swept.load(Relaxed);
    t.sweep(hour + 1, Duration::from_secs(60));
    assert_eq!(t.swept.load(Relaxed), first, "swept twice within a second");
    assert_eq!(UrlReason::SessionEnded.as_str(), "session_ended");
    assert_eq!(URL_REASONS.len(), 7);
}

/// The key derivation and the MAC are fit's: the query fit's `SignURL` gives for policy `site`
/// with keys k1 and k2, `/vod/films/a/`, subject `user 42`, session s1 and expiry 1800000060
/// (`fit-url-vector.txt`, made with fit b6c5cebf).
#[test]
fn fit_vector() {
    let p = policy(|_| {});
    let q = signed(&p, RES, &viewer("user 42", "s1"), T0 + 60, "");
    assert_eq!(q, include_str!("fit-url-vector.txt").trim_end());
}
