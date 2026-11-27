//! How the manager reaches a member over HTTP, fit's `cluster/transport.go`: akio's asynchronous
//! HTTP client (rule 1), one a member, whose TLS checks the member's certificate as its `tls`
//! says (a pin, its CAs, or the system's), every call within `timeout` and its answer within
//! `max_response_bytes`; and what went wrong, told apart as fit's problem kinds.
//!
//! The certificate a member presented is read by a probe of its own (akio request A277's
//! stopgap): akio's client neither gives a response's peer chain nor lets a certificate it
//! refused be seen, as fit's `VerifyConnection` does. The probe opens a TLS connection that
//! verifies nothing, sends a request with no credentials, reads the chain once the handshake is
//! done, closes, and checks the chain as the member's `tls` says (the pin's SHA-256, or
//! akio-x509's verifier with the CAs and the URL's host). It runs for a test, after a call that
//! failed on an `https` member, and at a member's first poll; never with a credential.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use akio::tls::{ClientConfig, RootCerts};
use akio_crypto::hash::Sha256;
use akio_http::client::{CancelToken, Client, ClientError, ClientOptions, ClientRequest};

use crate::json::{self, Obj};
use crate::member::{Member, format_fingerprint, host_of};

/// How long a connect may take at most, below a call's own timeout: fit's `maxDialTimeout`.
pub const MAX_DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a connection is kept between calls: fit's `idleConnTimeout`.
pub const IDLE_CONN_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest answer's head: fit's `maxHeaderBytes`.
pub const MAX_HEADER_BYTES: usize = 64 << 10;
/// What a probe sends once its handshake is done: a request without credentials, which a
/// member answers with an error, to have it send something back.
const PROBE_REQUEST: &[u8] = b"GET /node HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n";

/// The kinds of a [`Problem`], fit's: what went wrong reaching a member, each told apart so that a
/// person knows what to fix.
pub const KIND_UNREACHABLE: &str = "unreachable";
/// The member did not answer in time.
pub const KIND_TIMEOUT: &str = "timeout";
/// The member's certificate is not trusted by the system's CAs or the member's `ca`.
pub const KIND_TLS_UNTRUSTED: &str = "tls_untrusted";
/// The member's certificate is not the one pinned.
pub const KIND_TLS_PIN: &str = "tls_pin_mismatch";
/// The TLS handshake failed otherwise.
pub const KIND_TLS: &str = "tls";
/// The user name and password, or the token, were refused.
pub const KIND_AUTH: &str = "auth_failed";
/// The member refuses logins for a while.
pub const KIND_RATE_LIMITED: &str = "rate_limited";
/// The member's console asks for its temporary password to be changed.
pub const KIND_PASSWORD_CHANGE: &str = "password_change_required";
/// The member refused the call (403).
pub const KIND_FORBIDDEN: &str = "forbidden";
/// The member does not serve what the manager reads.
pub const KIND_UNSUPPORTED: &str = "unsupported";
/// The member answered an unexpected status.
pub const KIND_STATUS: &str = "http_status";
/// The member's answer is not the JSON expected.
pub const KIND_BAD_ANSWER: &str = "bad_answer";
/// The member's answer is larger than `max_response_bytes`.
pub const KIND_TOO_LARGE: &str = "too_large";
/// A member that links out is not linked (I139b).
pub const KIND_NOT_LINKED: &str = "not_linked";

/// The message of a call to a member removed, or of a manager closing.
pub const CLOSED: &str = "the member's connections were closed";

/// What bounds a member's calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A call's timeout, from its connect to the last byte of its answer.
    pub timeout: Duration,
    /// The largest answer's body.
    pub max_bytes: usize,
}

/// The certificate a member presented: its fingerprint, which a person confirms before it is
/// pinned, and what it says. fit's `CertInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertInfo {
    /// The SHA-256 of the leaf, as fit writes pins.
    pub fingerprint: String,
    /// Its subject and issuer.
    pub subject: String,
    /// See [`subject`](CertInfo::subject).
    pub issuer: String,
    /// The DNS names and IP addresses it is for.
    pub names: Vec<String>,
    /// Its validity, seconds since the epoch.
    pub not_before: i64,
    /// See [`not_before`](CertInfo::not_before).
    pub not_after: i64,
    /// Whether it signed itself.
    pub self_signed: bool,
    /// How it was trusted, `pin`, `ca` or `system`, or empty when it was not.
    pub verified: String,
}

