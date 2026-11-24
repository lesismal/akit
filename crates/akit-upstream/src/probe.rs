//! Active health checks (I35), as fit's prober: a probe of each address of an upstream every
//! `interval`, plus a random `jitter`, on akio's clients. `fails` probes in a row that fail take
//! the address out of rotation ([`Health::set_probed_down`]), and `passes` in a row that pass put
//! it back; probes and the passive checks (`max_fails`) take it out each on their own account.
//!
//! - **http** sends `GET path` (with the check's `Host` when it gives one) on a connection of its
//!   own, through an akio-http [`Client`] of the prober's own that keeps no idle connection: a
//!   probe sees whether the address takes new connections, and never waits behind the pool's
//!   requests. Only the status counts: the body is given up as soon as the head has arrived. The
//!   request's timeout, which akio arms from the start, connecting included, is the check's.
//! - **http** over an h2 or h2c upstream (I18) does the same over HTTP/2, as fit probes over the
//!   upstream's protocol: a connection of its own ([`H2Client::connect`]), closed once the head
//!   has arrived; the check's timeout is kept by a timer of the prober's, as for tcp, since akio's
//!   h2 dial has the engine's connect timeout (akio API request A53).
//! - **tcp** passes once a connection opens ([`HappyEyeballs::connect`]), and closes it at once.
//!   akio dials a lone address with its engine's connect timeout (akio API request A12), so the
//!   check's timeout is kept by a timer of the prober's.
//!
//! No thread waits for a probe: each is started by a job of akit's probe timer ([`crate::timer`],
//! the stopgap for akio API request A45) and finished in akio's callback, on an I/O thread (or
//! the resolver's, or a timer's), which records the result and schedules the next probe: one
//! address's probes follow one another. The prober holds its pool weakly: once the pool is
//! closed (its graph retired) or dropped, no probe starts and the result of one under way is
//! dropped.

use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::dns::HappyEyeballs;
use akio::{Conn, Engine, Handler};
use akio_http::Headers;
use akio_http::client::{
    Client, ClientError, ClientOptions, ClientRequest, ResponseBody, ResponseHandler, ResponseHead,
};
use akio_http::h2client::{H2Client, H2Event};
use akit_config::{HealthCheck, HealthCheckType};

use crate::Inner;
use crate::health::Health;
use crate::timer;

/// The `User-Agent` of http probes.
pub const USER_AGENT: &str = "akit-health-check";

/// The largest response head an http probe reads, as fit's prober.
const MAX_HEAD_BYTES: usize = 64 << 10;

/// A probe's result, given once.
type Done = Box<dyn FnOnce(bool) + Send>;

/// One upstream's probes.
pub(crate) struct Prober {
    check: HealthCheck,
    pool: Weak<Inner>,
    engine: Arc<Engine>,
    /// For http probes; `None` for tcp.
    client: Option<Client>,
    targets: Box<[Target]>,
    /// The jitter's random numbers (xorshift64*).
    rng: AtomicU64,
}

/// One address's probes, and the run of results they have had.
struct Target {
    /// The address's index in the pool.
    backend: usize,
    /// Where probes connect: the address's host (without brackets) and port, or the check's.
    host: String,
    port: u16,
    /// For http probes.
    url: String,
    /// The probes in a row that went against the address's state: failures while it is in
    /// rotation, passes while it is out. Only its own probes, one after another, touch it.
    streak: AtomicUsize,
}

/// The host and port of an upstream address (`host:port`, `[v6]:port`), which akit-config has
/// checked.
fn split(address: &str) -> Option<(&str, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Some((host, port))
}

