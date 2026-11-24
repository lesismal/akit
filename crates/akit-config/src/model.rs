//! The config model, v0: the parts of fit's config akit reads so far, with fit's field names,
//! defaults and checks. Every other field fit reads is refused as "not supported yet" by the
//! object that holds it (decision 002), and shrinks from the lists below as iterations land.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::json::Node;

/// The config schema version akit reads, as fit's `schema_version`.
pub const SCHEMA_VERSION: i64 = 1;

/// A whole config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `schema_version`: 1, the default.
    pub schema_version: i64,
    /// `listeners`: the addresses served, and their protocols.
    pub listeners: Vec<Listener>,
    /// `routes`: how requests are matched and what handles them.
    pub routes: Vec<Route>,
    /// `upstreams`: the servers requests can be proxied to, by name.
    pub upstreams: Vec<Upstream>,
    /// `limits`: what clients can make akit hold for them. Each listener carries the ones fixed
    /// in its binding ([`Listener::max_body_bytes`]).
    pub limits: Limits,
    /// `access_log`: where a line is written for each request; no access log when left out.
    pub access_log: Option<AccessLog>,
    /// `admin`: the Admin API (I26); off when left out.
    pub admin: Option<Admin>,
    /// `metrics`: count and time requests by listener and route, for the Admin API's
    /// `/metrics`, which serves the rest of the metrics without it; off when left out, since it
    /// costs every request a little. Needs `admin`, as in fit.
    pub metrics: Option<Metrics>,
}

/// `metrics`, as fit's `config.Metrics`: it turns on the counting of requests, and has no
/// settings yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metrics {}

/// fit's default `admin.address`.
pub const DEFAULT_ADMIN_ADDRESS: &str = "127.0.0.1:2019";
/// fit's default `admin.history`: the configs kept, the running one included.
pub const DEFAULT_ADMIN_HISTORY: usize = 10;
/// fit's default `admin.max_config_bytes`: 4 MiB.
pub const DEFAULT_ADMIN_MAX_CONFIG_BYTES: usize = 4 << 20;
/// fit's default `admin.max_conns`.
pub const DEFAULT_ADMIN_MAX_CONNS: usize = 16;

/// `admin`: the Admin API, as fit's `config.Admin` (decision 006): so far its address, history
/// and limits. Its token, TLS, access lists, store, console and the rest are refused as not
/// supported yet; so is a Unix socket (`unix/<path>`, akio API request A23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admin {
    /// `address`: a loopback host and port; [`DEFAULT_ADMIN_ADDRESS`] when left out. Without a
    /// token (I27), a host that is not loopback is refused, as fit refuses it.
    pub address: String,
    /// `history`: how many configs, the running one included, are kept to be read back;
    /// [`DEFAULT_ADMIN_HISTORY`] when left out or 0.
    pub history: usize,
    /// `max_config_bytes`: the largest config the API takes, larger ones getting 413;
    /// [`DEFAULT_ADMIN_MAX_CONFIG_BYTES`] when left out or 0.
    pub max_config_bytes: usize,
    /// `max_conns`: the most connections the API keeps open at once, one past it closed as it
    /// is accepted; [`DEFAULT_ADMIN_MAX_CONNS`] when left out or 0.
    pub max_conns: usize,
}

impl Default for Admin {
    fn default() -> Admin {
        Admin {
            address: DEFAULT_ADMIN_ADDRESS.to_owned(),
            history: DEFAULT_ADMIN_HISTORY,
            max_config_bytes: DEFAULT_ADMIN_MAX_CONFIG_BYTES,
            max_conns: DEFAULT_ADMIN_MAX_CONNS,
        }
    }
}

/// fit's default `limits.max_body_bytes`: 16 MiB.
pub const DEFAULT_MAX_BODY_BYTES: u64 = 16 << 20;

/// `limits`, as fit's `config.Limits`: so far `max_body_bytes`; its other fields are refused as
/// not supported yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `max_body_bytes`: the largest request body a listener takes, whether it arrives whole or
    /// streams through; a larger one gets 413, or, once it has begun to stream, its connection
    /// or stream is ended. [`DEFAULT_MAX_BODY_BYTES`] when left out or 0.
    pub max_body_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

/// `access_log`: where and how a line is written for each request, as fit's `config.AccessLog`.
/// Lines are buffered and written by a thread of their own; lines past `buffer_bytes` are
/// dropped and counted. `rotate` is refused as not supported yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLog {
    /// `output`: `"stdout"`, `"stderr"`, or the path of a file lines are appended to;
    /// `"stdout"` when left out (or empty).
    pub output: String,
    /// `flush_interval`: how long a line may wait in the buffer before it is written;
    /// [`DEFAULT_FLUSH_INTERVAL`] when left out or 0.
    pub flush_interval: Duration,
    /// `buffer_bytes`: how many bytes of lines may wait to be written; [`DEFAULT_BUFFER_BYTES`]
    /// when left out or 0.
    pub buffer_bytes: usize,
    /// `level`: the least level of the requests written, by their status.
    pub level: AccessLevel,
    /// `format`: how a line is written.
    pub format: LogFormat,
    /// `time_zone`, as written: `local` (or empty, the default), `utc`, or an IANA zone name.
    /// That it names a zone is checked when the log is opened (akit-observe's `Zone::load`).
    pub time_zone: String,
}

impl AccessLog {
    /// The least status of the responses whose requests are written, by `level`, as fit's
    /// `MinStatus`.
    #[must_use]
    pub fn min_status(&self) -> u16 {
        match self.level {
            AccessLevel::Info => 0,
            AccessLevel::Warn => 400,
            AccessLevel::Error => 500,
        }
    }
}

/// fit's default `flush_interval`.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// fit's default `buffer_bytes`: 4 MiB.
pub const DEFAULT_BUFFER_BYTES: usize = 4 << 20;

/// `access_log.level`: a response with a status below 400 is info, a 4xx warn, a 5xx error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessLevel {
    /// `info`, the default: every request.
    #[default]
    Info,
    /// `warn`: the requests answered with 4xx and 5xx.
    Warn,
    /// `error`: the requests answered with 5xx.
    Error,
}

/// `access_log.format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// `json`, the default: a JSON object a line.
    #[default]
    Json,
    /// `text`: `time=… level=… msg=access` and the fields as `key=value` (logfmt).
    Text,
    /// `line`: the time and level, then `access` and the fields as `key=value`, as fit's own log
    /// writes lines.
    Line,
}

/// A set of servers requests can be proxied to, and how the connections to them are kept, as
/// fit's `config.Upstream`. The limits hold fit's defaults where the config leaves them out (or,
/// for `max_conns` and `max_pending`, sets 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// `name`, unique among the upstreams, by which handlers refer to this one.
    pub name: String,
    /// `addresses`: each server's host and port, as `"10.0.0.1:8080"` or `"backend:80"`.
    pub addresses: Vec<String>,
    /// `max_conns`: the most connections open, or being opened, to one address; a request past
    /// it waits for one. [`DEFAULT_MAX_CONNS`] when left out or 0.
    pub max_conns: usize,
    /// `max_pending`: how many requests may wait for a connection to one address once its
    /// `max_conns` are busy; one more is refused at once. [`DEFAULT_MAX_PENDING`] when left out
    /// or 0.
    pub max_pending: usize,
    /// `max_idle_conns`: idle connections to one address kept for later requests; 0 keeps none.
    /// [`DEFAULT_MAX_IDLE_CONNS`] when left out.
    pub max_idle_conns: usize,
    /// `idle_timeout`: how long an idle connection is kept. [`DEFAULT_IDLE_TIMEOUT`] when left
    /// out or 0.
    pub idle_timeout: Duration,
    /// `connect_timeout`: how long opening a connection may take. [`DEFAULT_CONNECT_TIMEOUT`]
    /// when left out or 0.
    pub connect_timeout: Duration,
    /// `weights`: each address's weight, from 1 to 1000, in the order of `addresses`; empty when
    /// left out, which weighs each address 1.
    pub weights: Vec<u32>,
    /// `load_balancing`: how requests are spread over the addresses. When left out, fit's
    /// default: [`LoadBalancing::RoundRobin`], or [`LoadBalancing::WeightedRoundRobin`] when an
    /// address weighs other than 1.
    pub load_balancing: LoadBalancing,
    /// `max_fails`: how many failed requests to an address within `fail_timeout` take it out of
    /// rotation for `fail_timeout`, as nginx's `max_fails`; 0 keeps every address in rotation.
    /// [`DEFAULT_MAX_FAILS`] when left out.
    pub max_fails: usize,
    /// `fail_timeout`: the window `max_fails` are counted in, and how long an address that
    /// reached them stays out. [`DEFAULT_FAIL_TIMEOUT`] when left out or 0.
    pub fail_timeout: Duration,
    /// `health_check`: probes of each address on a schedule, which take one that fails them out
    /// of rotation until it passes them again (I35); `None`, no probes, when left out. Probes
    /// and `max_fails` take an address out each on their own account.
    pub health_check: Option<HealthCheck>,
}

/// An upstream's active health check, as fit's `config.HealthCheck`: a probe sent to each of its
/// addresses every `interval`. Every duration and count holds its default where the config
/// leaves it out (or sets 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheck {
    /// `type`: what a probe does. [`HealthCheckType::Http`] when left out.
    pub kind: HealthCheckType,
    /// `path`: with http, the path, and query, a probe asks for; `/` when left out.
    pub path: String,
    /// `host`: with http, the `Host` field a probe sends; the address's host when left out.
    pub host: Option<String>,
    /// `port`: the port probes go to, in place of each address's own.
    pub port: Option<u16>,
    /// `expect_status`: with http, the statuses that pass; any from 200 to 399 when empty.
    pub expect_status: Vec<u16>,
    /// `interval`: from the start of one probe of an address to the start of the next.
    /// [`DEFAULT_HEALTH_INTERVAL`] when left out or 0.
    pub interval: Duration,
    /// `timeout`: how long a probe may take, connecting included, before it fails; no longer
    /// than `interval`. [`DEFAULT_HEALTH_TIMEOUT`] when left out or 0.
    pub timeout: Duration,
    /// `jitter`: up to how much longer than `interval` each wait between probes may be, chosen
    /// at random each time; less than `interval`. A tenth of `interval` when left out or 0.
    pub jitter: Duration,
    /// `fails`: probes in a row that must fail to take an address out of rotation.
    /// [`DEFAULT_HEALTH_THRESHOLD`] when left out or 0.
    pub fails: usize,
    /// `passes`: probes in a row that must pass to put an address that was out back in.
    /// [`DEFAULT_HEALTH_THRESHOLD`] when left out or 0.
    pub passes: usize,
}

impl HealthCheck {
    /// Whether an http probe answered with `status` passes: one of `expect_status`, or any from
    /// 200 to 399 without it.
    #[must_use]
    pub fn expects(&self, status: u16) -> bool {
        if self.expect_status.is_empty() {
            (200..400).contains(&status)
        } else {
            self.expect_status.contains(&status)
        }
    }
}

/// A health check's `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HealthCheckType {
    /// `http`: a `GET` request, passing on an expected status.
    #[default]
    Http,
    /// `tcp`: passes once a connection opens.
    Tcp,
}

/// fit's default health check `interval`.
pub const DEFAULT_HEALTH_INTERVAL: Duration = Duration::from_secs(5);
/// fit's default health check `timeout`.
pub const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// fit's default health check `fails` and `passes`.
pub const DEFAULT_HEALTH_THRESHOLD: usize = 2;

impl Upstream {
    /// The weight of the address at `i`: its `weights` entry, or 1.
    #[must_use]
    pub fn weight(&self, i: usize) -> u32 {
        self.weights.get(i).copied().unwrap_or(1)
    }
}

/// An upstream's `load_balancing` policy. fit's `random`, `p2c` and hash policies are refused as
/// not supported yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum LoadBalancing {
    /// `round_robin`: the addresses in turn.
    #[default]
    RoundRobin,
    /// `weighted_round_robin`: in turn by weight, smoothly, as nginx does.
    WeightedRoundRobin,
    /// `least_conn`: to the address with the fewest requests under way for its weight.
    LeastConn,
}

impl LoadBalancing {
    /// The policy's name in the config.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LoadBalancing::RoundRobin => "round_robin",
            LoadBalancing::WeightedRoundRobin => "weighted_round_robin",
            LoadBalancing::LeastConn => "least_conn",
        }
    }
}

/// The most an address of an upstream may weigh (fit's `max` on `weights`).
pub const MAX_WEIGHT: u32 = 1000;

/// fit's default `max_conns`: large enough that high concurrency against a slow upstream does not
/// reach it, and still a bound.
pub const DEFAULT_MAX_CONNS: usize = 10_000;
/// fit's default `max_pending`.
pub const DEFAULT_MAX_PENDING: usize = 50_000;
/// fit's default `max_idle_conns`.
pub const DEFAULT_MAX_IDLE_CONNS: usize = 32;
/// fit's default `idle_timeout`.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// fit's default `connect_timeout`.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's default `max_fails`.
pub const DEFAULT_MAX_FAILS: usize = 1;
/// fit's default `fail_timeout`.
pub const DEFAULT_FAIL_TIMEOUT: Duration = Duration::from_secs(10);

