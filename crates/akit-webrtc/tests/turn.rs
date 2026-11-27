//! The TURN server role, `webrtc.turn` (I111), on an engine of its own as `akit run` runs it:
//! fit's `webrtc/turn/turn_test.go` cases ported, driven by a small client written with
//! akio-turn's STUN codec over UDP, TCP and TLS.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio_turn::stun::{
    Class, Message, MessageBuilder, Method, attr, long_term_key, transaction_id,
};
use akit_roles::host::Manager;

const REALM: &str = "example.com";
const SECRET: &str = "a-shared-secret";

/// The role on an engine of its own.
struct Env {
    engine: Arc<akio::Engine>,
    roles: Manager,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

impl Env {
    fn start(member: &str) -> Env {
        let engine = akio::Engine::builder()
            .client_threads(1)
            .udp_client_threads(2)
            .build()
            .unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let mut roles = Manager::new(&[akit_webrtc::TURN], Arc::clone(&engine), None);
        roles.set_drain(Duration::from_secs(30));
        let mut env = Env { engine, roles };
        env.apply(member);
        env
    }

    fn try_apply(&mut self, member: &str) -> Vec<String> {
        let doc = format!(r#"{{"webrtc": {{"turn": {member}}}}}"#);
        let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
        self.roles.apply(&config)
    }

    fn apply(&mut self, member: &str) {
        let errs = self.try_apply(member);
        assert!(errs.is_empty(), "{errs:?}");
    }

    /// The UDP, TCP and TLS addresses the engine's listeners are bound at, in that order of
    /// kinds as added.
    fn addrs(&self) -> Vec<(SocketAddr, bool)> {
        self.engine
            .listeners()
            .into_iter()
            .map(|l| (l.local_addr, l.udp))
            .collect()
    }

    fn udp(&self) -> SocketAddr {
        self.addrs().into_iter().find(|(_, u)| *u).unwrap().0
    }

    fn tcp(&self, n: usize) -> SocketAddr {
        self.addrs()
            .into_iter()
            .filter(|(_, u)| !*u)
            .nth(n)
            .unwrap()
            .0
    }

    fn metrics(&self) -> String {
        let mut out = Vec::new();
        akit_roles::host::write_meters(&self.roles.meters(), &mut out);
        String::from_utf8(out).unwrap()
    }
}

/// A member on loopback with `extra` fields, `alice`'s password `pw`; `extra`'s `listen`,
/// `users` or `secret` replace the defaults.
fn member(extra: &str) -> String {
    let mut out = format!(r#"{{"realm": "{REALM}", "relay_ip": "127.0.0.1""#);
    for (key, default) in [
        ("listen", r#"["127.0.0.1:0"]"#.to_owned()),
        (
            "users",
            r#"[{"name": "alice", "password": "pw"}]"#.to_owned(),
        ),
        ("secret", format!(r#""{SECRET}""#)),
    ] {
        if !extra.contains(&format!(r#""{key}":"#)) {
            out.push_str(&format!(r#", "{key}": {default}"#));
        }
    }
    out.push_str(extra);
    out.push('}');
    out
}

/// A UDP port free now on loopback.
fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn waits(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A UDP peer that the clients relay to.
struct Peer(UdpSocket);

impl Peer {
    fn new() -> Peer {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Peer(s)
    }

    fn addr(&self) -> SocketAddr {
        self.0.local_addr().unwrap()
    }

    fn recv(&self) -> Option<(Vec<u8>, SocketAddr)> {
        let mut b = vec![0u8; 4096];
        let (n, from) = self.0.recv_from(&mut b).ok()?;
        b.truncate(n);
        Some((b, from))
    }
}

/// How a client reaches the server.
enum Wire {
    Udp(UdpSocket, SocketAddr),
    /// A TCP or TLS connection the client engine dialed: what it sends goes through `conn`,
    /// what it receives arrives on `rx`, framed by [`Wire::next`].
    Stream {
        conn: akio::Conn<StreamHandler>,
        rx: Receiver<Vec<u8>>,
        buf: Vec<u8>,
        _engine: Arc<akio::Engine>,
    },
}

/// The client side of a stream: every read sent to the test.
struct StreamHandler(Sender<Vec<u8>>);

impl akio::Handler for StreamHandler {
    fn on_data(&mut self, _c: &akio::Conn<Self>, data: &[u8]) {
        let _ = self.0.send(data.to_vec());
    }
}

impl Wire {
    fn send(&self, b: &[u8]) {
        match self {
            Wire::Udp(s, to) => {
                s.send_to(b, *to).unwrap();
            }
            Wire::Stream { conn, .. } => {
                let _ = conn.send(b);
            }
        }
    }

    /// The next message: a datagram, or a STUN or ChannelData message framed on the stream.
    fn next(&mut self) -> Option<Vec<u8>> {
        match self {
            Wire::Udp(s, _) => {
                let mut b = vec![0u8; 4096];
                let (n, _) = s.recv_from(&mut b).ok()?;
                b.truncate(n);
                Some(b)
            }
            Wire::Stream { rx, buf, .. } => loop {
                if buf.len() >= 4 {
                    let len = usize::from(u16::from_be_bytes([buf[2], buf[3]]));
                    let total = if buf[0] & 0xc0 == 0x40 {
                        // ChannelData, padded to four bytes on a stream.
                        (4 + len + 3) & !3
                    } else {
                        20 + len
                    };
                    if buf.len() >= total {
                        let mut m: Vec<u8> = buf.drain(..total).collect();
                        if m[0] & 0xc0 == 0x40 {
                            m.truncate(4 + len);
                        }
                        return Some(m);
                    }
                }
                let more = rx.recv_timeout(Duration::from_secs(5)).ok()?;
                buf.extend_from_slice(&more);
            },
        }
    }
}

/// A TURN client of one user.
struct Client {
    wire: Wire,
    user: String,
    key: [u8; 16],
    nonce: String,
}

impl Client {
    fn udp(server: SocketAddr, user: &str, pass: &str) -> Client {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        Client::over(Wire::Udp(s, server), user, pass)
    }

    fn stream(server: SocketAddr, tls: bool, user: &str, pass: &str) -> Client {
        let engine = Arc::new(akio::Engine::builder().client_threads(1).build().unwrap());
        engine.start().unwrap();
        let (tx, rx) = channel();
        let (ctx, crx) = channel();
        let opened = Arc::new(Mutex::new(Some(ctx)));
        let acceptor = akio::fn_acceptor(move |c: &akio::Conn<StreamHandler>| {
            if let Some(tx) = opened.lock().unwrap().take() {
                let _ = tx.send(c.clone());
            }
            StreamHandler(tx.clone())
        });
        if tls {
            let cfg = akio::tls::ClientConfig::builder()
                .insecure_skip_verify()
                .build()
                .unwrap();
            let a = akio::tls::Tls::client(cfg, "localhost", acceptor).unwrap();
            engine.dial(server, a, |e| panic!("dial: {e}"));
        } else {
            engine.dial(server, acceptor, |e| panic!("dial: {e}"));
        }
        let conn = crx.recv_timeout(Duration::from_secs(5)).expect("dialed");
        Client::over(
            Wire::Stream {
                conn,
                rx,
                buf: Vec::new(),
                _engine: engine,
            },
            user,
            pass,
        )
    }

    fn over(wire: Wire, user: &str, pass: &str) -> Client {
        Client {
            wire,
            user: user.to_owned(),
            key: long_term_key(user, REALM, pass),
            nonce: String::new(),
        }
    }

    /// Sends a request, again after a 401 or 438 challenge; the response, or `None` when none
    /// came.
    fn request(
        &mut self,
        m: Method,
        f: impl Fn(MessageBuilder) -> MessageBuilder,
    ) -> Option<Vec<u8>> {
        for _ in 0..3 {
            let tid = transaction_id();
            let mut b = f(MessageBuilder::new(m, Class::Request, tid));
            if !self.nonce.is_empty() {
                b = b
                    .attr(attr::USERNAME, self.user.as_bytes())
                    .attr(attr::REALM, REALM.as_bytes())
                    .attr(attr::NONCE, self.nonce.as_bytes())
                    .integrity(&self.key);
            }
            self.wire.send(&b.fingerprint().build());
            let got = loop {
                let got = self.wire.next()?;
                if Message::parse(&got).unwrap().transaction_id() == tid {
                    break got;
                }
            };
            let resp = Message::parse(&got).unwrap();
            match resp.error_code() {
                Some((401, _)) if self.nonce.is_empty() => {}
                Some((438, _)) => {}
                _ => return Some(got),
            }
            self.nonce = resp.get_str(attr::NONCE).unwrap().to_owned();
            assert_eq!(resp.get_str(attr::REALM), Some(REALM));
            assert_eq!(resp.get_str(attr::SOFTWARE), Some("akit"));
        }
        panic!("still challenged");
    }

    /// Allocates: the relayed address, or the error code.
    fn allocate(&mut self) -> Result<SocketAddr, u16> {
        let got = self
            .request(Method::ALLOCATE, |b| {
                b.u32(attr::REQUESTED_TRANSPORT, 17 << 24)
            })
            .expect("an answer");
        let m = Message::parse(&got).unwrap();
        match m.error_code() {
            Some((code, _)) => Err(code),
            None => Ok(m.get_xor_addr(attr::XOR_RELAYED_ADDRESS).unwrap()),
        }
    }

    fn code_of(m: Option<Vec<u8>>) -> Option<u16> {
        let m = m.expect("an answer");
        Message::parse(&m).unwrap().error_code().map(|(c, _)| c)
    }

    /// Refreshes the allocation for `lifetime` seconds: the error code, or `None`.
    fn refresh(&mut self, lifetime: u32) -> Option<u16> {
        Client::code_of(self.request(Method::REFRESH, |b| b.u32(attr::LIFETIME, lifetime)))
    }

    fn permit(&mut self, peer: SocketAddr) -> Option<u16> {
        Client::code_of(self.request(Method::CREATE_PERMISSION, |b| {
            b.xor_addr(attr::XOR_PEER_ADDRESS, peer)
        }))
    }

    fn bind(&mut self, channel: u16, peer: SocketAddr) -> Option<u16> {
        Client::code_of(self.request(Method::CHANNEL_BIND, |b| {
            b.u32(attr::CHANNEL_NUMBER, u32::from(channel) << 16)
                .xor_addr(attr::XOR_PEER_ADDRESS, peer)
        }))
    }

    fn send_indication(&self, peer: SocketAddr, data: &[u8]) {
        let ind = MessageBuilder::new(Method::SEND, Class::Indication, transaction_id())
            .xor_addr(attr::XOR_PEER_ADDRESS, peer)
            .attr(attr::DATA, data)
            .build();
        self.wire.send(&ind);
    }

    fn send_channel(&self, channel: u16, data: &[u8]) {
        let mut b = channel.to_be_bytes().to_vec();
        b.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
        b.extend_from_slice(data);
        if matches!(self.wire, Wire::Stream { .. }) {
            while b.len() % 4 != 0 {
                b.push(0);
            }
        }
        self.wire.send(&b);
    }

    /// What the server relayed from a peer: by Data indication or ChannelData.
    fn relayed(&mut self) -> Option<Vec<u8>> {
        let got = self.wire.next()?;
        if got[0] & 0xc0 == 0x40 {
            return Some(got[4..].to_vec());
        }
        let m = Message::parse(&got).unwrap();
        assert_eq!((m.method(), m.class()), (Method::DATA, Class::Indication));
        Some(m.get(attr::DATA).unwrap().to_vec())
    }

    fn delete(&mut self) {
        assert_eq!(self.refresh(0), None);
    }
}

/// Relays `msg` both ways between `c` and `p`, by channel 0x4000 when `bind`.
fn exchange(c: &mut Client, p: &Peer, relayed: SocketAddr, msg: &[u8], bind: bool) {
    if bind {
        c.send_channel(0x4000, msg);
    } else {
        c.send_indication(p.addr(), msg);
    }
    let (got, from) = p.recv().expect("the peer got nothing");
    assert_eq!(got, msg);
    assert_eq!(from, relayed);
    p.0.send_to(msg, relayed).unwrap();
    assert_eq!(c.relayed().expect("the client got nothing"), msg);
}

/// fit's `TestRelay`: a password, a key and the secret's credentials allocate, and relay both
/// ways by indication and by channel; a client's delete frees its allocation.
#[test]
fn relay() {
    let p = Peer::new();
    let key: String = long_term_key("bob", REALM, "bobpw")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let env = Env::start(&member(&format!(
        r#", "users": [{{"name": "alice", "password": "pw"}}, {{"name": "bob", "key": "0x{key}"}}],
            "allowed_peers": ["{}"]"#,
        p.addr()
    )));
    let (eu, ep) =
        akio_turn::ephemeral_credentials(SECRET.as_bytes(), "carol", Duration::from_secs(3600));
    for (user, pass, bind) in [
        ("alice", "pw", false),
        ("alice", "pw", true),
        ("bob", "bobpw", true),
        (eu.as_str(), ep.as_str(), true),
    ] {
        let mut c = Client::udp(env.udp(), user, pass);
        let relayed = c.allocate().unwrap_or_else(|e| panic!("{user}: {e}"));
        assert_eq!(
            relayed.ip(),
            "127.0.0.1".parse::<std::net::IpAddr>().unwrap()
        );
        assert_eq!(c.permit(p.addr()), None);
        if bind {
            assert_eq!(c.bind(0x4000, p.addr()), None);
        }
        for msg in [&b"one"[..], b"two", &[b'x'; 1200]] {
            exchange(&mut c, &p, relayed, msg, bind);
        }
        c.delete();
    }
    waits("the allocations' end", || {
        env.metrics().contains("akit_turn_allocations 0")
    });
    let m = env.metrics();
    for want in [
        "akit_turn_restarts_total 0",
        "akit_turn_draining_servers 0",
        r#"akit_turn_relayed_datagrams_total{direction="to_peer"} 12"#,
        r#"akit_turn_relayed_datagrams_total{direction="from_peer"} 12"#,
    ] {
        assert!(m.contains(want), "metrics lack {want}:\n{m}");
    }
}

/// fit's `TestRefusedCredentials`: a wrong password, an unknown user, expired credentials and
/// those of another secret are refused with 401.
#[test]
fn refused_credentials() {
    let env = Env::start(&member(""));
    let (expired_u, expired_p) = {
        // A username that expired a minute ago, with the secret's password for it.
        let (u, _) = akio_turn::ephemeral_credentials(SECRET.as_bytes(), "carol", Duration::ZERO);
        let (_, rest) = u.split_once(':').unwrap();
        let exp: u64 = u.split(':').next().unwrap().parse().unwrap();
        let user = format!("{}:{rest}", exp - 60);
        let mac = akio_crypto::mac::Hmac::<akio_crypto::hash::Sha1>::mac(
            SECRET.as_bytes(),
            user.as_bytes(),
        );
        let pass = akio_crypto::base64::STANDARD.encode(mac);
        (user, pass)
    };
    let (other_u, other_p) =
        akio_turn::ephemeral_credentials(b"another secret", "carol", Duration::from_secs(3600));
    for (user, pass) in [
        ("alice", "nope"),
        ("mallory", "pw"),
        (expired_u.as_str(), expired_p.as_str()),
        (other_u.as_str(), other_p.as_str()),
    ] {
        let mut c = Client::udp(env.udp(), user, pass);
        assert_eq!(c.allocate(), Err(401), "{user}");
    }
    assert!(env.metrics().contains("akit_turn_allocations 0"));
}

/// fit's `TestQuotas`: one allocation more than `max_allocations_per_user` is refused with 486,
/// one more than `max_allocations` with 508.
#[test]
fn quotas() {
    let env = Env::start(&member(
        r#", "users": [{"name": "alice", "password": "pw"}, {"name": "bob", "password": "pw"}],
            "max_allocations": 2, "max_allocations_per_user": 1"#,
    ));
    let mut a = Client::udp(env.udp(), "alice", "pw");
    a.allocate().unwrap();
    assert_eq!(Client::udp(env.udp(), "alice", "pw").allocate(), Err(486));
    let mut b = Client::udp(env.udp(), "bob", "pw");
    b.allocate().unwrap();
    let (u, p) =
        akio_turn::ephemeral_credentials(SECRET.as_bytes(), "carol", Duration::from_secs(60));
    assert_eq!(Client::udp(env.udp(), &u, &p).allocate(), Err(508));
}

/// fit's `TestCaps`: one permission past `max_permissions` and one channel past `max_channels`
/// are refused with 508.
#[test]
fn caps() {
    let env = Env::start(&member(r#", "max_permissions": 2, "max_channels": 1"#));
    let mut c = Client::udp(env.udp(), "alice", "pw");
    c.allocate().unwrap();
    assert_eq!(c.permit("203.0.113.1:9".parse().unwrap()), None);
    assert_eq!(c.permit("203.0.113.2:9".parse().unwrap()), None);
    assert_eq!(c.permit("203.0.113.3:9".parse().unwrap()), Some(508));
    assert_eq!(c.bind(0x4000, "203.0.113.1:9".parse().unwrap()), None);
    assert_eq!(c.bind(0x4001, "203.0.113.2:9".parse().unwrap()), Some(508));
}

/// fit's `TestPrivatePeers`: internal addresses are refused unless `allowed_peers` lists them;
/// `denied_peers` refuses what `allowed_peers` does not list first.
#[test]
fn private_peers() {
    let mut env = Env::start(&member(""));
    let mut c = Client::udp(env.udp(), "alice", "pw");
    c.allocate().unwrap();
    assert_eq!(c.permit("10.0.0.2:9".parse().unwrap()), Some(403));
    assert_eq!(c.permit("100.64.0.1:9".parse().unwrap()), Some(403));
    assert_eq!(c.permit("8.8.8.8:53".parse().unwrap()), None);
    // In place: the allocation is kept, the new rules apply to its next request.
    env.apply(&member(
        r#", "allowed_peers": ["10.0.0.3"], "denied_peers": ["10.0.0.0/8", "8.8.8.8:53"]"#,
    ));
    assert_eq!(c.permit("10.0.0.3:9".parse().unwrap()), None);
    assert_eq!(c.permit("10.0.0.2:9".parse().unwrap()), Some(403));
    // A permission is of the address, whatever port; a denied port refuses a channel.
    assert_eq!(c.bind(0x4000, "8.8.8.8:53".parse().unwrap()), Some(403));
    assert_eq!(c.bind(0x4001, "8.8.8.8:54".parse().unwrap()), None);
}

/// fit's `TestOwnAddress`, with akio-turn's stopgap (decision 039): `relay_ip` is relayed to on
/// the ports `allowed_peers` lists with it alone, never on its other ports, where akit's own
/// listeners are, whatever the prefixes say.
#[test]
fn own_address() {
    let p = Peer::new();
    let mut env = Env::start(&member(r#", "allowed_peers": ["127.0.0.0/8"]"#));
    let mut c = Client::udp(env.udp(), "alice", "pw");
    let relayed = c.allocate().unwrap();
    // A permission for the address is granted, as fit's; its datagrams are not relayed.
    assert_eq!(c.permit(p.addr()), None);
    p.0.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    for _ in 0..5 {
        c.send_indication(p.addr(), b"in");
        assert!(
            p.recv().is_none(),
            "a port of relay_ip that is not listed got a datagram"
        );
    }
    assert_eq!(c.bind(0x4000, p.addr()), Some(403));
    p.0.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    env.apply(&member(&format!(
        r#", "allowed_peers": ["127.0.0.0/8", "{}"]"#,
        p.addr()
    )));
    exchange(&mut c, &p, relayed, b"listed", false);
}

/// fit's `TestReload`: users, the secret, the peers and the access change in place, the
/// allocations kept; a change of the server's settings starts a new one (A258: the quotas among
/// them), which on the same address ends the old one's allocations, and on another address lets
/// the old one drain.
#[test]
fn reload() {
    let p = Peer::new();
    let port = free_port();
    let listen = format!(r#", "listen": ["127.0.0.1:{port}"]"#);
    let allowed = format!(r#", "allowed_peers": ["{}"]"#, p.addr());
    let mut env = Env::start(&member(&format!("{listen}{allowed}")));
    let addr = env.udp();
    let mut c = Client::udp(addr, "alice", "pw");
    let relayed = c.allocate().unwrap();
    assert_eq!(c.permit(p.addr()), None);
    exchange(&mut c, &p, relayed, b"before", false);

    // The users and the secret change in place: the allocation and its nonce go on.
    let users = format!(
        r#"{{"listen": ["127.0.0.1:{port}"], "realm": "{REALM}", "users": [{{"name": "alice", "password": "pw"}}, {{"name": "dave", "password": "pw2"}}],
            "relay_ip": "127.0.0.1"{allowed}}}"#
    );
    env.apply(&users);
    assert!(env.metrics().contains("akit_turn_restarts_total 0"));
    exchange(&mut c, &p, relayed, b"after", false);
    assert!(Client::udp(addr, "dave", "pw2").allocate().is_ok());
    let (u, pw) =
        akio_turn::ephemeral_credentials(SECRET.as_bytes(), "carol", Duration::from_secs(60));
    assert_eq!(Client::udp(addr, &u, &pw).allocate(), Err(401));
    assert_eq!(c.refresh(600), None);

    // A change of the realm on the same address: a new server, the old allocations ended.
    let realm = users.replace(REALM, "other.example");
    env.apply(&realm);
    assert!(env.metrics().contains("akit_turn_restarts_total 1"));
    assert!(env.metrics().contains("akit_turn_allocations 0"));
    assert_eq!(env.udp(), addr);

    // Back, then a new address: the old server drains, its allocation still relays.
    env.apply(&member(&format!("{listen}{allowed}")));
    let mut c = Client::udp(addr, "alice", "pw");
    let relayed = c.allocate().unwrap();
    assert_eq!(c.permit(p.addr()), None);
    env.apply(&member(&allowed));
    assert!(
        env.metrics().contains("akit_turn_draining_servers 1"),
        "{}",
        env.metrics()
    );
    exchange(&mut c, &p, relayed, b"draining", false);
    let new = env
        .addrs()
        .into_iter()
        .filter(|(a, u)| *u && *a != addr)
        .map(|(a, _)| a)
        .next()
        .unwrap();
    assert!(Client::udp(new, "alice", "pw").allocate().is_ok());
}

/// A reload that cannot start the new server keeps the one running (fit's `TestRestartFallsBack`
/// and `TestReloadRefused`): a port range, which akit refuses until akio A256, and an address in
/// use.
#[test]
fn reload_refused() {
    let mut env = Env::start(&member(""));
    let addr = env.udp();
    let mut c = Client::udp(addr, "alice", "pw");
    c.allocate().unwrap();
    let errs = env.try_apply(&member(r#", "port_min": 49152, "port_max": 49999"#));
    assert!(errs.iter().any(|e| e.contains("A256")), "{errs:?}");
    let busy = UdpSocket::bind("127.0.0.1:0").unwrap();
    let errs = env.try_apply(&format!(
        r#"{{"listen": ["{}"], "realm": "{REALM}", "secret": "s", "relay_ip": "127.0.0.1"}}"#,
        busy.local_addr().unwrap()
    ));
    assert!(!errs.is_empty());
    assert_eq!(env.udp(), addr);
    assert_eq!(c.refresh(600), None);
}

/// fit's `TestStream`: allocations over TCP and TLS relay UDP both ways, by indication and by
/// channel, the ChannelData padded on the stream.
#[test]
fn stream() {
    let dir = std::env::temp_dir().join(format!("akit-webrtc-turn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key) = self_signed();
    let (cf, kf) = (dir.join("c.pem"), dir.join("k.pem"));
    std::fs::write(&cf, cert).unwrap();
    std::fs::write(&kf, key).unwrap();
    let p = Peer::new();
    let env = Env::start(&member(&format!(
        r#", "listen_tcp": ["127.0.0.1:0"], "listen_tls": ["127.0.0.1:0"],
            "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}, "allowed_peers": ["{}"]"#,
        cf.display(),
        kf.display(),
        p.addr()
    )));
    for (n, tls) in [(0, false), (1, true)] {
        let mut c = Client::stream(env.tcp(n), tls, "alice", "pw");
        let relayed = c.allocate().unwrap();
        assert_eq!(c.permit(p.addr()), None);
        exchange(&mut c, &p, relayed, b"by indication", false);
        assert_eq!(c.bind(0x4000, p.addr()), None);
        exchange(&mut c, &p, relayed, b"odd", true);
        c.delete();
    }
    let files = env.roles.files();
    assert_eq!(files.len(), 2, "{files:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// fit's `TestAccess`: a client the member's access refuses gets no answer; a reload of the
/// access applies to the next client, without a new server.
#[test]
fn access() {
    let mut env = Env::start(&member(r#", "access": {"deny": ["127.0.0.1"]}"#));
    let addr = env.udp();
    let mut c = Client::udp(addr, "alice", "pw");
    assert!(
        c.request(Method::ALLOCATE, |b| b
            .u32(attr::REQUESTED_TRANSPORT, 17 << 24))
            .is_none()
    );
    env.apply(&member(""));
    assert!(env.metrics().contains("akit_turn_restarts_total 0"));
    let mut c = Client::udp(addr, "alice", "pw");
    assert!(c.allocate().is_ok());
}

/// A self-signed certificate for `localhost` and its key, in PEM.
fn self_signed() -> (String, String) {
    let key = akio_crypto::p256::SecretKey::generate().unwrap();
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let mut p = akio_x509::CertificateParams::new(vec!["localhost"]);
    p.not_before = 1_700_000_000;
    p.not_after = 4_000_000_000;
    let cert = p.self_signed(&key).unwrap();
    (
        cert.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    )
}

/// akio-turn's own client (the one akio-webrtc's relayed candidates use) allocates through
/// akit's server and relays both ways, by indication and by channel.
#[test]
fn akio_client() {
    use akio_turn::client::{Client as AkioClient, ClientConfig, ClientEvent};
    let p = Peer::new();
    let env = Env::start(&member(&format!(r#", "allowed_peers": ["{}"]"#, p.addr())));
    p.0.set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    for channels in [false, true] {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let cfg = ClientConfig::new(env.udp(), "alice", "pw").use_channels(channels);
        let mut c = AkioClient::new(cfg, Instant::now());
        let mut relayed = None;
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sent = 0;
        while got.len() < 3 {
            assert!(
                Instant::now() < deadline,
                "channels={channels}: got {got:?}"
            );
            let now = Instant::now();
            c.handle_timeout(now);
            while let Some(t) = c.poll_transmit() {
                s.send_to(&t, env.udp()).unwrap();
            }
            while let Some(e) = c.poll_event() {
                match e {
                    ClientEvent::Allocated { relayed: r, .. } => relayed = Some(r),
                    ClientEvent::Failed(e) => panic!("{e:?}"),
                    _ => {}
                }
            }
            if let Some(r) = relayed {
                if sent == got.len() && sent < 3 {
                    c.send_to(now, p.addr(), format!("m{sent}").as_bytes());
                    sent += 1;
                }
                // The peer echoes what reached it from the relay.
                if let Some((d, from)) = p.recv() {
                    assert_eq!(from, r);
                    p.0.send_to(&d, r).unwrap();
                }
            }
            let mut b = [0u8; 2048];
            if let Ok((n, _)) = s.recv_from(&mut b) {
                if let Some((peer, d)) = c.handle_input(Instant::now(), &b[..n]) {
                    assert_eq!(peer, p.addr());
                    got.push(String::from_utf8_lossy(d).into_owned());
                }
            }
        }
        assert_eq!(got, ["m0", "m1", "m2"]);
    }
}
