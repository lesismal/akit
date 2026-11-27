//! fit's `mediaauth/mediaauth_test.go` and `edges_test.go`, ported (I121), and the interop of
//! signed tokens with fit's.

use std::sync::mpsc;

use akit_config::media::{MediaToken, MediaUser};

use super::*;

const STATIC_TOKEN: &str = "0123456789abcdef-static";

fn hash_of(pw: &str) -> String {
    bcrypt::hash(pw.as_bytes(), bcrypt::MIN_COST).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn v(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| (*x).to_owned()).collect()
}

/// A policy with a static token, its hashed twin, a plaintext user, a bcrypt user and a secret.
fn policy_config() -> MediaAuth {
    MediaAuth {
        name: "p".into(),
        tokens: vec![
            MediaToken {
                name: "cam".into(),
                token: STATIC_TOKEN.into(),
                publish: v(&["live/*"]),
                play: v(&["*"]),
                ..MediaToken::default()
            },
            MediaToken {
                token_sha256: hex(&Sha256::digest(b"another-token-of-16+")),
                play: v(&["room1-*"]),
                ..MediaToken::default()
            },
        ],
        users: vec![
            MediaUser {
                name: "alice".into(),
                password: "alice-pw".into(),
                publish: v(&["alice"]),
                play: v(&["*"]),
                ..MediaUser::default()
            },
            MediaUser {
                name: "bob".into(),
                password_hash: hash_of("bob-pw"),
                play: v(&["live/*"]),
                ..MediaUser::default()
            },
        ],
        secret: "a-secret-of-sixteen-bytes".into(),
        ..MediaAuth::default()
    }
}

fn tok(t: &str) -> Credentials {
    Credentials {
        token: t.into(),
        ..Credentials::default()
    }
}

fn user(u: &str, p: &str) -> Credentials {
    Credentials {
        user: u.into(),
        password: p.into(),
        ..Credentials::default()
    }
}

/// fit's `TestMatch`.
#[test]
fn patterns_match() {
    let long = "a".repeat(60);
    for (pattern, stream, want) in [
        ("*", "", true),
        ("*", "a/b/c", true),
        ("live/*", "live/cam1", true),
        ("live/*", "live/a/b", true),
        ("live/*", "live", false),
        ("live/*", "lives/x", false),
        ("room1-*", "room1-alice", true),
        ("room1-*", "room2-alice", false),
        ("cam", "cam", true),
        ("cam", "cam1", false),
        ("*-cam", "a-b-cam", true),
        ("a*b*c", "aXbYc", true),
        ("a*b*c", "aXbY", false),
        ("**", "x", true),
        ("a*a*a*a*b", long.as_str(), false),
    ] {
        assert_eq!(matches(pattern, stream), want, "{pattern} {stream}");
    }
}

