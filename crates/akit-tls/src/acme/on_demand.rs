//! On-demand TLS (I34), as fit's `tlsmgr/ondemand.go`: a certificate obtained during the
//! handshake for a name no listener lists, when the `acme.on_demand.ask` URL allows it, at most
//! `rate` every `interval` and `max_names` in all, each dropped once no handshake has asked for it
//! for `idle_timeout`.
//!
//! Nothing here blocks the handshake's I/O thread (rules.md rule 1). A handshake asks
//! [`Acme::on_demand`] in its call: a name with a certificate is answered there and then; any
//! other leaves its [`Waiter`] (the handshake's akio-tls `CertDone`) with the name and returns.
//! The ask goes through akio-http's asynchronous client, on an engine of its own, whose callback
//! takes the name on; the ACME workers obtain its certificate as they do a listed name's, and the
//! waiters are answered when they settle it. A handshake waits at most `acme.on_demand.timeout`
//! (the listener's `cert_resolver_timeout`), and one that gives up leaves the certificate being
//! obtained for the next.
//!
//! The ClientHello is not authenticated, so every step is bounded (rule 11): at most
//! [`MAX_ASKS`] asks under way and one each [`ASK_EVERY`] on average, [`ASK_BURST`] at once,
//! whatever the answers; refusals remembered for [`REFUSED_FOR`], at most [`MAX_REFUSED`] of them;
//! at most [`MAX_WAITERS`] handshakes waiting for one name (and akio's
//! `max_pending_cert_resolutions` per listener). A handshake past a bound fails at once.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::{CertifiedKey, ClientConfig, RootCerts};
use akio_http::client::{Client, ClientOptions, ClientRequest, RedirectPolicy};

use super::{Acme, Inner, lock, normalize};

/// How long a name the ask URL refused is refused without asking again (fit's `refusedFor`).
pub const REFUSED_FOR: Duration = Duration::from_secs(60);
/// The most refused names remembered; past it they are all forgotten (fit's `maxRefused`).
pub const MAX_REFUSED: usize = 10_000;
/// The most asks under way at once (fit's `maxAsks`).
pub const MAX_ASKS: usize = 8;
/// The average time between two asks (fit's `askEvery`).
pub const ASK_EVERY: Duration = Duration::from_millis(200);
/// How many asks may be made at once after a quiet time (fit's `askBurst`).
pub const ASK_BURST: u32 = 20;
/// The most handshakes waiting for one name's certificate; past it a handshake fails at once.
pub const MAX_WAITERS: usize = 1024;
/// How often a handshake records that it used a name taken on: at most once a second, so that
/// handshakes for one name do not all write to it (fit's `usedEvery`).
const USED_EVERY: Duration = Duration::from_secs(1);
/// The largest answer read from the ask URL: only its status counts.
const ASK_MAX_BYTES: usize = 64 << 10;
/// The redirects an ask follows, as Go's client does.
const ASK_REDIRECTS: u32 = 10;

/// Why a handshake gets no certificate on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnDemandError {
    /// The ask URL refused the name (fit's `ErrOnDemandRefused`).
    Refused,
    /// `rate` certificates were obtained in the last `interval` (`ErrOnDemandRateLimited`).
    RateLimited,
    /// As many names as `max_names` have certificates (`ErrOnDemandTooMany`).
    TooMany,
    /// Too many names were asked about lately, or too many handshakes wait for this one
    /// (`ErrOnDemandBusy`).
    Busy,
    /// Not a host name a certificate can be obtained for.
    BadName,
    /// `acme.on_demand` is not configured.
    Off,
    /// The ask URL did not answer.
    Ask(String),
    /// Obtaining the certificate failed; the name is dropped.
    Failed(String),
    /// The manager was stopped.
    Stopped,
}

impl fmt::Display for OnDemandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("on-demand TLS: ")?;
        match self {
            OnDemandError::Refused => f.write_str("the ask URL refused the name"),
            OnDemandError::RateLimited => f.write_str("too many certificates obtained lately"),
            OnDemandError::TooMany => f.write_str("as many names as max_names have certificates"),
            OnDemandError::Busy => f.write_str("too many names asked about lately"),
            OnDemandError::BadName => {
                f.write_str("not a host name a certificate can be obtained for")
            }
            OnDemandError::Off => f.write_str("on-demand TLS is not configured"),
            OnDemandError::Ask(e) => write!(f, "asking: {e}"),
            OnDemandError::Failed(e) => f.write_str(e),
            OnDemandError::Stopped => f.write_str("stopped"),
        }
    }
}

impl std::error::Error for OnDemandError {}

/// What a handshake waiting for a certificate on demand is answered through: once, from any
/// thread, never blocking.
pub trait Waiter: Send {
    /// Answers the handshake.
    fn answer(self: Box<Self>, r: Result<CertifiedKey, OnDemandError>);

    /// Whether the handshake no longer waits (its connection closed, or it timed out): such a
    /// waiter is let go of without an answer.
    fn abandoned(&self) -> bool {
        false
    }
}

