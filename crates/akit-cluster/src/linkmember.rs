//! A member that links out to its manager (I139b, fit's `cluster/linkmember.go`):
//! `admin.cluster_member`, for an akit behind NAT or with no port open to the manager. It dials
//! the manager's link URL (`wss://`, the manager's certificate pinned by its fingerprint),
//! enrolls once with its token for a credential of its own, which it keeps (in the store's
//! section `cluster_member`, fit's, or in a file beside the config), and links with that
//! credential from then on; the manager's calls arrive over the link and are answered by this
//! akit's Admin API. A link that ends is dialed again after a backoff, 1 s doubling to 1 min with
//! jitter, as fit's: the reconnection's time. A token or credential the manager refuses for good
//! is not tried again.
//!
//! The Admin API answers the link's calls through a hop to its own listener on loopback until
//! akio serves a request made in process (akio's A306): the API must listen on TCP.

#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime};

use akio_http::ws::{MessageRef, WsHandler, WsSender};
use akit_config::cluster::ClusterMember;
use akit_config::value::Value;
use akit_roles::linktoken::{self, Secret};

use crate::json::{self, Obj};
use crate::link::{
    ANSWER_BUSY, ANSWER_TIMEOUT, ANSWER_TOO_LARGE, CREDENTIAL_PREFIX, DEAD, HANDSHAKE,
    MAX_LINK_CALLS, MODE_ENROLL, MODE_LINK, MSG_CANCEL, MSG_CHALLENGE, MSG_HELLO, MSG_REFUSE,
    MSG_REQUEST, MSG_RESPONSE, MSG_REVOKED, MSG_WELCOME, PING, REASON_EXPIRED, REASON_PROOF,
    REASON_REVOKED, REASON_UNKNOWN, REASON_USED, Tr, b64, bytes_of, call_msg, header_json,
    header_of, msg, read_call, secret_of,
};
use crate::node::Log;

/// The store's section a member keeps its credential in: fit's.
pub const SECTION: &str = "cluster_member";
/// The suffix of the file beside the config a member without a store keeps it in: fit's
/// `.cluster-member`.
pub const FILE_SUFFIX: &str = ".cluster-member";
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the member keeps its credential.
pub trait Keeper: Send + Sync {
    /// What is kept, or nothing.
    ///
    /// # Errors
    ///
    /// It cannot be read.
    fn load(&self) -> Result<Option<Vec<u8>>, String>;

    /// Keeps `data`, or forgets what is kept for `None`.
    ///
    /// # Errors
    ///
    /// It cannot be written.
    fn save(&self, data: Option<&[u8]>) -> Result<(), String>;
}

/// The store's section [`SECTION`].
#[derive(Clone)]
pub struct StoreKeeper(pub Arc<dyn crate::Store>);

impl std::fmt::Debug for StoreKeeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreKeeper")
    }
}

impl Keeper for StoreKeeper {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.section(SECTION))
    }

    fn save(&self, data: Option<&[u8]>) -> Result<(), String> {
        self.0.set_section(SECTION, data.unwrap_or_default())
    }
}

/// A file, written whole and renamed into place, 0600.
#[derive(Debug, Clone)]
pub struct FileKeeper(pub std::path::PathBuf);

impl Keeper for FileKeeper {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(&self.0) {
            Ok(d) => Ok(Some(d)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, data: Option<&[u8]>) -> Result<(), String> {
        let Some(data) = data else {
            return match std::fs::remove_file(&self.0) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        };
        let mut tmp = self.0.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = o.open(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.0)
        };
        write().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }
}

/// A call of the manager's, for the Admin API to answer.
#[derive(Debug, Clone)]
pub struct Request {
    /// Its method, path and header fields.
    pub method: String,
    /// See [`method`](Request::method).
    pub path: String,
    /// See [`method`](Request::method).
    pub headers: Vec<(String, String)>,
    /// Its body.
    pub body: Vec<u8>,
    /// How long it may take.
    pub timeout: Duration,
    /// The largest answer the manager takes.
    pub max: usize,
}