/// fit's `TestAllow`.
#[test]
fn allow() {
    use Action::{Play, Publish};
    let p = Policy::from_config(&policy_config()).unwrap();
    let both = Credentials {
        token: "bad-bad-bad-bad-bad".into(),
        user: "alice".into(),
        password: "alice-pw".into(),
    };
    type Case<'a> = (
        &'a str,
        Action,
        &'a str,
        Credentials,
        Result<&'a str, Refusal>,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            "none",
            Play,
            "x",
            Credentials::default(),
            Err(Refusal::NoCredentials),
        ),
        (
            "static publish",
            Publish,
            "live/cam1",
            tok(STATIC_TOKEN),
            Ok("token:cam"),
        ),
        (
            "static forbidden",
            Publish,
            "other",
            tok(STATIC_TOKEN),
            Err(Refusal::Forbidden),
        ),
        (
            "static unknown",
            Play,
            "x",
            tok("nope-nope-nope-nope"),
            Err(Refusal::BadCredentials),
        ),
        (
            "hashed token",
            Play,
            "room1-x",
            tok("another-token-of-16+"),
            Ok("token"),
        ),
        (
            "hashed forbidden",
            Publish,
            "room1-x",
            tok("another-token-of-16+"),
            Err(Refusal::Forbidden),
        ),
        (
            "user",
            Publish,
            "alice",
            user("alice", "alice-pw"),
            Ok("alice"),
        ),
        (
            "user forbidden",
            Publish,
            "bob",
            user("alice", "alice-pw"),
            Err(Refusal::Forbidden),
        ),
        (
            "user wrong",
            Publish,
            "alice",
            user("alice", "nope"),
            Err(Refusal::BadCredentials),
        ),
        (
            "unknown user",
            Publish,
            "alice",
            user("carol", "alice-pw"),
            Err(Refusal::Pending),
        ),
        (
            "bcrypt user",
            Play,
            "live/x",
            user("bob", "bob-pw"),
            Err(Refusal::Pending),
        ),
        (
            "token first",
            Publish,
            "alice",
            both,
            Err(Refusal::BadCredentials),
        ),
    ];
    for (name, a, stream, c, want) in cases {
        let got = p.allow(a, stream, &c);
        assert_eq!(got.as_deref().map_err(|e| *e), want, "{name}");
    }
    assert_eq!(Refusal::NoCredentials.status(), 401);
    assert_eq!(Refusal::BadCredentials.status(), 401);
    assert_eq!(Refusal::Forbidden.status(), 403);
    assert_eq!(Refusal::Busy.status(), 503);
    assert_eq!(Refusal::Unavailable.status(), 503);
    assert_eq!(p.password("alice").as_deref(), Some("alice-pw"));
    assert_eq!(p.password("bob"), None);
    assert_eq!(p.password("carol"), None);
    assert!(p.has_users() && p.name() == "p");
    assert_eq!(p.ttl(), akit_config::media::DEFAULT_MEDIA_TOKEN_TTL);
    // Without a bcrypt user, an unknown user is refused at once.
    let mut cfg = policy_config();
    cfg.users.truncate(1);
    let p = Policy::from_config(&cfg).unwrap();
    assert_eq!(
        p.allow(Publish, "alice", &user("carol", "x")),
        Err(Refusal::BadCredentials)
    );
}

/// fit's `TestPublic`.
#[test]
fn public() {
    let mut cfg = policy_config();
    cfg.public = vec![Action::Play];
    let p = Policy::from_config(&cfg).unwrap();
    assert_eq!(
        p.allow(Action::Play, "anything", &Credentials::default()),
        Ok(String::new())
    );
    assert_eq!(
        p.allow(Action::Publish, "live/x", &Credentials::default()),
        Err(Refusal::NoCredentials)
    );
    assert!(p.is_public(Action::Play) && !p.is_public(Action::Publish));
}

fn check(p: &Arc<Policy>, c: &Credentials, a: Action, stream: &str) -> Result<String, Refusal> {
    let (tx, rx) = mpsc::channel();
    p.check(a, stream, c, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(30)).unwrap()
}

/// fit's `TestBcrypt`: a bcrypt check on the pool, a password found right remembered, a wrong one
/// and an unknown user refused alike.
#[test]
fn bcrypt_on_the_pool() {
    let p = Policy::from_config(&policy_config()).unwrap();
    assert_eq!(
        check(&p, &user("bob", "wrong"), Action::Play, "live/x"),
        Err(Refusal::BadCredentials)
    );
    assert_eq!(
        check(&p, &user("carol", "x"), Action::Play, "live/x"),
        Err(Refusal::BadCredentials)
    );
    assert_eq!(
        check(&p, &user("bob", "bob-pw"), Action::Publish, "live/x"),
        Err(Refusal::Forbidden)
    );
    assert_eq!(
        check(&p, &user("bob", "bob-pw"), Action::Play, "live/x"),
        Ok("bob".into())
    );
    // Remembered: at once now.
    assert_eq!(
        p.allow(Action::Play, "live/x", &user("bob", "bob-pw")),
        Ok("bob".into())
    );
    assert_eq!(
        p.allow(Action::Play, "live/x", &user("bob", "other")),
        Err(Refusal::Pending)
    );
    // A check with nothing slow answers before it returns.
    let (tx, rx) = mpsc::channel();
    p.check(Action::Play, "x", &tok(STATIC_TOKEN), move |r| {
        let _ = tx.send(r);
    });
    assert_eq!(rx.try_recv(), Ok(Ok("token:cam".into())));
}

