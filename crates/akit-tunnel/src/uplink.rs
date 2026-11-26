//! A hub a node or proxy links to (fit's `tunnel/internal/uplink`): `pool_size` links, each
//! dialing the hub's first endpoint not disabled and falling back to the next, registering (with a
//! token: a challenge first; with the shared password: a MAC), checking the hub's proof before it
//! serves anything the hub sends, and dialing again `retry_interval` after a failure or a loss. One
//! link measures the round trip to the hub every `ping_interval`.
//!
//! Every step is a callback: a dial's on akio's DNS and engine, a call's answer on the link's I/O
//! thread, a retry and a ping on akio's timer thread. Nothing waits.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use akit_config::roles::{TunnelProtocol, TunnelUplink, parse_endpoint};
use akit_roles::linktoken::Secret;

use crate::crypt::Coder;
use crate::link::{CallError, ConnWire, Link, LinkHandler, Router};
use crate::msg::{self, Info, RegisterReq, RegisterRsp, Stats, method};

/// What a role tells its uplinks of: what to register as, and what to do once registered.
pub trait Owner: Send + Sync + 'static {
    /// What the role registers as.
    fn info(&self) -> Info;
    /// A link to the hub is registered and proven.
    fn registered(&self, _u: &Arc<Uplink>, _l: &Arc<Link>, _first: bool) {}
    /// What a ping tells the hub besides the round trip: a node's services' connects.
    fn ping_extra(&self) -> String {
        String::new()
    }
    /// The hub's answer to a ping.
    fn pong(&self, _u: &Arc<Uplink>, _body: &[u8]) {}
}

/// The state of an uplink, as the pages show it.
pub const STATE_CONNECTING: &str = "connecting";
/// Registered on at least one link.
pub const STATE_REGISTERED: &str = "registered";
/// The last dial or registration failed.
pub const STATE_FAILED: &str = "failed";

struct UpState {
    links: HashMap<u64, (Arc<Link>, Option<RegisterRsp>)>,
    state: &'static str,
    last_err: String,
    hub: String,
    rtt: Stats,
}

/// A hub a role links to.
pub struct Uplink {
    /// The hub's name here.
    pub name: String,
    /// Its config.
    pub cfg: TunnelUplink,
    engine: Arc<akio::Engine>,
    router: Arc<dyn Router>,
    owner: Weak<dyn Owner>,
    coder: Option<Coder>,
    token: Option<Secret>,
    /// The TLS of `tls://` endpoints, made when the uplink is (on the control thread: it reads
    /// `ca_file`).
    tls: Option<akio::tls::ClientConfig>,
    /// The TLS of `quic://` endpoints, which `quic_insecure` may leave unchecked.
    quic: Option<akio::tls::ClientConfig>,
    /// The TLS of `wss://` endpoints: the `tls://` check, ALPN `http/1.1`.
    wss: Option<akio::tls::ClientConfig>,
    proven: Arc<Mutex<HashSet<u64>>>,
    st: Mutex<UpState>,
    seq: AtomicU32,
    stopped: AtomicBool,
    pinging: AtomicBool,
}

impl std::fmt::Debug for Uplink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Uplink")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn lock(m: &Mutex<UpState>) -> MutexGuard<'_, UpState> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The addresses a link dials, in order: the endpoints not disabled.
#[must_use]
pub fn dials(cfg: &TunnelUplink) -> Vec<String> {
    cfg.endpoints
        .iter()
        .filter(|e| !e.disabled && !e.address.is_empty())
        .map(|e| e.address.clone())
        .collect()
}

