//! Members behind NAT, linked out to the manager (I139b, fit's `cluster/linkserver.go`,
//! `linkproto.go` and `enroll.go`, its I149): the manager's link listener, `link_address`, a
//! WebSocket over TLS with a self-signed certificate the manager makes once and keeps in the
//! store's document (`link_cert`, fit's), which each member pins; enrollment tokens, made on the
//! Nodes page, used once for a credential of the member's own; the member's Admin API carried over
//! its link, as the manager calls the others over HTTP.
//!
//! The protocol is fit's byte for byte (decision 041, 12), so that an akit member links to a fit
//! manager and the reverse: its messages (a type byte, then JSON, or a call's id, head and body),
//! its label `fit cluster link v1`, its proofs (fit's `linktoken`, decision 038), and a credential
//! written on the wire with fit's prefix `fitc1.`. A person's enrollment token is `akite1.`; a fit
//! manager's `fite1.` is written with akit's prefix to link to it (decision 041).
//!
//! Nothing blocks an I/O thread: a hello is checked on it, an enrollment kept on the manager's
//! pool. A link is idle-checked by its own pings every 15 s, and closed when the member was not
//! heard from in 45 s: keep-alive's time.

#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime};

use akio_crypto::base64::STANDARD;
use akio_http::ws::{MessageRef, WsHandler, WsSender};
use akio_http::{Context, Method};
use akit_config::value::Value;
use akit_roles::linktoken::{self, Secret, Transcript};
use akit_roles::pages::{fail, json as answer};

use crate::json::{self, Obj, quoted};
use crate::member::{ACCESS_LINK, Document, Member, format_fingerprint};
use crate::transport::{
    KIND_NOT_LINKED, KIND_TIMEOUT, KIND_TOO_LARGE, KIND_UNREACHABLE, Problem, Reply,
};
use crate::{Inner, Manager, Refusal};

pub(crate) const MSG_CHALLENGE: u8 = b'c';
pub(crate) const MSG_HELLO: u8 = b'h';
pub(crate) const MSG_WELCOME: u8 = b'w';
pub(crate) const MSG_REFUSE: u8 = b'r';
pub(crate) const MSG_REVOKED: u8 = b'g';
pub(crate) const MSG_REQUEST: u8 = b'q';
pub(crate) const MSG_RESPONSE: u8 = b's';
pub(crate) const MSG_CANCEL: u8 = b'x';

pub(crate) const MODE_ENROLL: &str = "enroll";
pub(crate) const MODE_LINK: &str = "link";
/// The label the proofs bind: fit's.
pub(crate) const LABEL: &str = "fit cluster link v1";
/// The prefix of a member's credential on the wire and where the member keeps it: fit's.
pub(crate) const CREDENTIAL_PREFIX: &str = "fitc1.";

/// Why a link is refused: fit's reasons.
pub const REASON_UNKNOWN: &str = "unknown";
/// See [`REASON_UNKNOWN`].
pub const REASON_PROOF: &str = "proof";
/// See [`REASON_UNKNOWN`].
pub const REASON_USED: &str = "used";
/// See [`REASON_UNKNOWN`].
pub const REASON_EXPIRED: &str = "expired";
/// See [`REASON_UNKNOWN`].
pub const REASON_BUSY: &str = "busy";
/// See [`REASON_UNKNOWN`].
pub const REASON_REVOKED: &str = "revoked";
/// See [`REASON_UNKNOWN`].
pub const REASON_REPLACED: &str = "replaced";
/// See [`REASON_UNKNOWN`].
pub const REASON_BAD_HELLO: &str = "bad_hello";

pub(crate) const MAX_HELLO: usize = 4 << 10;
/// The calls a link carries at once.
pub const MAX_LINK_CALLS: usize = 16;
pub(crate) const MAX_HEAD: usize = 64 << 10;
const MAX_PENDING: usize = 32;
const LINKS_SLACK: usize = 16;
/// How often a side pings the other, and how long it waits to hear from it.
pub const PING: Duration = Duration::from_secs(15);
/// See [`PING`].
pub const DEAD: Duration = Duration::from_secs(45);
/// The longest a link's handshake takes.
pub const HANDSHAKE: Duration = Duration::from_secs(10);
/// The shortest enrollment token's life.
pub const MIN_ENROLL_TTL: Duration = Duration::from_secs(60);

pub(crate) const ANSWER_TOO_LARGE: &str = "too_large";
pub(crate) const ANSWER_BUSY: &str = "busy";
pub(crate) const ANSWER_TIMEOUT: &str = "timeout";

const CLOSE_REFUSED: u16 = 4001;
const CLOSE_REVOKED: u16 = 4002;

const ACTION_ENROLL_TOKEN: &str = "node.enroll_token";
const ACTION_ENROLLED: &str = "node.enrolled";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A token of `prefix` read: its ID and secret.
pub(crate) fn secret_of(tok: &str, prefix: &str) -> Option<Secret> {
    let rest = tok.strip_prefix(prefix)?;
    Secret::parse(&format!("{}{rest}", akit_config::roles::LINK_TOKEN_PREFIX)).ok()
}

/// `s` written with `prefix`.
pub(crate) fn token_as(s: &Secret, prefix: &str) -> String {
    s.token()
        .replacen(akit_config::roles::LINK_TOKEN_PREFIX, prefix, 1)
}

/// The proofs' transcript of a handshake: fit's `transcript`.
pub(crate) struct Tr {
    id: String,
    member: Vec<u8>,
    manager: Vec<u8>,
    mode: [&'static str; 1],
}

impl Tr {
    pub(crate) fn new(id: &str, member: &[u8], manager: &[u8], mode: &str) -> Tr {
        Tr {
            id: id.to_owned(),
            member: member.to_vec(),
            manager: manager.to_vec(),
            mode: [if mode == MODE_ENROLL {
                MODE_ENROLL
            } else {
                MODE_LINK
            }],
        }
    }

    pub(crate) fn t(&self) -> Transcript<'_> {
        Transcript {
            label: LABEL,
            token: &self.id,
            dialer: &self.member,
            listener: &self.manager,
            extra: &self.mode,
        }
    }
}

/// A message of type `t` with JSON `body`.
pub(crate) fn msg(t: u8, body: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len().saturating_add(1));
    b.push(t);
    b.extend_from_slice(body.as_bytes());
    b
}

/// A call's message: its type, id, head's length, head and body: fit's `encodeCall`.
pub(crate) fn call_msg(t: u8, id: u64, head: &str, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(
        13usize
            .saturating_add(head.len())
            .saturating_add(body.len()),
    );
    b.push(t);
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(&u32::try_from(head.len()).unwrap_or(u32::MAX).to_be_bytes());
    b.extend_from_slice(head.as_bytes());
    b.extend_from_slice(body);
    b
}

