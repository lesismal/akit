//! fit's `mesh/dial.go`: the dial targets, each kept linked until it is dropped, dialed again
//! after a backoff with full jitter when its link fails or is lost; the hello on an outbound link.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::timer::{SendTimer, after_send};
use akit_roles::linktoken::Secret;

use super::{Inner, MConn, Mesh, bad_request, lock, validate_addr};
use crate::model::{DialReq, DialTarget, Identity, METHOD_HELLO};
use crate::wire::{self, Error};

/// One dial's bound, its handshakes included.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// The backoff's first step.
const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// The shortest wait between two dials.
const BACKOFF_FLOOR: Duration = Duration::from_millis(100);
/// A link that lived this long resets the backoff.
const STABLE_CONN: Duration = Duration::from_secs(10);
/// The shortest wait after a refused hello.
const REFUSED_RETRY: Duration = Duration::from_secs(30);
/// A hello's bound, and the wait before trying it again on a live link.
const HELLO_CALL: Duration = Duration::from_secs(10);
const HELLO_AGAIN: Duration = Duration::from_secs(5);

fn rand_below(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut b = [0u8; 8];
    let _ = akio_crypto::rand::fill_random(&mut b);
    u64::from_le_bytes(b).checked_rem(n).unwrap_or(0)
}

/// The wait before the next dial after `n` failures in a row (full jitter): a random duration up
/// to `min(ceiling, 1s·2ⁿ)`, at least 100 ms.
#[must_use]
pub fn backoff(n: u32, ceiling: Duration) -> Duration {
    let mut step = ceiling;
    if n < 30 {
        let s = BACKOFF_BASE.saturating_mul(1u32 << n);
        if s < ceiling {
            step = s;
        }
    }
    let ns = u64::try_from(step.as_nanos()).unwrap_or(u64::MAX);
    Duration::from_nanos(rand_below(ns).saturating_add(1)).max(BACKOFF_FLOOR)
}

/// The wait after a refused hello: at least `max(30s, ceiling)`, plus up to half of it.
#[must_use]
pub fn refused_backoff(ceiling: Duration) -> Duration {
    let base = REFUSED_RETRY.max(ceiling);
    let half = u64::try_from(base.as_nanos() >> 1).unwrap_or(u64::MAX);
    base.saturating_add(Duration::from_nanos(rand_below(half.saturating_add(1))))
}

#[derive(Default)]
struct TState {
    conn: Option<Arc<MConn>>,
    err: String,
    stopped: bool,
    next_delay: Duration,
    fails: u32,
    since: Option<Instant>,
    timer: Option<SendTimer>,
    dialing: Option<wire::conn::DialHandle>,
}

/// A dial target.
pub(crate) struct Target {
    pub(crate) addr: String,
    req: DialReq,
    password: String,
    token: String,
    sec: Option<Secret>,
    tls: Option<akio::tls::ClientConfig>,
    st: Mutex<TState>,
}

impl Target {
    pub(crate) fn view(&self) -> DialTarget {
        let g = lock(&self.st);
        let mut t = DialTarget {
            addr: self.addr.clone(),
            role: self.req.role.clone(),
            tls: self.tls.is_some(),
            pin: self.req.pin.clone(),
            ca_file: self.req.ca_file.clone(),
            server_name: self.req.server_name.clone(),
            token_id: self
                .sec
                .as_ref()
                .map(|s| s.id().to_owned())
                .unwrap_or_default(),
            error: g.err.clone(),
            ..DialTarget::default()
        };
        if let Some(c) = &g.conn {
            if let Some(p) = c.peer().filter(|_| c.live()) {
                t.connected = true;
                t.peer = Some(p);
                t.error.clear();
            }
        }
        t
    }

    fn set_err(&self, e: String) {
        lock(&self.st).err = e;
    }
}