/// What the Admin API answered.
#[derive(Debug, Clone)]
pub struct Answer {
    /// Its status, header fields and body.
    pub status: u16,
    /// See [`status`](Answer::status).
    pub headers: Vec<(String, String)>,
    /// See [`status`](Answer::status).
    pub body: Vec<u8>,
}

/// The Admin API, called for the manager: its answer, or why none.
pub type Api = Arc<dyn Fn(Request, Box<dyn FnOnce(Result<Answer, String>) + Send>) + Send + Sync>;

/// The Admin API reached on its own listener at `addr` (`tls` for one over TLS, this process's
/// own, its certificate not checked), with `token` when it asks for one: the stopgap until
/// akio's A306 serves a request made in process.
#[must_use]
pub fn loopback_api(
    engine: &Arc<akio::Engine>,
    addr: std::net::SocketAddr,
    tls: bool,
    token: Option<String>,
) -> Api {
    let mut opts = akio_http::client::ClientOptions::default();
    opts.max_idle_per_host = 2;
    opts.max_idle = 2;
    let mut client = akio_http::client::Client::new(Arc::clone(engine)).with_options(opts);
    if tls {
        if let Ok(cfg) = akio::tls::ClientConfig::builder()
            .resumption(false)
            .insecure_skip_verify()
            .build()
        {
            client = client.with_tls(cfg);
        }
    }
    let host = if addr.ip().is_unspecified() {
        match addr {
            std::net::SocketAddr::V4(_) => format!("127.0.0.1:{}", addr.port()),
            std::net::SocketAddr::V6(_) => format!("[::1]:{}", addr.port()),
        }
    } else {
        addr.to_string()
    };
    let base = format!("{}://{host}", if tls { "https" } else { "http" });
    Arc::new(move |r: Request, done| {
        let method = match r.method.as_str() {
            "GET" => akio_http::Method::Get,
            "HEAD" => akio_http::Method::Head,
            "POST" => akio_http::Method::Post,
            "PUT" => akio_http::Method::Put,
            "PATCH" => akio_http::Method::Patch,
            "DELETE" => akio_http::Method::Delete,
            other => return done(Err(format!("the method {other} is not passed on"))),
        };
        let mut req = akio_http::client::ClientRequest::new(method, &format!("{base}{}", r.path))
            .timeout(r.timeout);
        for (n, v) in &r.headers {
            if ![
                "host",
                "authorization",
                "connection",
                "content-length",
                "origin",
            ]
            .iter()
            .any(|x| x.eq_ignore_ascii_case(n))
            {
                req = req.header(n, v);
            }
        }
        if let Some(t) = &token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        if !r.body.is_empty() || !matches!(r.method.as_str(), "GET" | "HEAD") {
            req = req.body(r.body);
        }
        client.request(req, move |res| {
            done(match res {
                Ok(resp) => Ok(Answer {
                    status: resp.status,
                    headers: resp
                        .headers
                        .iter()
                        .map(|(n, v)| (n.to_owned(), v.to_owned()))
                        .collect(),
                    body: resp.body,
                }),
                Err(e) => Err(e.to_string()),
            });
        });
    })
}

/// What a [`MemberLink`] needs.
pub struct Options {
    /// The engine it dials on.
    pub engine: Arc<akio::Engine>,
    /// `admin.cluster_member`.
    pub settings: ClusterMember,
    /// The Admin API, which answers the manager's calls.
    pub api: Api,
    /// The largest request the manager sends.
    pub max_request_bytes: usize,
    /// Where the credential is kept.
    pub keeper: Arc<dyn Keeper>,
    /// This akit's node ID and host, which the hello names.
    pub node_id: String,
    /// See [`node_id`](Options::node_id).
    pub hostname: String,
    /// Says how the link goes; never a token or credential.
    pub log: Log,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

/// What the member keeps: fit's `kept`.
#[derive(Debug, Clone, Default)]
struct Kept {
    manager: String,
    fingerprint: String,
    token_id: String,
    credential: String,
    name: String,
    enrolled: String,
}

impl Kept {
    fn json(&self) -> String {
        Obj::new()
            .str("manager", &self.manager)
            .str("fingerprint", &self.fingerprint)
            .str("token_id", &self.token_id)
            .str("credential", &self.credential)
            .str("name", &self.name)
            .str("enrolled", &self.enrolled)
            .end()
    }
}

/// How the member's link is.
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// Whether it is linked, as which member and since when.
    pub linked: bool,
    /// See [`linked`](Status::linked).
    pub name: String,
    /// See [`linked`](Status::linked).
    pub since: Option<SystemTime>,
    /// Why it stopped linking for good.
    pub stopped: String,
    /// Why the last link ended.
    pub err: String,
}