fn epoch_time(secs: i64) -> String {
    let t = if secs >= 0 {
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(secs.unsigned_abs()))
            .unwrap_or(SystemTime::UNIX_EPOCH)
    } else {
        SystemTime::UNIX_EPOCH
    };
    json::time(t)
}

impl CertInfo {
    /// As the API answers it.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .str("fingerprint", &self.fingerprint)
            .str("subject", &self.subject)
            .str("issuer", &self.issuer)
            .raw("names", &json::string_list(&self.names))
            .raw("not_before", &epoch_time(self.not_before))
            .raw("not_after", &epoch_time(self.not_after))
            .bool("self_signed", self.self_signed)
            .str_omit("verified", &self.verified)
            .end()
    }

    /// What `der`, a leaf, says.
    #[must_use]
    pub fn of(der: &[u8]) -> CertInfo {
        let fingerprint = format_fingerprint(&Sha256::digest(der));
        let Ok(c) = akio_x509::Certificate::from_der(der.to_vec()) else {
            return CertInfo {
                fingerprint,
                ..CertInfo::default()
            };
        };
        let names = c
            .subject_alt_names()
            .iter()
            .filter_map(|n| match n {
                akio_x509::GeneralName::Dns(d) => Some(d.clone()),
                akio_x509::GeneralName::Ip(ip) => ip_string(ip),
                _ => None,
            })
            .collect();
        let self_signed = c.issuer().as_der() == c.subject().as_der()
            && c.public_key().is_ok_and(|k| c.verify_signed_by(&k).is_ok());
        CertInfo {
            fingerprint,
            subject: c.subject().to_string(),
            issuer: c.issuer().to_string(),
            names,
            not_before: c.not_before(),
            not_after: c.not_after(),
            self_signed,
            verified: String::new(),
        }
    }
}

fn ip_string(ip: &[u8]) -> Option<String> {
    match ip.len() {
        4 => <[u8; 4]>::try_from(ip)
            .ok()
            .map(|a| std::net::Ipv4Addr::from(a).to_string()),
        16 => <[u8; 16]>::try_from(ip)
            .ok()
            .map(|a| std::net::Ipv6Addr::from(a).to_string()),
        _ => None,
    }
}

/// Why a call to a member failed: fit's `Problem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// One of the `KIND_*`.
    pub kind: &'static str,
    /// What happened, the member's credentials taken out, bounded.
    pub message: String,
    /// The member's status, when it answered.
    pub status: u16,
    /// When.
    pub at: SystemTime,
    /// The seconds the member asks to wait, for [`KIND_RATE_LIMITED`].
    pub retry_after: u64,
    /// The certificate refused, for [`KIND_TLS_UNTRUSTED`] and [`KIND_TLS_PIN`].
    pub certificate: Option<CertInfo>,
}

impl Problem {
    /// A problem of `kind`, now.
    #[must_use]
    pub fn new(kind: &'static str, message: impl Into<String>) -> Problem {
        Problem {
            kind,
            message: message.into(),
            status: 0,
            at: SystemTime::now(),
            retry_after: 0,
            certificate: None,
        }
    }

    /// With the member's status.
    #[must_use]
    pub fn with_status(mut self, status: u16) -> Problem {
        self.status = status;
        self
    }

    /// As the API answers it.
    #[must_use]
    pub fn json(&self) -> String {
        let mut o = Obj::new()
            .str("kind", self.kind)
            .str("message", &self.message);
        if self.status != 0 {
            o = o.num("status", self.status);
        }
        o = o.raw("at", &json::time(self.at));
        if self.retry_after != 0 {
            o = o.num("retry_after", self.retry_after);
        }
        if let Some(c) = &self.certificate {
            o = o.raw("certificate", &c.json());
        }
        o.end()
    }
}

/// A member's answer, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// Its status.
    pub status: u16,
    /// Its header fields.
    pub headers: Vec<(String, String)>,
    /// Its body, within `max_response_bytes`.
    pub body: Vec<u8>,
    /// How long the call took.
    pub took: Duration,
}