/// One address akit listens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    /// `name`, which routes refer to.
    pub name: String,
    /// `address`, a host and port as in `":8080"` or `"127.0.0.1:8080"`.
    pub address: String,
    /// `protocols`; fit's default when left out: HTTP/1.1 alone, or HTTP/1.1 and h2 with `tls`.
    pub protocols: Vec<Protocol>,
    /// `tls`: the listener serves HTTPS, with these certificates.
    pub tls: Option<Tls>,
    /// The config's `limits.max_body_bytes`, which fit fixes in each listener's binding: a
    /// reload that changes it changes every listener, as fit's `binding.same` compares it.
    pub max_body_bytes: u64,
}

/// A listener's TLS: its certificates from files, chosen by the name a client asks for (SNI),
/// and how it authenticates clients. As fit's, it allows TLS 1.2 and 1.3, with only AEAD cipher
/// suites with forward secrecy for 1.2 (ECDHE with AES-GCM or ChaCha20-Poly1305).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tls {
    /// `certificates`: a client is given the first whose names match the one it asks for, or the
    /// first of all when none does or it asks for none.
    pub certificates: Vec<Certificate>,
    /// `client_auth`: ask clients for certificates and verify them (mutual TLS).
    pub client_auth: Option<ClientAuth>,
    /// `max_version`: the newest TLS version allowed; 1.3 when left out.
    pub max_version: TlsVersion,
}

/// A certificate and its key, from PEM files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// `cert_file`: the certificate, followed by any intermediates.
    pub cert_file: String,
    /// `key_file`: its private key.
    pub key_file: String,
}

/// How a TLS listener authenticates its clients by their certificates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAuth {
    /// `ca_files`: PEM files of the authorities a client's certificate must chain to.
    pub ca_files: Vec<String>,
    /// `mode`: `require` when left out.
    pub mode: ClientAuthMode,
}

/// `client_auth.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuthMode {
    /// `require`: a client without a valid certificate is refused in the handshake.
    Require,
    /// `verify_if_given`: a client may send none, but one it sends must be valid.
    VerifyIfGiven,
}

/// A TLS version, as `max_version` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVersion {
    /// `1.2`.
    Tls12,
    /// `1.3`.
    Tls13,
}

/// A protocol a listener serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `http/1.1`.
    Http11,
    /// `h2c`, HTTP/2 in the clear.
    H2c,
    /// `h2`, HTTP/2 over TLS, chosen through ALPN; needs `tls`.
    H2,
    /// `h3`, HTTP/3 over QUIC, on the listener's port in UDP and announced to its HTTP/1 and
    /// HTTP/2 clients with `Alt-Svc`; needs `tls`.
    H3,
}

impl Protocol {
    /// The name the config writes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http11 => "http/1.1",
            Protocol::H2c => "h2c",
            Protocol::H2 => "h2",
            Protocol::H3 => "h3",
        }
    }
}

/// fit's default protocols for a listener without TLS: HTTP/1.1 alone (fit's
/// `config.DefaultProtocols`).
pub const DEFAULT_PROTOCOLS: [Protocol; 1] = [Protocol::Http11];

/// fit's default protocols for a listener with TLS: HTTP/1.1 and h2 (fit's
/// `config.DefaultTLSProtocols`).
pub const DEFAULT_TLS_PROTOCOLS: [Protocol; 2] = [Protocol::Http11, Protocol::H2];

/// One route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// `name`, optional.
    pub name: Option<String>,
    /// `listeners` the route serves on; every HTTP listener when empty.
    pub listeners: Vec<String>,
    /// `priority`: higher first; routes of equal priority, most specific first.
    pub priority: i64,
    /// `match`; every request when left out.
    pub matcher: Option<Match>,
    /// `middleware`, in the order written: the first is the outermost, the first a request goes
    /// through (I40). Each is one of [`Handler`]'s middleware variants ([`Handler::is_middleware`]).
    pub middleware: Vec<Handler>,
    /// `handler`.
    pub handler: Handler,
}

/// What a route matches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Match {
    /// `host`: exact names, or `*.` / `**.` wildcards.
    pub host: Vec<String>,
    /// `path`: exact paths, or prefixes ending in `*`.
    pub path: Vec<String>,
    /// `path_regexp`: regular expressions matched against the path, anywhere in it unless
    /// anchored.
    pub path_regexp: Vec<String>,
    /// `method`: methods, compared as written.
    pub method: Vec<String>,
    /// `header`: header names (ignoring case), each with the values one of its fields must have
    /// (any value when none are listed), in the order written.
    pub header: Vec<(String, Vec<String>)>,
    /// `query`: query parameters, alike, compared once unescaped.
    pub query: Vec<(String, Vec<String>)>,
    /// `remote_ip`: IP addresses and CIDR prefixes the connection's peer must be in.
    pub remote_ip: Vec<String>,
    /// `expr`: an expression the request must match (`akit_router::Expr`); empty when not set.
    pub expr: String,
}

/// A module of a route and its params: its handler (`respond`, `static`, `proxy`) or, from I40,
/// one of its middleware (`rewrite`, `redirect`, `headers`). One type for both, as akit-runtime's
/// registry provisions every module from the same params type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// `respond`: a fixed status, header fields and body.
    Respond(Respond),
    /// `static`: the files under a root directory.
    Static(Static),
    /// `proxy`: requests forwarded to an upstream (I15).
    Proxy(Proxy),
    /// The `rewrite` middleware: the request's path changed before the handler sees it (I40).
    Rewrite(Rewrite),
    /// The `redirect` middleware: requests answered with a redirect (I40).
    Redirect(Redirect),
    /// The `headers` middleware: request and response header fields changed (I40).
    Headers(Headers),
    /// The `rate_limit` middleware: requests a second by key, by token bucket (I41).
    RateLimit(RateLimit),
    /// The `concurrency_limit` middleware: requests under way at once (I41).
    ConcurrencyLimit(ConcurrencyLimit),
    /// The `body_limit` middleware: the size of a request's body (I41).
    BodyLimit(BodyLimit),
}

impl Handler {
    /// The module's name, as the config writes it in `module`.
    #[must_use]
    pub fn module(&self) -> &'static str {
        match self {
            Handler::Respond(_) => "respond",
            Handler::Static(_) => "static",
            Handler::Proxy(_) => "proxy",
            Handler::Rewrite(_) => "rewrite",
            Handler::Redirect(_) => "redirect",
            Handler::Headers(_) => "headers",
            Handler::RateLimit(_) => "rate_limit",
            Handler::ConcurrencyLimit(_) => "concurrency_limit",
            Handler::BodyLimit(_) => "body_limit",
        }
    }

    /// Whether the module is a middleware (in a route's `middleware`) rather than a handler.
    #[must_use]
    pub fn is_middleware(&self) -> bool {
        matches!(
            self,
            Handler::Rewrite(_)
                | Handler::Redirect(_)
                | Handler::Headers(_)
                | Handler::RateLimit(_)
                | Handler::ConcurrencyLimit(_)
                | Handler::BodyLimit(_)
        )
    }
}

/// The `rewrite` middleware's params, as fit's `middleware/rewrite`. `regexp` and `replacement`
/// are refused as not supported yet: akio-regex does not find where a pattern matches yet (akio
/// API request A32).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rewrite {
    /// `strip_prefix`: prefixes, each starting with `/`; the first the path starts with, at a
    /// segment boundary, is removed. None when left out.
    pub strip_prefix: Vec<String>,
    /// `add_prefix`: a prefix put before the path, starting with `/` and not ending with one;
    /// empty when left out.
    pub add_prefix: String,
}

/// The `redirect` middleware's params, as fit's `middleware/redirect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// `to`: where to, with `{scheme}`, `{host}`, `{path}`, `{query}` and `{uri}` standing for
    /// the request's. Groups of `regexp` (`$1`, `${name}`) in it are refused as not supported yet
    /// (akio API request A32); with a `regexp`, `$$` stands for `$`.
    pub to: String,
    /// `status`: 301, 302, 303, 307 or 308; 302 when left out.
    pub status: u16,
    /// `regexp`: what the escaped path must match for the request to be redirected; every
    /// request is when empty.
    pub regexp: String,
}

/// The `headers` middleware's params, as fit's `middleware/headers`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Headers {
    /// `request`: changes to the request's fields, made before the handler sees them.
    pub request: HeaderOps,
    /// `response`: changes to the response's fields, made just before it is sent.
    pub response: HeaderOps,
}

/// Changes to a header, as fit's `headers.Ops`: deletions first, then fields set, then fields
/// added. Names are as written, checked to be HTTP tokens; values are checked to hold no line
/// break or NUL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeaderOps {
    /// `set`: fields each replacing any of the same name, sorted by name.
    pub set: Vec<(String, String)>,
    /// `add`: values added to fields after any already there, sorted by name.
    pub add: Vec<(String, Vec<String>)>,
    /// `delete`: names, or prefixes ending in `*` (`"X-Debug-*"`; `"*"` is every field).
    pub delete: Vec<String>,
}

impl HeaderOps {
    /// Whether there is nothing to change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.add.is_empty() && self.delete.is_empty()
    }
}

/// What the `rate_limit` middleware counts requests by: its `key`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RateKey {
    /// `ip`: the client's address (the default).
    #[default]
    Ip,
    /// `route`: every request of the route together.
    Route,
    /// `header:Name`: a header field's first value, or the client's address without one. The
    /// name as written; compared ignoring case.
    Header(String),
}

/// The `rate_limit` middleware's params, as fit's `middleware/limit` `Rate`: a token bucket for
/// each key, holding up to `burst` tokens, refilled at `rate` a second; a request takes one or is
/// refused with `status` and `Retry-After`.
#[derive(Debug, Clone)]
pub struct RateLimit {
    /// `rate`: requests a second each key may make, on average; at least 0.001.
    pub rate: f64,
    /// `burst`: the bucket's size; the rate rounded up when left out or 0.
    pub burst: u64,
    /// `key`: what requests are counted by.
    pub key: RateKey,
    /// `trusted_proxies`: proxies whose `X-Forwarded-For` names the client.
    pub trusted_proxies: Vec<Network>,
    /// `ipv6_prefix`: the leading bits of an IPv6 address that name a client; 64 when left out
    /// or 0.
    pub ipv6_prefix: u8,
    /// `max_keys`: the most keys kept at once; 100000 when left out or 0.
    pub max_keys: u64,
    /// `status`: the status of a request refused; 429 when left out or 0.
    pub status: u16,
}

// The rate is a finite number (the decoder refuses others), so comparing its bits is an
// equivalence.
impl PartialEq for RateLimit {
    fn eq(&self, o: &RateLimit) -> bool {
        self.rate.to_bits() == o.rate.to_bits()
            && self.burst == o.burst
            && self.key == o.key
            && self.trusted_proxies == o.trusted_proxies
            && self.ipv6_prefix == o.ipv6_prefix
            && self.max_keys == o.max_keys
            && self.status == o.status
    }
}

impl Eq for RateLimit {}

/// fit's `max_keys` when left out or 0.
pub const DEFAULT_RATE_MAX_KEYS: u64 = 100_000;

/// The `concurrency_limit` middleware's params, as fit's `middleware/limit` `Concurrency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConcurrencyLimit {
    /// `max`: the most requests under way at once; at least 1.
    pub max: u64,
    /// `status`: the status of a request refused; 429 when left out or 0.
    pub status: u16,
}

/// The `body_limit` middleware's params, as fit's `middleware/limit` `Body`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyLimit {
    /// `max_bytes`: the most bytes a request's body may have.
    pub max_bytes: u64,
}

/// The `respond` handler's params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Respond {
    /// `status`, 200 when left out.
    pub status: u16,
    /// `headers`, in the order written.
    pub headers: Vec<(String, String)>,
    /// `body`.
    pub body: String,
}

/// The `static` handler's params, as fit's `handler/static`. akit reads the ones akio-http's
/// `Files` can act on; the rest are refused as not supported yet (akio API requests A14 and A15,
/// and I47 for `compress`, bring them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Static {
    /// `root`: the directory files are served from. Required; that it is a directory is checked
    /// when the handler is provisioned, as fit's.
    pub root: String,
    /// `index`: the file that stands for a directory. fit takes a list, tried in order,
    /// `["index.html"]` when left out; akit takes one name so far (a list of other than one is
    /// refused as not supported yet, akio API request A14).
    pub index: String,
    /// `max_file_bytes`: the largest file kept in memory, [`DEFAULT_MAX_FILE_BYTES`] when left
    /// out or 0. Larger ones are sent from disk.
    pub max_file_bytes: u64,
    /// `dotfiles`: whether paths with a segment starting with a dot are served.
    pub dotfiles: Dotfiles,
    /// `check_interval`: how long what was learned of a file is trusted before the disk is
    /// looked at again, where no watch reports its changes; 0 looks at the disk for every
    /// request. [`DEFAULT_CHECK_INTERVAL`] when left out.
    pub check_interval: Duration,
    /// `precompressed`: the encodings whose compressed twins (`name.br`, `name.zst`,
    /// `name.gz`) are served in place of a file to a client that accepts one, in the order
    /// written. None when left out.
    pub precompressed: Vec<Encoding>,
    /// `strip_prefix`: a prefix removed from the request path before it is looked up under the
    /// root; a request whose path lacks it is answered 404. Empty when left out.
    pub strip_prefix: String,
    /// `try_files`: paths tried in turn for a request, as nginx's `try_files`; `{path}` stands
    /// for the request's path, and one ending in `/` names a directory. None when left out: the
    /// request's path alone is served; an empty list, as fit's, finds nothing (every request is
    /// answered 404).
    pub try_files: Option<Vec<String>>,
    /// `browse`: a directory without an index file is listed rather than answered 404.
    pub browse: bool,
    /// `browse_max_entries`: the most entries a listing reads and shows,
    /// [`DEFAULT_BROWSE_MAX_ENTRIES`] when left out or 0.
    pub browse_max_entries: usize,
}

