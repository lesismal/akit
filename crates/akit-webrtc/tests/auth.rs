//! `auth` on `whip` and `whep` (I126): fit's `webrtc/media/auth_test.go` `TestHandlerAuth`,
//! ported. A POST is let on by the `media_auth` policy its handler names (akit-media's guard), on
//! an akit listener with the roles `media_auth` and `webrtc.media` on the same engine, akio-webrtc
//! `Endpoint`s as the peers over loopback: 401 with a Bearer challenge without credentials or with
//! bad ones, 403 for what they do not allow, 201 for what they do, by token, signed token, query
//! token, Basic and a bcrypt password; OPTIONS and a session's DELETE need no credentials; a reload
//! changes the policy in place, and `public` opens an action to all; a reload without `auth` opens
//! the routes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use akio_crypto::base64::STANDARD as B64;
use akio_crypto::bcrypt;
use akio_webrtc::{Direction, Endpoint, MediaKind, Peer, PeerHandler};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

const PUB_TOKEN: &str = "publisher-token-0123456789";

struct Akit {
    engine: Arc<akio::Engine>,
    roles: Manager,
    cell: GraphCell,
    registry: Registry<HandlerParams>,
    slot: EngineSlot,
    addr: SocketAddr,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// whip and whep behind the policy "live" (`policy`, its JSON), or open without one.
fn doc(policy: Option<&str>) -> String {
    let (ma, auth) = match policy {
        Some(p) => (
            format!(r#""media_auth": [{p}],"#),
            r#", "params": {"auth": "live"}"#,
        ),
        None => (String::new(), ""),
    };
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], {ma}
            "routes": [{{"match": {{"path": ["/whip/*"]}}, "handler": {{"module": "whip"{auth}}}}},
                       {{"match": {{"path": ["/whep/*"]}}, "handler": {{"module": "whep"{auth}}}}}],
            "webrtc": {{"media": {{"listen": "127.0.0.1:0"}}}}}}"#
    )
}

fn live_policy(hash: &str, extra: &str) -> String {
    format!(
        r#"{{"name": "live", "secret": "a-secret-of-sixteen-bytes",
            "tokens": [{{"name": "cam", "token": "{PUB_TOKEN}", "publish": ["cam*"]}}],
            "users": [{{"name": "alice", "password": "alice-pw", "play": ["cam1"]}},
                      {{"name": "bob", "password_hash": "{hash}", "publish": ["bob"], "play": ["*"]}}]{extra}}}"#
    )
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let registry: Registry<HandlerParams> = Registry::new(&[akit_webrtc::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine =
            Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        let mut roles = Manager::new(
            &[akit_media::role::MEDIA_AUTH, akit_webrtc::MEDIA],
            Arc::clone(&engine),
            None,
        );
        let errs = roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        Akit {
            engine,
            roles,
            cell,
            registry,
            slot,
            addr,
        }
    }

    /// A reload: the roles, then the routes, as `akit run` applies a config.
    fn reload(&mut self, json: &str) {
        let config = akit_config::from_json(json, None).unwrap();
        let errs = self.roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        let graph = compile(&config, &self.registry, &Env::new(self.slot.clone())).unwrap();
        self.cell.swap(graph);
    }
}

struct Quiet;
impl PeerHandler for Quiet {}

/// An offer of a new peer of `ep`, sending (`whip`) or receiving (`whep`) audio and video.
fn offer(ep: &Endpoint, dir: Direction) -> (Peer, String) {
    ep.create_offer(Quiet, |api| {
        api.add_media(MediaKind::Audio, dir, None, None);
        api.add_media(MediaKind::Video, dir, None, None);
    })
    .unwrap()
}