/// The TLS a target of `req` at `host` is dialed with, `None` without: the listener's
/// certificate checked by its pin, or by the authorities of its CA file with the name; no session
/// resumed. Reads the CA file: the control thread's.
fn client_tls(req: &DialReq, host: &str) -> Result<Option<akio::tls::ClientConfig>, Error> {
    if !req.tls() {
        if !akit_config::roles::loopback(host) {
            return Err(bad_request(format!(
                "{} is not a loopback address: set pin to the fingerprint of the peer listener's certificate, as its pages show it, or ca_file, so that the link is made over TLS to that listener alone",
                req.addr
            )));
        }
        return Ok(None);
    }
    if !req.pin.is_empty() && !req.ca_file.is_empty() {
        return Err(bad_request("pin or ca_file, one of the two"));
    }
    let b = akio::tls::ClientConfig::builder()
        .tls13_only(true)
        .resumption(false);
    let b = if req.pin.is_empty() {
        let pem = std::fs::read(&req.ca_file).map_err(|e| bad_request(format!("ca_file: {e}")))?;
        let mut roots = akio::tls::RootCerts::empty();
        let n = roots
            .add_pem(&pem)
            .map_err(|e| bad_request(format!("ca_file: {e}")))?;
        if n == 0 {
            return Err(bad_request(format!(
                "ca_file {} holds no PEM certificate",
                req.ca_file
            )));
        }
        b.roots(roots)
    } else {
        let pin = akit_config::roles::parse_pin(&req.pin).ok_or_else(|| {
            bad_request(format!(
                "bad certificate pin {:?}: it is the SHA-256 of the certificate, 64 hex digits",
                req.pin
            ))
        })?;
        b.pin_sha256(vec![pin])
    };
    b.build().map(Some).map_err(|e| bad_request(e.to_string()))
}

fn host_of(addr: &str) -> String {
    match addr.rsplit_once(':') {
        Some((h, _)) => h.trim_start_matches('[').trim_end_matches(']').to_owned(),
        None => addr.to_owned(),
    }
}

impl Mesh {
    /// Checks that `role` is one this role links to.
    ///
    /// # Errors
    ///
    /// A bad request.
    pub fn validate_role(&self, role: &str) -> Result<(), Error> {
        let me = self.identity().role;
        if role.is_empty()
            || role == crate::model::ROLE_LOCAL
            || role == me
            || !self.0.cfg.accept_roles.iter().any(|r| r == role)
        {
            return Err(bad_request(format!(
                "role must be one of {:?}",
                self.0.cfg.accept_roles
            )));
        }
        Ok(())
    }

    /// Keeps a link to `req.addr` until [`Mesh::undial`]; the peer must say it is `req.role`.
    /// Dialing a target again with another token, password, role or TLS replaces it; neither a
    /// token nor a password keeps the ones it has.
    ///
    /// # Errors
    ///
    /// A bad address, role, token or TLS setting; the mesh stopped.
    pub fn dial(&self, req: &DialReq) -> Result<(), Error> {
        let addr = validate_addr(&req.addr, true)?;
        self.validate_role(&req.role)?;
        let tls = client_tls(req, &host_of(&addr))?;
        if self.0.stopped.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let mut sec = match req.token.as_str() {
            "" => None,
            t => Some(Secret::parse(t).map_err(bad_request)?),
        };
        let (t, old) = {
            let mut st = lock(&self.0.st);
            let old = st.targets.get(&addr).cloned();
            let (mut password, mut token) = (req.password.clone(), req.token.clone());
            if !token.is_empty() {
                password.clear();
            }
            if password.is_empty() && token.is_empty() {
                if let Some(o) = &old {
                    password.clone_from(&o.password);
                    token.clone_from(&o.token);
                    sec.clone_from(&o.sec);
                }
            }
            if password.is_empty() && token.is_empty() {
                return Err(bad_request("token or password required"));
            }
            if let Some(o) = &old {
                if o.password == password
                    && o.token == token
                    && o.req.role == req.role
                    && o.req.same_tls(req)
                {
                    return Ok(());
                }
            }
            let t = Arc::new(Target {
                addr: addr.clone(),
                req: DialReq {
                    addr: addr.clone(),
                    ..req.clone()
                },
                password,
                token,
                sec,
                tls,
                st: Mutex::new(TState::default()),
            });
            st.targets.insert(addr.clone(), Arc::clone(&t));
            (t, old)
        };
        if let Some(o) = old {
            self.0.stop_target(&o);
        }
        akit_observe::info!(Llm, "mesh: dial target set: addr={addr} role={}", req.role);
        self.0.dial_target(&t);
        Ok(())
    }

    /// Stops dialing `addr` and closes its link; false when it is not a target.
    pub fn undial(&self, addr: &str) -> bool {
        let t = lock(&self.0.st).targets.remove(addr);
        let Some(t) = t else { return false };
        self.0.stop_target(&t);
        akit_observe::info!(Llm, "mesh: dial target removed: addr={addr}");
        true
    }
}