/// One dial, and the link it makes.
struct Attempt {
    run: u64,
    mode: &'static str,
    secret: Secret,
    ws: Option<WsSender>,
    nonce: Vec<u8>,
    manager_nonce: Vec<u8>,
    open: bool,
    ended: bool,
    inflight: HashMap<u64, bool>,
}

struct St {
    kept: Option<Kept>,
    bad_token: String,
    status: Status,
    stopped: bool,
    run: u64,
    backoff: Duration,
    cur: Option<Attempt>,
}

struct Inner {
    o: Options,
    fp: String,
    pin: [u8; 32],
    token: Option<Secret>,
    st: Mutex<St>,
    heard: AtomicU64,
    epoch: Instant,
}

/// A member's link to its manager: fit's `MemberLink`. Cheap to clone.
#[derive(Clone)]
pub struct MemberLink(Arc<Inner>);

impl std::fmt::Debug for MemberLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberLink").finish_non_exhaustive()
    }
}

/// Outcomes of an attempt: fit's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Out {
    Retry,
    Linked,
    Forget,
}

impl MemberLink {
    /// Starts linking out as `o` says: fit's `StartMember`. Reads the token's file and the
    /// credential kept: the control thread's, at start.
    ///
    /// # Errors
    ///
    /// A fingerprint or token that is not one, or a token file that cannot be read.
    pub fn start(o: Options) -> Result<MemberLink, String> {
        let s = &o.settings;
        let pin = akit_config::roles::parse_pin(&s.fingerprint)
            .ok_or("admin.cluster_member.fingerprint is not a SHA-256 fingerprint")?;
        let mut tok = s.token.clone();
        if !s.token_file.is_empty() {
            tok = std::fs::read_to_string(&s.token_file)
                .map_err(|e| format!("admin.cluster_member.token_file: {e}"))?
                .trim()
                .to_owned();
        }
        let token = if tok.is_empty() {
            None
        } else {
            Some(
                secret_of(&tok, akit_config::cluster::CLUSTER_ENROLL_PREFIX).ok_or_else(|| {
                    format!(
                        "admin.cluster_member's token: {}",
                        akit_config::cluster::check_enroll_token(&tok).unwrap_or_default()
                    )
                })?,
            )
        };
        let kept = match o.keeper.load() {
            Err(e) => {
                (o.log)(format!(
                    "cluster: the credential kept for the manager cannot be read ({e}): this akit \
                     enrolls again"
                ));
                None
            }
            Ok(None) => None,
            Ok(Some(d)) if d.is_empty() => None,
            Ok(Some(d)) => {
                let v = json::parse(&d).unwrap_or(Value::Null);
                let k = Kept {
                    manager: json::str_of(&v, "manager").to_owned(),
                    fingerprint: json::str_of(&v, "fingerprint").to_owned(),
                    token_id: json::str_of(&v, "token_id").to_owned(),
                    credential: json::str_of(&v, "credential").to_owned(),
                    name: json::str_of(&v, "name").to_owned(),
                    enrolled: json::str_of(&v, "enrolled").to_owned(),
                };
                if k.credential.is_empty() {
                    (o.log)(
                        "cluster: the credential kept for the manager is not one: this akit \
                         enrolls again"
                            .to_owned(),
                    );
                    None
                } else {
                    Some(k)
                }
            }
        };
        let l = MemberLink(Arc::new(Inner {
            fp: crate::member::format_fingerprint(&pin),
            pin,
            token,
            st: Mutex::new(St {
                kept,
                bad_token: String::new(),
                status: Status::default(),
                stopped: false,
                run: 0,
                backoff: BACKOFF_MIN,
                cur: None,
            }),
            heard: AtomicU64::new(0),
            epoch: Instant::now(),
            o,
        }));
        l.attempt();
        Ok(l)
    }