/// A content coding a file's compressed twin is in: `precompressed`'s values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// `br`: the twin `name.br`.
    Br,
    /// `zstd`: the twin `name.zst`.
    Zstd,
    /// `gzip`: the twin `name.gz`.
    Gzip,
}

impl Encoding {
    /// The name `Accept-Encoding` and `Content-Encoding` give it, as fit's config does.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Br => "br",
            Encoding::Zstd => "zstd",
            Encoding::Gzip => "gzip",
        }
    }

    /// The extension of its twin, after the file's name and a dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Encoding::Br => "br",
            Encoding::Zstd => "zst",
            Encoding::Gzip => "gz",
        }
    }
}

/// Whether `path` has a segment `dotfiles` `deny` hides: one starting with a dot, but `.` and
/// `..` and `.well-known` as the first segment (fit's `dotfile.Scan`, on a path as written in
/// the config). A path with a `..` segment is refused 400 at request time rather than hidden,
/// as fit's `check` answers it, so it is not.
fn hides_dotfile(path: &str) -> bool {
    if path.split('/').any(|s| s == "..") {
        return false;
    }
    path.split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .enumerate()
        .any(|(i, s)| s.starts_with('.') && !(i == 0 && s == ".well-known"))
}

/// fit's default `max_file_bytes`: 1 MiB.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1 << 20;
/// fit's default `browse_max_entries`.
pub const DEFAULT_BROWSE_MAX_ENTRIES: usize = 10_000;
/// fit's default `check_interval`.
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// The `static` handler's `dotfiles`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dotfiles {
    /// `deny`, the default: a path with a segment starting with a dot (but `/.well-known/` at
    /// the root) is answered 404, as a file not there.
    #[default]
    Deny,
    /// `allow`: dotfiles are served as other files.
    Allow,
}

/// The `proxy` handler's params: what I15 reads of fit's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    /// `upstream`: the name of the upstream requests go to. That it names one of the config's
    /// upstreams is checked when the handler is provisioned, once every file's are joined.
    pub upstream: String,
    /// `trusted_proxies`: the proxies in front of this one, whose `X-Forwarded-*` and
    /// `Forwarded` fields are believed. None when left out.
    pub trusted_proxies: Vec<Network>,
    /// `host_header`: the `Host` sent upstream; the client's when empty.
    pub host_header: String,
}

/// An IP network, as fit's `netip.Prefix`: an address alone is a network of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    /// The network's address, its host bits cleared.
    pub addr: IpAddr,
    /// The prefix length: up to 32 for IPv4, 128 for IPv6.
    pub bits: u8,
}

impl Network {
    /// Parses `"10.0.0.1"` or `"10.0.0.0/8"` as fit's `parsePrefix` does: an IPv4 address
    /// written as IPv6 (`::ffff:10.0.0.1`) is taken as IPv4 when it stands alone, and a network's
    /// host bits are cleared.
    #[must_use]
    pub fn parse(s: &str) -> Option<Network> {
        let Some((addr, bits)) = s.split_once('/') else {
            let addr = unmap(s.parse::<IpAddr>().ok()?);
            let bits = if addr.is_ipv4() { 32 } else { 128 };
            return Some(Network { addr, bits });
        };
        let addr = addr.parse::<IpAddr>().ok()?;
        // Go's prefix lengths are decimal digits, no sign, no leading zero but "0" itself.
        if bits.is_empty()
            || !bits.bytes().all(|b| b.is_ascii_digit())
            || (bits.len() > 1 && bits.starts_with('0'))
        {
            return None;
        }
        let bits = bits.parse::<u8>().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        if bits > max {
            return None;
        }
        Some(Network {
            addr: mask(addr, bits),
            bits,
        })
    }

    /// Whether `ip` is in the network. An IPv4 address is never in an IPv6 network, nor the
    /// other way round, as in Go: unmap an IPv4-mapped address first.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (ip, self.addr) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                mask(ip, self.bits) == self.addr
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.bits)
    }
}

/// `ip`, an IPv4-mapped IPv6 address taken as the IPv4 address it maps.
#[must_use]
pub(crate) fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// `ip` with all but its first `bits` bits cleared.
fn mask(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let m = u32::MAX
                .checked_shl(32u32.saturating_sub(u32::from(bits)))
                .unwrap_or(0);
            IpAddr::V4(Ipv4Addr::from(u32::from(v4) & m))
        }
        IpAddr::V6(v6) => {
            let m = u128::MAX
                .checked_shl(128u32.saturating_sub(u32::from(bits)))
                .unwrap_or(0);
            IpAddr::V6(Ipv6Addr::from(u128::from(v6) & m))
        }
    }
}

// What akit reads, and what fit reads that akit does not yet, object by object.

const CONFIG_FIELDS: &[&str] = &[
    "schema_version",
    "listeners",
    "routes",
    "upstreams",
    "limits",
    "access_log",
    "admin",
    "metrics",
];
const CONFIG_FIT_ONLY: &[&str] = &[
    "services",
    "log",
    "telemetry",
    "access",
    "engine",
    "auto_https",
    "pprof",
    "acme",
    "llm",
    "tunnel",
    "webrtc",
    "media_auth",
    "streaming",
    "media_library",
    "sync",
    "bittorrent",
    "providers",
];
// I26: the Admin API's address, history and limits; its token and TLS come with I27, its
// access lists with fit's access, a Unix socket with akio API request A23.
const ADMIN_FIELDS: &[&str] = &["address", "history", "max_config_bytes", "max_conns"];
const ADMIN_FIT_ONLY: &[&str] = &[
    "mptcp",
    "token_file",
    "cert_file",
    "key_file",
    "client_ca_file",
    "node_id",
    "disable_dashboard",
    "store",
    "console",
    "assistant",
    "cluster",
    "cluster_member",
    "access",
];
const LIMITS_FIELDS: &[&str] = &["max_body_bytes"];
const LIMITS_FIT_ONLY: &[&str] = &[
    "max_pending_bytes",
    "max_header_bytes",
    "max_conns",
    "max_write_queue_bytes",
    "write_stall_timeout",
];
const ACCESS_LOG_FIELDS: &[&str] = &[
    "output",
    "flush_interval",
    "buffer_bytes",
    "level",
    "format",
    "time_zone",
];
// rotate comes with the file sink's rotation (fit's logs package).
const ACCESS_LOG_FIT_ONLY: &[&str] = &["rotate"];
const LISTENER_FIELDS: &[&str] = &["name", "address", "protocols", "tls"];
const LISTENER_FIT_ONLY: &[&str] = &["mptcp", "socks", "access", "proxy_protocol"];
const FIT_PROTOCOLS: &[&str] = &["tcp", "udp", "socks"];
const TLS_FIELDS: &[&str] = &["certificates", "client_auth", "max_version"];
// acme and on_demand come with I31 and I34; kernel_tls_offload with engine's.
const TLS_FIT_ONLY: &[&str] = &["acme", "on_demand", "kernel_tls_offload"];
const CERTIFICATE_FIELDS: &[&str] = &["cert_file", "key_file"];
const CLIENT_AUTH_FIELDS: &[&str] = &["ca_files", "mode"];
const ROUTE_FIELDS: &[&str] = &[
    "name",
    "listeners",
    "priority",
    "match",
    "middleware",
    "handler",
];
const ROUTE_FIT_ONLY: &[&str] = &["access"];
const MATCH_FIELDS: &[&str] = &[
    "host",
    "path",
    "path_regexp",
    "method",
    "header",
    "query",
    "remote_ip",
    "expr",
];
const MATCH_FIT_ONLY: &[&str] = &[];
const MODULE_FIELDS: &[&str] = &["module", "params"];
/// fit's handler modules (its `http.handlers` namespace) that akit does not have yet.
const FIT_HANDLERS: &[&str] = &[
    "bt_publish",
    "bt_stream",
    "bt_tracker",
    "fastcgi",
    "forward_proxy",
    "hls",
    "httpflv",
    "media_pages",
    "media_tokens",
    "stream_whep",
    "stream_whip",
    "sync_ingest",
    "vod",
    "webrtc_proxy",
    "websockify",
    "whep",
    "whip",
];
/// fit's middleware modules (its `http.middleware` namespace) that akit does not have yet.
const FIT_MIDDLEWARE: &[&str] = &[
    "basic_auth",
    "cache",
    "circuit_breaker",
    "client_cert",
    "compress",
    "forward_auth",
    "jwt",
    "wasm",
];
const REWRITE_FIELDS: &[&str] = &["strip_prefix", "add_prefix"];
/// akio-regex answers whether a pattern matches, not where (akio API request A32).
const REWRITE_FIT_ONLY: &[&str] = &["regexp", "replacement"];
const REDIRECT_FIELDS: &[&str] = &["to", "status", "regexp"];
const HEADERS_FIELDS: &[&str] = &["request", "response"];
const HEADER_OPS_FIELDS: &[&str] = &["set", "add", "delete"];
const RATE_LIMIT_FIELDS: &[&str] = &[
    "rate",
    "burst",
    "key",
    "trusted_proxies",
    "ipv6_prefix",
    "max_keys",
    "status",
];
const CONCURRENCY_LIMIT_FIELDS: &[&str] = &["max", "status"];
const BODY_LIMIT_FIELDS: &[&str] = &["max_bytes"];
const RESPOND_FIELDS: &[&str] = &["status", "headers", "body"];
const STATIC_FIELDS: &[&str] = &[
    "root",
    "index",
    "max_file_bytes",
    "dotfiles",
    "disk_io",
    "check_interval",
    "precompressed",
    "browse",
    "browse_max_entries",
    "strip_prefix",
    "try_files",
];
// max_cache_bytes and default_type wait on akio API requests A15 and A14; compress (copies
// compressed in memory) on I47's compression.
const STATIC_FIT_ONLY: &[&str] = &["max_cache_bytes", "compress", "default_type"];
const PROXY_FIELDS: &[&str] = &["upstream", "trusted_proxies", "host_header"];
// upstreams, failover and mirror come with I19 and later; retry and timeout with I17 and I45;
// tunnel_idle_timeout with I49; splice_uploads with I16's streamed bodies.
const PROXY_FIT_ONLY: &[&str] = &[
    "upstreams",
    "failover",
    "mirror",
    "retry",
    "tunnel_idle_timeout",
    "timeout",
    "splice_uploads",
];
const UPSTREAM_FIELDS: &[&str] = &[
    "name",
    "addresses",
    "protocol",
    "max_conns",
    "max_pending",
    "max_idle_conns",
    "idle_timeout",
    "connect_timeout",
    "weights",
    "load_balancing",
    "max_fails",
    "fail_timeout",
    "health_check",
];
const HEALTH_CHECK_FIELDS: &[&str] = &[
    "type",
    "path",
    "host",
    "port",
    "expect_status",
    "interval",
    "timeout",
    "jitter",
    "fails",
    "passes",
];
const UPSTREAM_FIT_ONLY: &[&str] = &[
    "hash_key",
    "sticky_cookie",
    "deny_internal",
    "tls_server_name",
    "tls_ca_file",
    "tls_cert_file",
    "tls_key_file",
    "response_header_timeout",
    "response_idle_timeout",
    "response_timeout",
];

/// One file's part of a config, before the parts are merged.
pub(crate) struct Part {
    /// `schema_version`, with its position, when the file gives it.
    schema_version: Option<(i64, usize)>,
    listeners: Vec<At<Listener>>,
    routes: Vec<At<Route>>,
    upstreams: Vec<At<Upstream>>,
    /// `limits`, with its position, when the file gives it.
    limits: Option<(Limits, usize)>,
    /// `access_log`, with the position of its key, when the file gives it.
    access_log: Option<(AccessLog, usize)>,
    /// `admin`, with the position of its key, when the file gives it.
    admin: Option<(Admin, usize)>,
    /// `metrics`, with the position of its key, when the file gives it.
    metrics: Option<(Metrics, usize)>,
    /// The position of the file's top object.
    doc_off: usize,
}