/// Starts the probes of `pool`'s addresses, dialing through `engine`, each first one within a
/// jitter of now. Does nothing for a pool without a health check.
pub(crate) fn start(pool: &Arc<Inner>, engine: &Arc<Engine>) {
    let Some(check) = pool.health_check.clone() else {
        return;
    };
    let targets: Box<[Target]> = pool
        .backends
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let (host, own) = split(&b.address)?;
            let port = check.port.unwrap_or(own);
            let shown = if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            };
            Some(Target {
                backend: i,
                host: host.to_owned(),
                port,
                url: format!("http://{shown}{}", check.path),
                streak: AtomicUsize::new(0),
            })
        })
        .collect();
    let client = (check.kind == HealthCheckType::Http).then(|| {
        let mut opts = ClientOptions::default();
        // A connection of its own for each probe.
        opts.max_idle_per_host = 0;
        opts.max_idle = 0;
        opts.max_head_bytes = MAX_HEAD_BYTES;
        opts.happy_eyeballs.connect_timeout = check.timeout;
        Client::new(Arc::clone(engine)).with_options(opts)
    });
    let seed = std::collections::hash_map::RandomState::new().hash_one(Instant::now()) | 1;
    let pr = Arc::new(Prober {
        check,
        pool: Arc::downgrade(pool),
        engine: Arc::clone(engine),
        client,
        targets,
        rng: AtomicU64::new(seed),
    });
    for i in 0..pr.targets.len() {
        let me = Arc::clone(&pr);
        timer::after(pr.spread(), move || me.probe(i));
    }
}