/// fit's `TestIssue`.
#[test]
fn issue_and_verify() {
    let p = Policy::from_config(&policy_config()).unwrap();
    let g = Grant {
        publish: v(&["live/*"]),
        play: Vec::new(),
    };
    let (t, exp) = p.issue("pub1", &g, Duration::from_secs(3600)).unwrap();
    assert!(t.starts_with(TOKEN_PREFIX));
    assert!(exp >= now().saturating_add(3599));
    assert_eq!(
        p.allow(Action::Publish, "live/a", &tok(&t)),
        Ok("pub1".into())
    );
    assert_eq!(
        p.allow(Action::Play, "live/a", &tok(&t)),
        Err(Refusal::Forbidden)
    );
    // A policy of the same secret and name checks it; another name or secret does not.
    let same = Policy::from_config(&policy_config()).unwrap();
    assert!(same.allow(Action::Publish, "live/a", &tok(&t)).is_ok());
    let mut other = policy_config();
    other.name = "q".into();
    let other = Policy::from_config(&other).unwrap();
    assert_eq!(
        other.allow(Action::Publish, "live/a", &tok(&t)),
        Err(Refusal::BadCredentials)
    );
    let mut other = policy_config();
    other.secret = "another-secret-of-16-bytes".into();
    let other = Policy::from_config(&other).unwrap();
    assert_eq!(
        other.allow(Action::Publish, "live/a", &tok(&t)),
        Err(Refusal::BadCredentials)
    );
    // Without a secret, the key is the policy's own.
    let mut no_secret = policy_config();
    no_secret.secret.clear();
    let (a, b) = (
        Policy::from_config(&no_secret).unwrap(),
        Policy::from_config(&no_secret).unwrap(),
    );
    let all = Grant {
        play: v(&["*"]),
        ..Grant::default()
    };
    let (t2, _) = a.issue("x", &all, Duration::ZERO).unwrap();
    assert!(a.allow(Action::Play, "s", &tok(&t2)).is_ok());
    assert_eq!(
        b.allow(Action::Play, "s", &tok(&t2)),
        Err(Refusal::BadCredentials)
    );
    // Tampering: the payload, the MAC, the form.
    let body = t.strip_prefix(TOKEN_PREFIX).unwrap();
    let (payload, mac) = body.split_once('.').unwrap();
    let raw = String::from_utf8(
        akio_crypto::base64::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap(),
    )
    .unwrap();
    let forged = format!(
        "{TOKEN_PREFIX}{}.{mac}",
        b64(raw.replace("live/*", "*").as_bytes())
    );
    let cut = mac.get(..mac.len() - 2).unwrap();
    for bad in [
        forged,
        format!("{TOKEN_PREFIX}{payload}.{cut}AA"),
        format!("{TOKEN_PREFIX}{payload}"),
        format!("{TOKEN_PREFIX}!!!.{mac}"),
        format!("{TOKEN_PREFIX}{}", "A".repeat(MAX_TOKEN_BYTES)),
    ] {
        assert_eq!(
            p.allow(Action::Publish, "live/a", &tok(&bad)),
            Err(Refusal::BadCredentials),
            "{bad:.40}"
        );
    }
    // A valid MAC over what is not claims.
    let key = sign_key_for("p", b"a-secret-of-sixteen-bytes");
    let junk = b64(b"not json");
    let t3 = format!("{TOKEN_PREFIX}{junk}.{}", sign(&key, &junk));
    assert_eq!(
        p.allow(Action::Play, "x", &tok(&t3)),
        Err(Refusal::BadCredentials)
    );
    // Expiry.
    let (t4, exp4) = issue(&key, "e", &all, Duration::from_secs(60), 1000).unwrap();
    assert_eq!(exp4, 1060);
    assert!(verify(&key, &t4, 1059).is_ok());
    assert_eq!(verify(&key, &t4, 1060), Err(Refusal::BadCredentials));
    let long = "x".repeat(129);
    let many = Grant {
        play: (b'a'..=b'm')
            .map(|c| (c as char).to_string().repeat(250))
            .collect(),
        ..Grant::default()
    };
    let bad_pattern = Grant {
        play: v(&["bad pattern"]),
        ..Grant::default()
    };
    for (subject, g) in [
        ("", &all),
        (long.as_str(), &all),
        ("a\nb", &all),
        ("x", &bad_pattern),
        ("x", &many),
    ] {
        assert!(
            p.issue(subject, g, Duration::from_secs(60)).is_err(),
            "{subject:.10}"
        );
    }
}