impl Inner {
    pub(crate) fn stop_target(&self, t: &Arc<Target>) {
        let (conn, timer, dialing) = {
            let mut g = lock(&t.st);
            g.stopped = true;
            (g.conn.take(), g.timer.take(), g.dialing.take())
        };
        if let Some(t) = timer {
            t.cancel();
        }
        if let Some(d) = dialing {
            d.abort();
        }
        if let Some(c) = conn {
            c.wc.close();
            self.disconnected(&c.wc, None);
        }
    }

    fn gone(&self, t: &Target) -> bool {
        lock(&t.st).stopped
            || self.stopped.load(Ordering::Acquire)
            || self.draining.load(Ordering::Acquire)
    }

    /// Dials `t` once, unless it or the mesh stopped.
    pub(crate) fn dial_target(self: &Arc<Self>, t: &Arc<Target>) {
        if self.gone(t) {
            return;
        }
        let limits = *lock(&self.limits);
        let mut session = limits.session();
        session.dialer = true;
        let (m, tt) = (Arc::downgrade(self), Arc::clone(t));
        let (m2, m3) = (m.clone(), m.clone());
        let cfg = wire::DialConfig {
            session,
            timing: limits.timing(),
            token: t.sec.clone(),
            password: if t.sec.is_some() {
                String::new()
            } else {
                t.password.clone()
            },
            tls: t.tls.clone(),
            server_name: (!t.req.server_name.is_empty()).then(|| t.req.server_name.clone()),
            timeout: DIAL_TIMEOUT,
            handlers: Arc::new(move |wc| match m.upgrade() {
                Some(m) => m.handler_of(wc, Some(Arc::clone(&tt))),
                None => Arc::new(super::NoMesh),
            }),
            on_close: Some(Arc::new(move |wc, e| {
                if let Some(m) = m2.upgrade() {
                    m.lost(wc, e);
                }
            })),
        };
        let scheme = if t.tls.is_some() { "wss://" } else { "ws://" };
        let url = format!("{scheme}{}{}", t.addr, wire::conn::PATH);
        let t2 = Arc::clone(t);
        let h = wire::conn::dial(&self.cfg.engine, &url, cfg, move |r| {
            if let Some(m) = m3.upgrade() {
                m.dialed(&t2, r);
            }
        });
        let mut g = lock(&t.st);
        if g.stopped {
            drop(g);
            h.abort();
        } else {
            g.dialing = Some(h);
        }
    }

    fn dialed(self: &Arc<Self>, t: &Arc<Target>, r: Result<Arc<wire::Conn>, Error>) {
        lock(&t.st).dialing = None;
        match r {
            Ok(wc) => {
                let Some(c) = self.conn_of(&wc) else {
                    return self.redial(t, None);
                };
                {
                    let mut g = lock(&t.st);
                    if g.stopped {
                        drop(g);
                        wc.close();
                        self.disconnected(&wc, None);
                        return;
                    }
                    g.conn = Some(Arc::clone(&c));
                    g.since = Some(Instant::now());
                }
                self.hello(&c, 0);
            }
            Err(e) => {
                if e == Error::Closed && self.gone(t) {
                    return;
                }
                t.set_err(e.to_string());
                let (delay, fails) = {
                    let mut g = lock(&t.st);
                    let d = backoff(g.fails, lock(&self.limits).retry);
                    g.fails = g.fails.saturating_add(1);
                    (d, g.fails)
                };
                if fails % 10 == 1 || lock(&self.limits).retry >= Duration::from_secs(30) {
                    akit_observe::warn!(
                        Llm,
                        "mesh: dial failed, retrying: addr={} retry_in={delay:?} failures={fails} error={e}",
                        t.addr
                    );
                }
                self.redial(t, Some(delay));
            }
        }
    }

    /// Dials `t` again after `delay` (none: its backoff).
    fn redial(self: &Arc<Self>, t: &Arc<Target>, delay: Option<Duration>) {
        if self.gone(t) {
            return;
        }
        let delay = delay.unwrap_or_else(|| {
            let mut g = lock(&t.st);
            let d = backoff(g.fails, lock(&self.limits).retry);
            g.fails = g.fails.saturating_add(1);
            d
        });
        let (m, tt) = (Arc::downgrade(self), Arc::clone(t));
        let timer = after_send(delay, move || {
            if let Some(m) = m.upgrade() {
                lock(&tt.st).timer = None;
                m.dial_target(&tt);
            }
        });
        if let Ok(timer) = timer {
            let mut g = lock(&t.st);
            if g.stopped {
                drop(g);
                timer.cancel();
            } else {
                g.timer = Some(timer);
            }
        }
    }