/// Decodes one file's config; `file` is its index among the files merged.
pub(crate) fn part(d: &mut Decoder<'_>, n: &Node, file: usize) -> Option<Part> {
    let mut o = d.object(n, "")?;
    let schema_version = o.take("schema_version").and_then(|v| {
        d.int(v, "schema_version", SCHEMA_VERSION, SCHEMA_VERSION)
            .map(|s| (s, v.off))
    });
    // Each element keeps its file, path and position for the checks across elements.
    let listeners = o
        .take("listeners")
        .map(|v| {
            d.array(v, "listeners", |d, n, p| {
                Some(At::new(file, p, n, listener(d, n, p)?))
            })
        })
        .unwrap_or_default();
    let routes = o
        .take("routes")
        .map(|v| {
            d.array(v, "routes", |d, n, p| {
                Some(At::new(file, p, n, route(d, n, p)?))
            })
        })
        .unwrap_or_default();
    let upstreams = o
        .take("upstreams")
        .map(|v| {
            d.array(v, "upstreams", |d, n, p| {
                Some(At::new(file, p, n, upstream(d, n, p)?))
            })
        })
        .unwrap_or_default();
    let limits = o
        .take("limits")
        .and_then(|v| Some((limits(d, v, "limits")?, v.off)));
    let access_log = o
        .take("access_log")
        .and_then(|v| access_log(d, v, "access_log").map(|a| (a, v.off)));
    let admin = o
        .take("admin")
        .and_then(|v| admin(d, v, "admin").map(|a| (a, v.off)));
    let metrics = o.take("metrics").and_then(|v| {
        let m = d.object(v, "metrics")?;
        // No settings yet: any field is unknown, as in fit.
        m.finish(d, &[], &[]);
        Some((Metrics {}, v.off))
    });
    let doc_off = o.off;
    o.finish(d, CONFIG_FIELDS, CONFIG_FIT_ONLY);
    Some(Part {
        schema_version,
        listeners,
        routes,
        upstreams,
        limits,
        access_log,
        admin,
        metrics,
        doc_off,
    })
}

/// Merges the files' parts, `ds` their decoders in the same order, as fit merges a config's
/// files: the lists at the top (`listeners`, `routes`, `upstreams`) are joined in the files' order, and
/// `schema_version` may be given by each file if they agree; any other field (`limits`) by one file
/// only. The checks across elements (names
/// unique, routes naming listeners that exist) run on the merged lists; each problem is reported
/// in the file and at the position of the element it is about, with the element's path in that
/// file.
pub(crate) fn merge(ds: &mut [Decoder<'_>], parts: Vec<Part>) -> Config {
    let mut schema: Option<(i64, usize)> = None;
    let mut listeners = Vec::new();
    let mut routes = Vec::new();
    let mut upstreams = Vec::new();
    let mut limits: Option<(Limits, usize)> = None;
    let mut first_doc: Option<(usize, usize)> = None;
    let mut access_log: Option<(AccessLog, usize)> = None;
    let mut admin: Option<(Admin, usize)> = None;
    let mut metrics: Option<(Metrics, usize, usize)> = None;
    for (file, p) in parts.into_iter().enumerate() {
        first_doc.get_or_insert((file, p.doc_off));
        if let Some((v, off)) = p.schema_version {
            match schema {
                Some((first, first_file)) if first != v => {
                    let name = ds
                        .get(first_file)
                        .and_then(Decoder::file)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(d) = ds.get_mut(file) {
                        d.error(
                            off,
                            "schema_version",
                            format!("differs from the one in {name}"),
                        );
                    }
                }
                Some(_) => {}
                None => schema = Some((v, file)),
            }
        }
        if let Some((l, off)) = p.limits {
            match limits {
                Some((_, first_file)) => {
                    let name = ds
                        .get(first_file)
                        .and_then(Decoder::file)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(d) = ds.get_mut(file) {
                        // fit's message (adapter.Set.conflict).
                        d.error(
                            off,
                            "limits",
                            format!(
                                "also given in {name}: only the lists at the top of a config \
                                 (listeners, routes, upstreams, services, providers) may be split \
                                 across files"
                            ),
                        );
                    }
                }
                None => limits = Some((l, file)),
            }
        }
        if let Some((a, off)) = p.access_log {
            match &access_log {
                // As fit's adapter merges files: only the lists at the top may be split.
                Some((_, first_file)) => {
                    let name = ds
                        .get(*first_file)
                        .and_then(Decoder::file)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(d) = ds.get_mut(file) {
                        d.error(
                            off,
                            "access_log",
                            format!(
                                "also given in {name}: only the lists at the top of a config (listeners, routes, upstreams, services, providers) may be split across files"
                            ),
                        );
                    }
                }
                None => access_log = Some((a, file)),
            }
        }
        if let Some((a, off)) = p.admin {
            match &admin {
                Some((_, first_file)) => {
                    let name = ds
                        .get(*first_file)
                        .and_then(Decoder::file)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(d) = ds.get_mut(file) {
                        d.error(
                            off,
                            "admin",
                            format!(
                                "also given in {name}: only the lists at the top of a config (listeners, routes, upstreams, services, providers) may be split across files"
                            ),
                        );
                    }
                }
                None => admin = Some((a, file)),
            }
        }
        if let Some((m, off)) = p.metrics {
            match &metrics {
                Some((_, first_file, _)) => {
                    let name = ds
                        .get(*first_file)
                        .and_then(Decoder::file)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(d) = ds.get_mut(file) {
                        d.error(
                            off,
                            "metrics",
                            format!(
                                "also given in {name}: only the lists at the top of a config (listeners, routes, upstreams, services, providers) may be split across files"
                            ),
                        );
                    }
                }
                None => metrics = Some((m, file, off)),
            }
        }
        listeners.extend(p.listeners);
        routes.extend(p.routes);
        upstreams.extend(p.upstreams);
    }
    check_listeners(ds, &listeners);
    if listeners.is_empty() && !routes.is_empty() {
        if let Some(d) = first_doc.and_then(|(f, off)| ds.get_mut(f).map(|d| (d, off))) {
            // fit serves such a config over HTTPS by default, with certificates from ACME.
            d.0.not_yet(
                d.1,
                "listeners",
                "a config with no listeners, served over HTTPS by default (auto_https)",
            );
        }
    }
    check_routes(ds, &routes, &listeners);
    check_upstreams(ds, &upstreams);
    if let Some((_, file, off)) = metrics {
        if admin.is_none() {
            if let Some(d) = ds.get_mut(file) {
                // fit's message (config/validate.go).
                d.error(
                    off,
                    "metrics",
                    "the Admin API serves metrics: set admin too",
                );
            }
        }
    }
    let limits = limits.map(|(l, _)| l).unwrap_or_default();
    Config {
        schema_version: schema.map_or(SCHEMA_VERSION, |(v, _)| v),
        listeners: listeners
            .into_iter()
            .map(|a| Listener {
                max_body_bytes: limits.max_body_bytes,
                ..a.v
            })
            .collect(),
        routes: routes.into_iter().map(|a| a.v).collect(),
        upstreams: upstreams.into_iter().map(|a| a.v).collect(),
        limits,
        access_log: access_log.map(|(a, _)| a),
        admin: admin.map(|(a, _)| a),
        metrics: metrics.map(|(m, _, _)| m),
    }
}

/// A decoded element with its file, path and the position of its node.
struct At<T> {
    file: usize,
    path: String,
    off: usize,
    v: T,
}

impl<T> At<T> {
    fn new(file: usize, path: &str, n: &Node, v: T) -> At<T> {
        At {
            file,
            path: path.to_owned(),
            off: n.off,
            v,
        }
    }

    /// Records a problem about this element, in its file.
    fn error(&self, ds: &mut [Decoder<'_>], path: &str, msg: String) {
        if let Some(d) = ds.get_mut(self.file) {
            d.error(self.off, path, msg);
        }
    }
}

fn access_log(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<AccessLog> {
    let mut o = d.object(n, path)?;
    let output = o
        .take("output")
        .and_then(|v| d.string(v, &o.at("output")))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "stdout".to_owned());
    let flush_interval = o
        .take("flush_interval")
        .and_then(|v| d.duration(v, &o.at("flush_interval")))
        .filter(|t| !t.is_zero())
        .unwrap_or(DEFAULT_FLUSH_INTERVAL);
    let buffer_bytes = o
        .take("buffer_bytes")
        .and_then(|v| d.int(v, &o.at("buffer_bytes"), 0, i64::MAX))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(DEFAULT_BUFFER_BYTES);
    let level = match o.take("level") {
        None => AccessLevel::Info,
        Some(v) => {
            let p = o.at("level");
            match d.string(v, &p).as_deref() {
                Some("info") | None => AccessLevel::Info,
                Some("warn") => AccessLevel::Warn,
                Some("error") => AccessLevel::Error,
                Some(other) => {
                    d.error(
                        v.off,
                        &p,
                        format!("{other:?} is not one of \"info\", \"warn\", \"error\""),
                    );
                    AccessLevel::Info
                }
            }
        }
    };
    let format = match o.take("format") {
        None => LogFormat::Json,
        Some(v) => {
            let p = o.at("format");
            match d.string(v, &p).as_deref() {
                Some("json") | None => LogFormat::Json,
                Some("text") => LogFormat::Text,
                Some("line") => LogFormat::Line,
                Some(other) => {
                    d.error(
                        v.off,
                        &p,
                        format!("{other:?} is not one of \"json\", \"text\", \"line\""),
                    );
                    LogFormat::Json
                }
            }
        }
    };
    let time_zone = o
        .take("time_zone")
        .and_then(|v| d.string(v, &o.at("time_zone")))
        .unwrap_or_default();
    o.finish(d, ACCESS_LOG_FIELDS, ACCESS_LOG_FIT_ONLY);
    Some(AccessLog {
        output,
        flush_interval,
        buffer_bytes,
        level,
        format,
        time_zone,
    })
}

fn listener(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Listener> {
    let mut o = d.object(n, path)?;
    let name = o
        .require(d, "name")
        .and_then(|v| d.string(v, &o.at("name")));
    let address = o.require(d, "address").and_then(|v| {
        let p = o.at("address");
        let a = d.string(v, &p)?;
        if let Err(msg) = check_address(&a) {
            d.error(v.off, &p, msg);
        }
        Some(a)
    });
    // Each protocol with its position and path, checked against tls once it is read.
    let mut protocols: Vec<(usize, String, Protocol)> = Vec::new();
    if let Some(v) = o.take("protocols") {
        let p = o.at("protocols");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            let proto = match s.as_str() {
                "http/1.1" => Protocol::Http11,
                "h2c" => Protocol::H2c,
                "h2" => Protocol::H2,
                "h3" => Protocol::H3,
                other if FIT_PROTOCOLS.contains(&other) => {
                    d.not_yet(off, &at, &format!("the {other:?} protocol"));
                    continue;
                }
                other => {
                    d.error(
                        off,
                        &at,
                        format!(
                            "{other:?} is not one of \"http/1.1\", \"h2c\", \"h2\", \"h3\", \"tcp\", \"udp\", \"socks\""
                        ),
                    );
                    continue;
                }
            };
            if protocols.iter().any(|(_, _, q)| *q == proto) {
                d.error(off, &at, format!("{s:?} is listed twice"));
                continue;
            }
            protocols.push((off, at, proto));
        }
    }
    let tls = o.take("tls").and_then(|v| tls(d, v, &o.at("tls")));
    o.finish(d, LISTENER_FIELDS, LISTENER_FIT_ONLY);
    for (off, at, proto) in &protocols {
        match proto {
            Protocol::H2 if tls.is_none() => {
                d.error(*off, at, "h2 needs tls; h2c is HTTP/2 in the clear");
            }
            Protocol::H3 if tls.is_none() => d.error(*off, at, "h3 needs tls"),
            Protocol::H2c if tls.is_some() => d.error(
                *off,
                at,
                "h2c is HTTP/2 in the clear, which a listener with tls does not serve; use h2",
            ),
            _ => {}
        }
    }
    let mut protocols: Vec<Protocol> = protocols.into_iter().map(|(_, _, p)| p).collect();
    if protocols.is_empty() {
        protocols = if tls.is_some() {
            DEFAULT_TLS_PROTOCOLS.to_vec()
        } else {
            DEFAULT_PROTOCOLS.to_vec()
        };
    }
    Some(Listener {
        name: name?,
        address: address?,
        protocols,
        tls,
        // Set from the config's limits once the files are merged.
        max_body_bytes: DEFAULT_MAX_BODY_BYTES,
    })
}

/// `admin`, as fit's `config.Admin` and its `checkAdminAddress`.
fn admin(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Admin> {
    let mut o = d.object(n, path)?;
    let mut a = Admin::default();
    if let Some(v) = o.take("address") {
        let p = o.at("address");
        if let Some(s) = d.string(v, &p).filter(|s| !s.is_empty()) {
            if s.starts_with("unix/") {
                d.not_yet(v.off, &p, "a Unix socket (akio API request A23)");
            } else if let Err(msg) = check_address(&s) {
                d.error(v.off, &p, msg);
            } else if !loopback_host(&s) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{s:?} is not a loopback address: the Admin API can change everything \
                         akit serves, so listen on localhost, 127.0.0.1 or [::1]; off loopback it \
                         needs a token (token_file) and TLS (cert_file and key_file), which akit \
                         does not support yet"
                    ),
                );
            }
            a.address = s;
        }
    }
    // fit's ints, at least 0; 0 is the default.
    fn count(o: &mut Obj<'_>, d: &mut Decoder<'_>, key: &str, slot: &mut usize) {
        if let Some(v) = o.take(key) {
            if let Some(n) = d
                .int(v, &o.at(key), 0, i64::MAX)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n != 0)
            {
                *slot = n;
            }
        }
    }
    count(&mut o, d, "history", &mut a.history);
    count(&mut o, d, "max_config_bytes", &mut a.max_config_bytes);
    count(&mut o, d, "max_conns", &mut a.max_conns);
    o.finish(d, ADMIN_FIELDS, ADMIN_FIT_ONLY);
    Some(a)
}

