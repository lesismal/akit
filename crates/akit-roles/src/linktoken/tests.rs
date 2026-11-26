//! fit's `internal/linktoken/linktoken_test.go`, ported (I99), and fit's own vectors.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use super::*;

fn transcript<'a>(d: &'a [u8], l: &'a [u8], extra: &'a [&'a str]) -> Transcript<'a> {
    Transcript {
        label: "test",
        token: "0123456789abcdef",
        dialer: d,
        listener: l,
        extra,
    }
}

const ONES: [u8; 32] = [1; 32];
const TWOS: [u8; 32] = [2; 32];
const THREES: [u8; 32] = [3; 32];

/// The token, keys and proofs fit's `internal/linktoken` gives at fit 7daee573 for a fixed secret
/// and transcript (printed by a Go program calling it): akit's must be the same, byte for byte
/// (decision 038).
#[test]
fn fits_vectors() {
    let s = Secret::parse("akitt1.0123456789abcdef.AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8")
        .unwrap();
    assert_eq!(
        s.hash(),
        "306ef24509864020fa09084622c9613e0734c0613b23550e683863af0ce76c96"
    );
    assert_eq!(
        hex(&body_key(&s.stored())),
        "10dcaef722d58ea90eb9c827a0253eb9631000aea56b006a86b947735cf1e06c"
    );
    let d: Vec<u8> = (0x40..0x60).collect();
    let l: Vec<u8> = (0x80..0xa0).collect();
    let t = Transcript {
        label: "fit tunnel register v1",
        token: s.id(),
        dialer: &d,
        listener: &l,
        extra: &["node", "lab"],
    };
    assert_eq!(
        hex(&s.prove(&t)),
        "203c5612f36a87b7e36614745996082981ded9a6ec60d71ddcf86e7ec9d4e009"
    );
    assert_eq!(
        hex(&listener_proof(&s.stored(), &t, &["ok", "hub"])),
        "8d531b6072ab1cad08af60d9d18589b37228c8d71260f78e17acb44ba25c0217"
    );
    assert!(verify(&s.stored(), &t, &s.prove(&t)));
}

#[test]
fn secret_round_trip() {
    let s = Secret::new().unwrap();
    assert!(valid_id(s.id()));
    let p = Secret::parse(&s.token()).unwrap();
    assert_eq!(p, s);
    assert_eq!(p.hash(), s.hash());
    assert!(stored_of(&s.hash()).is_some());
    assert!(stored_of("zz").is_none());
    for bad in [
        "",
        "akitt1.",
        "akitt1.0123456789abcdef",
        "akitt1.0123456789ABCDEF.aaaaaaaaaaaaaaaaaaaaaaaa",
        "akitt2.0123456789abcdef.aaaaaaaaaaaaaaaaaaaaaaaa",
        "akitt1.0123456789abcdef.aaaa",
        "akitt1.0123456789abcdef.aaaaaaaaaaaaaaaaaaaaaaa!",
        "akitt1.0123456789abcdef.aaaaaaaaaaaaaaaaaaaaaaaaa",
        // fit's prefix is not akit's (decision 037).
        "fitt1.0123456789abcdef.AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
    ] {
        assert!(Secret::parse(bad).is_err(), "{bad:?} parsed");
    }
    // The secret is never printed.
    assert!(!format!("{s:?}").contains(&s.token()[20..]));
}

#[test]
fn handshake() {
    let s = Secret::new().unwrap();
    let stored = s.stored();
    let extra = ["node", "lab"];
    let tr = transcript(&ONES, &TWOS, &extra);
    let proof = s.prove(&tr);
    assert!(verify(&stored, &tr, &proof));
    // What the listener keeps does not prove the token.
    let forged = hmac(&stored, &[&tr.bytes(LABEL_DIALER)]);
    assert!(!verify(&stored, &tr, &forged));
    // Another listener nonce, dialer nonce, label, token or extra: replayed.
    let proxy = ["proxy", "lab"];
    for other in [
        transcript(&ONES, &THREES, &extra),
        transcript(&THREES, &TWOS, &extra),
        Transcript {
            label: "other",
            ..tr
        },
        Transcript {
            token: "fedcba9876543210",
            ..tr
        },
        transcript(&ONES, &TWOS, &proxy),
    ] {
        assert!(!verify(&stored, &other, &proof));
    }
    assert!(!verify(&stored, &tr, &proof[..10]));
    assert!(!verify(&stored[..10], &tr, &proof));
    assert!(!verify(&Secret::new().unwrap().stored(), &tr, &proof));
    let lp = listener_proof(&stored, &tr, &["answer"]);
    assert!(s.verify_listener(&tr, &lp, &["answer"]));
    assert!(!s.verify_listener(&tr, &lp, &["other"]));
    // Reflection: neither proof checks as the other.
    assert!(!s.verify_listener(&tr, &proof, &[]));
    assert!(!verify(&stored, &tr, &listener_proof(&stored, &tr, &[])));
    assert!(
        !Secret::new()
            .unwrap()
            .verify_listener(&tr, &lp, &["answer"])
    );
    assert_ne!(
        body_key(&stored),
        body_key(&Secret::new().unwrap().stored())
    );
    assert_ne!(nonce().unwrap(), nonce().unwrap());
}

/// A clock tests move.
fn clock(start: i64) -> (Arc<AtomicI64>, Clock) {
    let t = Arc::new(AtomicI64::new(start));
    let c = Arc::clone(&t);
    (t, Arc::new(move || c.load(Ordering::SeqCst)))
}

fn rec(name: &str, exp: i64, members: &[&str]) -> (Secret, LinkToken) {
    let s = Secret::new().unwrap();
    let r = LinkToken {
        id: s.id().to_owned(),
        name: name.to_owned(),
        hash: s.hash(),
        created_at: 1,
        expires_at: exp,
        members: members.iter().map(|m| (*m).to_owned()).collect(),
        ..LinkToken::default()
    };
    (s, r)
}

fn counter() -> (Arc<AtomicUsize>, Closer) {
    let n = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&n);
    (
        n,
        Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }),
    )
}