/// A token fit's `media-token` signed, for policy `live` with the secret
/// `0123456789abcdef-secret` (`fit media-token -policy live -subject fit -publish 'live/*' -play
/// '*' -ttl 876000h`), by fit's binary built from fit's source at `b6c5cebf`. akit checks it as
/// fit does.
const FIT_TOKEN: &str = include_str!("fit-token.txt");

#[test]
fn takes_fit_tokens() {
    let cfg = MediaAuth {
        name: "live".into(),
        secret: "0123456789abcdef-secret".into(),
        ..MediaAuth::default()
    };
    let p = Policy::from_config(&cfg).unwrap();
    let t = FIT_TOKEN.trim();
    assert_eq!(
        p.allow(Action::Publish, "live/cam", &tok(t)),
        Ok("fit".into())
    );
    assert_eq!(
        p.allow(Action::Play, "any/thing", &tok(t)),
        Ok("fit".into())
    );
    assert_eq!(
        p.allow(Action::Publish, "other/cam", &tok(t)),
        Err(Refusal::Forbidden)
    );
    // The same claims akit signs carry the same MAC: the key and the MAC are fit's.
    let rest = t.strip_prefix(TOKEN_PREFIX).unwrap();
    let (payload, mac) = rest.split_once('.').unwrap();
    let key = sign_key_for("live", b"0123456789abcdef-secret");
    assert_eq!(sign(&key, payload), mac);
    // And akit writes the claims as fit's Go writes them.
    let g = Grant {
        publish: v(&["live/*"]),
        play: v(&["*"]),
    };
    let (mine, _) = issue(&key, "fit", &g, Duration::from_secs(4_949_344_838), 0).unwrap();
    assert_eq!(mine, t);
}

/// fit's `TestRegistry`: a policy changed in place by a config applied, removed by one that
/// leaves it out, and a module's policy made before the role applies it.
#[test]
fn registry_in_place() {
    let key = 0x5eed_0001;
    let mut cfg = policy_config();
    let p = registry::provision(key, &cfg).unwrap();
    assert!(p.allow(Action::Play, "live/x", &tok(STATIC_TOKEN)).is_ok());
    registry::apply(key, std::slice::from_ref(&cfg)).unwrap();
    let same = registry::lookup(key, "p").unwrap();
    assert!(Arc::ptr_eq(&p, &same));
    // Remembered across a reload that keeps the hash.
    assert!(check(&p, &user("bob", "bob-pw"), Action::Play, "live/x").is_ok());
    cfg.tokens.remove(0);
    registry::apply(key, std::slice::from_ref(&cfg)).unwrap();
    assert_eq!(
        p.allow(Action::Play, "live/x", &tok(STATIC_TOKEN)),
        Err(Refusal::BadCredentials)
    );
    assert!(
        p.allow(Action::Play, "live/x", &user("bob", "bob-pw"))
            .is_ok()
    );
    // A provision after the role applied leaves the policy to it.
    let mut stale = policy_config();
    stale.public = vec![Action::Play];
    let q = registry::provision(key, &stale).unwrap();
    assert!(Arc::ptr_eq(&p, &q));
    assert!(!p.is_public(Action::Play));
    // One that does not compile changes nothing.
    let mut bad = cfg.clone();
    bad.secret.clear();
    bad.secret_file = "/nonexistent/akit-media-secret".into();
    assert!(registry::apply(key, &[bad]).is_err());
    assert!(p.loaded());
    // Left out: removed.
    registry::apply(key, &[]).unwrap();
    assert!(!p.loaded() && registry::lookup(key, "p").is_none());
    assert_eq!(
        p.allow(Action::Play, "live/x", &tok(STATIC_TOKEN)),
        Err(Refusal::Unavailable)
    );
    registry::remove(key);
}