/// Whether `address`'s host (it is a host and port, [`check_address`]) is `localhost` or a
/// loopback address, as fit's `isLoopback`.
fn loopback_host(address: &str) -> bool {
    let host = address
        .rsplit_once(':')
        .map_or(address, |(h, _)| h)
        .trim_start_matches('[')
        .trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| unmap(ip).is_loopback())
}

fn limits(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Limits> {
    let mut o = d.object(n, path)?;
    let mut l = Limits::default();
    if let Some(v) = o.take("max_body_bytes") {
        // fit's int64, at least 0; 0 is the default.
        match d.int(v, &o.at("max_body_bytes"), 0, i64::MAX) {
            Some(0) | None => {}
            Some(b) => l.max_body_bytes = u64::try_from(b).unwrap_or(DEFAULT_MAX_BODY_BYTES),
        }
    }
    o.finish(d, LIMITS_FIELDS, LIMITS_FIT_ONLY);
    Some(l)
}

fn tls(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Tls> {
    let mut o = d.object(n, path)?;
    let certs_node = o.take("certificates");
    // Whether any is written, read or not: one with a problem is not missing.
    let some_certs =
        certs_node.is_some_and(|v| !matches!(&v.kind, crate::json::Kind::Array(a) if a.is_empty()));
    let certificates = certs_node
        .map(|v| d.array(v, &o.at("certificates"), certificate))
        .unwrap_or_default();
    let client_auth = o
        .take("client_auth")
        .and_then(|v| client_auth(d, v, &o.at("client_auth")));
    let max_version = match o.take("max_version") {
        Some(v) => tls_version(d, v, &o.at("max_version")),
        None => TlsVersion::Tls13,
    };
    let off = o.off;
    // Not taken: finish refuses them as not supported yet.
    let has_acme = o.has("acme") || o.has("on_demand");
    o.finish(d, TLS_FIELDS, TLS_FIT_ONLY);
    if !some_certs && !has_acme {
        // fit obtains certificates for the listener's routes' host names with ACME then
        // (auto_https), or refuses a listener whose routes match none.
        d.not_yet(
            off,
            path,
            "a tls without certificates, whose certificates are obtained with ACME for its routes' host names (auto_https)",
        );
    }
    Some(Tls {
        certificates,
        client_auth,
        max_version,
    })
}

fn certificate(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Certificate> {
    let mut o = d.object(n, path)?;
    let cert_file = o
        .require(d, "cert_file")
        .and_then(|v| d.string(v, &o.at("cert_file")));
    let key_file = o
        .require(d, "key_file")
        .and_then(|v| d.string(v, &o.at("key_file")));
    o.finish(d, CERTIFICATE_FIELDS, &[]);
    Some(Certificate {
        cert_file: cert_file?,
        key_file: key_file?,
    })
}

fn client_auth(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<ClientAuth> {
    let mut o = d.object(n, path)?;
    let ca_files = o.require(d, "ca_files").map(|v| {
        let p = o.at("ca_files");
        let files: Vec<String> = d.strings(v, &p).into_iter().map(|(_, s)| s).collect();
        if files.is_empty() && matches!(v.kind, crate::json::Kind::Array(_)) {
            d.error(v.off, &p, "at least one file is needed");
        }
        files
    });
    let mode = match o.take("mode") {
        Some(v) => match d.string(v, &o.at("mode")).as_deref() {
            Some("require") | None => ClientAuthMode::Require,
            Some("verify_if_given") => ClientAuthMode::VerifyIfGiven,
            Some(other) => {
                d.error(
                    v.off,
                    &o.at("mode"),
                    format!("{other:?} is not one of \"require\", \"verify_if_given\""),
                );
                ClientAuthMode::Require
            }
        },
        None => ClientAuthMode::Require,
    };
    o.finish(d, CLIENT_AUTH_FIELDS, &[]);
    Some(ClientAuth {
        ca_files: ca_files?,
        mode,
    })
}

/// `max_version`: "1.2" or "1.3", quoted or bare, as fit takes it.
fn tls_version(d: &mut Decoder<'_>, n: &Node, path: &str) -> TlsVersion {
    use crate::json::Kind;
    let raw = match &n.kind {
        Kind::Str(s) | Kind::Number(s) => s.as_str(),
        _ => {
            d.error(
                n.off,
                path,
                format!("expected a string, got {}", n.kind.name()),
            );
            return TlsVersion::Tls13;
        }
    };
    match raw {
        "1.3" => TlsVersion::Tls13,
        "1.2" => {
            // akio-tls has no public way to cap a server at TLS 1.2 (akio API request A9).
            d.not_yet(n.off, path, "max_version 1.2 (TLS 1.2 alone)");
            TlsVersion::Tls12
        }
        other => {
            d.error(
                n.off,
                path,
                format!("{other:?} is not one of \"1.2\", \"1.3\""),
            );
            TlsVersion::Tls13
        }
    }
}

fn route(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Route> {
    let mut o = d.object(n, path)?;
    let name = o.take("name").and_then(|v| d.string(v, &o.at("name")));
    let listeners = o
        .take("listeners")
        .map(|v| {
            d.strings(v, &o.at("listeners"))
                .into_iter()
                .map(|(_, s)| s)
                .collect()
        })
        .unwrap_or_default();
    let priority = match o.take("priority") {
        Some(v) => d.int(v, &o.at("priority"), i64::MIN, i64::MAX).unwrap_or(0),
        None => 0,
    };
    let matcher = o
        .take("match")
        .and_then(|v| route_match(d, v, &o.at("match")));
    let middleware = o
        .take("middleware")
        .map(|v| d.array(v, &o.at("middleware"), middleware))
        .unwrap_or_default();
    let handler = o
        .require(d, "handler")
        .and_then(|v| handler(d, v, &o.at("handler")));
    o.finish(d, ROUTE_FIELDS, ROUTE_FIT_ONLY);
    Some(Route {
        name,
        listeners,
        priority,
        matcher,
        middleware,
        handler: handler?,
    })
}

fn route_match(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Match> {
    let mut o = d.object(n, path)?;
    let mut m = Match::default();
    if let Some(v) = o.take("host") {
        let p = o.at("host");
        for (i, (off, h)) in d.strings(v, &p).into_iter().enumerate() {
            if let Err(msg) = check_host(&h) {
                d.error(off, &crate::error::join_index(&p, i), msg);
            }
            m.host.push(h);
        }
    }
    if let Some(v) = o.take("path") {
        let p = o.at("path");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            if !s.starts_with('/') {
                d.error(off, &at, format!("{s:?} does not start with '/'"));
            } else if s.strip_suffix('*').unwrap_or(&s).contains('*') {
                d.error(off, &at, format!("{s:?} has a '*' other than at its end"));
            }
            m.path.push(s);
        }
    }
    // fit's checks are the router's own builders (`config/validate.go`).
    if let Some(v) = o.take("path_regexp") {
        let p = o.at("path_regexp");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            if let Err(e) = akit_router::Matcher::path_regexp(&[&s]) {
                d.error(off, &crate::error::join_index(&p, i), e.to_string());
            }
            m.path_regexp.push(s);
        }
    }
    if let Some(v) = o.take("method") {
        let p = o.at("method");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            if !is_token(&s) {
                d.error(
                    off,
                    &crate::error::join_index(&p, i),
                    format!("{s:?} is not a valid method"),
                );
            }
            m.method.push(s);
        }
    }
    if let Some(v) = o.take("header") {
        m.header = string_lists(d, v, &o.at("header"), |name| {
            (!is_token(name)).then(|| format!("{name:?} is not a valid header name"))
        });
    }
    if let Some(v) = o.take("query") {
        m.query = string_lists(d, v, &o.at("query"), |key| {
            key.is_empty()
                .then(|| "the query parameter's name is empty".to_owned())
        });
    }
    if let Some(v) = o.take("remote_ip") {
        let p = o.at("remote_ip");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            if let Err(e) = akit_router::parse_prefix(&s) {
                d.error(off, &crate::error::join_index(&p, i), e.to_string());
            }
            m.remote_ip.push(s);
        }
    }
    if let Some(v) = o.take("expr") {
        let p = o.at("expr");
        if let Some(s) = d.string(v, &p) {
            if !s.is_empty() {
                if let Err(e) = akit_router::Expr::parse(&s) {
                    d.error(v.off, &p, e.to_string());
                }
            }
            m.expr = s;
        }
    }
    o.finish(d, MATCH_FIELDS, MATCH_FIT_ONLY);
    Some(m)
}

/// An object of string arrays (`map<array<string>>`), in the order written; `check` names what
/// is wrong with a key, reported where its value starts, in the keys' sorted order, as fit
/// reports them.
fn string_lists(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
    check: impl Fn(&str) -> Option<String>,
) -> Vec<(String, Vec<String>)> {
    let crate::json::Kind::Object(members) = &n.kind else {
        // Reports the error.
        let _ = d.object(n, path);
        return Vec::new();
    };
    let mut bad: Vec<(&str, usize, String)> = members
        .iter()
        .filter_map(|m| check(&m.key).map(|msg| (m.key.as_str(), m.value.off, msg)))
        .collect();
    bad.sort_by(|a, b| a.0.cmp(b.0));
    for (key, off, msg) in bad {
        d.error(off, &crate::error::join_key(path, key), msg);
    }
    members
        .iter()
        .map(|m| {
            let values = d
                .strings(&m.value, &crate::error::join_key(path, &m.key))
                .into_iter()
                .map(|(_, s)| s)
                .collect();
            (m.key.clone(), values)
        })
        .collect()
}

fn handler(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Handler> {
    let mut o = d.object(n, path)?;
    let module = o
        .require(d, "module")
        .and_then(|v| d.string(v, &o.at("module")).map(|s| (v.off, s)));
    let params = o.take("params");
    let params_path = o.at("params");
    let module_path = o.at("module");
    o.finish(d, MODULE_FIELDS, &[]);
    let (off, module) = module?;
    match module.as_str() {
        "" => {
            d.error(off, &module_path, "the module name is empty");
            None
        }
        "respond" => respond(d, params, &params_path).map(Handler::Respond),
        "static" => static_files(d, params, off, &params_path).map(Handler::Static),
        "proxy" => proxy(d, params, &params_path, off).map(Handler::Proxy),
        m if FIT_HANDLERS.contains(&m) => {
            d.not_yet(off, &module_path, &format!("the {m:?} handler"));
            None
        }
        m => {
            d.error(
                off,
                &module_path,
                format!("there is no handler module {m:?}"),
            );
            None
        }
    }
}

/// One of a route's `middleware`: a module reference, as a handler's, naming a middleware module.
fn middleware(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Handler> {
    let mut o = d.object(n, path)?;
    let module = o
        .require(d, "module")
        .and_then(|v| d.string(v, &o.at("module")).map(|s| (v.off, s)));
    let params = o.take("params");
    let params_path = o.at("params");
    let module_path = o.at("module");
    o.finish(d, MODULE_FIELDS, &[]);
    let (off, module) = module?;
    match module.as_str() {
        "" => {
            d.error(off, &module_path, "the module name is empty");
            None
        }
        "rewrite" => rewrite(d, params, off, &params_path).map(Handler::Rewrite),
        "redirect" => redirect(d, params, off, &params_path).map(Handler::Redirect),
        "headers" => headers(d, params, off, &params_path).map(Handler::Headers),
        "rate_limit" => rate_limit(d, params, off, &params_path).map(Handler::RateLimit),
        "concurrency_limit" => {
            concurrency_limit(d, params, off, &params_path).map(Handler::ConcurrencyLimit)
        }
        "body_limit" => body_limit(d, params, off, &params_path).map(Handler::BodyLimit),
        m if FIT_MIDDLEWARE.contains(&m) => {
            d.not_yet(off, &module_path, &format!("the {m:?} middleware"));
            None
        }
        m => {
            d.error(
                off,
                &module_path,
                format!("there is no middleware module {m:?}"),
            );
            None
        }
    }
}

/// The `rewrite` middleware's params, checked as fit's `Rewrite.Provision` checks them.
fn rewrite(d: &mut Decoder<'_>, params: Option<&Node>, off: usize, path: &str) -> Option<Rewrite> {
    let mut r = Rewrite::default();
    let mut ok = true;
    let mut at = off;
    let mut regexp = false;
    if let Some(n) = params {
        let mut o: Obj<'_> = d.object(n, path)?;
        at = o.off;
        if let Some(v) = o.take("strip_prefix") {
            let p = o.at("strip_prefix");
            for (off, s) in d.strings(v, &p) {
                if !s.starts_with('/') {
                    d.error(off, &p, format!("{s:?} does not start with '/'"));
                    ok = false;
                }
                r.strip_prefix.push(s);
            }
        }
        if let Some(v) = o.take("add_prefix") {
            let p = o.at("add_prefix");
            r.add_prefix = d.string(v, &p).unwrap_or_default();
            let a = &r.add_prefix;
            if !a.is_empty() && (!a.starts_with('/') || a.ends_with('/')) {
                d.error(
                    v.off,
                    &p,
                    format!("{a:?} does not start with '/', or ends with one"),
                );
                ok = false;
            }
        }
        // Refused as not supported yet, naming the module (the redirect's regexp is read); they
        // still count as what the rewrite does.
        for field in REWRITE_FIT_ONLY {
            if let Some(v) = o.take(field) {
                regexp = true;
                d.not_yet(
                    v.off,
                    &o.at(field),
                    &format!(
                        "the \"rewrite\" middleware's {field}: akio-regex does not find where a pattern matches yet (akio API request A32)"
                    ),
                );
            }
        }
        o.finish(d, REWRITE_FIELDS, REWRITE_FIT_ONLY);
    }
    if ok && !regexp && r.strip_prefix.is_empty() && r.add_prefix.is_empty() {
        d.error(at, path, "strip_prefix, regexp or add_prefix is needed");
        ok = false;
    }
    ok.then_some(r)
}

/// The `redirect` middleware's params, checked as fit's `Redirect.Provision` checks them.
fn redirect(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<Redirect> {
    let Some(n) = params else {
        d.error(
            off,
            &crate::error::join_field(path, "to"),
            "missing required field",
        );
        return None;
    };
    let mut o: Obj<'_> = d.object(n, path)?;
    let mut ok = true;
    let to = o
        .require(d, "to")
        .and_then(|v| d.string(v, &o.at("to")).map(|s| (v.off, s)));
    let mut status = 302;
    if let Some(v) = o.take("status") {
        let p = o.at("status");
        match d.int(v, &p, i64::MIN, i64::MAX) {
            Some(0) => {}
            Some(s @ (301 | 302 | 303 | 307 | 308)) => status = u16::try_from(s).unwrap_or(302),
            Some(s) => {
                d.error(
                    v.off,
                    &p,
                    format!("{s} is not a redirect status: use 301, 302, 303, 307 or 308"),
                );
                ok = false;
            }
            None => ok = false,
        }
    }
    let mut regexp = String::new();
    if let Some(v) = o.take("regexp") {
        let p = o.at("regexp");
        regexp = d.string(v, &p).unwrap_or_default();
        if !regexp.is_empty() {
            if let Err(e) = akit_router::regexp(regexp.as_bytes()) {
                d.error(v.off, &p, e.to_string());
                ok = false;
            }
        }
    }
    let to_path = o.at("to");
    o.finish(d, REDIRECT_FIELDS, &[]);
    let (to_off, to) = to?;
    if to.contains(['\r', '\n', '\0']) {
        d.error(to_off, &to_path, "the URL holds a line break or NUL");
        ok = false;
    } else if !regexp.is_empty() && names_a_group(&to) {
        d.not_yet(
            to_off,
            &to_path,
            "a group of regexp ($1, ${name}) in to: akio-regex does not capture groups yet (akio API request A32)",
        );
        ok = false;
    }
    ok.then_some(Redirect { to, status, regexp })
}

/// Whether the template `to` names a group of a regexp as Go's `Regexp.Expand` reads it: a `$`
/// (not `$$`, which stands for `$`) followed by a name of letters, digits and `_`, or by such a
/// name in braces. Any other `$` stands for itself.
fn names_a_group(to: &str) -> bool {
    let mut rest = to;
    while let Some(i) = rest.find('$') {
        let after = rest.get(i.saturating_add(1)..).unwrap_or("");
        if let Some(r) = after.strip_prefix('$') {
            rest = r;
            continue;
        }
        let (brace, name) = match after.strip_prefix('{') {
            Some(n) => (true, n),
            None => (false, after),
        };
        let len = name
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(name.len());
        if len > 0 && (!brace || name.get(len..).is_some_and(|r| r.starts_with('}'))) {
            return true;
        }
        rest = after;
    }
    false
}

/// The `headers` middleware's params, checked as fit's `Headers.Provision` checks them.
fn headers(d: &mut Decoder<'_>, params: Option<&Node>, off: usize, path: &str) -> Option<Headers> {
    let mut h = Headers::default();
    let mut ok = true;
    let mut at = off;
    if let Some(n) = params {
        let mut o: Obj<'_> = d.object(n, path)?;
        at = o.off;
        if let Some(v) = o.take("request") {
            h.request = header_ops(d, v, &o.at("request"), &mut ok);
        }
        if let Some(v) = o.take("response") {
            h.response = header_ops(d, v, &o.at("response"), &mut ok);
        }
        o.finish(d, HEADERS_FIELDS, &[]);
    }
    if ok && h.request.is_empty() && h.response.is_empty() {
        d.error(at, path, "request or response changes are needed");
        ok = false;
    }
    ok.then_some(h)
}

/// One side's changes of the `headers` middleware; problems are reported at `path`, as fit
/// reports them at `request` or `response`, and clear `ok`.
fn header_ops(d: &mut Decoder<'_>, n: &Node, path: &str, ok: &mut bool) -> HeaderOps {
    let mut ops = HeaderOps::default();
    let Some(mut o) = d.object(n, path) else {
        *ok = false;
        return ops;
    };
    if let Some(v) = o.take("delete") {
        for (off, name) in d.strings(v, &o.at("delete")) {
            let bad = match name.strip_suffix('*') {
                Some(prefix) => (!prefix.is_empty() && !is_token(prefix))
                    .then(|| format!("{name:?} is not a valid header name or prefix")),
                None => (!is_token(&name)).then(|| format!("{name:?} is not a valid header name")),
            };
            if let Some(msg) = bad {
                d.error(off, path, msg);
                *ok = false;
            }
            ops.delete.push(name);
        }
    }
    let check = |d: &mut Decoder<'_>, off: usize, name: &str, value: &str| {
        if !is_token(name) {
            d.error(off, path, format!("{name:?} is not a valid header name"));
            false
        } else if value.contains(['\r', '\n', '\0']) {
            d.error(
                off,
                path,
                format!("the value of {name} holds a line break or NUL"),
            );
            false
        } else {
            true
        }
    };
    if let Some(v) = o.take("set") {
        let mut set = d.string_map(v, &o.at("set"));
        set.sort_by(|a, b| a.1.cmp(&b.1));
        for (off, name, value) in set {
            *ok &= check(d, off, &name, &value);
            ops.set.push((name, value));
        }
    }
    if let Some(v) = o.take("add") {
        let at = o.at("add");
        let mut add = string_lists(d, v, &at, |_| None);
        add.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, values) in add {
            for value in &values {
                // The position of the object: string_lists keeps no positions of its keys.
                *ok &= check(d, v.off, &name, value);
            }
            ops.add.push((name, values));
        }
    }
    o.finish(d, HEADER_OPS_FIELDS, &[]);
    ops
}

/// A limit middleware's params object, or the "missing required field" problem of `required`
/// when there is none.
fn limit_params<'n>(
    d: &mut Decoder<'_>,
    params: Option<&'n Node>,
    off: usize,
    path: &str,
    required: &str,
) -> Option<Obj<'n>> {
    let Some(n) = params else {
        d.error(
            off,
            &crate::error::join_field(path, required),
            "missing required field",
        );
        return None;
    };
    d.object(n, path)
}

/// A refusal's `status`: 0 for `default`, else an error status (400 to 599), as fit's limits
/// check it.
fn error_status(d: &mut Decoder<'_>, o: &mut Obj<'_>, default: u16, ok: &mut bool) -> u16 {
    let Some(v) = o.take("status") else {
        return default;
    };
    match d.int(v, &o.at("status"), i64::MIN, i64::MAX) {
        Some(0) => default,
        Some(s @ 400..=599) => u16::try_from(s).unwrap_or(default),
        Some(s) => {
            d.error(
                v.off,
                &o.at("status"),
                format!("{s} is not an error status"),
            );
            *ok = false;
            default
        }
        None => {
            *ok = false;
            default
        }
    }
}

/// The `rate_limit` middleware's params, checked as fit's `Rate.Provision` checks them.
fn rate_limit(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<RateLimit> {
    let mut o = limit_params(d, params, off, path, "rate")?;
    let mut ok = true;
    let rate = o.require(d, "rate").and_then(|v| {
        let p = o.at("rate");
        let r = d.number(v, &p)?;
        if r < 0.001 {
            d.error(v.off, &p, format!("{r} is below 0.001"));
            return None;
        }
        Some(r)
    });
    let mut burst = 0;
    if let Some(v) = o.take("burst") {
        match d.int(v, &o.at("burst"), 0, i64::MAX) {
            Some(b) => burst = u64::try_from(b).unwrap_or(0),
            None => ok = false,
        }
    }
    let mut key = RateKey::Ip;
    if let Some(v) = o.take("key") {
        let p = o.at("key");
        match d.string(v, &p) {
            Some(k) => match k.as_str() {
                "" | "ip" => {}
                "route" => key = RateKey::Route,
                _ => match k.strip_prefix("header:").filter(|n| is_token(n)) {
                    Some(name) => key = RateKey::Header(name.to_owned()),
                    None => {
                        d.error(v.off, &p, format!("{k:?} is not ip, route or header:Name"));
                        ok = false;
                    }
                },
            },
            None => ok = false,
        }
    }
    let mut trusted_proxies = Vec::new();
    if let Some(v) = o.take("trusted_proxies") {
        let at = o.at("trusted_proxies");
        for (i, (off, s)) in d.strings(v, &at).into_iter().enumerate() {
            match Network::parse(&s) {
                Some(net) => trusted_proxies.push(net),
                None => {
                    ok = false;
                    d.error(
                        off,
                        &crate::error::join_index(&at, i),
                        format!("{s:?} is not an IP address or a CIDR prefix"),
                    );
                }
            }
        }
    }
    let mut ipv6_prefix = 64;
    if let Some(v) = o.take("ipv6_prefix") {
        match d.int(v, &o.at("ipv6_prefix"), 0, 128) {
            Some(0) => {}
            Some(b) => ipv6_prefix = u8::try_from(b).unwrap_or(64),
            None => ok = false,
        }
    }
    let mut max_keys = DEFAULT_RATE_MAX_KEYS;
    if let Some(v) = o.take("max_keys") {
        match d.int(v, &o.at("max_keys"), 0, i64::MAX) {
            Some(0) => {}
            Some(n) => max_keys = u64::try_from(n).unwrap_or(DEFAULT_RATE_MAX_KEYS),
            None => ok = false,
        }
    }
    let status = error_status(d, &mut o, 429, &mut ok);
    o.finish(d, RATE_LIMIT_FIELDS, &[]);
    let rate = rate?;
    if burst == 0 {
        // The rate rounded up, as fit's int(rate + 0.999); a float's cast saturates.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = (rate + 0.999) as u64;
        burst = b.max(1);
    }
    ok.then_some(RateLimit {
        rate,
        burst,
        key,
        trusted_proxies,
        ipv6_prefix,
        max_keys,
        status,
    })
}

/// The `concurrency_limit` middleware's params, checked as fit's `Concurrency.Provision` checks
/// them.
fn concurrency_limit(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<ConcurrencyLimit> {
    let mut o = limit_params(d, params, off, path, "max")?;
    let mut ok = true;
    let max = o
        .require(d, "max")
        .and_then(|v| d.int(v, &o.at("max"), 1, i64::MAX))
        .and_then(|m| u64::try_from(m).ok());
    let status = error_status(d, &mut o, 429, &mut ok);
    o.finish(d, CONCURRENCY_LIMIT_FIELDS, &[]);
    let max = max?;
    ok.then_some(ConcurrencyLimit { max, status })
}

/// The `body_limit` middleware's params.
fn body_limit(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<BodyLimit> {
    let mut o = limit_params(d, params, off, path, "max_bytes")?;
    let max_bytes = o
        .require(d, "max_bytes")
        .and_then(|v| d.int(v, &o.at("max_bytes"), 0, i64::MAX))
        .and_then(|m| u64::try_from(m).ok());
    o.finish(d, BODY_LIMIT_FIELDS, &[]);
    Some(BodyLimit {
        max_bytes: max_bytes?,
    })
}

fn respond(d: &mut Decoder<'_>, params: Option<&Node>, path: &str) -> Option<Respond> {
    let mut r = Respond {
        status: 200,
        headers: Vec::new(),
        body: String::new(),
    };
    let Some(n) = params else {
        return Some(r);
    };
    let mut o: Obj<'_> = d.object(n, path)?;
    let mut status_off = o.off;
    if let Some(v) = o.take("status") {
        status_off = v.off;
        let s = d.int(v, &o.at("status"), 100, 599);
        r.status = s.and_then(|s| u16::try_from(s).ok()).unwrap_or(200);
        if s.is_some() && r.status < 200 {
            d.error(
                v.off,
                &o.at("status"),
                format!(
                    "{} is an informational status, which cannot be a final response",
                    r.status
                ),
            );
        }
    }
    if let Some(v) = o.take("headers") {
        let p = o.at("headers");
        for (off, name, value) in d.string_map(v, &p) {
            if !is_token(&name) {
                d.error(off, &p, format!("{name:?} is not a valid header name"));
            } else if value.contains(['\r', '\n', '\0']) {
                d.error(
                    off,
                    &p,
                    format!("the value of {name} holds a line break or NUL"),
                );
            }
            r.headers.push((name, value));
        }
    }
    if let Some(v) = o.take("body") {
        r.body = d.string(v, &o.at("body")).unwrap_or_default();
        if !r.body.is_empty() && matches!(r.status, 204 | 304) {
            d.error(
                status_off,
                &o.at("body"),
                format!("a {} response cannot have a body", r.status),
            );
        }
    }
    o.finish(d, RESPOND_FIELDS, &[]);
    Some(r)
}

fn static_files(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    module_off: usize,
    path: &str,
) -> Option<Static> {
    let Some(n) = params else {
        d.error(
            module_off,
            &crate::error::join_field(path, "root"),
            "missing required field",
        );
        return None;
    };
    let mut o: Obj<'_> = d.object(n, path)?;
    let root = o.require(d, "root").and_then(|v| {
        let p = o.at("root");
        let s = d.string(v, &p)?;
        if s.is_empty() {
            d.error(v.off, &p, "the root is empty");
        }
        Some(s)
    });
    let dotfiles = match o.take("dotfiles") {
        None => Dotfiles::Deny,
        Some(v) => {
            let p = o.at("dotfiles");
            match d.string(v, &p).as_deref() {
                Some("deny") | None => Dotfiles::Deny,
                Some("allow") => Dotfiles::Allow,
                Some(other) => {
                    d.error(
                        v.off,
                        &p,
                        format!("{other:?} is not one of \"deny\", \"allow\""),
                    );
                    Dotfiles::Deny
                }
            }
        }
    };
    let mut index = String::from("index.html");
    if let Some(v) = o.take("index") {
        let p = o.at("index");
        let names = d.strings(v, &p);
        for (i, (off, name)) in names.iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
                d.error(*off, &at, format!("{name:?} is not a file name"));
            } else if dotfiles == Dotfiles::Deny && name.starts_with('.') {
                d.error(
                    *off,
                    &at,
                    format!(
                        "{name:?} starts with a dot, which dotfiles deny hides; set dotfiles to allow to serve it"
                    ),
                );
            }
        }
        match names.as_slice() {
            [(_, name)] => index.clone_from(name),
            // An array of strings with a problem was reported already.
            _ if !matches!(&v.kind, crate::json::Kind::Array(_)) => {}
            [] => d.not_yet(v.off, &p, "an empty index list, which serves no index file"),
            _ => d.not_yet(
                v.off,
                &p,
                "an index list of more than one name, tried in order",
            ),
        }
    }
    let max_file_bytes = o
        .take("max_file_bytes")
        .and_then(|v| d.int(v, &o.at("max_file_bytes"), 0, i64::MAX))
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(DEFAULT_MAX_FILE_BYTES);
    if let Some(v) = o.take("disk_io") {
        // akit looks at the disk on akio's file pool only (rules.md rule 4): "pool", fit's
        // default. fit's "auto" looks at it on the worker serving the request.
        let p = o.at("disk_io");
        match d.string(v, &p).as_deref() {
            Some("pool") | None => {}
            Some("auto") => d.not_yet(
                v.off,
                &p,
                "\"auto\", which looks at the disk on the thread serving the request",
            ),
            Some(other) => d.error(
                v.off,
                &p,
                format!("{other:?} is not one of \"pool\", \"auto\""),
            ),
        }
    }
    let check_interval = match o.take("check_interval") {
        None => DEFAULT_CHECK_INTERVAL,
        Some(v) => d
            .duration(v, &o.at("check_interval"))
            .unwrap_or(DEFAULT_CHECK_INTERVAL),
    };
    let precompressed = o
        .take("precompressed")
        .map(|v| {
            let p = o.at("precompressed");
            d.strings(v, &p)
                .into_iter()
                .enumerate()
                .filter_map(|(i, (off, name))| match name.as_str() {
                    "br" => Some(Encoding::Br),
                    "zstd" => Some(Encoding::Zstd),
                    "gzip" => Some(Encoding::Gzip),
                    _ => {
                        d.error(
                            off,
                            &crate::error::join_index(&p, i),
                            format!("{name:?} is not one of \"br\", \"zstd\", \"gzip\""),
                        );
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mut try_files = None;
    if let Some(v) = o.take("try_files") {
        let list: &mut Vec<String> = try_files.get_or_insert_default();
        let p = o.at("try_files");
        for (i, (off, t)) in d.strings(v, &p).into_iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            if !t.starts_with('/') && !t.starts_with("{path}") {
                d.error(
                    off,
                    &at,
                    format!("{t:?} starts with neither / nor {{path}}"),
                );
            } else if dotfiles == Dotfiles::Deny && hides_dotfile(&t.replace("{path}", "/x")) {
                d.error(
                    off,
                    &at,
                    format!(
                        "{t:?} names a path with a segment starting with a dot, which dotfiles deny hides; set dotfiles to allow to serve it"
                    ),
                );
            } else {
                list.push(t);
            }
        }
    }
    let mut strip_prefix = String::new();
    if let Some(v) = o.take("strip_prefix") {
        let p = o.at("strip_prefix");
        if let Some(s) = d.string(v, &p) {
            if !s.is_empty() && !s.starts_with('/') {
                d.error(v.off, &p, format!("{s:?} does not start with /"));
            }
            strip_prefix = s;
        }
    }
    let browse = match o.take("browse") {
        None => false,
        Some(v) => match &v.kind {
            crate::json::Kind::Bool(b) => *b,
            other => {
                d.error(
                    v.off,
                    &o.at("browse"),
                    format!("expected a boolean, got {}", other.name()),
                );
                false
            }
        },
    };
    let browse_max_entries = o
        .take("browse_max_entries")
        .and_then(|v| d.int(v, &o.at("browse_max_entries"), 0, i64::MAX))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(DEFAULT_BROWSE_MAX_ENTRIES);
    o.finish(d, STATIC_FIELDS, STATIC_FIT_ONLY);
    Some(Static {
        root: root?,
        index,
        max_file_bytes,
        dotfiles,
        check_interval,
        precompressed,
        strip_prefix,
        try_files,
        browse,
        browse_max_entries,
    })
}

/// The `proxy` handler's params, with fit's checks and messages (its `Provision`), but for the
/// upstream's name, which akit-runtime's `compile` looks up. `off` is where
/// the handler's module is named, for a handler with no params.
fn proxy(d: &mut Decoder<'_>, params: Option<&Node>, path: &str, off: usize) -> Option<Proxy> {
    let mut p = Proxy {
        upstream: String::new(),
        trusted_proxies: Vec::new(),
        host_header: String::new(),
    };
    let upstream_path = crate::error::join_field(path, "upstream");
    let Some(n) = params else {
        d.error(off, &upstream_path, "upstream or upstreams is needed");
        return None;
    };
    let mut o: Obj<'_> = d.object(n, path)?;
    let mut ok = true;
    let mut upstream_off = o.off;
    if let Some(v) = o.take("upstream") {
        upstream_off = v.off;
        p.upstream = d.string(v, &o.at("upstream")).unwrap_or_default();
    }
    if let Some(v) = o.take("trusted_proxies") {
        let at = o.at("trusted_proxies");
        for (i, (off, s)) in d.strings(v, &at).into_iter().enumerate() {
            if let Some(net) = Network::parse(&s) {
                p.trusted_proxies.push(net);
            } else {
                ok = false;
                d.error(
                    off,
                    &crate::error::join_index(&at, i),
                    format!(
                        "{s:?} is not an IP address or a network, as in \"10.0.0.1\" or \"10.0.0.0/8\""
                    ),
                );
            }
        }
    }
    if let Some(v) = o.take("host_header") {
        let at = o.at("host_header");
        p.host_header = d.string(v, &at).unwrap_or_default();
        if p.host_header.contains([' ', '\t', '\r', '\n', '\0', '/']) {
            ok = false;
            d.error(v.off, &at, format!("{:?} is not a host", p.host_header));
        }
    }
    // Leaving both out is fit's error; upstreams alone is refused as not supported yet.
    let upstreams = o.has("upstreams");
    o.finish(d, PROXY_FIELDS, PROXY_FIT_ONLY);
    if p.upstream.is_empty() && !upstreams {
        d.error(
            upstream_off,
            &upstream_path,
            "upstream or upstreams is needed",
        );
        ok = false;
    }
    ok.then_some(p)
}

fn upstream(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Upstream> {
    let mut o = d.object(n, path)?;
    let name = o
        .require(d, "name")
        .and_then(|v| d.string(v, &o.at("name")));
    let addresses = o.require(d, "addresses").map(|v| {
        let p = o.at("addresses");
        let list = d.strings(v, &p);
        if list.is_empty() && matches!(v.kind, crate::json::Kind::Array(_)) {
            d.error(v.off, &p, "an upstream needs at least one address");
        }
        let mut seen: Vec<&str> = Vec::new();
        for (i, (off, a)) in list.iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            if let Err(msg) = check_upstream_address(a) {
                d.error(*off, &at, msg);
            } else if seen.contains(&a.as_str()) {
                d.error(*off, &at, format!("{a:?} is listed twice"));
            }
            seen.push(a);
        }
        list.into_iter().map(|(_, a)| a).collect::<Vec<_>>()
    });
    if let Some(v) = o.take("protocol") {
        let p = o.at("protocol");
        match d.string(v, &p).as_deref() {
            Some("http/1.1") | None => {}
            Some(other @ ("h2c" | "h2")) => {
                d.not_yet(v.off, &p, &format!("the {other:?} protocol for upstreams"));
            }
            Some(other) => d.error(
                v.off,
                &p,
                format!("{other:?} is not one of \"http/1.1\", \"h2c\", \"h2\""),
            ),
        }
    }
    let count = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, zero: usize| {
        let v = o.take(key)?;
        let n = d.int(v, &o.at(key), 0, i64::MAX)?;
        Some(match usize::try_from(n) {
            Ok(0) => zero,
            Ok(n) => n,
            Err(_) => usize::MAX,
        })
    };
    let max_conns = count(d, &mut o, "max_conns", DEFAULT_MAX_CONNS).unwrap_or(DEFAULT_MAX_CONNS);
    let max_pending =
        count(d, &mut o, "max_pending", DEFAULT_MAX_PENDING).unwrap_or(DEFAULT_MAX_PENDING);
    // 0 keeps no idle connection: only leaving it out means the default.
    let max_idle_conns = count(d, &mut o, "max_idle_conns", 0).unwrap_or(DEFAULT_MAX_IDLE_CONNS);
    let mut duration = |d: &mut Decoder<'_>, key: &str, default: Duration| {
        let v = o.take(key);
        let p = o.at(key);
        v.and_then(|v| d.duration(v, &p))
            .filter(|t| !t.is_zero())
            .unwrap_or(default)
    };
    let idle_timeout = duration(d, "idle_timeout", DEFAULT_IDLE_TIMEOUT);
    let connect_timeout = duration(d, "connect_timeout", DEFAULT_CONNECT_TIMEOUT);
    let fail_timeout = duration(d, "fail_timeout", DEFAULT_FAIL_TIMEOUT);
    // 0 keeps every address in rotation: only leaving it out means the default.
    let max_fails = count(d, &mut o, "max_fails", 0).unwrap_or(DEFAULT_MAX_FAILS);
    let weights_at = o.take("weights").map(|v| {
        let p = o.at("weights");
        let w = d.array(v, &p, |d, item, at| {
            d.int(item, at, 1, i64::from(MAX_WEIGHT))
                .and_then(|w| u32::try_from(w).ok())
        });
        let given = match &v.kind {
            crate::json::Kind::Array(items) => items.len(),
            _ => 0,
        };
        (v.off, p, given, w)
    });
    // fit's policies, as its `load_balancing` enum lists them; those akit lacks are not
    // supported yet.
    let policy = o.take("load_balancing").map(|v| {
        let p = o.at("load_balancing");
        let lb = match d.string(v, &p).as_deref() {
            Some("round_robin") => Some(LoadBalancing::RoundRobin),
            Some("weighted_round_robin") => Some(LoadBalancing::WeightedRoundRobin),
            Some("least_conn") => Some(LoadBalancing::LeastConn),
            Some(other @ ("random" | "p2c" | "ip_hash" | "uri_hash" | "header_hash" | "cookie_hash")) => {
                d.not_yet(v.off, &p, &format!("the {other:?} load balancing policy"));
                None
            }
            Some(other) => {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{other:?} is not one of \"round_robin\", \"weighted_round_robin\", \"least_conn\", \"random\", \"p2c\", \"ip_hash\", \"uri_hash\", \"header_hash\", \"cookie_hash\""
                    ),
                );
                None
            }
            None => None,
        };
        (v.off, p, lb)
    });
    let health_check = o
        .take("health_check")
        .and_then(|v| health_check(d, v, &o.at("health_check")));
    o.finish(d, UPSTREAM_FIELDS, UPSTREAM_FIT_ONLY);
    // fit's checks of the two together (config.validate).
    let mut weights = Vec::new();
    let mut given = 0;
    if let Some((off, p, n, w)) = weights_at {
        given = n;
        if let Some(a) = &addresses {
            if given > 0 && given != a.len() {
                d.error(
                    off,
                    &p,
                    format!(
                        "{given} weights for {} addresses; give one weight for each address",
                        a.len()
                    ),
                );
            }
        }
        weights = w;
    }
    let load_balancing = match policy {
        Some((off, p, Some(LoadBalancing::RoundRobin))) if given > 0 => {
            d.error(
                off,
                &p,
                "round_robin takes no weights; use weighted_round_robin or least_conn",
            );
            LoadBalancing::RoundRobin
        }
        Some((_, _, Some(lb))) => lb,
        _ if weights.iter().any(|&w| w != 1) => LoadBalancing::WeightedRoundRobin,
        _ => LoadBalancing::RoundRobin,
    };
    Some(Upstream {
        name: name?,
        addresses: addresses?,
        max_conns,
        max_pending,
        max_idle_conns,
        idle_timeout,
        connect_timeout,
        weights,
        load_balancing,
        max_fails,
        fail_timeout,
        health_check,
    })
}

/// An upstream's `health_check`, with fit's checks (config.validate): the http fields refused on
/// a tcp check, a path starting with `/`, a host name or IP address, a timeout no longer than the
/// interval and a jitter less than it. A problem with a field left out is reported where the
/// check's object starts, as fit reports it.
fn health_check(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<HealthCheck> {
    let mut o = d.object(n, path)?;
    let mut ok = true;
    let kind = match o.take("type") {
        Some(v) => match d.string(v, &o.at("type")).as_deref() {
            Some("http") => HealthCheckType::Http,
            Some("tcp") => HealthCheckType::Tcp,
            Some(other) => {
                d.error(
                    v.off,
                    &o.at("type"),
                    format!("{other:?} is not one of \"http\", \"tcp\""),
                );
                ok = false;
                HealthCheckType::Http
            }
            None => {
                ok = false;
                HealthCheckType::Http
            }
        },
        None => HealthCheckType::Http,
    };
    let mut string = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        let v = o.take(key)?;
        let s = d.string(v, &o.at(key));
        ok &= s.is_some();
        Some((v.off, s.unwrap_or_default()))
    };
    let path_at = string(d, &mut o, "path");
    let host_at = string(d, &mut o, "host");
    let port = o.take("port").and_then(|v| {
        let n = d.int(v, &o.at("port"), 1, 65_535);
        ok &= n.is_some();
        n.and_then(|n| u16::try_from(n).ok())
    });
    let status_at = o.take("expect_status").map(|v| {
        let p = o.at("expect_status");
        let given = match &v.kind {
            crate::json::Kind::Array(items) => items.len(),
            _ => usize::MAX,
        };
        let list = d.array(v, &p, |d, item, at| {
            d.int(item, at, 100, 599)
                .and_then(|s| u16::try_from(s).ok())
        });
        ok &= list.len() == given;
        (v.off, list)
    });
    let mut duration = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        let v = o.take(key)?;
        let t = d.duration(v, &o.at(key));
        ok &= t.is_some();
        Some((v.off, t.unwrap_or_default()))
    };
    let interval_at = duration(d, &mut o, "interval");
    let timeout_at = duration(d, &mut o, "timeout");
    let jitter_at = duration(d, &mut o, "jitter");
    let mut count = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        let v = o.take(key)?;
        let n = d.int(v, &o.at(key), 0, i64::MAX);
        ok &= n.is_some();
        n.map(|n| usize::try_from(n).unwrap_or(usize::MAX))
    };
    let fails = count(d, &mut o, "fails").filter(|&n| n != 0);
    let passes = count(d, &mut o, "passes").filter(|&n| n != 0);
    let obj_off = o.off;
    let at = |key: &str| crate::error::join_field(path, key);
    o.finish(d, HEALTH_CHECK_FIELDS, &[]);
    if kind == HealthCheckType::Tcp {
        let http_only = [
            ("path", path_at.as_ref().map(|(off, _)| *off)),
            ("host", host_at.as_ref().map(|(off, _)| *off)),
            ("expect_status", status_at.as_ref().map(|(off, _)| *off)),
        ];
        for (name, off) in http_only {
            if let Some(off) = off {
                d.error(
                    off,
                    &at(name),
                    format!("{name} is for http health checks; a tcp one only opens a connection"),
                );
                ok = false;
            }
        }
    }
    if let Some((off, p)) = &path_at {
        if !p.is_empty() && !p.starts_with('/') {
            d.error(*off, &at("path"), format!("{p:?} does not start with '/'"));
            ok = false;
        }
    }
    if let Some((off, h)) = &host_at {
        if !h.is_empty() && (h.starts_with('*') || check_host(h).is_err()) {
            d.error(
                *off,
                &at("host"),
                format!("{h:?} is not a host name or IP address"),
            );
            ok = false;
        }
    }
    let given = |v: &Option<(usize, Duration)>| v.map(|(_, t)| t).filter(|t| !t.is_zero());
    let interval = given(&interval_at).unwrap_or(DEFAULT_HEALTH_INTERVAL);
    let timeout = given(&timeout_at).unwrap_or(DEFAULT_HEALTH_TIMEOUT);
    let jitter = given(&jitter_at).unwrap_or(interval.checked_div(10).unwrap_or_default());
    let show = crate::duration::format;
    if timeout > interval {
        d.error(
            timeout_at.map_or(obj_off, |(off, _)| off),
            &at("timeout"),
            format!(
                "the timeout, {}, is longer than the interval, {}",
                show(timeout),
                show(interval)
            ),
        );
        ok = false;
    }
    if jitter >= interval {
        d.error(
            jitter_at.map_or(obj_off, |(off, _)| off),
            &at("jitter"),
            format!(
                "the jitter, {}, is not less than the interval, {}",
                show(jitter),
                show(interval)
            ),
        );
        ok = false;
    }
    let path = path_at
        .map(|(_, p)| p)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "/".to_owned());
    ok.then(|| HealthCheck {
        kind,
        path,
        host: host_at.map(|(_, h)| h).filter(|h| !h.is_empty()),
        port,
        expect_status: status_at.map(|(_, l)| l).unwrap_or_default(),
        interval,
        timeout,
        jitter,
        fails: fails.unwrap_or(DEFAULT_HEALTH_THRESHOLD),
        passes: passes.unwrap_or(DEFAULT_HEALTH_THRESHOLD),
    })
}