/// What [`Acme`] keeps to take names on during handshakes. Times are nanoseconds on its own
/// clock ([`OnDemand::now`]).
pub(super) struct OnDemand {
    config: akit_config::OnDemand,
    engine: Arc<Engine>,
    client: Client,
    base: Instant,
    /// Added to the clock, by tests.
    offset: AtomicU64,
    /// The clock stopped at this time, by tests; `u64::MAX` when it runs.
    fixed: AtomicU64,
    st: Mutex<OdState>,
}

#[derive(Default)]
struct OdState {
    /// When the last names were taken on, at most `rate` of them, oldest first.
    taken: VecDeque<u64>,
    /// The names taken on that the manager keeps.
    names: usize,
    /// The names the ask URL refused, until when.
    refused: HashMap<String, u64>,
    /// The handshakes waiting for each name being taken on.
    waits: HashMap<String, Vec<Box<dyn Waiter>>>,
    /// The asks under way, and when the rate of asks allows the next with none saved up.
    asking: usize,
    ask_at: u64,
    closed: bool,
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

impl OnDemand {
    /// The manager of `config`, its ask client trusting the system's CAs for an `https://` ask
    /// URL, read here, as the config is compiled, never on an I/O thread.
    pub(super) fn new(config: &akit_config::OnDemand) -> Result<OnDemand, crate::Error> {
        OnDemand::with_roots(config, RootCerts::system)
    }

    /// The manager of `config`, its ask client trusting the CAs `roots` gives for an `https://`
    /// ask URL. akio-tls's `ClientConfig::builder()` starts with no roots: built without them,
    /// every `https://` ask failed its handshake (I80, found by the security review's survey).
    fn with_roots(
        config: &akit_config::OnDemand,
        roots: impl FnOnce() -> Result<RootCerts, akio::tls::Error>,
    ) -> Result<OnDemand, crate::Error> {
        let fail = |e: String| crate::Error::new("acme.on_demand", e);
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| fail(format!("the ask client's engine: {e}")))?;
        engine
            .start()
            .map_err(|e| fail(format!("the ask client's engine: {e}")))?;
        let engine = Arc::new(engine);
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = ASK_MAX_BYTES;
        opts.max_idle_per_host = MAX_ASKS;
        opts.max_idle = MAX_ASKS;
        let mut client = Client::new(Arc::clone(&engine))
            .with_options(opts)
            .with_redirects(RedirectPolicy::limit(ASK_REDIRECTS))
            .with_timeout(config.timeout);
        if config.ask.starts_with("https://") {
            let roots = roots().map_err(|e| fail(format!("the ask client's CAs: {e}")))?;
            let tls = ClientConfig::builder()
                .roots(roots)
                .build()
                .map_err(|e| fail(format!("the ask client's TLS: {e}")))?;
            client = client.with_tls(tls);
        }
        Ok(OnDemand {
            config: config.clone(),
            engine,
            client,
            base: Instant::now(),
            offset: AtomicU64::new(0),
            fixed: AtomicU64::new(u64::MAX),
            st: Mutex::new(OdState::default()),
        })
    }

    /// The time on its clock.
    pub(super) fn now(&self) -> u64 {
        let fixed = self.fixed.load(Ordering::Relaxed);
        if fixed != u64::MAX {
            return fixed;
        }
        nanos(self.base.elapsed()).saturating_add(self.offset.load(Ordering::Relaxed))
    }

    /// `t` on its clock as an instant.
    fn instant(&self, t: u64) -> Instant {
        let now = self.now();
        let real = Instant::now();
        if t >= now {
            real.checked_add(Duration::from_nanos(t.saturating_sub(now)))
                .unwrap_or(real)
        } else {
            real.checked_sub(Duration::from_nanos(now.saturating_sub(t)))
                .unwrap_or(real)
        }
    }

    pub(super) fn timeout(&self) -> Duration {
        self.config.timeout
    }

    pub(super) fn idle(&self) -> Duration {
        self.config.idle_timeout
    }

    /// Whether another name may be taken on at `now`, counting it with `take`.
    fn room(
        st: &mut OdState,
        c: &akit_config::OnDemand,
        now: u64,
        take: bool,
    ) -> Result<(), OnDemandError> {
        if st.names >= c.max_names {
            return Err(OnDemandError::TooMany);
        }
        let full = st.taken.len() >= c.rate;
        if full
            && st
                .taken
                .front()
                .is_some_and(|t| now.saturating_sub(*t) < nanos(c.interval))
        {
            return Err(OnDemandError::RateLimited);
        }
        if take {
            if full {
                st.taken.pop_front();
            }
            st.taken.push_back(now);
            st.names = st.names.saturating_add(1);
        }
        Ok(())
    }

    /// Takes a place for an ask at `now`, within [`MAX_ASKS`] and the rate of asks.
    fn start_ask(st: &mut OdState, now: u64) -> Result<(), OnDemandError> {
        let next = st.ask_at.max(now);
        let burst = nanos(ASK_EVERY).saturating_mul(u64::from(ASK_BURST));
        if st.asking >= MAX_ASKS || next.saturating_sub(now) >= burst {
            return Err(OnDemandError::Busy);
        }
        st.asking = st.asking.saturating_add(1);
        st.ask_at = next.saturating_add(nanos(ASK_EVERY));
        Ok(())
    }