/// A call's message read: its id, head and body: fit's `decodeCall`.
pub(crate) fn read_call(data: &[u8]) -> Option<(u64, Value, &[u8])> {
    let id = u64::from_be_bytes(data.get(..8)?.try_into().ok()?);
    let n = usize::try_from(u32::from_be_bytes(data.get(8..12)?.try_into().ok()?)).ok()?;
    if n > MAX_HEAD {
        return None;
    }
    let end = 12usize.checked_add(n)?;
    let head = json::parse(data.get(12..end)?)?;
    Some((id, head, data.get(end..)?))
}

/// Base64 of `b`, as Go writes `[]byte` in JSON.
pub(crate) fn b64(b: &[u8]) -> String {
    quoted(&STANDARD.encode(b))
}

/// The bytes of member `k`, base64 as Go writes `[]byte`.
pub(crate) fn bytes_of(v: &Value, k: &str) -> Vec<u8> {
    STANDARD.decode(json::str_of(v, k)).unwrap_or_default()
}

/// Header fields as Go's `http.Header` writes them: a name, and its values.
pub(crate) fn header_json(h: &[(String, String)]) -> String {
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for (n, v) in h {
        match grouped.iter_mut().find(|(g, _)| g.eq_ignore_ascii_case(n)) {
            Some((_, vs)) => vs.push(v.clone()),
            None => grouped.push((n.clone(), vec![v.clone()])),
        }
    }
    let mut o = Obj::new();
    for (n, vs) in &grouped {
        o = o.raw(n, &json::string_list(vs));
    }
    o.end()
}

/// Header fields from Go's `http.Header` as JSON.
pub(crate) fn header_of(v: Option<&Value>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(Value::Object(ms)) = v {
        for (n, vs) in ms {
            if let Value::Array(a) = vs {
                for x in a {
                    if let Value::String(s) = x {
                        out.push((n.clone(), s.clone()));
                    }
                }
            }
        }
    }
    out
}

/// How a member's link is: fit's `LinkState`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    since: Option<SystemTime>,
    remote: String,
    host: String,
    refused: Option<(SystemTime, String, String, String)>,
}

/// The link listener and the links open.
#[derive(Default)]
pub(crate) struct Hub {
    listener: Option<(akio::ListenerId, SocketAddr)>,
    fingerprint: String,
    links: HashMap<String, Arc<MLink>>,
    seen: HashMap<String, Seen>,
    conns: usize,
    pending: usize,
}

/// A call over a link.
struct Call {
    done: Option<Box<dyn FnOnce(Result<Reply, Problem>) + Send>>,
    timer: Option<akio::timer::SendTimer>,
    start: Instant,
}

#[derive(Default)]
struct LinkSt {
    open: bool,
    closed: bool,
    cred: String,
    name: String,
    host: String,
    since: Option<SystemTime>,
    calls: HashMap<u64, Call>,
    next: u64,
}

/// A member's link, as the manager holds it.
pub(crate) struct MLink {
    ws: WsSender,
    remote: String,
    nonce: [u8; 32],
    st: Mutex<LinkSt>,
    /// When the member was last heard from, in ms since `epoch`.
    heard: AtomicU64,
    epoch: Instant,
}

impl MLink {
    fn touch(&self) {
        let ms = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.heard.store(ms, Ordering::Release);
    }

    fn quiet(&self) -> Duration {
        let ms = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(ms.saturating_sub(self.heard.load(Ordering::Acquire)))
    }

    fn refuse_with(&self, t: u8, code: u16, reason: &str, msg_text: &str) {
        let _ = self.ws.send_binary(&msg(
            t,
            &Obj::new()
                .str("reason", reason)
                .str("message", msg_text)
                .end(),
        ));
        self.ws.close(code, reason);
    }

    /// Ends the calls under way with `p`.
    fn fail_all(&self, why: &str) {
        let calls: Vec<Call> = {
            let mut st = lock(&self.st);
            st.closed = true;
            st.calls.drain().map(|(_, c)| c).collect()
        };
        for mut c in calls {
            if let Some(t) = c.timer.take() {
                let _ = t.cancel();
            }
            if let Some(d) = c.done.take() {
                d(Err(Problem::new(KIND_UNREACHABLE, why.to_owned())));
            }
        }
    }

    /// Sends `method path` to the member: its answer, or why none: fit's `mlink.call`.
    #[allow(clippy::too_many_arguments)]
    fn call(
        self: &Arc<Self>,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
        max: usize,
        done: Box<dyn FnOnce(Result<Reply, Problem>) + Send>,
    ) {
        let body = body.unwrap_or_default();
        if body.len() > max {
            return done(Err(Problem::new(
                KIND_TOO_LARGE,
                "the request is larger than max_response_bytes, which bounds a link's messages \
                 both ways",
            )));
        }
        let id = {
            let mut st = lock(&self.st);
            if !st.open || st.closed {
                drop(st);
                return done(Err(Problem::new(
                    KIND_UNREACHABLE,
                    "the member's link closed",
                )));
            }
            if st.calls.len() >= MAX_LINK_CALLS {
                drop(st);
                return done(Err(Problem::new(
                    KIND_UNREACHABLE,
                    "the member's link carries as many calls as it takes",
                )));
            }
            st.next = st.next.wrapping_add(1);
            let id = st.next;
            st.calls.insert(
                id,
                Call {
                    done: Some(done),
                    timer: None,
                    start: Instant::now(),
                },
            );
            id
        };
        let weak = Arc::downgrade(self);
        let timer = akio::timer::after_send(timeout, move || {
            if let Some(l) = weak.upgrade() {
                l.give_up(id, KIND_TIMEOUT, "the member did not answer in time");
            }
        })
        .ok();
        if let Some(c) = lock(&self.st).calls.get_mut(&id) {
            c.timer = timer;
        }
        let head = Obj::new()
            .str("m", method)
            .str("p", path)
            .raw("h", &header_json(headers))
            .num("t", json::millis(timeout))
            .num("x", max)
            .end();
        if self
            .ws
            .send_binary(&call_msg(MSG_REQUEST, id, &head, &body))
            .is_err()
        {
            self.give_up(id, KIND_UNREACHABLE, "the member's link closed");
        }
    }

    fn take(&self, id: u64) -> Option<Call> {
        let mut c = lock(&self.st).calls.remove(&id)?;
        if let Some(t) = c.timer.take() {
            let _ = t.cancel();
        }
        Some(c)
    }