/// What is wrong with `addr` as an upstream's address, as fit's `checkUpstreamAddress` says it:
/// a host and a port from 1 to 65535.
fn check_upstream_address(addr: &str) -> Result<(), String> {
    if addr.contains("://") {
        return Err(format!(
            "{addr} is a URL; write the host and port alone, as in \"10.0.0.1:8080\""
        ));
    }
    let (host, port) = split_host_port(addr)
        .ok_or_else(|| format!("{addr:?} is not a host and port, as in \"10.0.0.1:8080\""))?;
    if host.is_empty() {
        return Err(format!("{addr:?} has no host"));
    }
    let digits = port.strip_prefix('+').unwrap_or(port);
    match digits.parse::<u32>() {
        Ok(1..=65535) if digits.bytes().all(|b| b.is_ascii_digit()) => Ok(()),
        _ => Err(format!("{addr:?} has no valid port")),
    }
}

/// Go's `net.SplitHostPort`: `host:port` or `[host]:port`, the host holding no `:` unless
/// bracketed.
fn split_host_port(a: &str) -> Option<(&str, &str)> {
    if let Some(rest) = a.strip_prefix('[') {
        let (h, p) = rest.split_once(']')?;
        let p = p.strip_prefix(':')?;
        if h.contains(['[', ']']) || p.contains(['[', ']']) {
            return None;
        }
        return Some((h, p));
    }
    let (h, p) = a.rsplit_once(':')?;
    if h.contains([':', '[', ']']) || p.contains(['[', ']']) {
        return None;
    }
    Some((h, p))
}