impl Reply {
    /// Header `name`'s first value.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What a call gives its caller.
pub type Done = Box<dyn FnOnce(Result<Reply, Problem>) + Send>;

/// How a member's certificate is trusted.
#[derive(Clone)]
enum Trust {
    Pin([u8; 32]),
    Roots(RootCerts, &'static str),
}

/// What the last probe saw: the certificate, and the problem it is, if any.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub(crate) cert: CertInfo,
    pub(crate) problem: Option<(&'static str, String)>,
}

/// A member's connections.
pub struct Transport {
    client: Client,
    engine: Arc<akio::Engine>,
    base: String,
    host: String,
    port: u16,
    trust: Option<Trust>,
    lim: Limits,
    seen: Mutex<Option<Seen>>,
    calls: Mutex<HashMap<u64, CancelToken>>,
    next: AtomicU64,
    closed: AtomicBool,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Transport {
    /// The connections of `m`, a member checked, on `engine`; `system` are the system's CAs,
    /// read once when the manager started.
    ///
    /// # Errors
    ///
    /// Its URL or CAs do not do: the member was not checked.
    pub fn new(
        engine: &Arc<akio::Engine>,
        m: &Member,
        lim: Limits,
        system: &RootCerts,
    ) -> Result<Transport, String> {
        let base = m.url.trim_end_matches('/').to_owned();
        let (scheme, rest) = base
            .split_once("://")
            .ok_or_else(|| format!("{:?} is not a URL", m.url))?;
        let authority = rest.split('/').next().unwrap_or("");
        let host = host_of(authority).ok_or_else(|| format!("{:?} has no host", m.url))?;
        let https = scheme == "https";
        let port = authority
            .rsplit_once(':')
            .filter(|(h, _)| !h.ends_with(']') || authority.starts_with('['))
            .and_then(|(_, p)| p.parse().ok())
            .unwrap_or(if https { 443 } else { 80 });
        let mut opts = ClientOptions::default();
        opts.max_head_bytes = MAX_HEADER_BYTES;
        opts.max_body_bytes = lim.max_bytes;
        opts.idle_timeout = Some(IDLE_CONN_TIMEOUT);
        opts.max_idle_per_host = 1;
        opts.max_idle = 1;
        opts.response_header_timeout = Some(lim.timeout);
        opts.happy_eyeballs.connect_timeout = lim.timeout.min(MAX_DIAL_TIMEOUT);
        let mut client = Client::new(Arc::clone(engine))
            .with_options(opts)
            .with_timeout(lim.timeout);
        let mut trust = None;
        if https {
            let t = if let Some(pin) = akit_config::roles::parse_pin(&m.tls.fingerprint) {
                Trust::Pin(pin)
            } else if !m.tls.ca.is_empty() {
                Trust::Roots(crate::member::ca_roots(&m.tls.ca)?, "ca")
            } else {
                Trust::Roots(system.clone(), "system")
            };
            let b = ClientConfig::builder().resumption(false);
            let cfg = match &t {
                Trust::Pin(p) => b.pin_sha256(vec![*p]),
                Trust::Roots(r, _) => b.roots(r.clone()),
            }
            .build()
            .map_err(|e| e.to_string())?;
            client = client.with_tls(cfg);
            trust = Some(t);
        }
        Ok(Transport {
            client,
            engine: Arc::clone(engine),
            base,
            host,
            port,
            trust,
            lim,
            seen: Mutex::new(None),
            calls: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        })
    }

    /// Whether the member is reached over TLS.
    #[must_use]
    pub fn tls(&self) -> bool {
        self.trust.is_some()
    }

    /// Sends `method path` with `headers` and `body` to the member, and calls `done` once with
    /// its answer, whatever its status, or why there is none: from an I/O thread or the caller's.
    /// Never blocks.
    pub fn call(
        self: &Arc<Self>,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        done: Done,
    ) {
        if self.closed.load(Ordering::Acquire) {
            return done(Err(Problem::new(KIND_UNREACHABLE, CLOSED)));
        }
        let method_v = match method {
            "GET" => akio_http::Method::Get,
            "HEAD" => akio_http::Method::Head,
            "POST" => akio_http::Method::Post,
            "PUT" => akio_http::Method::Put,
            "PATCH" => akio_http::Method::Patch,
            "DELETE" => akio_http::Method::Delete,
            other => {
                return done(Err(Problem::new(
                    KIND_STATUS,
                    format!("the method {other} is not passed on"),
                )));
            }
        };
        let mut req = ClientRequest::new(method_v, &format!("{}{path}", self.base))
            .header("Accept", "application/json")
            .header("User-Agent", "akit-cluster-manager")
            .timeout(self.lim.timeout)
            .response_header_timeout(self.lim.timeout);
        let has_type = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("Content-Type"));
        for (n, v) in headers {
            req = req.header(n, v);
        }
        if let Some(b) = body {
            if !has_type {
                req = req.header("Content-Type", "application/json");
            }
            req = req.body(b);
        }
        let token = CancelToken::new();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.calls).insert(id, token.clone());
        let req = req.cancel_with(&token);
        let start = Instant::now();
        let me = Arc::clone(self);
        self.client.request(req, move |r| {
            lock(&me.calls).remove(&id);
            match r {
                Ok(resp) => done(Ok(Reply {
                    status: resp.status,
                    headers: resp
                        .headers
                        .iter()
                        .map(|(n, v)| (n.to_owned(), v.to_owned()))
                        .collect(),
                    body: resp.body,
                    took: start.elapsed(),
                })),
                Err(e) => me.failure(&e, done),
            }
        });
    }