    /// Answers the handshakes waiting for `name`.
    pub(super) fn finish(&self, name: &str, r: &Result<CertifiedKey, OnDemandError>) {
        let waiters = lock(&self.st).waits.remove(name);
        for w in waiters.into_iter().flatten() {
            if !w.abandoned() {
                w.answer(r.clone());
            }
        }
    }

    /// Counts a name taken on as no longer kept on demand.
    pub(super) fn release(&self) {
        let mut st = lock(&self.st);
        st.names = st.names.saturating_sub(1);
    }

    /// Records that a handshake asked for a name taken on, last recorded at `used`.
    pub(super) fn touch(&self, used: &AtomicU64) {
        let now = self.now();
        if now.saturating_sub(used.load(Ordering::Relaxed)) >= nanos(USED_EVERY) {
            used.store(now, Ordering::Relaxed);
        }
    }

    /// When a name last used at `used` is dropped if no handshake asks for it before.
    pub(super) fn idle_until(&self, used: &AtomicU64) -> u64 {
        used.load(Ordering::Relaxed)
            .saturating_add(nanos(self.config.idle_timeout))
    }

    /// `t` on its clock as an instant, for a worker's wait.
    pub(super) fn at(&self, t: u64) -> Instant {
        self.instant(t)
    }

    /// Stops it: the asks under way fail, and every waiting handshake is answered.
    pub(super) fn close(&self) {
        let waits = {
            let mut st = lock(&self.st);
            st.closed = true;
            std::mem::take(&mut st.waits)
        };
        for w in waits.into_values().flatten() {
            w.answer(Err(OnDemandError::Stopped));
        }
        self.engine.shutdown();
        self.engine.wait();
    }