fn check_upstreams(ds: &mut [Decoder<'_>], upstreams: &[At<Upstream>]) {
    let mut seen: Vec<&str> = Vec::new();
    for u in upstreams {
        let path = crate::error::join_field(&u.path, "name");
        if !valid_name(&u.v.name) {
            u.error(ds, &path, bad_name(&u.v.name));
        } else if seen.contains(&u.v.name.as_str()) {
            u.error(
                ds,
                &path,
                format!("another upstream is already named {:?}", u.v.name),
            );
        }
        seen.push(&u.v.name);
    }
}

fn check_listeners(ds: &mut [Decoder<'_>], listeners: &[At<Listener>]) {
    let mut seen: Vec<&str> = Vec::new();
    for l in listeners {
        let path = crate::error::join_field(&l.path, "name");
        if !valid_name(&l.v.name) {
            l.error(ds, &path, bad_name(&l.v.name));
        } else if seen.contains(&l.v.name.as_str()) {
            l.error(
                ds,
                &path,
                format!("another listener is already named {:?}", l.v.name),
            );
        }
        seen.push(&l.v.name);
    }
}

fn check_routes(ds: &mut [Decoder<'_>], routes: &[At<Route>], listeners: &[At<Listener>]) {
    let mut seen: Vec<&str> = Vec::new();
    for r in routes {
        if let Some(name) = r.v.name.as_deref().filter(|s| !s.is_empty()) {
            let path = crate::error::join_field(&r.path, "name");
            if !valid_name(name) {
                r.error(ds, &path, bad_name(name));
            } else if seen.contains(&name) {
                r.error(
                    ds,
                    &path,
                    format!("another route is already named {name:?}"),
                );
            }
            seen.push(name);
        }
        for (j, l) in r.v.listeners.iter().enumerate() {
            if !listeners.iter().any(|x| &x.v.name == l) {
                let path =
                    crate::error::join_index(&crate::error::join_field(&r.path, "listeners"), j);
                r.error(ds, &path, format!("there is no listener named {l:?}"));
            }
        }
    }
}

/// fit's names: letters, digits, '_', '.' and '-', starting with a letter or digit.
fn valid_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

fn bad_name(s: &str) -> String {
    format!(
        "{s:?} is not a valid name: use letters, digits, '_', '.' and '-', starting with a letter or digit"
    )
}

/// A host and port, as Go's `net.SplitHostPort` and fit's `checkAddress` take them.
fn check_address(a: &str) -> Result<(), String> {
    let bad = || format!("{a:?} is not a host and port, as in \":8080\" or \"127.0.0.1:8080\"");
    let (host, port) = if let Some(rest) = a.strip_prefix('[') {
        let (h, p) = rest.split_once(']').ok_or_else(bad)?;
        (h, p.strip_prefix(':').ok_or_else(bad)?)
    } else {
        let (h, p) = a.rsplit_once(':').ok_or_else(bad)?;
        if h.contains(':') {
            return Err(bad());
        }
        (h, p)
    };
    if host.contains(['[', ']']) {
        return Err(bad());
    }
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err()
    {
        return Err(format!("{port:?} is not a port between 0 and 65535"));
    }
    Ok(())
}

/// A route's host: a name, an IP address, or a name under a `*.` (one label) or `**.` (one or
/// more labels) wildcard, as fit's `checkHost` takes it.
fn check_host(host: &str) -> Result<(), String> {
    let h = host
        .strip_prefix("**.")
        .map_or(host.to_owned(), |rest| format!("*.{rest}"));
    let wild = h.starts_with("*.");
    let name = h.strip_prefix("*.").unwrap_or(&h);
    if name.is_empty() {
        return Err("the host is empty".into());
    }
    if name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() {
        return if wild {
            Err(format!("{host:?} puts a wildcard on an IP address"))
        } else {
            Ok(())
        };
    }
    let label_ok = |l: &str| {
        let b = l.as_bytes();
        !b.is_empty()
            && b.first().is_some_and(u8::is_ascii_alphanumeric)
            && b.last().is_some_and(u8::is_ascii_alphanumeric)
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
    };
    if name.split('.').all(label_ok) {
        Ok(())
    } else {
        Err(format!("{host:?} is not a valid host name"))
    }
}

/// An HTTP token (RFC 9110 §5.6.2), what a header name must be.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        for ok in [
            ":8080",
            "127.0.0.1:80",
            "[::1]:443",
            "localhost:0",
            "example.com:65535",
        ] {
            assert_eq!(check_address(ok), Ok(()), "{ok}");
        }
        for bad in ["8080", "::1:80", "a:b:c", "[::1]80", "[::1"] {
            assert!(
                check_address(bad).unwrap_err().contains("host and port"),
                "{bad}"
            );
        }
        assert!(
            check_address(":65536")
                .unwrap_err()
                .contains("port between")
        );
        assert!(check_address(":http").unwrap_err().contains("port between"));
        assert!(check_address(":").unwrap_err().contains("port between"));
    }

    #[test]
    fn hosts() {
        for ok in [
            "a.example",
            "*.example.com",
            "**.example.com",
            "192.0.2.1",
            "[::1]",
            "x-1.y",
        ] {
            assert_eq!(check_host(ok), Ok(()), "{ok}");
        }
        assert_eq!(check_host("*."), Err("the host is empty".into()));
        assert!(
            check_host("*.192.0.2.1")
                .unwrap_err()
                .contains("wildcard on an IP")
        );
        for bad in ["-a.example", "a..b", "a_b.c", "a*.b"] {
            assert!(
                check_host(bad).unwrap_err().contains("not a valid host"),
                "{bad}"
            );
        }
    }

    #[test]
    fn names_and_tokens() {
        assert!(valid_name("web-1.a_b"));
        assert!(!valid_name("-web"));
        assert!(!valid_name(""));
        assert!(is_token("X-Custom"));
        assert!(!is_token("X Custom"));
        assert!(!is_token(""));
    }
}