    /// A link closed: forgotten, and its target dialed again.
    fn lost(self: &Arc<Self>, wc: &Arc<wire::Conn>, err: &Error) {
        let c = self.conn_of(wc);
        self.disconnected(wc, Some(err));
        let Some(t) = c.and_then(|c| c.target.clone().map(|t| (c, t))) else {
            return;
        };
        let (c, t) = t;
        let delay = {
            let mut g = lock(&t.st);
            if !g.conn.as_ref().is_some_and(|x| Arc::ptr_eq(x, &c)) {
                return; // replaced or stopped
            }
            g.conn = None;
            if g.since.is_some_and(|s| s.elapsed() >= STABLE_CONN) {
                g.fails = 0;
            }
            let next = std::mem::take(&mut g.next_delay);
            if next.is_zero() {
                let d = backoff(g.fails, lock(&self.limits).retry);
                g.fails = g.fails.saturating_add(1);
                d
            } else {
                next
            }
        };
        if !self.gone(&t) {
            akit_observe::info!(
                Llm,
                "mesh: connection lost, dialing again: addr={} retry_in={delay:?} reason={err}",
                t.addr
            );
        }
        self.redial(&t, Some(delay));
    }

    /// Says hello on an outbound link, trying again while it lives.
    fn hello(self: &Arc<Self>, c: &Arc<MConn>, attempt: u32) {
        let Some(t) = c.target.clone() else { return };
        if !c.live() || self.stopped.load(Ordering::Acquire) {
            return;
        }
        let me = (self.cfg.identity)().to_json();
        let (m, c2) = (Arc::downgrade(self), Arc::clone(c));
        c.wc.session()
            .call(METHOD_HELLO, me.as_bytes(), HELLO_CALL, move |r| {
                let Some(m) = m.upgrade() else { return };
                m.hello_done(&c2, &t, attempt, r);
            });
    }

    fn hello_done(
        self: &Arc<Self>,
        c: &Arc<MConn>,
        t: &Arc<Target>,
        attempt: u32,
        r: Result<Vec<u8>, Error>,
    ) {
        let (refused, err) = match r {
            Ok(b) => {
                // The peer answered: a bad identity will not change by trying again at once.
                let e = match Identity::parse(&b) {
                    Err(e) => Err(format!("a bad hello: {e}")),
                    Ok(id) => self
                        .check_peer(&id)
                        .and_then(|()| {
                            if id.role == t.req.role {
                                Ok(())
                            } else {
                                Err(format!(
                                    "{} announced role {}, but it is configured as a {} peer",
                                    t.addr, id.role, t.req.role
                                ))
                            }
                        })
                        .and_then(|()| self.register(c, id)),
                };
                match e {
                    Ok(peer) => {
                        t.set_err(String::new());
                        return self.up(&peer);
                    }
                    Err(e) => (true, e),
                }
            }
            Err(e @ Error::Remote { .. }) => (true, e.to_string()),
            Err(e) => (false, format!("rpc {METHOD_HELLO}: {e}")),
        };
        if !c.live() {
            return;
        }
        t.set_err(err.clone());
        if refused {
            // Refused (a wrong role, linked to itself...): dropped, and dialed again later.
            let d = refused_backoff(lock(&self.limits).retry);
            akit_observe::warn!(
                Llm,
                "mesh: hello refused, dialing again later: addr={} retry_after={d:?} error={err}",
                t.addr
            );
            lock(&t.st).next_delay = d;
            c.wc.close();
            return;
        }
        if attempt % 10 == 0 {
            akit_observe::warn!(Llm, "mesh: hello failed: addr={} error={err}", t.addr);
        }
        let (m, c2) = (Arc::downgrade(self), Arc::clone(c));
        let _ = after_send(HELLO_AGAIN, move || {
            if let Some(m) = m.upgrade() {
                m.hello(&c2, attempt.saturating_add(1));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_jitters() {
        let ceiling = Duration::from_secs(5);
        for n in 0..40 {
            let d = backoff(n, ceiling);
            assert!(d >= BACKOFF_FLOOR && d <= ceiling, "{n}: {d:?}");
            if n < 2 {
                assert!(d <= BACKOFF_BASE.saturating_mul(1 << n));
            }
        }
        let r = refused_backoff(Duration::from_secs(5));
        assert!(r >= REFUSED_RETRY && r <= REFUSED_RETRY.saturating_mul(3) / 2);
        let r = refused_backoff(Duration::from_secs(60));
        assert!(r >= Duration::from_secs(60) && r <= Duration::from_secs(90));
    }
}