    fn give_up(&self, id: u64, kind: &'static str, why: &str) {
        if let Some(mut c) = self.take(id) {
            let mut cancel = vec![MSG_CANCEL];
            cancel.extend_from_slice(&id.to_be_bytes());
            let _ = self.ws.send_binary(&cancel);
            if let Some(d) = c.done.take() {
                d(Err(Problem::new(kind, why.to_owned())));
            }
        }
    }

    /// A member's answer: fit's `mlink.answer`.
    fn answer(&self, data: &[u8]) {
        let Some((id, head, body)) = read_call(data) else {
            self.ws.abort();
            return;
        };
        let Some(mut c) = self.take(id) else { return };
        let Some(done) = c.done.take() else { return };
        done(match json::str_of(&head, "e") {
            "" => Ok(Reply {
                status: u16::try_from(json::uint_of(&head, "s")).unwrap_or(500),
                headers: header_of(head.get("h")),
                body: body.to_vec(),
                took: c.start.elapsed(),
            }),
            ANSWER_TOO_LARGE => Err(Problem::new(
                KIND_TOO_LARGE,
                "the member's answer is larger than max_response_bytes",
            )),
            ANSWER_TIMEOUT => Err(Problem::new(
                KIND_TIMEOUT,
                "the member did not answer in time",
            )),
            _ => Err(Problem::new(
                KIND_UNREACHABLE,
                "the member's link carries as many calls as it takes",
            )),
        });
    }
}

/// The handler of a link's WebSocket on the manager.
struct Conn {
    m: Weak<Inner>,
    l: Option<Arc<MLink>>,
}

impl WsHandler for Conn {
    fn on_open(&mut self, ws: &WsSender) {
        let Some(inner) = self.m.upgrade() else {
            ws.abort();
            return;
        };
        let m = Manager(inner);
        let max = m.max_nodes().saturating_add(LINKS_SLACK);
        {
            let mut h = lock(&m.0.link);
            if h.conns >= max || h.pending >= MAX_PENDING {
                drop(h);
                ws.abort();
                return;
            }
            h.conns = h.conns.saturating_add(1);
            h.pending = h.pending.saturating_add(1);
        }
        let Ok(nonce) = linktoken::nonce() else {
            ws.abort();
            return;
        };
        let l = Arc::new(MLink {
            ws: ws.clone(),
            remote: ws.peer_addr().map(|a| a.to_string()).unwrap_or_default(),
            nonce,
            st: Mutex::new(LinkSt::default()),
            heard: AtomicU64::new(0),
            epoch: Instant::now(),
        });
        let weak = Arc::downgrade(&l);
        let _ = akio::timer::after_send(HANDSHAKE, move || {
            if let Some(l) = weak.upgrade() {
                if !lock(&l.st).open {
                    l.ws.abort();
                }
            }
        });
        let _ = ws.send_binary(&msg(
            MSG_CHALLENGE,
            &Obj::new().raw("nonce", &b64(&nonce)).end(),
        ));
        self.l = Some(l);
    }

    fn on_message_ref(&mut self, _ws: &WsSender, m: MessageRef<'_>) {
        let (Some(l), Some(inner)) = (self.l.clone(), self.m.upgrade()) else {
            return;
        };
        let data = m.as_bytes();
        let Some((&t, rest)) = data.split_first() else {
            return;
        };
        l.touch();
        let (open, verifying) = {
            let st = lock(&l.st);
            (st.open, !st.cred.is_empty() || st.closed)
        };
        let mgr = Manager(inner);
        if open {
            if t == MSG_RESPONSE {
                l.answer(rest);
            } else {
                l.ws.abort();
            }
        } else if verifying {
            // A second hello while the first is checked: not one.
            l.ws.abort();
        } else if t == MSG_HELLO && rest.len() <= MAX_HELLO {
            lock(&l.st).cred = "?".to_owned();
            mgr.on_hello(&l, rest);
        } else {
            mgr.refuse(&l, REASON_BAD_HELLO, "a link starts with a hello", "");
        }
    }

    fn on_pong(&mut self, _ws: &WsSender, _payload: &[u8]) {
        if let Some(l) = &self.l {
            l.touch();
        }
    }

    fn on_close(&mut self, _code: Option<u16>, reason: &str) {
        let (Some(l), Some(inner)) = (self.l.take(), self.m.upgrade()) else {
            return;
        };
        let m = Manager(inner);
        let was_open = lock(&l.st).open;
        {
            let mut h = lock(&m.0.link);
            h.conns = h.conns.saturating_sub(1);
            if !was_open {
                h.pending = h.pending.saturating_sub(1);
            }
        }
        l.fail_all("the member's link closed");
        if was_open {
            m.link_closed(&l, reason);
        }
    }
}

/// The link listener's certificate: a P-256 key, self-signed for `host`, ten years.
fn self_signed(host: &str) -> Result<(String, String), String> {
    let key = akio_crypto::p256::SecretKey::generate().map_err(|e| e.to_string())?;
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let mut p = akio_x509::CertificateParams::new([host]);
    let now = linktoken::unix_now();
    p.not_before = now.saturating_sub(3600);
    p.not_after = now.saturating_add(10 * 365 * 86_400);
    let cert = p.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((
        cert.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    ))
}

/// The fingerprint of the first certificate of `pem`, as fit writes pins.
fn fingerprint_of(pem: &str) -> Option<String> {
    let blocks = akio_crypto::pem::decode(pem.as_bytes()).ok()?;
    let der = blocks.into_iter().find(|(l, _)| l == "CERTIFICATE")?.1;
    Some(format_fingerprint(&akio_crypto::hash::Sha256::digest(&der)))
}

/// Where to listen for `address`, `host:port` or `:port` (every address): the control thread's,
/// at start, which may resolve a host's name.
fn listen_addr(address: &str) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let full = if address.starts_with(':') {
        format!("0.0.0.0{address}")
    } else {
        address.to_owned()
    };
    full.to_socket_addrs().ok()?.next()
}

fn time_text(t: SystemTime) -> String {
    json::time(t).trim_matches('"').to_owned()
}