#[test]
fn registry() {
    let (now, c) = clock(1000);
    let binds = Arc::new(Mutex::new(Vec::<String>::new()));
    let b2 = Arc::clone(&binds);
    let r = Registry::new(
        Some(Arc::new(move |id: &str, m: &str| {
            b2.lock().unwrap().push(format!("{id} {m}"));
        })),
        c,
    );
    let (a, ra) = rec("a", 0, &["node/old"]);
    let (b, mut rb) = rec("b", 2000, &[]);
    let (_, mut rx) = rec("x", 0, &[]);
    rx.hash = "bad".into();
    r.set(&[ra.clone(), rb.clone(), rx]);
    assert_eq!(r.len(), 2);
    assert_eq!(r.lookup(a.id()).unwrap(), a.stored());
    assert_eq!(r.lookup("nope"), Err(TokenError::Unknown));
    let (closed_a, ca) = counter();
    let (_, ca2) = counter();
    let ua = r.admit(a.id(), "node/lab", "1.2.3.4:5", Some(ca)).unwrap();
    let ua2 = r.admit(a.id(), "node/lab", "1.2.3.4:6", Some(ca2)).unwrap();
    assert_eq!(*binds.lock().unwrap(), [format!("{} node/lab", a.id())]);
    // Another token cannot take a bound member.
    assert!(matches!(
        r.admit(b.id(), "node/lab", "x", None),
        Err(TokenError::Bound { .. })
    ));
    assert!(matches!(
        r.admit(b.id(), "node/old", "x", None),
        Err(TokenError::Bound { .. })
    ));
    assert!(matches!(
        r.admit("nope", "m", "x", None),
        Err(TokenError::Unknown)
    ));
    let (closed_b, cb) = counter();
    let ub = r.admit(b.id(), "proxy/p", "y", Some(cb)).unwrap();
    let v = r.get(a.id()).unwrap();
    assert_eq!(
        (
            v.links.len(),
            v.members.len(),
            v.last_used,
            v.last_addr.as_str()
        ),
        (2, 2, 1000, "1.2.3.4:6")
    );
    assert!(r.get("nope").is_none());
    let list = r.list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "a");
    drop(ua);
    assert_eq!(r.get(a.id()).unwrap().links.len(), 1);
    // A set that keeps the tokens keeps the links and the runtime binding.
    r.set(&[ra.clone(), rb.clone()]);
    let v = r.get(a.id()).unwrap();
    assert_eq!((v.links.len(), v.members.len()), (1, 2));
    // Expiry: the sweep closes b's link.
    assert_eq!(r.next_sweep(), Some(MAX_SWEEP));
    now.store(2000, Ordering::SeqCst);
    r.sweep();
    assert_eq!(closed_b.load(Ordering::SeqCst), 1);
    assert_eq!(r.lookup(b.id()), Err(TokenError::Expired));
    assert!(matches!(
        r.admit(b.id(), "proxy/p", "y", None),
        Err(TokenError::Expired)
    ));
    drop(ub);
    // Revoking a closes its link and frees its members.
    r.set(&[rb.clone()]);
    assert_eq!(
        closed_a.load(Ordering::SeqCst),
        0,
        "ua2's closer is its own"
    );
    drop(ua2);
    rb.expires_at = 0;
    r.set(&[rb.clone()]);
    let _u = r.admit(b.id(), "node/lab", "z", None).unwrap();
    // A changed hash closes the links.
    let (closed, c1) = counter();
    let _u1 = r.admit(b.id(), "node/lab", "z", Some(c1)).unwrap();
    rb.hash = Secret::new().unwrap().hash();
    r.set(&[rb.clone()]);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    // An expiry set into the past closes at once.
    let (closed, c2) = counter();
    let _u2 = r.admit(b.id(), "node/lab", "z", Some(c2)).unwrap();
    rb.expires_at = 10;
    r.set(&[rb]);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
}