    #[cfg(test)]
    pub(super) fn advance(&self, d: Duration) {
        let fixed = self.fixed.load(Ordering::Relaxed);
        if fixed == u64::MAX {
            self.offset.fetch_add(nanos(d), Ordering::Relaxed);
        } else {
            self.fixed
                .store(fixed.saturating_add(nanos(d)), Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(super) fn freeze(&self) {
        self.fixed.store(self.now(), Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn counted(&self) -> usize {
        lock(&self.st).names
    }

    #[cfg(test)]
    pub(super) fn refused_len(&self) -> usize {
        lock(&self.st).refused.len()
    }
}

/// Whether `name` (normalized) is one a certificate may be obtained for on demand: a host name of
/// letters, digits and hyphens, in two labels or more, not an IP address and not a wildcard
/// (fit's `onDemandName`).
pub(crate) fn on_demand_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 || !name.contains('.') {
        return false;
    }
    if name.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    })
}

impl Acme {
    /// Whether `acme.on_demand` is configured: listeners whose `tls.on_demand` is set take names
    /// on during handshakes then.
    #[must_use]
    pub fn on_demand_enabled(&self) -> bool {
        self.inner.on_demand.is_some()
    }

    /// How long a handshake waits for a certificate obtained on demand (`acme.on_demand.timeout`),
    /// when it is configured.
    #[must_use]
    pub fn on_demand_timeout(&self) -> Option<Duration> {
        self.inner.on_demand.as_ref().map(|od| od.timeout())
    }

    /// The certificate of `name` for a handshake, obtained now if the manager has none (fit's
    /// `ACME.OnDemand`): if the ask URL allows the name and neither the rate nor `max_names` is
    /// reached. Called on the handshake's I/O thread, it never blocks: `w` is answered in the
    /// call when the certificate is at hand or the name is refused at once, and later, from
    /// another thread, otherwise. Handshakes for one name wait for one certificate.
    pub fn on_demand(&self, name: &str, w: Box<dyn Waiter>) {
        let Some(od) = self.inner.on_demand.as_ref() else {
            return w.answer(Err(OnDemandError::Off));
        };
        let name = normalize(name);
        if !on_demand_name(&name) {
            return w.answer(Err(OnDemandError::BadName));
        }
        let served = self.inner.certs.with(|m| {
            m.get(&name).map(|s| {
                if let Some(u) = &s.used {
                    od.touch(u);
                }
                s.certificate()
            })
        });
        if let Some(k) = served {
            return w.answer(Ok(k));
        }
        let now = od.now();
        let mut st = lock(&od.st);
        if st.closed {
            drop(st);
            return w.answer(Err(OnDemandError::Stopped));
        }
        if let Some(ws) = st.waits.get_mut(&name) {
            ws.retain(|w| !w.abandoned());
            if ws.len() >= MAX_WAITERS {
                drop(st);
                return w.answer(Err(OnDemandError::Busy));
            }
            ws.push(w);
            return;
        }
        if let Some(until) = st.refused.get(&name).copied() {
            if now < until {
                drop(st);
                return w.answer(Err(OnDemandError::Refused));
            }
            st.refused.remove(&name);
        }
        let ready = OnDemand::room(&mut st, &od.config, now, false)
            .and_then(|()| OnDemand::start_ask(&mut st, now));
        if let Err(e) = ready {
            drop(st);
            return w.answer(Err(e));
        }
        st.waits.insert(name.clone(), vec![w]);
        drop(st);
        self.ask(od, name);
    }

    /// Asks the ask URL about `name`, its answer handled on the ask client's I/O thread.
    fn ask(&self, od: &OnDemand, name: String) {
        let sep = if od.config.ask.contains('?') {
            '&'
        } else {
            '?'
        };
        // The name is letters, digits, dots and hyphens: nothing to escape.
        let url = format!("{}{sep}domain={name}", od.config.ask);
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let req = ClientRequest::get(&url)
            .timeout(od.config.timeout)
            .header("user-agent", "akit");
        od.client.request(req, move |r| {
            let Some(inner) = weak.upgrade() else { return };
            let a = Acme { inner };
            let r = match r {
                Ok(resp) if (200..300).contains(&resp.status) => Ok(()),
                Ok(resp) => Err((OnDemandError::Refused, Some(resp.status))),
                Err(e) => Err((OnDemandError::Ask(e.to_string()), None)),
            };
            a.asked(&name, r);
        });
    }

    /// What came of asking about `name`: takes it on if it was allowed and there is room.
    fn asked(&self, name: &str, r: Result<(), (OnDemandError, Option<u16>)>) {
        let Some(od) = self.inner.on_demand.as_ref() else {
            return;
        };
        let now = od.now();
        let r = {
            let mut st = lock(&od.st);
            st.asking = st.asking.saturating_sub(1);
            match r {
                Err((e, status)) => {
                    if e == OnDemandError::Refused {
                        if st.refused.len() >= MAX_REFUSED {
                            st.refused.clear();
                        }
                        st.refused
                            .insert(name.to_owned(), now.saturating_add(nanos(REFUSED_FOR)));
                    }
                    Err((e, status))
                }
                Ok(()) => OnDemand::room(&mut st, &od.config, now, true).map_err(|e| (e, None)),
            }
        };
        if let Err((e, status)) = r {
            let status = status.map(|s| format!(" ({s})")).unwrap_or_default();
            self.log(&format!("{name}: on demand: {e}{status}"));
            return od.finish(name, &Err(e));
        }
        // Taken on: the workers obtain it, and settling it answers the handshakes.
        let answer_now = {
            let mut st = lock(&self.inner.state);
            if st.closed {
                Some(Err(OnDemandError::Stopped))
            } else if let Some(m) = st.names.get(name) {
                // Listed meanwhile: the config's, not counted on demand, and worked on as it is.
                od.release();
                if m.running {
                    None
                } else {
                    Some(self.certificate(name).ok_or_else(|| {
                        OnDemandError::Failed(
                            m.last_error
                                .clone()
                                .unwrap_or_else(|| "no certificate yet".into()),
                        )
                    }))
                }
            } else {
                st.names.insert(
                    name.to_owned(),
                    super::Managed::new(Some(Arc::new(AtomicU64::new(now)))),
                );
                None
            }
        };
        match answer_now {
            Some(r) => {
                if matches!(r, Err(OnDemandError::Stopped)) {
                    od.release();
                }
                od.finish(name, &r);
            }
            None => self.inner.wake.notify_all(),
        }
    }

    /// Drops the names taken on demand that no handshake has asked for in `idle_timeout`, and
    /// returns when the next of the others will be idle that long (fit's `dropIdle`). A name
    /// being worked on waits for its run to end. Its certificate stays in the storage, for the
    /// next handshake that asks for it (the ask URL asked again). The state's lock is held.
    pub(super) fn drop_idle(&self, st: &mut super::State) -> Option<Instant> {
        let od = self.inner.on_demand.as_ref()?;
        let now = od.now();
        let mut next: Option<u64> = None;
        let mut dropped = Vec::new();
        for (n, m) in &st.names {
            let Some(used) = m.on_demand.as_ref().filter(|_| !m.running) else {
                continue;
            };
            let until = od.idle_until(used);
            if until > now {
                next = Some(next.map_or(until, |x| x.min(until)));
            } else {
                dropped.push(n.clone());
            }
        }
        if !dropped.is_empty() {
            for n in &dropped {
                st.names.remove(n);
                od.release();
                self.log(&format!(
                    "{n}: on demand: no handshake for {}s; dropped",
                    od.idle().as_secs()
                ));
            }
            self.prune_certs(st);
        }
        next.map(|t| od.at(t))
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]

    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::mpsc;

    use akio_x509::CertificateParams;

    use super::*;
    use crate::FileStorage;

    /// A handshake, as a test sees it.
    struct Chan(mpsc::Sender<Result<CertifiedKey, OnDemandError>>);

    impl Waiter for Chan {
        fn answer(self: Box<Self>, r: Result<CertifiedKey, OnDemandError>) {
            let _ = self.0.send(r);
        }
    }

    /// Asks for `name` as a handshake does; its answer within `wait`, or `None`, as a handshake
    /// past its `cert_resolver_timeout` gives up.
    fn ask_for(
        a: &Acme,
        name: &str,
        wait: Duration,
    ) -> Option<Result<CertifiedKey, OnDemandError>> {
        let (tx, rx) = mpsc::channel();
        a.on_demand(name, Box::new(Chan(tx)));
        rx.recv_timeout(wait).ok()
    }

    fn answer(a: &Acme, name: &str) -> Result<CertifiedKey, OnDemandError> {
        ask_for(a, name, Duration::from_secs(20)).unwrap_or_else(|| panic!("{name}: no answer"))
    }

    /// An ask URL of a few lines of HTTP/1: allows the names it is told, counts what it is
    /// asked, and, while `hold` is set, holds every ask until it is cleared.
    struct Asker {
        addr: SocketAddr,
        allowed: Arc<Mutex<Vec<String>>>,
        asked: Arc<Mutex<HashMap<String, usize>>>,
        in_flight: Arc<AtomicUsize>,
        hold: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    }

    impl Asker {
        fn start(allowed: &[&str]) -> Asker {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let a = Asker {
                addr: l.local_addr().unwrap(),
                allowed: Arc::new(Mutex::new(
                    allowed.iter().map(|s| (*s).to_owned()).collect(),
                )),
                asked: Arc::default(),
                in_flight: Arc::default(),
                hold: Arc::default(),
                stop: Arc::default(),
            };
            let (allowed, asked, in_flight, hold, stop) = (
                Arc::clone(&a.allowed),
                Arc::clone(&a.asked),
                Arc::clone(&a.in_flight),
                Arc::clone(&a.hold),
                Arc::clone(&a.stop),
            );
            std::thread::spawn(move || {
                for c in l.incoming() {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    let Ok(c) = c else { continue };
                    let (allowed, asked, in_flight, hold) = (
                        Arc::clone(&allowed),
                        Arc::clone(&asked),
                        Arc::clone(&in_flight),
                        Arc::clone(&hold),
                    );
                    std::thread::spawn(move || serve(c, &allowed, &asked, &in_flight, &hold));
                }
            });
            a
        }

        fn url(&self) -> String {
            format!("http://{}/check?key=1", self.addr)
        }

        fn count(&self, name: &str) -> usize {
            lock(&self.asked).get(name).copied().unwrap_or(0)
        }

        fn total(&self) -> usize {
            lock(&self.asked).values().sum()
        }
    }

    impl Drop for Asker {
        fn drop(&mut self) {
            self.hold.store(false, Ordering::Release);
            self.stop.store(true, Ordering::Release);
            let _ = TcpStream::connect(self.addr);
        }
    }

    fn serve(
        c: TcpStream,
        allowed: &Mutex<Vec<String>>,
        asked: &Mutex<HashMap<String, usize>>,
        in_flight: &AtomicUsize,
        hold: &AtomicBool,
    ) {
        let mut r = BufReader::new(c);
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let target = line.split(' ').nth(1).unwrap_or("").to_owned();
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 {
                    return;
                }
                if h == "\r\n" {
                    break;
                }
            }
            assert!(target.starts_with("/check?key=1&domain="), "{target}");
            let name = target.rsplit("domain=").next().unwrap_or("").to_owned();
            *lock(asked).entry(name.clone()).or_default() += 1;
            in_flight.fetch_add(1, Ordering::AcqRel);
            while hold.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            in_flight.fetch_sub(1, Ordering::AcqRel);
            let ok = lock(allowed).contains(&name);
            let status = if ok { "200 OK" } else { "403 Forbidden" };
            let mut c = r.get_ref();
            if write!(c, "HTTP/1.1 {status}\r\nContent-Length: 2\r\n\r\nno").is_err() {
                return;
            }
        }
    }