impl Manager {
    /// Starts the link listener, `link_address`, with the certificate the document keeps, or one
    /// made now and kept: fit's `startLinks`. The control thread's, at start.
    pub(crate) fn start_links(&self) -> Result<(), String> {
        let s = &self.0.o.settings;
        if s.link_address.is_empty() {
            return Ok(());
        }
        let kept = lock(&self.0.doc)
            .doc
            .rest
            .iter()
            .find(|(k, _)| k == "link_cert")
            .map(|(_, v)| {
                (
                    json::str_of(v, "cert").to_owned(),
                    json::str_of(v, "key").to_owned(),
                )
            })
            .filter(|(c, k)| !c.is_empty() && !k.is_empty());
        let (cert, key) = match kept {
            Some(ck) => ck,
            None => {
                let host = s
                    .link_url
                    .split_once("://")
                    .and_then(|(_, r)| r.split('/').next())
                    .and_then(crate::member::host_of)
                    .unwrap_or_else(|| "akit-cluster-manager".to_owned());
                let (c, k) = self_signed(&host)?;
                self.keep_link_cert(&c, &k);
                (c, k)
            }
        };
        let fp = fingerprint_of(&cert).ok_or("the link certificate does not parse")?;
        let chain = akio::tls::Certificate::from_pem(cert.as_bytes()).map_err(|e| e.to_string())?;
        let pkey = akio::tls::PrivateKey::from_pem(key.as_bytes()).map_err(|e| e.to_string())?;
        let cfg = akio::tls::ServerConfig::builder(chain, pkey)
            .alpn(["http/1.1"])
            .session_tickets(false)
            .build()
            .map_err(|e| e.to_string())?;
        let addr = listen_addr(&s.link_address)
            .ok_or_else(|| format!("link_address {:?} is not an address", s.link_address))?;
        let weak = Arc::downgrade(&self.0);
        let max = self
            .0
            .lim
            .max_bytes
            .saturating_add(MAX_HEAD)
            .saturating_add(64);
        let service = move |ctx: &mut Context| {
            if ctx.path() != akit_config::cluster::CLUSTER_LINK_PATH {
                ctx.text(404, "404 page not found\n");
                return;
            }
            if ctx.header("Origin").is_some_and(|o| !o.is_empty()) {
                ctx.text(403, "the link is not for web pages\n");
                return;
            }
            let mut o = akio_http::WsOptions::default();
            o.config.max_message_bytes = max;
            o.config.permessage_deflate = false;
            ctx.upgrade_websocket(
                Conn {
                    m: weak.clone(),
                    l: None,
                },
                &o,
            );
        };
        let mut hc = akio_http::HttpConfig::default();
        hc.max_header_bytes = 16 << 10;
        hc.max_body_bytes = 0;
        hc.h2 = None;
        hc.head_timeout = Some(HANDSHAKE);
        hc.keep_alive_timeout = Some(HANDSHAKE);
        let el = akio::Listener::tcp(addr)
            .threads(1)
            .serve(akio::tls::Tls::server(
                cfg,
                akio_http::serve_with(hc, service),
            ));
        let id = self
            .0
            .o
            .engine
            .add_listener(el)
            .map_err(|e| format!("link_address {}: {e}", s.link_address))?;
        let at = self
            .0
            .o
            .engine
            .listeners()
            .iter()
            .find(|i| i.id == id)
            .map_or(addr, |i| i.local_addr);
        let mut h = lock(&self.0.link);
        h.listener = Some((id, at));
        h.fingerprint = fp;
        drop(h);
        self.log(format!("cluster: links taken on {at}"));
        Ok(())
    }

    fn keep_link_cert(&self, cert: &str, key: &str) {
        let mut d = {
            let g = lock(&self.0.doc);
            if !g.read_only.is_empty() {
                drop(g);
                self.log(
                    "cluster: the link listener's certificate cannot be kept, as the document is \
                     read-only: members enrolled now cannot link after a restart"
                        .to_owned(),
                );
                return;
            }
            g.doc.clone()
        };
        let v = Value::Object(vec![
            ("cert".to_owned(), Value::String(cert.to_owned())),
            ("key".to_owned(), Value::String(key.to_owned())),
        ]);
        d.rest.retain(|(k, _)| k != "link_cert");
        d.rest.push(("link_cert".to_owned(), v));
        match self.write_doc(&mut d) {
            Ok(()) => lock(&self.0.doc).doc.rest.clone_from(&d.rest),
            Err(e) => self.log(format!("cluster: the link certificate was not kept: {e}")),
        }
    }

    /// Stops the link listener and closes the links.
    pub(crate) fn stop_links(&self) {
        let (listener, links) = {
            let mut h = lock(&self.0.link);
            (
                h.listener.take(),
                h.links.drain().map(|(_, l)| l).collect::<Vec<_>>(),
            )
        };
        if let Some((id, _)) = listener {
            let _ = self.0.o.engine.remove_listener(id);
        }
        for l in links {
            l.ws.abort();
        }
    }

    fn refuse(&self, l: &MLink, reason: &str, msg_text: &str, name: &str) {
        if !name.is_empty() {
            let mut h = lock(&self.0.link);
            let s = h.seen.entry(name.to_owned()).or_default();
            s.refused = Some((
                SystemTime::now(),
                reason.to_owned(),
                msg_text.to_owned(),
                l.remote.clone(),
            ));
        }
        self.log(format!(
            "cluster: a link from {} was refused ({reason}): {msg_text}",
            l.remote
        ));
        l.refuse_with(MSG_REFUSE, CLOSE_REFUSED, reason, msg_text);
    }

    /// A member's hello: its credential, or its enrollment token, proved: fit's `onHello`.
    fn on_hello(&self, l: &Arc<MLink>, data: &[u8]) {
        let h = json::parse(data).unwrap_or(Value::Null);
        let id = json::str_of(&h, "id").to_owned();
        let nonce = bytes_of(&h, "nonce");
        let proof = bytes_of(&h, "proof");
        let mode = json::str_of(&h, "mode").to_owned();
        if nonce.len() != linktoken::NONCE_SIZE || !akit_config::roles::valid_link_token_id(&id) {
            return self.refuse(l, REASON_BAD_HELLO, "the hello is not one", "");
        }
        let node = h.get("node").cloned().unwrap_or(Value::Null);
        lock(&l.st).host = crate::api::clip(json::str_of(&node, "hostname"), 255);
        let tr = Tr::new(&id, &nonce, &l.nonce, &mode);
        let d = self.doc();
        match mode.as_str() {
            MODE_LINK => {
                let Some((name, stored)) = credential(&d, &id) else {
                    return self.refuse(
                        l,
                        REASON_UNKNOWN,
                        "no member has this credential: it was removed, or enrolled again with \
                         another token",
                        "",
                    );
                };
                if !linktoken::verify(&stored, &tr.t(), &proof) {
                    return self.refuse(
                        l,
                        REASON_PROOF,
                        "the credential's proof does not check",
                        &name,
                    );
                }
                let p = linktoken::listener_proof(&stored, &tr.t(), &[&name, ""]);
                let w = Obj::new().str("name", &name).raw("proof", &b64(&p)).end();
                self.welcome(l, &name, &id, &w);
            }
            MODE_ENROLL => {
                let (found, used) = enroll_token(&d, &id);
                let Some((name, stored, expires)) = found else {
                    return match used {
                        Some(u) => self.refuse(
                            l,
                            REASON_USED,
                            "the enrollment token was used already: make the member a new one",
                            &u,
                        ),
                        None => self.refuse(
                            l,
                            REASON_UNKNOWN,
                            "no member has this enrollment token: it was replaced by another, or \
                             its member removed",
                            "",
                        ),
                    };
                };
                if expires.is_none_or(|e| SystemTime::now() >= e) {
                    return self.refuse(
                        l,
                        REASON_EXPIRED,
                        "the enrollment token expired: make the member a new one",
                        &name,
                    );
                }
                if !linktoken::verify(&stored, &tr.t(), &proof) {
                    return self.refuse(
                        l,
                        REASON_PROOF,
                        "the enrollment token's proof does not check",
                        &name,
                    );
                }
                let (me, l2, hostname) = (
                    self.clone(),
                    Arc::clone(l),
                    json::str_of(&node, "hostname").to_owned(),
                );
                let job = Box::new(move || match me.enrolled(&id, &l2.remote, &hostname) {
                    Ok((name, secret)) => {
                        let cred = token_as(&secret, CREDENTIAL_PREFIX);
                        let p = linktoken::listener_proof(&stored, &tr.t(), &[&name, &cred]);
                        let w = Obj::new()
                            .str("name", &name)
                            .str("credential", &cred)
                            .raw("proof", &b64(&p))
                            .end();
                        me.welcome(&l2, &name, secret.id(), &w);
                    }
                    Err((reason, why, name)) => me.refuse(&l2, &reason, &why, &name),
                });
                if !self.on_pool(job) {
                    self.refuse(
                        l,
                        REASON_BUSY,
                        "the manager is enrolling other members: try again",
                        &name,
                    );
                }
            }
            _ => self.refuse(l, REASON_BAD_HELLO, "the hello's mode is not one", ""),
        }
    }