    /// The problem of `e`, a call's failure: a certificate the probe finds refused, a timeout,
    /// an answer too large, or no connection.
    fn failure(self: &Arc<Self>, e: &ClientError, done: Done) {
        let p = if e.is_timeout() {
            Problem::new(KIND_TIMEOUT, e.to_string())
        } else if matches!(e, ClientError::TooLarge(_)) {
            Problem::new(
                KIND_TOO_LARGE,
                "the member's answer is larger than max_response_bytes",
            )
        } else if matches!(e, ClientError::Cancelled) {
            Problem::new(KIND_UNREACHABLE, CLOSED)
        } else {
            Problem::new(KIND_UNREACHABLE, e.to_string())
        };
        if p.kind != KIND_UNREACHABLE || !self.tls() || p.message == CLOSED {
            return done(Err(p));
        }
        // The connection failed over TLS: the probe tells a certificate refused from the rest.
        self.probe(Box::new(move |seen| {
            done(Err(match seen {
                Some(Seen {
                    cert,
                    problem: Some((kind, msg)),
                }) => Problem {
                    certificate: Some(cert),
                    ..Problem::new(kind, msg)
                },
                _ => p,
            }));
        }));
    }

    /// The certificate of the latest probe, as trusted or not.
    #[must_use]
    pub fn certificate(&self) -> Option<CertInfo> {
        lock(&self.seen).as_ref().map(|s| s.cert.clone())
    }

    /// The certificate of the latest probe and the problem it is, if any.
    #[must_use]
    pub fn certificate_problem(&self) -> Option<Problem> {
        lock(&self.seen).as_ref().and_then(|s| {
            s.problem.as_ref().map(|(k, m)| Problem {
                certificate: Some(s.cert.clone()),
                ..Problem::new(k, m.clone())
            })
        })
    }

    /// Reads the member's certificate with a probe of its own (akio request A277's stopgap),
    /// checks it as the member's `tls` says, keeps what it saw, and calls `done` with it: `None`
    /// when the member is not reached over TLS or the probe did not complete a handshake.
    pub(crate) fn probe(self: &Arc<Self>, done: Box<dyn FnOnce(Option<Seen>) + Send>) {
        let Some(trust) = self.trust.clone() else {
            return done(None);
        };
        let cfg = match ClientConfig::builder()
            .resumption(false)
            .insecure_skip_verify()
            .handshake_timeout(Some(self.lim.timeout))
            .build()
        {
            Ok(c) => c,
            Err(_) => return done(None),
        };
        let me = Arc::clone(self);
        let host = self.host.clone();
        let report: Report = Arc::new(Mutex::new(Some(Box::new(
            move |chain: Option<Vec<Vec<u8>>>| {
                let seen = chain.map(|c| me.judge(&trust, &c));
                if let Some(s) = &seen {
                    *lock(&me.seen) = Some(s.clone());
                }
                done(seen);
            },
        ))));
        let on_open = Arc::clone(&report);
        let acceptor = akio::fn_acceptor(move |c: &akio::Conn<Probe>| {
            let _ = c.send(PROBE_REQUEST);
            let r = Arc::clone(&on_open);
            let deadline = c.clone();
            // The probe's whole time, a handshake and an answer's first bytes: a timeout.
            let _ = akio::timer::after_send(MAX_DIAL_TIMEOUT, move || deadline.close());
            Probe(Some(r))
        });
        let sni = if host.parse::<std::net::IpAddr>().is_ok() {
            "probe.invalid".to_owned()
        } else {
            host.clone()
        };
        let fail_report = Arc::clone(&report);
        let fail = move |_e: std::io::Error| {
            let taken = lock(&fail_report).take();
            if let Some(f) = taken {
                f(None);
            }
        };
        match akio::tls::Tls::client(cfg, &sni, acceptor) {
            Ok(a) => {
                let mut he = akio::dns::HappyEyeballs::default();
                he.connect_timeout = self.lim.timeout.min(MAX_DIAL_TIMEOUT);
                he.connect(&self.engine, &host, self.port, a, fail);
            }
            Err(e) => fail(e),
        }
    }