/// fit's `TestSecretFileAndRotate`'s first half: a secret read from its file, its line breaks at
/// the end taken off, and reported for `-watch-certs`.
#[test]
fn secret_file_read() {
    let dir = std::env::temp_dir().join(format!("akit-media-secret-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("secret");
    std::fs::write(&path, "a-secret-of-sixteen-bytes\r\n").unwrap();
    let mut cfg = policy_config();
    cfg.secret.clear();
    cfg.secret_file = path.to_string_lossy().into_owned();
    let from_file = Policy::from_config(&cfg).unwrap();
    let inline = Policy::from_config(&policy_config()).unwrap();
    let (t, _) = inline
        .issue("x", &Grant::default(), Duration::from_secs(60))
        .unwrap();
    // The same key: the token checks, and grants nothing.
    assert_eq!(
        from_file.allow(Action::Play, "s", &tok(&t)),
        Err(Refusal::Forbidden)
    );
    let (file, sum) = from_file.file().unwrap();
    assert_eq!(file, cfg.secret_file);
    assert_eq!(sum, Sha256::digest(b"a-secret-of-sixteen-bytes\r\n"));
    std::fs::write(&path, "short\n").unwrap();
    assert!(
        Policy::from_config(&cfg)
            .unwrap_err()
            .contains("at least 16")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn query_credentials() {
    assert_eq!(Credentials::from_query("?token=a%2Bb").token, "a+b");
    let c = Credentials::from_query("user=u&pass=p%20w");
    assert_eq!((c.user.as_str(), c.password.as_str()), ("u", "p w"));
    // A token first, as fit's.
    assert_eq!(Credentials::from_query("user=u&token=t").token, "t");
    assert!(Credentials::from_query("x=1").is_empty());
}

/// fit's `TestPasskeys` (I159): users' BitTorrent passkeys, fit's byte for byte, their grants,
/// tokens taken as passkeys, another secret's refused, public play, and what is refused.
#[test]
fn passkeys() {
    let cfg = MediaAuth {
        name: "bt".into(),
        secret: "0123456789abcdef0123456789abcdef".into(),
        users: vec![
            MediaUser {
                name: "alice".into(),
                password: "alice-password".into(),
                play: v(&["bittorrent/*"]),
                ..MediaUser::default()
            },
            MediaUser {
                name: "bob".into(),
                password_hash: hash_of("bob-password"),
                play: v(&["bittorrent/aa"]),
                ..MediaUser::default()
            },
        ],
        tokens: vec![MediaToken {
            name: "t".into(),
            token: "token-0123456789ab".into(),
            play: v(&["bittorrent/*"]),
            ..MediaToken::default()
        }],
        ..MediaAuth::default()
    };
    let p = Policy::from_config(&cfg).unwrap();
    // fit's passkey of alice under this policy's secret.
    let alice = p.passkey("alice").unwrap();
    assert_eq!(alice, "c3751c196e55193e0c3be763bcc1b8ab");
    let bob = p.passkey("bob").unwrap();
    assert_ne!(bob, alice);
    assert!(p.passkey("carol").is_err());
    for (stream, key, want) in [
        ("bittorrent/aa", alice.as_str(), Ok("alice")),
        ("bittorrent/aa", bob.as_str(), Ok("bob")),
        ("bittorrent/bb", bob.as_str(), Err(Refusal::Forbidden)),
        ("bittorrent/bb", "token-0123456789ab", Ok("token:t")),
        ("bittorrent/bb", "", Err(Refusal::NoCredentials)),
        ("bittorrent/bb", "nope", Err(Refusal::BadCredentials)),
        (
            "bittorrent/bb",
            "alice-password",
            Err(Refusal::BadCredentials),
        ),
    ] {
        let got = p.allow_passkey(stream, key);
        assert_eq!(
            got.as_deref().map_err(|e| *e),
            want,
            "{stream} with {key:?}"
        );
    }
    let other = Policy::from_config(&MediaAuth {
        secret: "another-secret-0123456789".into(),
        ..cfg.clone()
    })
    .unwrap();
    assert_eq!(
        other.allow_passkey("bittorrent/aa", &alice),
        Err(Refusal::BadCredentials)
    );
    let public = Policy::from_config(&MediaAuth {
        public: vec![Action::Play],
        ..cfg
    })
    .unwrap();
    assert_eq!(public.allow_passkey("bittorrent/aa", ""), Ok(String::new()));
}