    /// Keeps the credential of the member whose token `id` is, the token used: fit's `enroll`.
    /// On the pool.
    fn enrolled(
        &self,
        id: &str,
        remote: &str,
        hostname: &str,
    ) -> Result<(String, Secret), (String, String, String)> {
        let secret = Secret::new().map_err(|e| {
            (
                REASON_BUSY.to_owned(),
                format!("the manager has no random bytes: {e}"),
                String::new(),
            )
        })?;
        let mut name = String::new();
        let r = self.change("(enrollment)", |d| {
            let now = crate::now_text();
            let at = SystemTime::now();
            for mem in &mut d.members {
                if mem.access != ACCESS_LINK {
                    continue;
                }
                let Some(Value::Object(link)) = mem
                    .extra
                    .iter_mut()
                    .find(|(k, _)| k == "link")
                    .map(|(_, v)| v)
                else {
                    continue;
                };
                let get = |link: &Vec<(String, Value)>, k: &str| {
                    link.iter()
                        .find(|(x, _)| x == k)
                        .map(|(_, v)| match v {
                            Value::String(s) => s.clone(),
                            _ => String::new(),
                        })
                        .unwrap_or_default()
                };
                if get(link, "token_id") != id || get(link, "token_hash").is_empty() {
                    continue;
                }
                name.clone_from(&mem.name);
                if crate::rollouts::parse_time(&get(link, "token_expires")).is_none_or(|e| at >= e)
                {
                    return Err(Refusal::Conflict(format!(
                        "{REASON_EXPIRED}\u{0}the enrollment token expired: make the member a new one"
                    )));
                }
                let set = |link: &mut Vec<(String, Value)>, k: &str, v: &str| {
                    link.retain(|(x, _)| x != k);
                    if !v.is_empty() {
                        link.push((k.to_owned(), Value::String(v.to_owned())));
                    }
                };
                set(link, "credential_id", secret.id());
                set(link, "credential_hash", &secret.hash());
                set(link, "enrolled", &now);
                set(link, "used_token_id", id);
                for k in ["token_id", "token_hash", "token_made", "token_expires"] {
                    set(link, k, "");
                }
                mem.updated.clone_from(&now);
                let mut detail = format!("from {remote}");
                if !hostname.is_empty() {
                    detail.push_str(&format!(", host {}", quoted(&crate::api::clip(hostname, 255))));
                }
                let s = |v: &str| Value::String(v.to_owned());
                return Ok((
                    vec![
                        ("action".to_owned(), s(ACTION_ENROLLED)),
                        ("node".to_owned(), s(&mem.name)),
                        ("detail".to_owned(), s(&detail)),
                    ],
                    (),
                ));
            }
            Err(Refusal::Conflict(format!(
                "{REASON_UNKNOWN}\u{0}no member has this enrollment token: it was replaced by \
                 another, or its member removed"
            )))
        });
        match r {
            Ok(()) => {
                self.log(format!(
                    "cluster: the member {name:?} enrolled from {remote}"
                ));
                Ok((name, secret))
            }
            Err(Refusal::Conflict(m)) if m.contains('\u{0}') => {
                let (reason, why) = m.split_once('\u{0}').unwrap_or((REASON_BUSY, &m));
                Err((reason.to_owned(), why.to_owned(), name))
            }
            Err(e) => Err((
                REASON_BUSY.to_owned(),
                format!(
                    "the manager could not keep the member's credential: {}",
                    refusal_text(&e)
                ),
                name,
            )),
        }
    }

    /// The link is the member's: welcomed, and its calls go over it: fit's `welcome`.
    fn welcome(&self, l: &Arc<MLink>, name: &str, cred: &str, w: &str) {
        let valid = self
            .doc()
            .members
            .iter()
            .any(|m| m.name == name && credential_id(m) == cred);
        if self.0.closed.load(Ordering::Acquire) {
            l.ws.abort();
            return;
        }
        if !valid {
            l.refuse_with(
                MSG_REVOKED,
                CLOSE_REVOKED,
                REASON_REVOKED,
                "the member was removed, or enrolled again, on the manager",
            );
            return;
        }
        if l.ws.send_binary(&msg(MSG_WELCOME, w)).is_err() {
            l.ws.abort();
            return;
        }
        let now = SystemTime::now();
        let host = {
            let mut st = lock(&l.st);
            st.open = true;
            cred.clone_into(&mut st.cred);
            name.clone_into(&mut st.name);
            st.since = Some(now);
            st.host.clone()
        };
        l.touch();
        let old = {
            let mut h = lock(&self.0.link);
            h.pending = h.pending.saturating_sub(1);
            let old = h.links.insert(cred.to_owned(), Arc::clone(l));
            let s = h.seen.entry(name.to_owned()).or_default();
            s.since = Some(now);
            s.remote.clone_from(&l.remote);
            s.host = host;
            s.refused = None;
            old
        };
        if let Some(o) = old.filter(|o| !Arc::ptr_eq(o, l)) {
            o.refuse_with(
                MSG_REFUSE,
                CLOSE_REFUSED,
                REASON_REPLACED,
                "another link of the member took this one's place",
            );
        }
        self.log(format!(
            "cluster: the member {name:?} linked from {}",
            l.remote
        ));
        keep_alive(Arc::downgrade(l));
        if let Some(n) = self.node(name) {
            n.next.store(0, Ordering::Release);
            self.poke();
        }
    }