#[test]
fn revoked_tokens_close_their_links() {
    let r = Registry::default();
    let (a, ra) = rec("a", 0, &[]);
    r.set(std::slice::from_ref(&ra));
    let (closed, c) = counter();
    let _u = r.admit(a.id(), "node/lab", "x", Some(c)).unwrap();
    r.set(&[]);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert_eq!(r.lookup(a.id()), Err(TokenError::Unknown));
}

#[test]
fn registry_list_order() {
    let r = Registry::default();
    let (_, a) = rec("same", 0, &[]);
    let (_, b) = rec("same", 0, &[]);
    r.set(&[a, b]);
    let l = r.list();
    assert!(l.len() == 2 && l[0].id < l[1].id);
}

#[test]
fn registry_full() {
    let r = Registry::default();
    let (s, rc) = rec("a", 0, &[]);
    r.set(&[rc]);
    let mut keep = Vec::new();
    for i in 0..MAX_LINK_TOKEN_MEMBERS {
        keep.push(r.admit(s.id(), &format!("m{i}"), "", None).unwrap());
    }
    assert!(matches!(
        r.admit(s.id(), "one-more", "", None),
        Err(TokenError::Full)
    ));
}

#[test]
fn ttls() {
    assert_eq!(
        parse_ttl("30d").unwrap(),
        Duration::from_secs(30 * 24 * 3600)
    );
    assert_eq!(parse_ttl("1h").unwrap(), Duration::from_secs(3600));
    for bad in ["", "0s", "-1h", "xd", "1y"] {
        assert!(parse_ttl(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn management() {
    let now = 10_000;
    let mut tokens = Vec::new();
    for req in [
        IssueReq {
            name: String::new(),
            ..IssueReq::default()
        },
        IssueReq {
            name: "n".repeat(65),
            ..IssueReq::default()
        },
        IssueReq {
            name: "a\u{1}".into(),
            ..IssueReq::default()
        },
        IssueReq {
            name: "a".into(),
            note: "n".repeat(1025),
            ..IssueReq::default()
        },
        IssueReq {
            name: "a".into(),
            expires_at: 5,
            ..IssueReq::default()
        },
        IssueReq {
            name: "a".into(),
            expires_at: -1,
            ..IssueReq::default()
        },
        IssueReq {
            name: "a".into(),
            expires_at: 20_000,
            ttl: "1h".into(),
            ..IssueReq::default()
        },
        IssueReq {
            name: "a".into(),
            ttl: "nope".into(),
            ..IssueReq::default()
        },
    ] {
        assert!(
            matches!(
                issue(&mut tokens, &req, now),
                Err(TokenError::BadRequest(_))
            ),
            "{req:?}"
        );
    }
    let is = issue(
        &mut tokens,
        &IssueReq {
            name: " lab ".into(),
            note: "the lab".into(),
            ttl: "1h".into(),
            ..IssueReq::default()
        },
        now,
    )
    .unwrap();
    assert_eq!(is.record.name, "lab");
    assert_eq!(is.record.expires_at, now + 3600);
    assert!(is.secret.token().starts_with(LINK_TOKEN_PREFIX));
    assert_eq!(
        tokens[0].hash,
        Secret::parse(&is.secret.token()).unwrap().hash()
    );
    let id = is.record.id.clone();
    assert_eq!(
        set_expiry(
            &mut tokens,
            &id,
            &ExpiryReq {
                expires_at: 50_000,
                ..ExpiryReq::default()
            },
            now
        ),
        Ok(50_000)
    );
    assert_eq!(
        set_expiry(&mut tokens, &id, &ExpiryReq::default(), now),
        Ok(0)
    );
    assert_eq!(
        set_expiry(&mut tokens, "nope", &ExpiryReq::default(), now),
        Err(TokenError::NotFound)
    );
    assert!(matches!(
        set_expiry(
            &mut tokens,
            &id,
            &ExpiryReq {
                ttl: "bad".into(),
                ..ExpiryReq::default()
            },
            now
        ),
        Err(TokenError::BadRequest(_))
    ));
    assert_eq!(revoke(&mut tokens, "nope"), Err(TokenError::NotFound));
    revoke(&mut tokens, &id).unwrap();
    assert!(tokens.is_empty());
    let mut full = vec![LinkToken::default(); MAX_LINK_TOKENS];
    assert!(matches!(
        issue(
            &mut full,
            &IssueReq {
                name: "a".into(),
                ..IssueReq::default()
            },
            now
        ),
        Err(TokenError::BadRequest(_))
    ));
    assert_eq!(expires_text(0), "never expiring");
    assert_eq!(
        expires_text(1_793_900_000),
        "expiring at 2026-11-05T17:33:20Z"
    );
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
}

#[test]
fn binding() {
    let mut ts = vec![LinkToken {
        id: "a".into(),
        ..LinkToken::default()
    }];
    assert!(bind(&mut ts, "a", "m"));
    assert!(!bind(&mut ts, "a", "m"));
    assert!(!bind(&mut ts, "b", "m"));
    assert_eq!(ts[0].members.len(), 1);
    ts[0].members = vec![String::new(); MAX_LINK_TOKEN_MEMBERS];
    assert!(!bind(&mut ts, "a", "n"));
}

#[test]
fn views_as_json() {
    let v = View {
        id: "0123456789abcdef".into(),
        name: "lab \"one\"".into(),
        expires_at: 5,
        members: vec!["node/lab".into()],
        links: vec![LinkView {
            member: "node/lab".into(),
            addr: "1.2.3.4:5".into(),
            since: 7,
        }],
        ..View::default()
    };
    assert_eq!(
        v.to_json(),
        r#"{"id":"0123456789abcdef","name":"lab \"one\"","expires_at":5,"expired":false,"members":["node/lab"],"links":[{"member":"node/lab","addr":"1.2.3.4:5","since":7}]}"#
    );
}
