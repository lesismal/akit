//! fit's `TestGuard` (`mediaauth/mediaauth_test.go`), ported (I121): an HTTP request checked
//! against a policy on akio, answered 401 with a Bearer challenge without credentials, 401 with
//! `invalid_token` for bad ones, 403 when they do not allow it, and let on otherwise; a bcrypt
//! user's first request suspended while the pool checks the password, then let on; the token of
//! the query written `REDACTED` in the target before the handler sees it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use akio::{Engine, Listener};
use akio_crypto::bcrypt;
use akio_http::{Context, Response};
use akit_config::media::{MediaAction, MediaAuth, MediaToken, MediaUser};
use akit_media::auth::Policy;

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

fn get(addr: SocketAddr, target: &str, fields: &[(&str, &str)]) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req = format!("GET {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    (&s).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':').unwrap();
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    let len: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; len];
    r.read_exact(&mut body).unwrap();
    Resp {
        status,
        headers,
        body: String::from_utf8(body).unwrap(),
    }
}

#[test]
fn guard_over_http() {
    let cfg = MediaAuth {
        name: "g".into(),
        tokens: vec![MediaToken {
            token: "0123456789abcdef-static".into(),
            publish: vec!["live/*".into()],
            ..MediaToken::default()
        }],
        users: vec![MediaUser {
            name: "bob".into(),
            password_hash: bcrypt::hash(b"bob-pw", bcrypt::MIN_COST).unwrap(),
            publish: vec!["live/*".into()],
            ..MediaUser::default()
        }],
        ..MediaAuth::default()
    };
    let policy: Arc<Policy> = Policy::from_config(&cfg).unwrap();
    let p = Arc::clone(&policy);
    let service = akio_http::serve(move |ctx: &mut Context| {
        let stream = ctx.path().trim_start_matches('/').to_owned();
        akit_media::http::guard(
            Some(&p),
            ctx,
            MediaAction::Publish,
            &stream,
            "*",
            |ctx, access| {
                let body = format!(
                    "subject={} target={} query_token={}",
                    access.subject,
                    ctx.req().target,
                    access.query_token
                );
                ctx.respond(Response::text(200, body));
            },
        );
    });
    let engine = Arc::new(
        Engine::builder()
            .listen(Listener::tcp("127.0.0.1:0".parse().unwrap()).serve(service))
            .build()
            .unwrap(),
    );
    engine.start().unwrap();
    let addr = engine.local_addrs()[0];

    let r = get(addr, "/live/cam", &[]);
    assert_eq!(r.status, 401);
    assert_eq!(r.header("www-authenticate"), Some("Bearer realm=\"akit\""));
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    let r = get(
        addr,
        "/live/cam",
        &[("Authorization", "Bearer nope-nope-nope-nope")],
    );
    assert_eq!(r.status, 401);
    assert_eq!(
        r.header("www-authenticate"),
        Some("Bearer realm=\"akit\", error=\"invalid_token\"")
    );
    let r = get(
        addr,
        "/other",
        &[("Authorization", "Bearer 0123456789abcdef-static")],
    );
    assert_eq!(r.status, 403);
    let r = get(addr, "/live/cam?a=1&token=0123456789abcdef-static", &[]);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        r.body,
        "subject=token target=/live/cam?a=1&token=REDACTED query_token=0123456789abcdef-static"
    );
    // A bcrypt user: checked on the pool while the request waits, then at once.
    let basic = format!(
        "Basic {}",
        akio_crypto::base64::STANDARD.encode(b"bob:bob-pw")
    );
    for _ in 0..2 {
        let r = get(addr, "/live/cam", &[("Authorization", &basic)]);
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.starts_with("subject=bob "), "{}", r.body);
    }
    let wrong = format!(
        "Basic {}",
        akio_crypto::base64::STANDARD.encode(b"bob:wrong")
    );
    assert_eq!(
        get(addr, "/live/cam", &[("Authorization", &wrong)]).status,
        401
    );
    engine.shutdown();
}