    fn link_closed(&self, l: &Arc<MLink>, why: &str) {
        let (cred, name) = {
            let st = lock(&l.st);
            (st.cred.clone(), st.name.clone())
        };
        let mut h = lock(&self.0.link);
        if h.links.get(&cred).is_none_or(|x| !Arc::ptr_eq(x, l)) {
            return;
        }
        h.links.remove(&cred);
        if let Some(s) = h.seen.get_mut(&name) {
            s.since = Some(SystemTime::now());
        }
        drop(h);
        let why = if why.is_empty() { "closed" } else { why };
        self.log(format!(
            "cluster: the member {name:?}'s link from {} closed: {why}",
            l.remote
        ));
    }

    /// Closes the links whose credentials are gone: fit's `syncLinks`. After every change.
    pub(crate) fn sync_links(&self) {
        let d = self.doc();
        let creds: Vec<String> = d
            .members
            .iter()
            .filter(|m| m.access == ACCESS_LINK)
            .map(credential_id)
            .filter(|c| !c.is_empty())
            .collect();
        let gone: Vec<Arc<MLink>> = {
            let mut h = lock(&self.0.link);
            let ids: Vec<String> = h
                .links
                .keys()
                .filter(|k| !creds.contains(k))
                .cloned()
                .collect();
            let names: Vec<String> = d.members.iter().map(|m| m.name.clone()).collect();
            h.seen.retain(|n, _| names.contains(n));
            ids.iter().filter_map(|k| h.links.remove(k)).collect()
        };
        for l in gone {
            let name = lock(&l.st).name.clone();
            self.log(format!(
                "cluster: the member {name:?}'s link from {} is closed: its credential is gone",
                l.remote
            ));
            l.refuse_with(
                MSG_REVOKED,
                CLOSE_REVOKED,
                REASON_REVOKED,
                "the member was removed, or enrolled again, on the manager",
            );
        }
    }

    /// A call to the member whose credential is `cred`, over its link.
    pub(crate) fn link_call(
        &self,
        cred: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        done: Box<dyn FnOnce(Result<Reply, Problem>) + Send>,
    ) {
        let l = if cred.is_empty() {
            None
        } else {
            lock(&self.0.link).links.get(cred).cloned()
        };
        let Some(l) = l else {
            let msg_text = if cred.is_empty() {
                "the member has not enrolled yet: paste the config line its enrollment token came \
                 with into its config, and start it"
            } else if lock(&self.0.link).listener.is_none() {
                "the manager takes no links: set admin.cluster.link_address"
            } else {
                "the member has no link to the manager now: it links again, with backoff, once it \
                 reaches the manager"
            };
            return done(Err(Problem::new(KIND_NOT_LINKED, msg_text)));
        };
        let lim = self.limits();
        l.call(
            method,
            path,
            headers,
            body,
            lim.timeout,
            lim.max_bytes,
            done,
        );
    }

    /// How member `m`'s link is, as its state shows it: fit's `linkStateOf`.
    pub(crate) fn link_state_json(&self, m: &Member) -> String {
        let cred = credential_id(m);
        let (seen, l) = {
            let h = lock(&self.0.link);
            (
                h.seen.get(&m.name).cloned().unwrap_or_default(),
                h.links.get(&cred).cloned(),
            )
        };
        let mut o = Obj::new().bool("linked", l.is_some());
        let (since, remote, host) = match &l {
            Some(l) => {
                let st = lock(&l.st);
                (st.since, l.remote.clone(), st.host.clone())
            }
            None => (seen.since, seen.remote.clone(), seen.host.clone()),
        };
        if let Some(t) = since {
            o = o.raw("since", &json::time(t));
        }
        o = o.str_omit("remote", &remote).str_omit("host", &host);
        let link = m.link().cloned().unwrap_or(Value::Null);
        o = o.bool(
            "enrolled",
            !json::str_of(&link, "credential_hash").is_empty(),
        );
        if !json::str_of(&link, "token_hash").is_empty() {
            o = o.str_omit("enroll_by", json::str_of(&link, "token_expires"));
        }
        if let Some((at, reason, message, remote)) = &seen.refused {
            o = o.raw(
                "refused",
                &Obj::new()
                    .raw("at", &json::time(*at))
                    .str("reason", reason)
                    .str("message", message)
                    .str_omit("remote", remote)
                    .end(),
            );
        }
        o.end()
    }

    /// `GET /api/link`: fit's `LinkInfo`.
    fn link_info(&self, host: &str) -> (bool, String, String, String) {
        let h = lock(&self.0.link);
        let Some((_, at)) = h.listener else {
            return (false, String::new(), String::new(), String::new());
        };
        let mut url = self.0.o.settings.link_url.clone();
        if url.is_empty() {
            let hostname = crate::member::host_of(host)
                .filter(|x| !x.is_empty())
                .unwrap_or_else(|| "localhost".to_owned());
            let hp = if hostname.contains(':') {
                format!("[{hostname}]:{}", at.port())
            } else {
                format!("{hostname}:{}", at.port())
            };
            url = format!("wss://{hp}{}", akit_config::cluster::CLUSTER_LINK_PATH);
        }
        (true, at.to_string(), url, h.fingerprint.clone())
    }

    fn link_info_json(&self, host: &str) -> String {
        let (enabled, address, url, fp) = self.link_info(host);
        let links = lock(&self.0.link).links.len();
        Obj::new()
            .bool("enabled", enabled)
            .str_omit("address", &address)
            .str_omit("url", &url)
            .str_omit("fingerprint", &fp)
            .num("links", links)
            .num(
                "default_ttl",
                akit_config::cluster::DEFAULT_CLUSTER_ENROLL_TTL.as_secs(),
            )
            .num(
                "max_ttl",
                akit_config::cluster::MAX_CLUSTER_ENROLL_TTL.as_secs(),
            )
            .end()
    }