impl Uplink {
    /// An uplink of `cfg`, its links served by `router` and marked in `proven` once the hub has
    /// proven itself on them. Nothing dials until [`Uplink::start`].
    ///
    /// # Errors
    ///
    /// A token that does not parse.
    pub fn new(
        engine: Arc<akio::Engine>,
        cfg: &TunnelUplink,
        router: Arc<dyn Router>,
        owner: Weak<dyn Owner>,
        proven: Arc<Mutex<HashSet<u64>>>,
    ) -> Result<Arc<Uplink>, String> {
        let token = if cfg.token.is_empty() {
            None
        } else {
            Some(Secret::parse(&cfg.token).map_err(|e| format!("token: {e}"))?)
        };
        let coder = Coder::new(&cfg.body_password).map(|c| match &token {
            Some(t) => Coder::token(
                t.id(),
                &akit_roles::linktoken::body_key(&t.stored()),
                &cfg.body_password,
            )
            .dialer(),
            None => c.dialer(),
        });
        let check = crate::tls::Check::of(&cfg.pin, &cfg.ca_file, false);
        let needs_tls = dials(cfg)
            .iter()
            .any(|a| parse_endpoint(a).is_some_and(|e| e.protocol == TunnelProtocol::Tls));
        let tls = if needs_tls {
            Some(crate::tls::client_config(&check)?)
        } else {
            None
        };
        let needs_wss = dials(cfg)
            .iter()
            .any(|a| parse_endpoint(a).is_some_and(|e| e.protocol == TunnelProtocol::Wss));
        let wss = if needs_wss {
            Some(crate::tls::client_config_alpn(&check, crate::ws::ALPN)?)
        } else {
            None
        };
        let needs_quic = dials(cfg)
            .iter()
            .any(|a| parse_endpoint(a).is_some_and(|e| e.protocol == TunnelProtocol::Quic));
        let quic = if needs_quic {
            let check = crate::tls::Check::of(&cfg.pin, &cfg.ca_file, cfg.quic_insecure);
            Some(crate::tls::client_config(&check)?)
        } else {
            None
        };
        Ok(Arc::new(Uplink {
            name: cfg.hub_name().to_owned(),
            cfg: cfg.clone(),
            engine,
            router,
            owner,
            coder,
            token,
            tls,
            quic,
            wss,
            proven,
            st: Mutex::new(UpState {
                links: HashMap::new(),
                state: STATE_CONNECTING,
                last_err: String::new(),
                hub: String::new(),
                rtt: Stats::default(),
            }),
            seq: AtomicU32::new(0),
            stopped: AtomicBool::new(false),
            pinging: AtomicBool::new(false),
        }))
    }

    /// Starts the pool's links and the pings.
    pub fn start(self: &Arc<Self>) {
        for _ in 0..self.cfg.pool() {
            self.keep(0);
        }
        self.schedule_ping();
    }