impl Prober {
    /// A random wait of up to the jitter.
    fn spread(&self) -> Duration {
        let jitter = u64::try_from(self.check.jitter.as_nanos()).unwrap_or(u64::MAX);
        if jitter == 0 {
            return Duration::ZERO;
        }
        let step = |mut x: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let prev = self
            .rng
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| Some(step(x)))
            .unwrap_or(1);
        Duration::from_nanos(
            step(prev)
                .wrapping_mul(0x2545_f491_4f6c_dd1d)
                .checked_rem(jitter)
                .unwrap_or(0),
        )
    }

    /// Whether the pool is still there and open.
    fn live(&self) -> Option<Arc<Inner>> {
        self.pool
            .upgrade()
            .filter(|p| !p.closing.load(Ordering::SeqCst))
    }

    /// Sends one probe to target `i`, and schedules the next once it is over.
    fn probe(self: Arc<Self>, i: usize) {
        if self.live().is_none() {
            return;
        }
        let Some(t) = self.targets.get(i) else { return };
        let began = Instant::now();
        let me = Arc::clone(&self);
        let done: Done = Box::new(move |passed| me.done(i, began, passed));
        let h2 = self
            .live()
            .and_then(|p| p.h2.as_ref().map(|t| t.tls.clone()));
        match (&self.client, h2) {
            (Some(_), Some(tls)) => self.probe_h2(t, tls, done),
            (Some(client), None) => Self::probe_http(&self, client, t, done),
            (None, _) => self.probe_tcp(t, done),
        }
    }

    /// Sends `GET` to `t` over h2 (with `tls`) or h2c, on a connection of its own, and passes on
    /// an expected status.
    fn probe_h2(
        &self,
        t: &Target,
        tls: Option<Result<akio::tls::ClientConfig, String>>,
        done: Done,
    ) {
        let once = Once::new(done);
        let late = once.clone();
        timer::after(self.check.timeout, move || late.settle(false));
        let tls = match tls {
            None => None,
            Some(Ok(cfg)) => Some(cfg),
            Some(Err(_)) => return once.settle(false),
        };
        // HTTP/2 has no Host field: the check's `host` is the request's `:authority`.
        let scheme = if tls.is_some() { "https" } else { "http" };
        let path = &self.check.path;
        let shown = t
            .url
            .strip_prefix("http://")
            .and_then(|u| u.strip_suffix(path.as_str()))
            .unwrap_or_default();
        let authority = self.check.host.as_deref().unwrap_or(shown);
        let url = format!("{scheme}://{authority}{path}");
        let req = ClientRequest::get(&url).header("User-Agent", USER_AGENT);
        let check = self.check.clone();
        let failed = once.clone();
        H2Client::connect(
            &self.engine,
            &t.host,
            t.port,
            tls.map(|c| (c, t.host.as_str())),
            akio_http::h2client::H2Options::default(),
            move |r| match r {
                Err(_) => failed.settle(false),
                Ok(c) => {
                    let conn = c.clone();
                    let up = c.open(req, move |e| match e {
                        H2Event::Head { status, .. } => {
                            once.settle(check.expects(status));
                            // The body is not read: its connection is closed.
                            conn.close();
                        }
                        H2Event::Error(_) | H2Event::End => once.settle(false),
                        _ => {}
                    });
                    up.finish();
                }
            },
        );
    }

    /// Records target `i`'s probe, begun at `began`, and schedules its next.
    fn done(self: Arc<Self>, i: usize, began: Instant, passed: bool) {
        let Some(pool) = self.live() else { return };
        let Some(t) = self.targets.get(i) else { return };
        if let Some(b) = pool.backends.get(t.backend) {
            self.record(t, &b.health, passed);
        }
        drop(pool);
        let next = self
            .check
            .interval
            .saturating_sub(began.elapsed())
            .saturating_add(self.spread());
        let me = Arc::clone(&self);
        timer::after(next, move || me.probe(i));
    }

    /// Counts a probe's result: `fails` failures in a row take the address out of rotation, and
    /// `passes` passes in a row put it back.
    fn record(&self, t: &Target, health: &Health, passed: bool) {
        let down = health.probed_down();
        if passed != down {
            // A result that agrees with the state ends any run against it.
            t.streak.store(0, Ordering::Relaxed);
            return;
        }
        let streak = t.streak.load(Ordering::Relaxed).saturating_add(1);
        let needed = if down {
            self.check.passes
        } else {
            self.check.fails
        };
        if streak >= needed {
            t.streak.store(0, Ordering::Relaxed);
            health.set_probed_down(!down);
        } else {
            t.streak.store(streak, Ordering::Relaxed);
        }
    }

    /// Passes once a connection to `t` opens, and closes it at once.
    fn probe_tcp(&self, t: &Target, done: Done) {
        let once = Once::new(done);
        let late = once.clone();
        // Refused only past the timers' bound: akio's connect timeout then ends the probe.
        timer::after(self.check.timeout, move || late.settle(false));
        let opened = once.clone();
        let failed = once;
        let mut dial = HappyEyeballs::default();
        dial.connect_timeout = self.check.timeout;
        dial.connect(
            &self.engine,
            &t.host,
            t.port,
            akio::fn_acceptor(move |c: &Conn<Discard>| {
                c.close();
                opened.settle(true);
                Discard
            }),
            move |_| failed.settle(false),
        );
    }

    /// Sends `GET` to `t`, and passes on an expected status.
    fn probe_http(self: &Arc<Self>, client: &Client, t: &Target, done: Done) {
        let mut req = ClientRequest::get(&t.url)
            .timeout(self.check.timeout)
            .header("User-Agent", USER_AGENT);
        if let Some(host) = &self.check.host {
            req = req.header("Host", host);
        }
        client.request_with(
            req,
            Status {
                prober: Arc::clone(self),
                done: Some(done),
            },
        );
    }
}

/// A probe's result, given once by whichever comes first: the connection, its failure, or the
/// timeout.
#[derive(Clone)]
struct Once(Arc<Mutex<Option<Done>>>);

impl Once {
    fn new(done: Done) -> Once {
        Once(Arc::new(Mutex::new(Some(done))))
    }

    fn settle(&self, passed: bool) {
        let done = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(done) = done {
            done(passed);
        }
    }
}

/// The handler of a tcp probe's connection, which is closed as soon as it opens.
struct Discard;

impl Handler for Discard {
    fn on_data(&mut self, _: &Conn<Self>, _: &[u8]) {}
}

/// An http probe's response: its status is all that counts.
struct Status {
    prober: Arc<Prober>,
    done: Option<Done>,
}

impl Status {
    fn settle(&mut self, passed: bool) {
        if let Some(done) = self.done.take() {
            done(passed);
        }
    }
}

impl ResponseHandler for Status {
    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        let passed = self.prober.check.expects(head.status);
        self.settle(passed);
        // The body is not read: its connection is closed.
        body.cancel();
    }

    fn on_data(&mut self, _: &[u8]) {}

    fn on_end(&mut self, _: &Headers) {}

    fn on_error(&mut self, _: ClientError) {
        self.settle(false);
    }
}