    /// A CA that never answers: it takes connections and holds them, so that names taken on stay
    /// pending.
    struct HangingCa {
        addr: SocketAddr,
        stop: Arc<AtomicBool>,
    }

    impl HangingCa {
        fn start() -> HangingCa {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = l.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let s = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut held = Vec::new();
                for c in l.incoming() {
                    if s.load(Ordering::Acquire) {
                        return;
                    }
                    held.extend(c.ok());
                }
            });
            HangingCa { addr, stop }
        }

        fn directory(&self) -> String {
            format!("https://{}/dir", self.addr)
        }
    }

    impl Drop for HangingCa {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            let _ = TcpStream::connect(self.addr);
        }
    }

    fn storage(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("akit-on-demand-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// A manager with `acme.on_demand` `od`, the CA at `directory`, its log lines kept.
    fn manager(
        directory: &str,
        od: akit_config::OnDemand,
        root: &std::path::Path,
    ) -> (Acme, Arc<Mutex<Vec<String>>>) {
        let a = Acme::with_storage(
            &akit_config::Acme {
                directory: directory.to_owned(),
                timeout: Duration::from_secs(30),
                request_timeout: Duration::from_secs(30),
                challenges: vec![akit_config::Challenge::Http01],
                on_demand: Some(od),
                ..akit_config::Acme::default()
            },
            Arc::new(FileStorage::new(root)),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let l = Arc::clone(&lines);
        a.set_log(Arc::new(move |line: &str| {
            eprintln!("acme: {line}");
            lock(&l).push(line.to_owned());
        }));
        (a, lines)
    }

    fn count(lines: &Mutex<Vec<String>>, part: &str) -> usize {
        lock(lines).iter().filter(|l| l.contains(part)).count()
    }

    fn eventually(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < end, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn od(a: &Acme) -> &OnDemand {
        a.inner.on_demand.as_ref().unwrap()
    }

    /// Stores a certificate for `name`, valid for long, so that taking it on needs no CA (fit's
    /// `storeIssued`).
    fn store_issued(a: &Acme, name: &str) {
        let key =
            akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
        let cert = CertificateParams::new([name]).self_signed(&key).unwrap();
        let pem = format!(
            "{}{}",
            cert.to_pem(),
            akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der())
        );
        a.inner
            .storage
            .store(
                &a.cert_key(name),
                pem.as_bytes(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
    }

    /// fit's `TestOnDemandRefused`: a refused name is refused from memory for a minute, then
    /// asked about again, and never taken on; an ask URL that does not answer refuses, but is
    /// asked again next time.
    #[test]
    fn refused() {
        let ca = HangingCa::start();
        let ask = Asker::start(&[]);
        let root = storage("refused");
        let (a, _) = manager(
            &ca.directory(),
            akit_config::OnDemand::new(ask.url()),
            &root,
        );
        for _ in 0..3 {
            assert_eq!(
                answer(&a, "no.akit.test").err(),
                Some(OnDemandError::Refused)
            );
        }
        assert_eq!(ask.count("no.akit.test"), 1, "asked again within a minute");
        od(&a).advance(REFUSED_FOR + Duration::from_secs(1));
        assert_eq!(
            answer(&a, "no.akit.test").err(),
            Some(OnDemandError::Refused)
        );
        assert_eq!(ask.count("no.akit.test"), 2);
        assert!(a.names().is_empty(), "a refused name was taken on");
        a.close();

        // An ask URL that does not answer.
        let down = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", down.local_addr().unwrap());
        drop(down);
        let (b, _) = manager(&ca.directory(), akit_config::OnDemand::new(url), &root);
        for _ in 0..2 {
            let e = answer(&b, "x.akit.test").err();
            assert!(matches!(e, Some(OnDemandError::Ask(_))), "{e:?}");
        }
        assert_eq!(
            od(&b).refused_len(),
            0,
            "remembered as refused without an answer"
        );
        b.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandRateLimited`: one name taken on within the hour; it stays pending, the
    /// handshake giving up; a second name within the hour is refused without asking, and an
    /// hour later is taken on.
    #[test]
    fn rate_limited() {
        let ca = HangingCa::start();
        let ask = Asker::start(&["a.akit.test", "b.akit.test"]);
        let root = storage("rate");
        let mut c = akit_config::OnDemand::new(ask.url());
        c.rate = 1;
        c.interval = Duration::from_secs(3600);
        let (a, _) = manager(&ca.directory(), c, &root);
        let t = Instant::now();
        assert!(ask_for(&a, "a.akit.test", Duration::from_millis(300)).is_none());
        assert!(t.elapsed() < Duration::from_secs(2));
        assert_eq!(
            answer(&a, "b.akit.test").err(),
            Some(OnDemandError::RateLimited)
        );
        assert_eq!(
            ask.count("b.akit.test"),
            0,
            "a name past the rate was asked about"
        );
        od(&a).advance(Duration::from_secs(3601));
        assert!(ask_for(&a, "b.akit.test", Duration::from_millis(300)).is_none());
        eventually("b taken on", || a.names().len() == 2);
        a.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandMaxNames`.
    #[test]
    fn max_names() {
        let ca = HangingCa::start();
        let ask = Asker::start(&["a.akit.test", "b.akit.test"]);
        let root = storage("max");
        let mut c = akit_config::OnDemand::new(ask.url());
        c.max_names = 1;
        let (a, _) = manager(&ca.directory(), c, &root);
        assert!(ask_for(&a, "a.akit.test", Duration::from_millis(200)).is_none());
        assert_eq!(
            answer(&a, "b.akit.test").err(),
            Some(OnDemandError::TooMany)
        );
        // Closing answers the handshake still waiting.
        let (tx, rx) = mpsc::channel();
        a.on_demand("a.akit.test", Box::new(Chan(tx)));
        a.close();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().err(),
            Some(OnDemandError::Stopped)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandDropped`: a CA that cuts every connection; the name taken on is
    /// dropped once its first certificate fails, the handshakes waiting for it hear why, and it
    /// no longer counts against max_names.
    #[test]
    fn dropped_when_its_first_certificate_fails() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for c in l.incoming() {
                drop(c);
            }
        });
        let ask = Asker::start(&["a.akit.test"]);
        let root = storage("dropped");
        let mut c = akit_config::OnDemand::new(ask.url());
        c.max_names = 1;
        let (a, lines) = manager(&format!("https://{addr}/dir"), c, &root);
        let (tx, rx) = mpsc::channel();
        a.on_demand("a.akit.test", Box::new(Chan(tx.clone())));
        a.on_demand("a.akit.test", Box::new(Chan(tx)));
        for _ in 0..2 {
            let e = rx.recv_timeout(Duration::from_secs(20)).unwrap().err();
            assert!(matches!(e, Some(OnDemandError::Failed(_))), "{e:?}");
        }
        assert_eq!(ask.count("a.akit.test"), 1, "one ask for two handshakes");
        assert!(a.names().is_empty());
        assert_eq!(od(&a).counted(), 0);
        assert_eq!(count(&lines, "dropped"), 1);
        a.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandNames`: names no certificate can be obtained for are refused without
    /// asking; without acme.on_demand nothing is obtained.
    #[test]
    fn names() {
        let ca = HangingCa::start();
        let ask = Asker::start(&[]);
        let root = storage("names");
        let (a, _) = manager(
            &ca.directory(),
            akit_config::OnDemand::new(ask.url()),
            &root,
        );
        let long = format!("{}.test", "a".repeat(64));
        for name in [
            "localhost",
            "127.0.0.1",
            "a..akit.test",
            "*.akit.test",
            "-a.akit.test",
            "a_b.akit.test",
            long.as_str(),
            "",
        ] {
            assert_eq!(
                answer(&a, name).err(),
                Some(OnDemandError::BadName),
                "{name}"
            );
        }
        assert_eq!(ask.total(), 0, "bad names were asked about");
        a.close();

        let b = Acme::with_storage(
            &akit_config::Acme {
                directory: ca.directory(),
                ..akit_config::Acme::default()
            },
            Arc::new(FileStorage::new(&root)),
        )
        .unwrap();
        assert!(!b.on_demand_enabled());
        assert_eq!(answer(&b, "x.akit.test").err(), Some(OnDemandError::Off));
        b.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandIdle`: two names taken on from the storage, with no CA; one asked for
    /// again before idle_timeout; the other dropped once idle that long, its certificate no
    /// longer served nor counted; asked for again, it is taken on again from the storage, with
    /// the ask URL's yes.
    #[test]
    fn idle_names_are_dropped() {
        let ask = Asker::start(&["idle.akit.test", "busy.akit.test", "third.akit.test"]);
        let root = storage("idle");
        let mut c = akit_config::OnDemand::new(ask.url());
        c.max_names = 2;
        c.idle_timeout = Duration::from_secs(3600);
        let (a, lines) = manager("https://127.0.0.1:1/dir", c, &root);
        for name in ["idle.akit.test", "busy.akit.test"] {
            store_issued(&a, name);
            assert!(answer(&a, name).is_ok(), "{name}");
        }
        assert_eq!(
            answer(&a, "third.akit.test").err(),
            Some(OnDemandError::TooMany)
        );
        let at = |mins: u64| {
            od(&a).advance(Duration::from_secs(mins * 60));
            a.inner.wake.notify_all();
        };
        at(40);
        assert!(answer(&a, "busy.akit.test").is_ok());
        at(19);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            count(&lines, "dropped"),
            0,
            "dropped before its idle_timeout"
        );
        assert!(a.certificate("idle.akit.test").is_some());
        at(2);
        eventually("idle dropped", || {
            count(&lines, "idle.akit.test: on demand: no handshake") == 1
        });
        assert!(a.certificate("idle.akit.test").is_none());
        assert!(a.certificate("busy.akit.test").is_some());
        assert_eq!(od(&a).counted(), 1);

        // Taken on again from the storage, asking again.
        assert!(answer(&a, "idle.akit.test").is_ok());
        assert_eq!(ask.count("idle.akit.test"), 2);
        // busy.akit.test was last asked for at 40 minutes.
        at(40);
        eventually("busy dropped", || {
            count(&lines, "busy.akit.test: on demand: no handshake") == 1
        });
        assert_eq!(od(&a).counted(), 1);
        a.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandIdleTimer`: the workers wake for an idle name by themselves.
    #[test]
    fn idle_timer() {
        let ask = Asker::start(&["idle.akit.test"]);
        let root = storage("idle-timer");
        let mut c = akit_config::OnDemand::new(ask.url());
        c.idle_timeout = Duration::from_millis(100);
        let (a, _) = manager("https://127.0.0.1:1/dir", c, &root);
        store_issued(&a, "idle.akit.test");
        assert!(answer(&a, "idle.akit.test").is_ok());
        eventually("dropped", || a.certificate("idle.akit.test").is_none());
        a.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A name taken on demand and then listed by the config is the config's: no longer counted,
    /// and kept by later reloads that list it; a reload listing nothing keeps names taken on.
    #[test]
    fn listed_names_are_the_configs() {
        let ask = Asker::start(&["a.akit.test"]);
        let root = storage("listed");
        let (a, _) = manager(
            "https://127.0.0.1:1/dir",
            akit_config::OnDemand::new(ask.url()),
            &root,
        );
        store_issued(&a, "a.akit.test");
        assert!(answer(&a, "a.akit.test").is_ok());
        a.manage::<&str>(&[]);
        assert_eq!(a.names(), ["a.akit.test"]);
        assert_eq!(od(&a).counted(), 1);
        a.manage(&["a.akit.test"]);
        assert_eq!(od(&a).counted(), 0);
        a.manage::<&str>(&[]);
        assert!(a.names().is_empty());
        a.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// fit's `TestOnDemandAskBounds`: handshakes for random names, as an attacker sends them, to
    /// an ask URL that refuses them all: at most MAX_ASKS asked at once, the one past them
    /// refused at once without asking; no more than ASK_BURST asked at once, then one each
    /// ASK_EVERY.
    #[test]
    fn ask_bounds() {
        let ca = HangingCa::start();
        let ask = Asker::start(&[]);
        ask.hold.store(true, Ordering::Release);
        let root = storage("bounds");
        let (a, _) = manager(
            &ca.directory(),
            akit_config::OnDemand::new(ask.url()),
            &root,
        );
        let (tx, rx) = mpsc::channel();
        for i in 0..MAX_ASKS {
            a.on_demand(&format!("held{i}.akit.test"), Box::new(Chan(tx.clone())));
        }
        eventually("every ask under way", || {
            ask.in_flight.load(Ordering::Acquire) == MAX_ASKS
        });
        let t = Instant::now();
        assert_eq!(
            answer(&a, "past.akit.test").err(),
            Some(OnDemandError::Busy)
        );
        assert!(t.elapsed() < Duration::from_secs(1));
        ask.hold.store(false, Ordering::Release);
        for _ in 0..MAX_ASKS {
            let r = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(r.err(), Some(OnDemandError::Refused));
        }
        assert_eq!(ask.total(), MAX_ASKS);
        a.close();

        // The rate, with the clock stopped.
        let ask = Asker::start(&[]);
        let (b, _) = manager(
            &ca.directory(),
            akit_config::OnDemand::new(ask.url()),
            &root,
        );
        od(&b).freeze();
        let mut busy = 0;
        for i in 0..200 {
            match answer(&b, &format!("r{i}.akit.test")).err() {
                Some(OnDemandError::Busy) => busy += 1,
                Some(OnDemandError::Refused) => {}
                e => panic!("a random name: {e:?}"),
            }
        }
        let burst = usize::try_from(ASK_BURST).unwrap();
        assert_eq!((ask.total(), busy), (burst, 200 - burst));
        // One more ask once ASK_EVERY has passed, and only one.
        od(&b).advance(ASK_EVERY);
        assert_eq!(
            answer(&b, "again.akit.test").err(),
            Some(OnDemandError::Refused)
        );
        assert_eq!(
            answer(&b, "again2.akit.test").err(),
            Some(OnDemandError::Busy)
        );
        b.close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn on_demand_names_as_fit() {
        for (n, ok) in [
            ("a.akit.test", true),
            ("x-1.b2.test", true),
            ("akit", false),
            ("::1", false),
            ("10.0.0.1", false),
            ("a-.test", false),
            ("A.test", false),
        ] {
            assert_eq!(on_demand_name(n), ok, "{n}");
        }
    }

    /// An ask URL over HTTPS (I80): the ask client trusts the CAs it is given, so an `https://`
    /// ask is answered, and one whose server those CAs did not sign fails its handshake, as
    /// with no roots at all (what every `https://` ask did before).
    #[test]
    fn https_ask() {
        use akio::tls::{Certificate, PrivateKey, ServerConfig, Tls as AkioTls};
        use akio_crypto::pkey::PrivateKey as Key;
        use akio_http::client::ClientResponse;

        let ca_key = Key::P256(akio_crypto::p256::SecretKey::generate().unwrap());
        let ca = CertificateParams::ca("akit ask test CA")
            .self_signed(&ca_key)
            .unwrap();
        let key = Key::P256(akio_crypto::p256::SecretKey::generate().unwrap());
        let leaf = CertificateParams::new(["127.0.0.1"])
            .signed_by(&key.public_key(), &ca, &ca_key)
            .unwrap();
        let chain = Certificate::from_pem(leaf.to_pem().as_bytes()).unwrap();
        let key_pem = akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der());
        let cfg = ServerConfig::builder(chain, PrivateKey::from_pem(key_pem.as_bytes()).unwrap())
            .build()
            .unwrap();
        let app = akio_http::Router::new().get("/check", |ctx: &mut akio_http::Context| {
            ctx.text(200, "yes");
        });
        let backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
            Ok("uring") => akio::Backend::IoUring(akio::UringConfig::default()),
            _ => akio::Backend::Epoll,
        };
        let server = Engine::builder()
            .listen(
                akio::Listener::tcp("127.0.0.1:0".parse().unwrap())
                    .backend(backend)
                    .threads(1)
                    .serve(AkioTls::server(cfg, akio_http::serve(app))),
            )
            .build()
            .unwrap();
        server.start().unwrap();
        let url = format!("https://{}/check", server.local_addrs()[0]);
        let ask = |od: &OnDemand| {
            let (tx, rx) = mpsc::channel();
            let req = ClientRequest::get(&format!("{url}?domain=a.akit.test"));
            od.client.request(req, move |r: Result<ClientResponse, _>| {
                let _ = tx.send(r.map(|resp| resp.status).map_err(|e| e.to_string()));
            });
            rx.recv_timeout(Duration::from_secs(10)).unwrap()
        };

        let mut roots = RootCerts::empty();
        assert_eq!(roots.add_pem(ca.to_pem().as_bytes()).unwrap(), 1);
        let config = akit_config::OnDemand::new(url.clone());
        let trusted = OnDemand::with_roots(&config, move || Ok(roots)).unwrap();
        assert_eq!(ask(&trusted), Ok(200));
        trusted.close();

        let none = OnDemand::with_roots(&config, || Ok(RootCerts::empty())).unwrap();
        assert!(
            ask(&none).is_err(),
            "an ask past an unknown CA was answered"
        );
        none.close();

        // An `http://` ask URL reads no CAs.
        let plain = akit_config::OnDemand::new("http://127.0.0.1:1/check".to_owned());
        let od = OnDemand::with_roots(&plain, || panic!("CAs read for http://")).unwrap();
        od.close();
        server.shutdown();
        server.wait();
    }
}