    /// An enrollment's TTL and URL checked: fit's `checkEnroll`.
    fn check_enroll(&self, v: &Value) -> Result<(Duration, String), Refusal> {
        if lock(&self.0.link).listener.is_none() {
            return Err(Refusal::Conflict(
                "the manager takes no links: set admin.cluster.link_address, and restart it"
                    .to_owned(),
            ));
        }
        let field = |f: &str, m: String| {
            Refusal::Field(crate::member::FieldError {
                field: f.to_owned(),
                msg: m,
            })
        };
        let ttl = match json::str_of(v, "ttl").trim() {
            "" => akit_config::cluster::DEFAULT_CLUSTER_ENROLL_TTL,
            t => linktoken::parse_ttl(t).map_err(|e| field("ttl", e))?,
        };
        if ttl < MIN_ENROLL_TTL || ttl > akit_config::cluster::MAX_CLUSTER_ENROLL_TTL {
            return Err(field(
                "ttl",
                "an enrollment token lasts 1m0s to 720h0m0s".to_owned(),
            ));
        }
        let url = json::str_of(v, "url").trim().to_owned();
        if !url.is_empty() {
            if let Some(m) = akit_config::cluster::check_link_url(&url) {
                return Err(field("url", m));
            }
        }
        Ok((ttl, url))
    }

    /// The enrollment answered: the member, its token, and the config line it takes: fit's
    /// `enrollment`.
    fn enrollment(&self, mem: &Member, secret: &Secret, url: &str, host: &str) -> String {
        let (_, _, def_url, fp) = self.link_info(host);
        let url = if url.is_empty() {
            def_url
        } else {
            url.to_owned()
        };
        let token = token_as(secret, akit_config::cluster::CLUSTER_ENROLL_PREFIX);
        let cfg = Obj::new()
            .str("manager", &url)
            .str("fingerprint", &fp)
            .str("token", &token)
            .end();
        let expires = mem
            .link()
            .map(|l| json::str_of(l, "token_expires").to_owned())
            .unwrap_or_default();
        Obj::new()
            .raw("node", &mem.view())
            .str("token", &token)
            .str("expires", &expires)
            .raw("config", &cfg)
            .str("config_line", &format!("\"cluster_member\": {cfg}"))
            .end()
    }

    /// `POST /api/enroll`: a member added by enrollment: fit's `Enroll`. On the pool.
    fn enroll(&self, user: &str, v: &Value, host: &str) -> Result<String, Refusal> {
        let (ttl, url) = self.check_enroll(v)?;
        let secret = Secret::new().map_err(|e| Refusal::Failed(e.to_string()))?;
        let name = json::str_of(v, "name").trim().to_owned();
        let labels = v.get("labels").cloned().unwrap_or(Value::Null);
        let max = self.max_nodes();
        let now = SystemTime::now();
        let expires = now.checked_add(ttl).unwrap_or(now);
        let mem = self.change(user, |d| {
            let mut m = Member::from_value(&Value::Object(vec![
                ("name".to_owned(), Value::String(name.clone())),
                ("labels".to_owned(), labels.clone()),
                ("url".to_owned(), Value::String(String::new())),
                ("access".to_owned(), Value::String(ACCESS_LINK.to_owned())),
                ("created".to_owned(), Value::String(time_text(now))),
                ("updated".to_owned(), Value::String(time_text(now))),
            ]));
            m.extra.push((
                "link".to_owned(),
                Value::Object(vec![
                    ("token_id".to_owned(), Value::String(secret.id().to_owned())),
                    ("token_hash".to_owned(), Value::String(secret.hash())),
                    ("token_made".to_owned(), Value::String(time_text(now))),
                    (
                        "token_expires".to_owned(),
                        Value::String(time_text(expires)),
                    ),
                ]),
            ));
            m.check().map_err(Refusal::Field)?;
            if d.index(&m.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a member is already named {}",
                    quoted(&m.name)
                )));
            }
            if d.members.len() >= max {
                return Err(Refusal::Conflict(format!(
                    "the manager keeps {max} members at most (max_nodes)"
                )));
            }
            d.members.push(m.clone());
            self.allow_in(d, user, crate::roles::ADMIN, &[&m])?;
            let s = |x: &str| Value::String(x.to_owned());
            Ok((
                vec![
                    ("action".to_owned(), s(ACTION_ENROLL_TOKEN)),
                    ("node".to_owned(), s(&m.name)),
                    (
                        "detail".to_owned(),
                        s(&format!(
                            "a new member; the token expires at {}",
                            crate::alerts::rfc3339(expires)
                        )),
                    ),
                ],
                m,
            ))
        })?;
        self.log(format!(
            "cluster: {user:?} added the member {:?} by enrollment, its token expiring at {}",
            mem.name,
            crate::alerts::rfc3339(expires)
        ));
        Ok(self.enrollment(&mem, &secret, &url, host))
    }

    /// `POST /api/nodes/<name>/enroll`: a new token for a member that links out: fit's
    /// `EnrollAgain`. On the pool.
    pub(crate) fn enroll_again(
        &self,
        user: &str,
        name: &str,
        v: &Value,
        host: &str,
    ) -> Result<String, Refusal> {
        self.allow(
            user,
            &crate::roles::Need::on(crate::roles::ADMIN, vec![name.to_owned()]),
        )?;
        let (ttl, url) = self.check_enroll(v)?;
        let secret = Secret::new().map_err(|e| Refusal::Failed(e.to_string()))?;
        let now = SystemTime::now();
        let expires = now.checked_add(ttl).unwrap_or(now);
        let mem = self.change(user, |d| {
            let i = d
                .index(name)
                .ok_or_else(|| Refusal::NotFound(format!("no member is named {}", quoted(name))))?;
            let m = d
                .members
                .get_mut(i)
                .ok_or_else(|| Refusal::NotFound(format!("no member is named {}", quoted(name))))?;
            if m.access != ACCESS_LINK {
                return Err(Refusal::Conflict(format!(
                    "{} is reached at its URL, not through a link: only a member that links out \
                     is enrolled",
                    quoted(name)
                )));
            }
            let mut link = match m.link() {
                Some(Value::Object(l)) => l.clone(),
                _ => Vec::new(),
            };
            for (k, val) in [
                ("token_id", secret.id().to_owned()),
                ("token_hash", secret.hash()),
                ("token_made", time_text(now)),
                ("token_expires", time_text(expires)),
            ] {
                link.retain(|(x, _)| x != k);
                link.push((k.to_owned(), Value::String(val)));
            }
            m.extra.retain(|(k, _)| k != "link");
            m.extra.push(("link".to_owned(), Value::Object(link)));
            m.updated = time_text(now);
            let s = |x: &str| Value::String(x.to_owned());
            Ok((
                vec![
                    ("action".to_owned(), s(ACTION_ENROLL_TOKEN)),
                    ("node".to_owned(), s(name)),
                    (
                        "detail".to_owned(),
                        s(&format!(
                            "the token expires at {}",
                            crate::alerts::rfc3339(expires)
                        )),
                    ),
                ],
                m.clone(),
            ))
        })?;
        self.log(format!(
            "cluster: {user:?} made the member {name:?} a new enrollment token, expiring at {}",
            crate::alerts::rfc3339(expires)
        ));
        Ok(self.enrollment(&mem, &secret, &url, host))
    }
}