struct Resp {
    status: u16,
    head: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

fn http(addr: SocketAddr, method: &str, path: &str, fields: &[(&str, &str)], body: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: akit\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let head = text.split_once("\r\n\r\n").map_or(&*text, |(h, _)| h);
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap();
    Resp {
        status,
        head: head.to_owned(),
    }
}

enum Cred<'a> {
    None,
    Bearer(&'a str),
    Basic(&'a str, &'a str),
}

/// POSTs a fresh offer of `ep` to `path`; the peer is closed unless it was taken.
fn post(a: &Akit, ep: &Endpoint, path: &str, c: Cred<'_>) -> Resp {
    let dir = if path.starts_with("/whip/") {
        Direction::SendOnly
    } else {
        Direction::RecvOnly
    };
    let (peer, sdp) = offer(ep, dir);
    let auth = match c {
        Cred::None => None,
        Cred::Bearer(t) => Some(format!("Bearer {t}")),
        Cred::Basic(u, p) => Some(format!("Basic {}", B64.encode(format!("{u}:{p}")))),
    };
    let mut fields = vec![("Content-Type", "application/sdp")];
    if let Some(a) = &auth {
        fields.push(("Authorization", a));
    }
    let r = http(a.addr, "POST", path, &fields, &sdp);
    if r.status != 201 {
        peer.close();
    }
    r
}

#[test]
fn handler_auth() {
    let hash = bcrypt::hash(b"bob-pw", bcrypt::MIN_COST).unwrap();
    let mut a = Akit::start(&doc(Some(&live_policy(&hash, ""))));
    let clients = akio::Engine::builder()
        .udp_client_threads(1)
        .build()
        .unwrap();
    clients.start().unwrap();
    let ep = Endpoint::bind(&clients, "127.0.0.1:0".parse().unwrap()).unwrap();

    let r = post(&a, &ep, "/whip/cam1", Cred::None);
    assert_eq!(r.status, 401);
    assert_eq!(r.header("www-authenticate"), Some("Bearer realm=\"akit\""));
    let r = post(&a, &ep, "/whip/cam1", Cred::Bearer("not-a-token-at-all"));
    assert_eq!(r.status, 401);
    assert!(
        r.header("www-authenticate")
            .unwrap()
            .contains("invalid_token")
    );
    assert_eq!(
        post(&a, &ep, "/whip/other", Cred::Bearer(PUB_TOKEN)).status,
        403
    );
    let r = post(&a, &ep, "/whip/cam1", Cred::Bearer(PUB_TOKEN));
    assert_eq!(r.status, 201);
    let session = r.header("location").unwrap().to_owned();

    // whep: alice plays cam1 and nothing else; bob's bcrypt password goes through the pool.
    assert_eq!(post(&a, &ep, "/whep/cam1", Cred::None).status, 401);
    assert_eq!(
        post(&a, &ep, "/whep/cam2", Cred::Basic("alice", "alice-pw")).status,
        403
    );
    assert_eq!(
        post(&a, &ep, "/whep/cam1", Cred::Basic("alice", "wrong")).status,
        401
    );
    assert_eq!(
        post(&a, &ep, "/whep/cam1", Cred::Basic("alice", "alice-pw")).status,
        201
    );
    assert_eq!(
        post(&a, &ep, "/whep/cam1", Cred::Basic("bob", "bob-pw")).status,
        201
    );
    assert_eq!(
        post(&a, &ep, "/whep/cam1", Cred::Basic("bob", "nope")).status,
        401
    );
    // A token akit signed, in the query.
    let key = akit_media::auth::registry::key(&a.engine);
    let p = akit_media::auth::registry::lookup(key, "live").unwrap();
    let grant = akit_media::auth::Grant {
        play: vec!["cam*".into()],
        ..akit_media::auth::Grant::default()
    };
    let (tok, _) = p.issue("viewer1", &grant, Duration::ZERO).unwrap();
    assert_eq!(
        post(&a, &ep, &format!("/whep/cam1?token={tok}"), Cred::None).status,
        201
    );
    assert_eq!(post(&a, &ep, "/whip/cam9", Cred::Bearer(&tok)).status, 403);
    // OPTIONS needs nothing; the session's DELETE needs nothing.
    assert_eq!(http(a.addr, "OPTIONS", "/whip/cam1", &[], "").status, 204);
    assert_eq!(http(a.addr, "DELETE", &session, &[], "").status, 200);

    // A reload changes the policy in place: the token may now publish other*, and play is
    // public.
    let changed =
        live_policy(&hash, r#", "public": ["play"]"#).replacen("\"cam*\"", "\"other*\"", 1);
    a.reload(&doc(Some(&changed)));
    let same = akit_media::auth::registry::lookup(key, "live").unwrap();
    assert!(
        Arc::ptr_eq(&p, &same),
        "the policy was replaced, not changed in place"
    );
    assert_eq!(
        post(&a, &ep, "/whip/cam2", Cred::Bearer(PUB_TOKEN)).status,
        403
    );
    assert_eq!(
        post(&a, &ep, "/whip/other1", Cred::Bearer(PUB_TOKEN)).status,
        201
    );
    assert_eq!(post(&a, &ep, "/whep/other1", Cred::None).status, 201);
    // A reload without auth opens the routes, and the policy goes.
    a.reload(&doc(None));
    assert!(akit_media::auth::registry::lookup(key, "live").is_none());
    assert_eq!(post(&a, &ep, "/whip/open1", Cred::None).status, 201);
    ep.close();
    clients.shutdown();
}