    /// How the link is.
    #[must_use]
    pub fn status(&self) -> Status {
        lock(&self.0.st).status.clone()
    }

    /// Ends the link, and dials no more.
    pub fn close(&self) {
        let ws = {
            let mut st = lock(&self.0.st);
            st.stopped = true;
            st.cur.take().and_then(|a| a.ws)
        };
        if let Some(ws) = ws {
            ws.close(1000, "the member stops");
        }
    }

    fn log(&self, m: String) {
        (self.0.o.log)(m);
    }

    fn touch(&self) {
        let ms = u64::try_from(self.0.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.0.heard.store(ms, Ordering::Release);
    }

    fn quiet(&self) -> Duration {
        let ms = u64::try_from(self.0.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(ms.saturating_sub(self.0.heard.load(Ordering::Acquire)))
    }

    /// The credential kept, or the enrollment token: fit's `choose`.
    fn choose(&self, st: &St) -> Result<(&'static str, Secret), String> {
        if let Some(k) = &st.kept {
            if k.fingerprint == self.0.fp
                && self.0.token.as_ref().is_none_or(|t| t.id() == k.token_id)
            {
                if let Some(s) = secret_of(&k.credential, CREDENTIAL_PREFIX) {
                    return Ok((MODE_LINK, s));
                }
            }
        }
        match &self.0.token {
            Some(t) if t.id() != st.bad_token => Ok((MODE_ENROLL, t.clone())),
            Some(_) => Err(
                "the manager refused the enrollment token, and this akit has no credential of its \
                 own: make it a new token on the manager's Nodes page, set it as \
                 admin.cluster_member.token, and restart akit"
                    .to_owned(),
            ),
            None => Err(
                "this akit has no credential for the manager and no enrollment token: make it a \
                 token on the manager's Nodes page (Add by enrollment), set it as \
                 admin.cluster_member.token, and restart akit"
                    .to_owned(),
            ),
        }
    }

    /// Dials the manager: fit's `attempt`.
    fn attempt(&self) {
        let (run, mode) = {
            let mut st = lock(&self.0.st);
            if st.stopped {
                return;
            }
            let (mode, secret) = match self.choose(&st) {
                Ok(x) => x,
                Err(why) => {
                    st.status.stopped.clone_from(&why);
                    st.status.linked = false;
                    drop(st);
                    self.log(format!("cluster: not linking to the manager: {why}"));
                    return;
                }
            };
            st.run = st.run.wrapping_add(1);
            st.cur = Some(Attempt {
                run: st.run,
                mode,
                secret,
                ws: None,
                nonce: Vec::new(),
                manager_nonce: Vec::new(),
                open: false,
                ended: false,
                inflight: HashMap::new(),
            });
            (st.run, mode)
        };
        let _ = mode;
        self.touch();
        let Ok(tls) = akio::tls::ClientConfig::builder()
            .resumption(false)
            .pin_sha256(vec![self.0.pin])
            .build()
        else {
            return self.ended(run, Out::Retry, "the TLS settings do not build".to_owned());
        };
        let mut o = akio_http::ws::client::DialOptions::default();
        o.config.max_message_bytes = self
            .0
            .o
            .max_request_bytes
            .saturating_add(crate::link::MAX_HEAD)
            .saturating_add(64);
        o.config.permessage_deflate = false;
        o.tls = Some(tls);
        o.handshake_timeout = HANDSHAKE;
        o.happy_eyeballs.connect_timeout = HANDSHAKE;
        o.headers = vec![("User-Agent".to_owned(), "akit-cluster-member".to_owned())];
        let me = self.clone();
        akio_http::ws::client::dial(
            &self.0.o.engine,
            &self.0.o.settings.manager,
            &o,
            Conn {
                m: Arc::downgrade(&self.0),
                run,
            },
            move |e| {
                me.ended(
                    run,
                    Out::Retry,
                    format!("the manager cannot be reached: {e}"),
                );
            },
        );
        // The handshake's time: a link not welcomed by then is ended.
        let weak = Arc::downgrade(&self.0);
        let _ = akio::timer::after_send(
            HANDSHAKE.saturating_add(Duration::from_secs(1)),
            move || {
                if let Some(inner) = weak.upgrade() {
                    let l = MemberLink(inner);
                    let ws = {
                        let st = lock(&l.0.st);
                        st.cur
                            .as_ref()
                            .filter(|a| a.run == run && !a.open)
                            .and_then(|a| a.ws.clone())
                    };
                    match ws {
                        Some(ws) => ws.abort(),
                        None => {
                            let waiting = lock(&l.0.st)
                                .cur
                                .as_ref()
                                .is_some_and(|a| a.run == run && !a.open);
                            if waiting {
                                l.ended(
                                    run,
                                    Out::Retry,
                                    "the handshake with the manager took too long".to_owned(),
                                );
                            }
                        }
                    }
                }
            },
        );
    }

    /// Attempt `run` ended `out`: dials again after the backoff, or forgets what was refused for
    /// good. fit's `run` loop.
    fn ended(&self, run: u64, out: Out, why: String) {
        let (wait, forget_credential) = {
            let mut st = lock(&self.0.st);
            let Some(a) = st.cur.as_ref().filter(|a| a.run == run && !a.ended) else {
                return;
            };
            let (mode, id) = (a.mode, a.secret.id().to_owned());
            if let Some(a) = st.cur.as_mut() {
                a.ended = true;
            }
            st.cur = None;
            st.status.linked = false;
            st.status.err.clone_from(&why);
            if st.stopped {
                return;
            }
            match out {
                Out::Forget => {
                    if mode == MODE_ENROLL {
                        st.bad_token = id;
                        (Duration::ZERO, false)
                    } else {
                        st.kept = None;
                        (Duration::ZERO, true)
                    }
                }
                Out::Linked | Out::Retry => {
                    if out == Out::Linked {
                        st.backoff = BACKOFF_MIN;
                    }
                    let b = st.backoff;
                    st.backoff = b.saturating_mul(2).min(BACKOFF_MAX);
                    let half = b.checked_div(2).unwrap_or_default();
                    let mut r = [0u8; 8];
                    let _ = akio_crypto::rand::fill_random(&mut r);
                    let span = u64::try_from(half.as_millis())
                        .unwrap_or(0)
                        .saturating_add(1);
                    let jitter =
                        Duration::from_millis(u64::from_le_bytes(r).checked_rem(span).unwrap_or(0));
                    (half.saturating_add(jitter), false)
                }
            }
        };
        if forget_credential {
            if let Err(e) = self.0.o.keeper.save(None) {
                self.log(format!(
                    "cluster: the credential the manager no longer knows could not be forgotten: {e}"
                ));
            }
        }
        if wait.is_zero() {
            return self.attempt();
        }
        self.log(format!(
            "cluster: the link to the manager at {} ended: {why}; linking again in {} ms",
            self.0.o.settings.manager,
            wait.as_millis()
        ));
        let weak = Arc::downgrade(&self.0);
        let _ = akio::timer::after_send(wait, move || {
            if let Some(inner) = weak.upgrade() {
                MemberLink(inner).attempt();
            }
        });
    }

    /// A message of attempt `run`'s link.
    fn on_message(&self, run: u64, ws: &WsSender, t: u8, data: &[u8]) {
        self.touch();
        let (open, mode) = {
            let st = lock(&self.0.st);
            match st.cur.as_ref().filter(|a| a.run == run) {
                Some(a) => (a.open, a.mode),
                None => return ws.abort(),
            }
        };
        match t {
            MSG_REQUEST if open => self.serve(run, ws, data),
            MSG_CANCEL if open => {
                if let Some(id) = data.get(..8).and_then(|b| b.try_into().ok()) {
                    if let Some(a) = lock(&self.0.st).cur.as_mut().filter(|a| a.run == run) {
                        a.inflight.insert(u64::from_be_bytes(id), true);
                    }
                }
            }
            MSG_CHALLENGE if !open => {
                let v = json::parse(data).unwrap_or(Value::Null);
                let theirs = bytes_of(&v, "nonce");
                let Ok(nonce) = linktoken::nonce() else {
                    return ws.abort();
                };
                let hello = {
                    let mut st = lock(&self.0.st);
                    let Some(a) = st
                        .cur
                        .as_mut()
                        .filter(|a| a.run == run && a.nonce.is_empty())
                    else {
                        drop(st);
                        return ws.abort();
                    };
                    if theirs.len() != linktoken::NONCE_SIZE {
                        drop(st);
                        return ws.abort();
                    }
                    a.nonce = nonce.to_vec();
                    a.manager_nonce = theirs;
                    a.ws = Some(ws.clone());
                    let tr = Tr::new(a.secret.id(), &a.nonce, &a.manager_nonce, mode);
                    let proof = a.secret.prove(&tr.t());
                    let mut node = Obj::new()
                        .str_omit("id", &self.0.o.node_id)
                        .str_omit("hostname", &self.0.o.hostname);
                    node = node.str_omit("fit_version", "");
                    Obj::new()
                        .str("mode", mode)
                        .str("id", a.secret.id())
                        .raw("nonce", &b64(&a.nonce))
                        .raw("proof", &b64(&proof))
                        .raw("node", &node.end())
                        .end()
                };
                if ws.send_binary(&msg(MSG_HELLO, &hello)).is_err() {
                    ws.abort();
                }
            }
            MSG_WELCOME if !open => self.welcomed(run, ws, data),
            MSG_REFUSE | MSG_REVOKED => {
                let v = json::parse(data).unwrap_or(Value::Null);
                let reason = json::str_of(&v, "reason");
                let what = if t == MSG_REVOKED {
                    "the manager revoked this akit's credential"
                } else {
                    "the manager refused the link"
                };
                let why = format!("{what} ({reason}): {}", json::str_of(&v, "message"));
                let out = if matches!(
                    reason,
                    REASON_UNKNOWN | REASON_PROOF | REASON_USED | REASON_EXPIRED | REASON_REVOKED
                ) {
                    self.log(format!("cluster: {why}"));
                    Out::Forget
                } else if open {
                    Out::Linked
                } else {
                    Out::Retry
                };
                ws.abort();
                self.ended(run, out, why);
            }
            _ => {
                ws.abort();
                self.ended(
                    run,
                    Out::Retry,
                    "the manager sent a message that is not one".to_owned(),
                );
            }
        }
    }

    /// The manager's welcome, its proof checked: fit's `msgWelcome`.
    fn welcomed(&self, run: u64, ws: &WsSender, data: &[u8]) {
        let v = json::parse(data).unwrap_or(Value::Null);
        let name = json::str_of(&v, "name").to_owned();
        let cred = json::str_of(&v, "credential").to_owned();
        let proof = bytes_of(&v, "proof");
        let (ok, mode, token_id) = {
            let st = lock(&self.0.st);
            let Some(a) = st.cur.as_ref().filter(|a| a.run == run) else {
                return ws.abort();
            };
            let tr = Tr::new(a.secret.id(), &a.nonce, &a.manager_nonce, a.mode);
            (
                a.secret
                    .verify_listener(&tr.t(), &proof, &[name.as_str(), cred.as_str()]),
                a.mode,
                a.secret.id().to_owned(),
            )
        };
        if !ok {
            ws.abort();
            return self.ended(
                run,
                Out::Retry,
                "the manager did not prove the token: it is not the manager that made it"
                    .to_owned(),
            );
        }
        if mode == MODE_ENROLL {
            if secret_of(&cred, CREDENTIAL_PREFIX).is_none() {
                ws.abort();
                return self.ended(
                    run,
                    Out::Retry,
                    "the manager's credential is not one".to_owned(),
                );
            }
            let k = Kept {
                manager: self.0.o.settings.manager.clone(),
                fingerprint: self.0.fp.clone(),
                token_id,
                credential: cred,
                name: name.clone(),
                enrolled: json::time(SystemTime::now()).trim_matches('"').to_owned(),
            };
            if let Err(e) = self.0.o.keeper.save(Some(k.json().as_bytes())) {
                self.log(format!(
                    "cluster: the credential the manager gave cannot be kept ({e}): this akit \
                     links with it until it stops, and then needs a new enrollment token"
                ));
            }
            lock(&self.0.st).kept = Some(k);
            self.log(format!(
                "cluster: enrolled with the manager at {} as the member {name:?}",
                self.0.o.settings.manager
            ));
        }
        {
            let mut st = lock(&self.0.st);
            if let Some(a) = st.cur.as_mut().filter(|a| a.run == run) {
                a.open = true;
            }
            st.status = Status {
                linked: true,
                name: name.clone(),
                since: Some(SystemTime::now()),
                ..Status::default()
            };
        }
        self.log(format!(
            "cluster: linked to the manager at {} as the member {name:?}",
            self.0.o.settings.manager
        ));
        self.keep_alive(run);
    }

    /// Pings the manager every [`PING`]; a link not heard from in [`DEAD`] is ended:
    /// keep-alive's time.
    fn keep_alive(&self, run: u64) {
        let weak = Arc::downgrade(&self.0);
        let _ = akio::timer::after_send(PING, move || {
            let Some(inner) = weak.upgrade() else { return };
            let l = MemberLink(inner);
            let ws = lock(&l.0.st)
                .cur
                .as_ref()
                .filter(|a| a.run == run && a.open)
                .and_then(|a| a.ws.clone());
            let Some(ws) = ws else { return };
            if l.quiet() > DEAD {
                ws.abort();
                return;
            }
            let _ = ws.ping(b"");
            l.keep_alive(run);
        });
    }

    /// A call of the manager's, answered by the Admin API: fit's `serve`.
    fn serve(&self, run: u64, ws: &WsSender, data: &[u8]) {
        let Some((id, head, body)) = read_call(data) else {
            return;
        };
        let reply = |h: String, b: &[u8]| {
            let _ = ws.send_binary(&call_msg(MSG_RESPONSE, id, &h, b));
        };
        let busy = Obj::new().str("e", ANSWER_BUSY).end();
        let path = json::str_of(&head, "p").to_owned();
        if !path.starts_with('/') || body.len() > self.0.o.max_request_bytes {
            return reply(
                Obj::new().num("s", 400).end(),
                br#"{"error":"the request is not one the link takes"}"#,
            );
        }
        {
            let mut st = lock(&self.0.st);
            let Some(a) = st.cur.as_mut().filter(|a| a.run == run) else {
                return;
            };
            if a.inflight.len() >= MAX_LINK_CALLS {
                drop(st);
                return reply(busy, b"");
            }
            a.inflight.insert(id, false);
        }
        let t = Duration::from_millis(json::uint_of(&head, "t"))
            .max(Duration::from_secs(1))
            .min(MAX_TIMEOUT);
        let max = usize::try_from(json::uint_of(&head, "x")).unwrap_or(usize::MAX);
        let me = self.clone();
        let ws2 = ws.clone();
        // The answer goes once: the Admin API's, or the timeout's.
        let once = Arc::new(Mutex::new(true));
        let once2 = Arc::clone(&once);
        let me2 = self.clone();
        let ws3 = ws.clone();
        let timer = akio::timer::after_send(t, move || {
            if std::mem::take(&mut *lock(&once2)) {
                let gone = me2.done1(run, id);
                if !gone {
                    let _ = ws3.send_binary(&call_msg(
                        MSG_RESPONSE,
                        id,
                        &Obj::new().str("e", ANSWER_TIMEOUT).end(),
                        b"",
                    ));
                }
            }
        })
        .ok();
        let req = Request {
            method: json::str_of(&head, "m").to_owned(),
            path,
            headers: header_of(head.get("h")),
            body: body.to_vec(),
            timeout: t,
            max,
        };
        (self.0.o.api)(
            req,
            Box::new(move |r| {
                if !std::mem::take(&mut *lock(&once)) {
                    return;
                }
                if let Some(tm) = timer {
                    let _ = tm.cancel();
                }
                if me.done1(run, id) {
                    return; // cancelled by the manager
                }
                let (h, b) = match r {
                    Ok(a) if max > 0 && a.body.len() > max => {
                        (Obj::new().str("e", ANSWER_TOO_LARGE).end(), Vec::new())
                    }
                    Ok(a) => {
                        let hs: Vec<(String, String)> = a
                            .headers
                            .into_iter()
                            .filter(|(n, _)| {
                                !["connection", "content-length", "transfer-encoding", "date"]
                                    .iter()
                                    .any(|x| x.eq_ignore_ascii_case(n))
                            })
                            .collect();
                        (
                            Obj::new()
                                .num("s", a.status)
                                .raw("h", &header_json(&hs))
                                .end(),
                            a.body,
                        )
                    }
                    Err(e) => (
                        Obj::new().num("s", 502).end(),
                        Obj::new()
                            .str("error", &format!("the Admin API: {e}"))
                            .end()
                            .into_bytes(),
                    ),
                };
                let _ = ws2.send_binary(&call_msg(MSG_RESPONSE, id, &h, &b));
            }),
        );
    }

    /// Call `id` of attempt `run` is over: whether the manager cancelled it.
    fn done1(&self, run: u64, id: u64) -> bool {
        lock(&self.0.st)
            .cur
            .as_mut()
            .filter(|a| a.run == run)
            .and_then(|a| a.inflight.remove(&id))
            .unwrap_or(true)
    }
}

/// The handler of the link's WebSocket on the member.
struct Conn {
    m: Weak<Inner>,
    run: u64,
}

impl WsHandler for Conn {
    fn on_open(&mut self, ws: &WsSender) {
        let Some(inner) = self.m.upgrade() else {
            return ws.abort();
        };
        let l = MemberLink(inner);
        let mut st = lock(&l.0.st);
        match st.cur.as_mut().filter(|a| a.run == self.run && !a.ended) {
            Some(a) => a.ws = Some(ws.clone()),
            None => {
                drop(st);
                ws.abort();
            }
        }
    }

    fn on_message_ref(&mut self, ws: &WsSender, m: MessageRef<'_>) {
        let Some(inner) = self.m.upgrade() else {
            return;
        };
        let data = m.as_bytes();
        if let Some((&t, rest)) = data.split_first() {
            MemberLink(inner).on_message(self.run, ws, t, rest);
        }
    }

    fn on_pong(&mut self, _ws: &WsSender, _payload: &[u8]) {
        if let Some(inner) = self.m.upgrade() {
            MemberLink(inner).touch();
        }
    }

    fn on_close(&mut self, code: Option<u16>, reason: &str) {
        let Some(inner) = self.m.upgrade() else {
            return;
        };
        let l = MemberLink(inner);
        let open = lock(&l.0.st)
            .cur
            .as_ref()
            .is_some_and(|a| a.run == self.run && a.open);
        let why = match code {
            Some(c) if c != 1000 && c != 1005 && c != 1006 => {
                format!("closed by the manager ({c} {reason})")
            }
            _ => "the link closed".to_owned(),
        };
        l.ended(self.run, if open { Out::Linked } else { Out::Retry }, why);
    }
}
