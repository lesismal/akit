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
    let (file, sum) = from_file.files().pop().unwrap();
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

/// A directory of the test's own, removed when it is dropped.
struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    let p = std::env::temp_dir().join(format!("akit-auth-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    Dir(p)
}

fn key_file(d: &Dir, name: &str, secret: &str) -> String {
    let p = d.0.join(name);
    std::fs::write(&p, secret).unwrap();
    p.to_string_lossy().into_owned()
}

/// fit's `TestSecretFileAndRotate` (I143).
#[test]
fn secret_file_and_rotate() {
    let d = dir("rotate");
    let file = key_file(&d, "secret", "a-secret-file-of-16+\n");
    let cfg = MediaAuth {
        name: "live".into(),
        secret_file: file.clone(),
        ..MediaAuth::default()
    };
    let p = Policy::from_config(&cfg).unwrap();
    assert_eq!(p.secret_file(), file);
    let g = Grant {
        publish: v(&["live/*"]),
        play: Vec::new(),
    };
    let (old, _) = p.issue("old", &g, Duration::from_secs(3600)).unwrap();
    assert!(p.allow(Action::Publish, "live/a", &tok(&old)).is_ok());
    let short = key_file(&d, "short", "short");
    for bad in [
        MediaAuth {
            name: "x".into(),
            secret_file: d.0.join("nope").to_string_lossy().into_owned(),
            ..MediaAuth::default()
        },
        MediaAuth {
            name: "x".into(),
            secret_file: short,
            ..MediaAuth::default()
        },
        MediaAuth {
            name: "x".into(),
            secret: "0123456789abcdef".into(),
            secret_file: file.clone(),
            ..MediaAuth::default()
        },
    ] {
        assert!(Policy::from_config(&bad).is_err(), "{bad:?}");
    }
    assert!(p.rotate("short").is_err());
    let second = "a-second-secret-of-16+";
    std::fs::write(&file, second).unwrap();
    p.rotate(second).unwrap();
    assert!(p.allow(Action::Publish, "live/a", &tok(&old)).is_err());
    let play = Grant {
        publish: Vec::new(),
        play: v(&["live/*"]),
    };
    let (fresh, _) = p.issue("new", &play, Duration::from_secs(3600)).unwrap();
    assert!(p.allow(Action::Play, "live/a", &tok(&fresh)).is_ok());
    let again = Policy::from_config(&cfg).unwrap();
    assert!(again.allow(Action::Play, "live/a", &tok(&fresh)).is_ok());
    assert!(again.allow(Action::Publish, "live/a", &tok(&old)).is_err());
}

/// fit's `TestKeyring` (I143): a keyring signs with its first key unless one is named, checks a
/// token against every key, and a key's rotation refuses only its tokens.
#[test]
fn keyring() {
    let d = dir("ring");
    let k1 = key_file(&d, "k1", "secret-of-k1-16+");
    let k2 = key_file(&d, "k2", "secret-of-k2-16+");
    let key = |n: &str, f: &str| akit_config::media::MediaKey {
        key: n.into(),
        secret_file: f.into(),
        ..akit_config::media::MediaKey::default()
    };
    let cfg = MediaAuth {
        name: "live".into(),
        keys: vec![key("k1", &k1), key("k2", &k2)],
        ..MediaAuth::default()
    };
    let p = Policy::from_config(&cfg).unwrap();
    assert_eq!(p.keys(), v(&["k1", "k2"]));
    assert_eq!(p.default_key().as_deref(), Some("k1"));
    assert_eq!(p.key_file("k2").as_deref(), Some(k2.as_str()));
    assert_eq!(p.key_file("nope"), None);
    assert_eq!(p.secret_file(), "");
    assert_eq!(p.files().len(), 2);
    let g = Grant {
        publish: v(&["mtg/*"]),
        play: Vec::new(),
    };
    let h = Duration::from_secs(3600);
    let (def, _) = p.issue("a", &g, h).unwrap();
    let (k2tok, _) = p.issue_key("k2", "b", &g, h).unwrap();
    for t in [&def, &k2tok] {
        assert!(p.allow(Action::Publish, "mtg/1", &tok(t)).is_ok());
    }
    assert!(p.issue_key("nope", "c", &g, h).is_err());
    assert!(p.rotate("another-secret-16+").is_err());
    assert!(p.rotate_key("nope", "another-secret-16+").is_err());
    p.rotate_key("k1", "rotated-secret-of-16+").unwrap();
    assert_eq!(
        p.allow(Action::Publish, "mtg/1", &tok(&def)),
        Err(Refusal::BadCredentials)
    );
    assert!(p.allow(Action::Publish, "mtg/1", &tok(&k2tok)).is_ok());
    let (newer, _) = p.issue_key("k1", "d", &g, h).unwrap();
    assert!(p.allow(Action::Publish, "mtg/1", &tok(&newer)).is_ok());
    std::fs::write(&k1, "rotated-secret-of-16+").unwrap();
    let again = Policy::from_config(&cfg).unwrap();
    for t in [&newer, &k2tok] {
        assert!(again.allow(Action::Publish, "mtg/1", &tok(t)).is_ok());
    }
    assert_eq!(
        again.allow(Action::Publish, "mtg/1", &tok(&def)),
        Err(Refusal::BadCredentials)
    );
    let shared = MediaAuth {
        name: "d".into(),
        keys: vec![
            akit_config::media::MediaKey {
                key: "a".into(),
                secret: "a-shared-secret-16+".into(),
                ..akit_config::media::MediaKey::default()
            },
            akit_config::media::MediaKey {
                key: "b".into(),
                secret: "a-shared-secret-16+".into(),
                ..akit_config::media::MediaKey::default()
            },
        ],
        ..MediaAuth::default()
    };
    assert!(
        Policy::from_config(&shared)
            .unwrap_err()
            .contains("share a secret")
    );
    // A single secret has no keyring.
    let single = Policy::from_config(&policy_config()).unwrap();
    assert!(single.issue_key("k1", "x", &g, h).is_err());
    assert!(single.rotate_key("k1", "another-secret-16+").is_err());
}

/// A token fit's binary signed with the key `b` of a keyring (`fit media-token -policy ring -key
/// b -subject fit-b -play 'vod/*' -ttl 876000h`, fit `b6c5cebf`): akit takes it, and refuses it
/// once `b` is rotated, not `a`.
const FIT_KEY_TOKEN: &str = include_str!("fit-token-key.txt");

#[test]
fn takes_fit_keyring_tokens() {
    let key = |n: &str, s: &str| akit_config::media::MediaKey {
        key: n.into(),
        secret: s.into(),
        ..akit_config::media::MediaKey::default()
    };
    let cfg = MediaAuth {
        name: "ring".into(),
        keys: vec![
            key("a", "0123456789abcdef-key-a"),
            key("b", "0123456789abcdef-key-b"),
        ],
        ..MediaAuth::default()
    };
    let p = Policy::from_config(&cfg).unwrap();
    let t = FIT_KEY_TOKEN.trim();
    assert_eq!(p.allow(Action::Play, "vod/x", &tok(t)), Ok("fit-b".into()));
    assert!(p.allow(Action::Publish, "vod/x", &tok(t)).is_err());
    p.rotate_key("a", "a-new-secret-of-16+").unwrap();
    assert_eq!(p.allow(Action::Play, "vod/x", &tok(t)), Ok("fit-b".into()));
    p.rotate_key("b", "b-new-secret-of-16+").unwrap();
    assert_eq!(
        p.allow(Action::Play, "vod/x", &tok(t)),
        Err(Refusal::BadCredentials)
    );
}