    /// Stops: no more dials; every link closed.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let links: Vec<Arc<Link>> = lock(&self.st)
            .links
            .values()
            .map(|(l, _)| Arc::clone(l))
            .collect();
        for l in links {
            l.close();
        }
    }

    /// Whether it is stopped.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn fail(&self, msg: &str) {
        let mut st = lock(&self.st);
        if !self.stopped() && st.links.values().all(|(_, r)| r.is_none()) {
            st.state = STATE_FAILED;
            msg.clone_into(&mut st.last_err);
        }
    }

    /// The hub kicked this side (fit's `fail(..., true)`): the state says so, whatever links
    /// are still registered, until a link registers again.
    pub fn kicked(&self, why: &str) {
        let mut st = lock(&self.st);
        if !self.stopped() {
            st.state = STATE_FAILED;
            st.last_err = format!("kicked by the hub: {why}");
        }
    }

    /// Dials the endpoint at `at` among the dials, falling back to the next; once every one has
    /// failed, waits `retry_interval` and starts over.
    fn keep(self: &Arc<Self>, at: usize) {
        if self.stopped() {
            return;
        }
        let ds = dials(&self.cfg);
        let Some(addr) = ds.get(at).cloned() else {
            if ds.is_empty() {
                self.fail("no address to dial");
            }
            self.retry();
            return;
        };
        let Some(ep) = parse_endpoint(&addr) else {
            self.fail(&format!("{addr}: not an address to dial"));
            self.keep(at.saturating_add(1));
            return;
        };
        let me = Arc::clone(self);
        let me2 = Arc::clone(self);
        let mut he = akio::dns::HappyEyeballs::default();
        he.connect_timeout = self.cfg.dial();
        let router = Arc::clone(&self.router);
        let coder = self.coder.clone();
        let url = addr.clone();
        let on_error = move |e: std::io::Error| {
            me2.fail(&format!("{addr}: {e}"));
            me2.keep(at.saturating_add(1));
        };
        match ep.protocol {
            TunnelProtocol::Tcp => he.connect(
                &self.engine,
                &ep.host,
                ep.port,
                akio::fn_acceptor(move |c: &akio::Conn<LinkHandler>| {
                    let l = Link::new(
                        Arc::new(ConnWire::new(c)),
                        Arc::clone(&router),
                        coder.clone(),
                    );
                    me.dialed(&l);
                    LinkHandler(Some(l))
                }),
                on_error,
            ),
            TunnelProtocol::Tls => {
                let Some(cfg) = self.tls.clone() else {
                    return on_error(std::io::Error::other("no TLS configured"));
                };
                // The link starts at once: what it sends waits for the handshake, which checks
                // the hub's certificate (its pin too) first.
                let inner = akio::fn_acceptor(move |c: &akio::Conn<LinkHandler>| {
                    let l = Link::new(
                        Arc::new(ConnWire::new(c)),
                        Arc::clone(&router),
                        coder.clone(),
                    );
                    me.dialed(&l);
                    LinkHandler(Some(l))
                });
                match akio::tls::Tls::client(cfg, &ep.host, inner) {
                    Ok(acceptor) => he.connect(&self.engine, &ep.host, ep.port, acceptor, on_error),
                    Err(e) => on_error(e),
                }
            }
            TunnelProtocol::Quic => {
                let Some(cfg) = self.quic.clone() else {
                    return on_error(std::io::Error::other("no TLS configured"));
                };
                crate::quic::dial(
                    &self.engine,
                    &ep.host,
                    ep.port,
                    &cfg,
                    self.cfg.dial(),
                    router,
                    coder,
                    Box::new(move |r| match r {
                        Ok(l) => me.dialed(&l),
                        Err(e) => on_error(std::io::Error::other(e)),
                    }),
                );
            }
            TunnelProtocol::Ws | TunnelProtocol::Wss => {
                let tls = if ep.protocol == TunnelProtocol::Wss {
                    let Some(cfg) = self.wss.clone() else {
                        return on_error(std::io::Error::other("no TLS configured"));
                    };
                    Some(cfg)
                } else {
                    None
                };
                let opts = crate::ws::dial_options(tls, self.cfg.dial());
                let open: crate::ws::Open = Box::new(move |ws| {
                    let l = Link::new(
                        Arc::new(crate::ws::WsWire::new(ws)),
                        Arc::clone(&router),
                        coder.clone(),
                    );
                    me.dialed(&l);
                    Some(l)
                });
                akio_http::ws::client::dial(
                    &self.engine,
                    &url,
                    &opts,
                    crate::ws::WsLink::new(open),
                    move |e| on_error(std::io::Error::other(e.to_string())),
                );
            }
        }
    }

    fn retry(self: &Arc<Self>) {
        if self.stopped() {
            return;
        }
        let weak = Arc::downgrade(self);
        let _ = akio::timer::after_send(self.cfg.retry(), move || {
            if let Some(u) = weak.upgrade() {
                u.keep(0);
            }
        });
    }

    fn dialed(self: &Arc<Self>, l: &Arc<Link>) {
        if self.stopped() {
            l.close();
            return;
        }
        lock(&self.st).links.insert(l.id, (Arc::clone(l), None));
        let Some(owner) = self.owner.upgrade() else {
            l.close();
            return;
        };
        let mut req = RegisterReq {
            info: owner.info(),
            ..RegisterReq::default()
        };
        match &self.token {
            Some(sec) => {
                let own = akit_roles::linktoken::nonce().unwrap_or([0; 32]);
                let body = msg::Obj::new()
                    .str("token", sec.id())
                    .str("nonce", &akit_roles::linktoken::hex(&own))
                    .end();
                let me = Arc::clone(self);
                let l2 = Arc::clone(l);
                let sec = sec.clone();
                l.call(
                    method::CHALLENGE,
                    body.as_bytes(),
                    self.cfg.call(),
                    None,
                    Box::new(move |r| {
                        let hub = match r {
                            Ok(b) => msg::parse(&b)
                                .and_then(|v| msg::decode_nonce(&msg::get_str(&v, "nonce"))),
                            Err(CallError::Remote(e))
                                if e.contains("register first") || e.contains("no method") =>
                            {
                                me.fail(&format!(
                                    "tunnel: the hub runs a fit without link tokens, older than this side's {}; upgrade it, or register with its password until then",
                                    msg::version()
                                ));
                                l2.close();
                                return;
                            }
                            Err(e) => {
                                me.fail(&format!("challenge: {e}"));
                                l2.close();
                                return;
                            }
                        };
                        let Some(hub) = hub else {
                            me.fail("tunnel: a challenge's nonce is 32 bytes in hex");
                            l2.close();
                            return;
                        };
                        msg::prove_token(&mut req, &sec, &own, &hub);
                        let check_req = req.clone();
                        me.register(
                            &l2,
                            req,
                            Box::new(move |rsp| {
                                if msg::check_token_proof(&check_req, rsp, &sec, &own, &hub) {
                                    Ok(())
                                } else {
                                    Err("tunnel: the hub did not prove it issued the token: it is not the hub, or the token is not its".to_owned())
                                }
                            }),
                        );
                    }),
                );
            }
            None => {
                msg::sign(&mut req, &self.cfg.password);
                let check_req = req.clone();
                let pw = self.cfg.password.clone();
                self.register(
                    l,
                    req,
                    Box::new(move |rsp| msg::check_proof(&check_req, rsp, &pw)),
                );
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn register(
        self: &Arc<Self>,
        l: &Arc<Link>,
        req: RegisterReq,
        check: Box<dyn FnOnce(&RegisterRsp) -> Result<(), String> + Send>,
    ) {
        let proven = Arc::clone(&self.proven);
        let lid = l.id;
        let me = Arc::clone(self);
        let l2 = Arc::clone(l);
        l.call(
            method::REGISTER,
            req.json().as_bytes(),
            self.cfg.call(),
            Some(Box::new(move |body: &[u8]| {
                let rsp = msg::parse(body)
                    .map(|v| RegisterRsp::from(&v))
                    .ok_or("tunnel: the hub's answer is not JSON")?;
                check(&rsp)?;
                proven
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(lid);
                Ok(())
            })),
            Box::new(move |r| match r {
                Ok(body) => {
                    let rsp = msg::parse(&body)
                        .map(|v| RegisterRsp::from(&v))
                        .unwrap_or_default();
                    me.registered(&l2, rsp);
                }
                Err(e) => {
                    me.fail(&format!("register: {e}"));
                    akit_observe::warn!(Tunnel, "hub {}: registering failed: {e}", me.name);
                    l2.close();
                }
            }),
        );
    }

    fn registered(self: &Arc<Self>, l: &Arc<Link>, rsp: RegisterRsp) {
        let first = {
            let mut st = lock(&self.st);
            if self.stopped() || !st.links.contains_key(&l.id) {
                drop(st);
                l.close();
                return;
            }
            let first = st.links.values().all(|(_, r)| r.is_none());
            if let Some(e) = st.links.get_mut(&l.id) {
                e.1 = Some(rsp.clone());
            }
            st.state = STATE_REGISTERED;
            st.last_err.clear();
            rsp.hub.clone_into(&mut st.hub);
            if first {
                st.rtt = Stats::default();
            }
            first
        };
        akit_observe::debug!(
            Tunnel,
            "hub {}: registered with {:?} as peer {}, link {} in place {}",
            self.name,
            rsp.hub,
            rsp.peer,
            rsp.conn,
            rsp.index
        );
        if let Some(o) = self.owner.upgrade() {
            o.registered(self, l, first);
        }
    }

    /// A link of the uplink closed: forgotten, and dialed again after `retry_interval`.
    /// Returns whether it was the uplink's.
    pub fn lost(self: &Arc<Self>, l: &Arc<Link>) -> bool {
        let was = {
            let mut st = lock(&self.st);
            let was = st.links.remove(&l.id).is_some();
            if was && st.links.values().all(|(_, r)| r.is_none()) && st.state == STATE_REGISTERED {
                st.state = STATE_CONNECTING;
            }
            was
        };
        self.proven
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&l.id);
        if was {
            self.retry();
        }
        was
    }

    /// Whether `l` is one of the uplink's links.
    #[must_use]
    pub fn owns(&self, l: &Link) -> bool {
        lock(&self.st).links.contains_key(&l.id)
    }

    /// The registered links, with the hub's numbers of them.
    #[must_use]
    pub fn registered_links(&self) -> Vec<(Arc<Link>, RegisterRsp)> {
        lock(&self.st)
            .links
            .values()
            .filter_map(|(l, r)| r.clone().map(|r| (Arc::clone(l), r)))
            .collect()
    }

    /// The next sequence of a tunnel this side opens.
    pub fn next_seq(&self) -> u32 {
        self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    /// The round trips to the hub.
    #[must_use]
    pub fn rtt(&self) -> Stats {
        lock(&self.st).rtt
    }

    /// The state, the last error and the hub's name.
    #[must_use]
    pub fn status(&self) -> (&'static str, String, String) {
        let st = lock(&self.st);
        (st.state, st.last_err.clone(), st.hub.clone())
    }

    fn schedule_ping(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let _ = akio::timer::after_send(self.cfg.ping(), move || {
            if let Some(u) = weak.upgrade() {
                if u.stopped() {
                    return;
                }
                u.ping();
                u.schedule_ping();
            }
        });
    }

    /// Measures the round trip to the hub on one registered link.
    pub fn ping(self: &Arc<Self>) {
        let Some((l, _)) = self.registered_links().into_iter().next() else {
            return;
        };
        if self.pinging.swap(true, Ordering::SeqCst) {
            return;
        }
        let extra = self
            .owner
            .upgrade()
            .map(|o| o.ping_extra())
            .unwrap_or_default();
        let mut body = msg::Obj::new().raw("rtt", &self.rtt().json());
        if !extra.is_empty() {
            body = body.raw("serviceRtt", &extra);
        }
        let start = Instant::now();
        let me = Arc::clone(self);
        l.call(
            method::PING,
            body.end().as_bytes(),
            self.cfg.ping_wait(),
            None,
            Box::new(move |r| {
                me.pinging.store(false, Ordering::SeqCst);
                match r {
                    Ok(b) => {
                        let ms = start.elapsed().as_secs_f64() * 1000.0;
                        lock(&me.st).rtt.add((ms * 1000.0).round() / 1000.0);
                        if let Some(o) = me.owner.upgrade() {
                            o.pong(&me, &b);
                        }
                    }
                    Err(_) => lock(&me.st).rtt.lost(),
                }
            }),
        );
    }
}

/// How long until `d` from now, for timers that take a duration.
#[must_use]
pub fn clamp(d: Duration) -> Duration {
    d.max(Duration::from_millis(10))
}