fn refusal_text(e: &Refusal) -> String {
    match e {
        Refusal::Field(f) => f.msg.clone(),
        Refusal::NotFound(m)
        | Refusal::Conflict(m)
        | Refusal::Busy(m)
        | Refusal::Failed(m)
        | Refusal::Unavailable(m) => m.clone(),
        Refusal::Forbidden(f) => f.message(),
    }
}

/// Pings the link every [`PING`], and closes it once the member was not heard from in [`DEAD`]:
/// keep-alive's time.
fn keep_alive(l: Weak<MLink>) {
    let _ = akio::timer::after_send(PING, move || {
        let Some(link) = l.upgrade() else { return };
        if lock(&link.st).closed {
            return;
        }
        if link.quiet() > DEAD {
            link.ws.abort();
            return;
        }
        let _ = link.ws.ping(b"");
        keep_alive(l);
    });
}

/// The credential ID of member `m`, when it links out.
pub(crate) fn credential_id(m: &Member) -> String {
    m.link()
        .map(|l| json::str_of(l, "credential_id").to_owned())
        .unwrap_or_default()
}

/// The member whose credential is `id`, and the credential's stored key.
fn credential(d: &Document, id: &str) -> Option<(String, [u8; 32])> {
    d.members.iter().find_map(|m| {
        let l = m.link()?;
        (m.access == ACCESS_LINK && json::str_of(l, "credential_id") == id)
            .then(|| {
                linktoken::stored_of(json::str_of(l, "credential_hash"))
                    .map(|s| (m.name.clone(), s))
            })
            .flatten()
    })
}

/// The member whose enrollment token is `id`, its stored key and expiry; or the member that used
/// it already.
fn enroll_token(
    d: &Document,
    id: &str,
) -> (
    Option<(String, [u8; 32], Option<SystemTime>)>,
    Option<String>,
) {
    let mut used = None;
    for m in d.members.iter().filter(|m| m.access == ACCESS_LINK) {
        let Some(l) = m.link() else { continue };
        if json::str_of(l, "token_id") == id && !json::str_of(l, "token_hash").is_empty() {
            let found = linktoken::stored_of(json::str_of(l, "token_hash")).map(|s| {
                (
                    m.name.clone(),
                    s,
                    crate::rollouts::parse_time(json::str_of(l, "token_expires")),
                )
            });
            return (found, None);
        }
        if json::str_of(l, "used_token_id") == id {
            used = Some(m.name.clone());
        }
    }
    (None, used)
}

/// The enrollment's endpoints; `false` for a path that is not theirs.
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    let host = ctx.header("Host").unwrap_or("").to_owned();
    match (path, &method) {
        ("/api/link", Method::Get | Method::Head) => {
            ctx.respond(answer(200, m.link_info_json(&host)));
        }
        ("/api/link", _) => {
            ctx.respond(fail(405, "the method is not allowed").header("Allow", "GET"));
        }
        ("/api/enroll", Method::Post) => {
            let Some(v) = body(ctx) else { return true };
            if !crate::api::if_match(m, ctx) {
                return true;
            }
            let (me, user) = (m.clone(), user.to_owned());
            crate::api::on_pool(m, ctx, move || match me.enroll(&user, &v, &host) {
                Ok(b) => answer(201, b).header("ETag", me.etag()),
                Err(e) => e.response(),
            });
        }
        ("/api/enroll", _) => {
            ctx.respond(fail(405, "the method is not allowed").header("Allow", "POST"));
        }
        _ => return false,
    }
    true
}

/// `POST /api/nodes/<name>/enroll`.
pub(crate) fn enroll_route(m: &Manager, ctx: &mut Context, user: &str, name: &str) {
    let v = if ctx.body().is_empty() {
        Value::Object(Vec::new())
    } else {
        match body(ctx) {
            Some(v) => v,
            None => return,
        }
    };
    if !crate::api::if_match(m, ctx) {
        return;
    }
    let host = ctx.header("Host").unwrap_or("").to_owned();
    let (me, user, name) = (m.clone(), user.to_owned(), name.to_owned());
    crate::api::on_pool(m, ctx, move || {
        match me.enroll_again(&user, &name, &v, &host) {
            Ok(b) => answer(200, b).header("ETag", me.etag()),
            Err(e) => e.response(),
        }
    });
}

fn body(ctx: &mut Context) -> Option<Value> {
    if ctx.body().len() > crate::member::MAX_BODY_BYTES {
        ctx.respond(fail(
            413,
            &format!(
                "an enrollment is {} bytes at most",
                crate::member::MAX_BODY_BYTES
            ),
        ));
        return None;
    }
    match akit_roles::pages::decode(ctx) {
        Ok(b) => Some(b.0),
        Err(r) => {
            ctx.respond(r);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn calls_and_proofs_as_fits() {
        let m = call_msg(MSG_REQUEST, 7, r#"{"m":"GET"}"#, b"body");
        assert_eq!(m[0], MSG_REQUEST);
        let (id, head, body) = read_call(&m[1..]).unwrap();
        assert_eq!(
            (id, json::str_of(&head, "m"), body),
            (7, "GET", &b"body"[..])
        );
        assert!(read_call(&[0; 11]).is_none());
        let s = Secret::new().unwrap();
        let c = token_as(&s, CREDENTIAL_PREFIX);
        assert!(c.starts_with("fitc1."));
        let back = secret_of(&c, CREDENTIAL_PREFIX).unwrap();
        assert_eq!(back.id(), s.id());
        let (n1, n2) = (linktoken::nonce().unwrap(), linktoken::nonce().unwrap());
        let tr = Tr::new(s.id(), &n1, &n2, MODE_LINK);
        let proof = s.prove(&tr.t());
        assert!(linktoken::verify(&s.stored(), &tr.t(), &proof));
        let other = Tr::new(s.id(), &n1, &n2, MODE_ENROLL);
        assert!(!linktoken::verify(&s.stored(), &other.t(), &proof));
        let lp = linktoken::listener_proof(&s.stored(), &tr.t(), &["a", ""]);
        assert!(s.verify_listener(&tr.t(), &lp, &["a", ""]));
        let h = header_json(&[("A".into(), "1".into()), ("a".into(), "2".into())]);
        assert_eq!(h, r#"{"A":["1","2"]}"#);
        assert_eq!(header_of(json::parse(h.as_bytes()).as_ref()).len(), 2);
    }
}