    /// Checks `chain`, the member's, as `trust` says.
    fn judge(&self, trust: &Trust, chain: &[Vec<u8>]) -> Seen {
        let Some(leaf) = chain.first() else {
            return Seen {
                cert: CertInfo::default(),
                problem: Some((
                    KIND_TLS_UNTRUSTED,
                    "the member sent no certificate".to_owned(),
                )),
            };
        };
        let mut cert = CertInfo::of(leaf);
        let problem = match trust {
            Trust::Pin(pin) => {
                let sum = Sha256::digest(leaf);
                if sum.as_slice() == pin.as_slice() {
                    cert.verified = "pin".to_owned();
                    None
                } else {
                    Some((
                        KIND_TLS_PIN,
                        format!(
                            "the member's certificate {} is not the one pinned",
                            cert.fingerprint
                        ),
                    ))
                }
            }
            Trust::Roots(roots, how) => {
                let verified = akio_x509::Certificate::from_der(leaf.clone()).and_then(|l| {
                    let inter: Vec<akio_x509::Certificate> = chain
                        .iter()
                        .skip(1)
                        .filter_map(|d| akio_x509::Certificate::from_der(d.clone()).ok())
                        .collect();
                    akio_x509::Verifier::new(roots.store()).verify_server(&l, &inter, &self.host)
                });
                match verified {
                    Ok(_) => {
                        cert.verified = (*how).to_owned();
                        None
                    }
                    Err(e) => Some((
                        KIND_TLS_UNTRUSTED,
                        format!(
                            "the member's certificate {} is not trusted: {e:?}",
                            cert.fingerprint
                        ),
                    )),
                }
            }
        };
        Seen { cert, problem }
    }

    /// Abandons the calls in flight: their `done` hear [`CLOSED`].
    pub fn abandon(&self) {
        let calls: Vec<CancelToken> = lock(&self.calls).drain().map(|(_, t)| t).collect();
        for t in calls {
            t.cancel();
        }
    }

    /// Abandons the calls in flight and closes the connections; calls after it fail at once.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.abandon();
        self.client.close_idle();
    }
}

/// Where a probe reports the chain it read, once.
type Report = Arc<Mutex<Option<Box<dyn FnOnce(Option<Vec<Vec<u8>>>) + Send>>>>;

/// A probe's connection: it reads the chain once the member answers, and closes.
struct Probe(Option<Report>);

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probe").finish_non_exhaustive()
    }
}

impl akio::Handler for Probe {
    fn on_data(&mut self, c: &akio::Conn<Self>, _data: &[u8]) {
        if let Some(r) = self.0.take() {
            let chain = akio::tls::tls_info(c)
                .map(|i| i.peer_certificates)
                .filter(|ch| !ch.is_empty());
            let taken = lock(&r).take();
            if let Some(f) = taken {
                f(chain);
            }
            c.close();
        }
    }

    fn on_close(&mut self, _c: &akio::Conn<Self>, _err: Option<akio::Error>) {
        if let Some(r) = self.0.take() {
            let taken = lock(&r).take();
            if let Some(f) = taken {
                f(None);
            }
        }
    }
}
