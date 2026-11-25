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
    /// `acme`: how certificates for the names listeners list in `tls.acme` are obtained (I31);
    /// the defaults when left out. Read when akit starts: a change takes a restart, as in fit.
    pub acme: Option<Acme>,
    /// `auto_https`: HTTPS by default (I33), on when left out; what it made of the listeners is
    /// already in [`listeners`](Self::listeners).
    pub auto_https: Option<crate::AutoHttps>,
    /// `services`: layer 4 services (I51), which proxy the connections of `tcp` listeners and
    /// the sessions of `udp` listeners to an upstream.
    pub services: Vec<Service>,
    /// `telemetry`: OpenTelemetry (I64), traces of requests and metrics exported over OTLP/HTTP;
    /// neither when left out. Read when akit starts: a change takes a restart, as in fit.
    pub telemetry: Option<Telemetry>,
    /// `providers`: sources of config other than the files and the Admin API (I57, I58), merged
    /// into it. Read when akit starts: a change takes a restart, as in fit.
    pub providers: Vec<crate::providers::Provider>,
}

/// fit's default `acme.directory`: Let's Encrypt's.
pub const DEFAULT_ACME_DIRECTORY: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// fit's default `acme.max_concurrent`.
pub const DEFAULT_ACME_MAX_CONCURRENT: usize = 2;
/// fit's default `acme.timeout`: how long obtaining one certificate may take.
pub const DEFAULT_ACME_TIMEOUT: Duration = Duration::from_secs(600);
/// fit's default `acme.request_timeout`: how long each request to the CA may take.
pub const DEFAULT_ACME_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// An ACME challenge (`acme.challenges`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Challenge {
    /// `http-01`: the CA asks for the answer on port 80, which every listener in the clear gives.
    Http01,
    /// `tls-alpn-01`: the CA asks in a TLS handshake on port 443, which the TLS listener that
    /// lists the name answers (I32).
    TlsAlpn01,
}

/// `acme`, as fit's `config.ACME` (decision 007): where certificates are obtained and kept, and
/// how fast, and how names are taken on during handshakes (`on_demand`, I34). `shared_storage`
/// is refused as not supported yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acme {
    /// `directory`: the URL of the CA's ACME directory; [`DEFAULT_ACME_DIRECTORY`] when left out.
    pub directory: String,
    /// `email`: the address the CA may write to about the account; none when empty.
    pub email: String,
    /// `ca_file`: a PEM file of the roots the directory's certificate is checked against, in
    /// place of the system's; none when empty.
    pub ca_file: String,
    /// `storage`: the directory accounts, keys and certificates are kept in;
    /// `$XDG_DATA_HOME/akit` or `~/.local/share/akit` when empty.
    pub storage: String,
    /// `max_concurrent`: how many certificates are obtained at once;
    /// [`DEFAULT_ACME_MAX_CONCURRENT`] when left out or 0.
    pub max_concurrent: usize,
    /// `timeout`: how long obtaining one certificate may take; [`DEFAULT_ACME_TIMEOUT`] when
    /// left out or 0.
    pub timeout: Duration,
    /// `request_timeout`: how long each request to the CA may take;
    /// [`DEFAULT_ACME_REQUEST_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
    /// `challenges`: the challenges answered, in the order tried, a failed attempt tried again
    /// with the next; HTTP-01 then TLS-ALPN-01 when left out, as fit's.
    pub challenges: Vec<Challenge>,
    /// `on_demand`: how certificates are obtained during handshakes, for the listeners whose
    /// `tls.on_demand` is set (I34).
    pub on_demand: Option<OnDemand>,
}

/// fit's default `acme.on_demand.rate`: certificates obtained on demand every `interval`.
pub const DEFAULT_ON_DEMAND_RATE: usize = 10;
/// fit's default `acme.on_demand.interval`.
pub const DEFAULT_ON_DEMAND_INTERVAL: Duration = Duration::from_secs(60);
/// fit's default `acme.on_demand.max_names`.
pub const DEFAULT_ON_DEMAND_MAX_NAMES: usize = 1000;
/// fit's default `acme.on_demand.timeout`: how long a handshake waits for its certificate.
pub const DEFAULT_ON_DEMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's default `acme.on_demand.idle_timeout`: a week.
pub const DEFAULT_ON_DEMAND_IDLE_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 3600);

/// `acme.on_demand`, as fit's `config.OnDemand`: certificates obtained during handshakes, for
/// names no listener lists, only for names the `ask` URL allows, at most `rate` every
/// `interval` and `max_names` in all, each dropped once no handshake has asked for it for
/// `idle_timeout`. A handshake for such a name waits for its certificate for at most `timeout`,
/// without holding its I/O thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnDemand {
    /// `ask` (required): the http or https URL asked, with `domain=<name>` added to its query,
    /// whether a name may have a certificate: 2xx allows it, any other answer refuses it.
    pub ask: String,
    /// `rate`: certificates obtained every `interval`; [`DEFAULT_ON_DEMAND_RATE`] when left out
    /// or 0.
    pub rate: usize,
    /// `interval`: what `rate` counts over; [`DEFAULT_ON_DEMAND_INTERVAL`] when left out.
    pub interval: Duration,
    /// `max_names`: names with certificates obtained on demand, in all;
    /// [`DEFAULT_ON_DEMAND_MAX_NAMES`] when left out or 0.
    pub max_names: usize,
    /// `timeout`: how long a handshake waits for its name's certificate;
    /// [`DEFAULT_ON_DEMAND_TIMEOUT`] when left out.
    pub timeout: Duration,
    /// `idle_timeout`: how long a name taken on demand is kept with no handshake asking for it;
    /// [`DEFAULT_ON_DEMAND_IDLE_TIMEOUT`] when left out.
    pub idle_timeout: Duration,
}

impl OnDemand {
    /// `acme.on_demand` asking `ask`, its other settings their defaults.
    #[must_use]
    pub fn new(ask: impl Into<String>) -> OnDemand {
        OnDemand {
            ask: ask.into(),
            rate: DEFAULT_ON_DEMAND_RATE,
            interval: DEFAULT_ON_DEMAND_INTERVAL,
            max_names: DEFAULT_ON_DEMAND_MAX_NAMES,
            timeout: DEFAULT_ON_DEMAND_TIMEOUT,
            idle_timeout: DEFAULT_ON_DEMAND_IDLE_TIMEOUT,
        }
    }
}

impl Default for Acme {
    fn default() -> Acme {
        Acme {
            directory: DEFAULT_ACME_DIRECTORY.to_owned(),
            email: String::new(),
            ca_file: String::new(),
            storage: String::new(),
            max_concurrent: DEFAULT_ACME_MAX_CONCURRENT,
            timeout: DEFAULT_ACME_TIMEOUT,
            request_timeout: DEFAULT_ACME_REQUEST_TIMEOUT,
            challenges: vec![Challenge::Http01, Challenge::TlsAlpn01],
            on_demand: None,
        }
    }
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

/// `admin`: the Admin API, as fit's `config.Admin` (decision 006): so far its address, history,
/// limits and token (I27). Its TLS, access lists, store, console and the rest are refused as not
/// supported yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admin {
    /// `address`: a loopback host and port, or `unix/` and the path of a Unix socket, made 0600
    /// (akio API request A23); [`DEFAULT_ADMIN_ADDRESS`] when left out. A host that is not
    /// loopback needs a token and TLS, as in fit, and akit does not serve the API over TLS yet:
    /// it is refused.
    pub address: String,
    /// `token_file`: a file holding a token that every request must carry, as
    /// `Authorization: Bearer <token>`, read when akit starts; none asked for when left out.
    pub token_file: Option<String>,
    /// `history`: how many configs, the running one included, are kept to be read back;
    /// [`DEFAULT_ADMIN_HISTORY`] when left out or 0.
    pub history: usize,
    /// `max_config_bytes`: the largest config the API takes, larger ones getting 413;
    /// [`DEFAULT_ADMIN_MAX_CONFIG_BYTES`] when left out or 0.
    pub max_config_bytes: usize,
    /// `max_conns`: the most connections the API keeps open at once, one past it closed as it
    /// is accepted; [`DEFAULT_ADMIN_MAX_CONNS`] when left out or 0.
    pub max_conns: usize,
    /// `disable_dashboard`: the read-only dashboard at `/dashboard/` is not served (I63); it is
    /// when left out.
    pub disable_dashboard: bool,
}

impl Default for Admin {
    fn default() -> Admin {
        Admin {
            address: DEFAULT_ADMIN_ADDRESS.to_owned(),
            token_file: None,
            history: DEFAULT_ADMIN_HISTORY,
            max_config_bytes: DEFAULT_ADMIN_MAX_CONFIG_BYTES,
            max_conns: DEFAULT_ADMIN_MAX_CONNS,
            disable_dashboard: false,
        }
    }
}

/// fit's default `limits.max_body_bytes`: 16 MiB.
pub const DEFAULT_MAX_BODY_BYTES: u64 = 16 << 20;

/// fit's default `limits.write_stall_timeout`: 30s.
pub const DEFAULT_WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// `limits`, as fit's `config.Limits`: so far `max_body_bytes` and `write_stall_timeout`; its
/// other fields are refused as not supported yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `max_body_bytes`: the largest request body a listener takes, whether it arrives whole or
    /// streams through; a larger one gets 413, or, once it has begun to stream, its connection
    /// or stream is ended. [`DEFAULT_MAX_BODY_BYTES`] when left out or 0.
    pub max_body_bytes: u64,
    /// `write_stall_timeout` (I17b): how long a response may wait for its client to take what
    /// it was sent before the client loses it. [`DEFAULT_WRITE_STALL_TIMEOUT`] when left out or
    /// 0. A reload applies it to the connections already open too.
    pub write_stall_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            write_stall_timeout: DEFAULT_WRITE_STALL_TIMEOUT,
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

/// `telemetry`: where akit sends its traces and metrics, over OTLP/HTTP as JSON, and which
/// requests it traces, as fit's `config.Telemetry`, with its defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    /// `endpoint`: the OTLP/HTTP receiver, `http://` or `https://` and a host and port; traces go
    /// to its `/v1/traces` and metrics to its `/v1/metrics`. [`DEFAULT_TELEMETRY_ENDPOINT`] when
    /// left out; a trailing `/` taken off.
    pub endpoint: String,
    /// `headers_file`: a file of `Name: value` lines sent with each export; none when empty.
    pub headers_file: String,
    /// `ca_file`: the CAs the receiver's certificate is verified with over https; the system's
    /// when empty.
    pub ca_file: String,
    /// `cert_file`: the client certificate sent over https, with `key_file`.
    pub cert_file: String,
    /// `key_file`: `cert_file`'s private key.
    pub key_file: String,
    /// `service_name`: the resource's `service.name`; [`DEFAULT_TELEMETRY_SERVICE_NAME`] when left
    /// out or empty.
    pub service_name: String,
    /// `timeout`: how long an export may take, connecting included;
    /// [`DEFAULT_TELEMETRY_TIMEOUT`] when left out or 0.
    pub timeout: Duration,
    /// `traces`: trace requests; no traces when left out.
    pub traces: Option<TracesExport>,
    /// `metrics`: export the metrics the Admin API's `/metrics` serves every interval; none when
    /// left out.
    pub metrics: Option<MetricsExport>,
}

/// `telemetry.traces`: which requests are traced, and how spans wait to be exported.
#[derive(Debug, Clone)]
pub struct TracesExport {
    /// `sample_ratio`: the share of requests traced among those whose caller did not decide,
    /// from 0 to 1; 0, only those a caller sampled, when left out.
    pub sample_ratio: f64,
    /// `max_queue_bytes`: the most bytes of spans waiting to be exported, besides the batch being
    /// sent; [`DEFAULT_TRACES_MAX_QUEUE_BYTES`] when left out or 0.
    pub max_queue_bytes: usize,
    /// `interval`: how often spans are exported (and sooner when 256 KiB wait);
    /// [`DEFAULT_TRACES_INTERVAL`] when left out or 0.
    pub interval: Duration,
}

impl PartialEq for TracesExport {
    fn eq(&self, other: &Self) -> bool {
        self.sample_ratio.to_bits() == other.sample_ratio.to_bits()
            && self.max_queue_bytes == other.max_queue_bytes
            && self.interval == other.interval
    }
}

impl Eq for TracesExport {}

/// `telemetry.metrics`: how often metrics are exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsExport {
    /// `interval`: [`DEFAULT_METRICS_INTERVAL`] when left out or 0.
    pub interval: Duration,
}

/// fit's default `telemetry.endpoint`.
pub const DEFAULT_TELEMETRY_ENDPOINT: &str = "http://127.0.0.1:4318";
/// fit's default `telemetry.service_name`.
pub const DEFAULT_TELEMETRY_SERVICE_NAME: &str = "fit";
/// fit's default `telemetry.timeout`.
pub const DEFAULT_TELEMETRY_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's default `telemetry.traces.max_queue_bytes`: 4 MiB.
pub const DEFAULT_TRACES_MAX_QUEUE_BYTES: usize = 4 << 20;
/// fit's default `telemetry.traces.interval`.
pub const DEFAULT_TRACES_INTERVAL: Duration = Duration::from_secs(5);
/// fit's default `telemetry.metrics.interval`.
pub const DEFAULT_METRICS_INTERVAL: Duration = Duration::from_secs(30);

/// Reads a `headers_file`'s contents, as fit's `config.ParseHeaders`: one `Name: value` a line,
/// blank lines and lines starting with `#` passed over, names and values trimmed.
///
/// # Errors
///
/// The number of the first line that is not a header field, from 1.
pub fn parse_headers(data: &str) -> Result<Vec<(String, String)>, usize> {
    let mut h = Vec::new();
    for (i, line) in data.split('\n').enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line.split_once(':').unwrap_or((line, ""));
        let name = name.trim();
        if !line.contains(':') || name.is_empty() || name.contains([' ', '\t']) {
            return Err(i.saturating_add(1));
        }
        h.push((name.to_owned(), value.trim().to_owned()));
    }
    Ok(h)
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
    /// `hash_key`: with [`LoadBalancing::HeaderHash`], the header, and with
    /// [`LoadBalancing::CookieHash`], the cookie, whose value requests are hashed by; empty for
    /// the other policies.
    pub hash_key: String,
    /// `sticky_cookie`: the cookie that keeps each client on one address (I36); `None`, no
    /// cookie, when left out.
    pub sticky_cookie: Option<StickyCookie>,
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
    /// `response_timeout`: how long a whole exchange may take, from the request's start, waiting
    /// for a connection included, to the end of its response's body: one past it before its
    /// response began is answered 504, and one past it later is cut short.
    /// [`DEFAULT_RESPONSE_TIMEOUT`] when left out or 0.
    pub response_timeout: Duration,
    /// `response_header_timeout` (I17b): how long the upstream may take, once the request is
    /// sent whole, to begin its response; one past it is answered 504.
    /// [`DEFAULT_RESPONSE_HEADER_TIMEOUT`] when left out or 0.
    pub response_header_timeout: Duration,
    /// `response_idle_timeout` (I17b): how long the upstream may go silent in the middle of a
    /// response's body (the time the proxy holds its reads for a slow client aside); the
    /// response is then cut short. [`DEFAULT_RESPONSE_IDLE_TIMEOUT`] when left out or 0.
    pub response_idle_timeout: Duration,
    /// `protocol`: how requests reach the upstream (I18). [`UpstreamProtocol::Http11`] when
    /// left out.
    pub protocol: UpstreamProtocol,
    /// `tls_server_name` (I54): with [`UpstreamProtocol::H2`], the name the servers'
    /// certificates are checked against, and sent in SNI; empty when left out, which takes each
    /// address's host.
    pub tls_server_name: String,
    /// `tls_ca_file` (I54): with h2, a PEM file of the certificates the servers' are checked
    /// against, in place of the system's roots; empty when left out. Read, and watched, when the
    /// config is compiled.
    pub tls_ca_file: String,
    /// `tls_cert_file` (I54): with h2, a PEM file of the client certificate, and the chain after
    /// it, presented to servers that ask for one (mutual TLS); given with `tls_key_file`, or
    /// neither is. Empty when left out.
    pub tls_cert_file: String,
    /// `tls_key_file` (I54): the PEM file of `tls_cert_file`'s private key; empty when left out.
    pub tls_key_file: String,
}

/// An upstream's `protocol`: how requests reach its servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum UpstreamProtocol {
    /// `http/1.1`, in the clear.
    #[default]
    Http11,
    /// `h2c`: HTTP/2 in the clear, with prior knowledge, which the servers must expect.
    H2c,
    /// `h2`: HTTP/2 over TLS, offered through ALPN. fit falls back to HTTP/1.1 when a server
    /// does not choose h2; akit's h2 client cannot yet (akio API request A53).
    H2,
}

impl UpstreamProtocol {
    /// The protocol's name in the config.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            UpstreamProtocol::Http11 => "http/1.1",
            UpstreamProtocol::H2c => "h2c",
            UpstreamProtocol::H2 => "h2",
        }
    }

    /// Whether requests go over HTTP/2.
    #[must_use]
    pub fn is_h2(self) -> bool {
        matches!(self, UpstreamProtocol::H2c | UpstreamProtocol::H2)
    }
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

/// An upstream's `sticky_cookie`, as fit's `config.StickyCookie`: a request whose cookie names an
/// address that is up goes there, and any other gets the cookie for the address it went to. The
/// cookie's value is a hash of the address, the same in every akit (and fit) with the same
/// addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickyCookie {
    /// `name`: the cookie's name; [`DEFAULT_STICKY_COOKIE_NAME`] when left out or empty.
    pub name: String,
    /// `path`: the cookie's `Path`; `/` when left out or empty.
    pub path: String,
    /// `max_age`: how long the client keeps the cookie, as its `Max-Age`; `None`, until the
    /// browser closes, when left out or 0.
    pub max_age: Option<Duration>,
    /// `secure`: mark the cookie `Secure`. It is always `HttpOnly` and `SameSite=Lax`.
    pub secure: bool,
}

/// fit's default sticky cookie name.
pub const DEFAULT_STICKY_COOKIE_NAME: &str = "fit_sticky";

/// An upstream's `load_balancing` policy: each of fit's.
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
    /// `random`: at random, by weight.
    Random,
    /// `p2c`: the less busy for its weight of two addresses drawn at random.
    P2c,
    /// `ip_hash`: a consistent hash of the client's IP address.
    IpHash,
    /// `uri_hash`: a consistent hash of the request's path and query.
    UriHash,
    /// `header_hash`: a consistent hash of the header `hash_key` names; a request without it goes
    /// in turn.
    HeaderHash,
    /// `cookie_hash`: a consistent hash of the cookie `hash_key` names; a request without it goes
    /// in turn.
    CookieHash,
}

impl LoadBalancing {
    /// The policy's name in the config.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LoadBalancing::RoundRobin => "round_robin",
            LoadBalancing::WeightedRoundRobin => "weighted_round_robin",
            LoadBalancing::LeastConn => "least_conn",
            LoadBalancing::Random => "random",
            LoadBalancing::P2c => "p2c",
            LoadBalancing::IpHash => "ip_hash",
            LoadBalancing::UriHash => "uri_hash",
            LoadBalancing::HeaderHash => "header_hash",
            LoadBalancing::CookieHash => "cookie_hash",
        }
    }

    /// Whether it hashes a key of the request onto a ring of the addresses.
    #[must_use]
    pub fn is_hash(self) -> bool {
        matches!(
            self,
            LoadBalancing::IpHash
                | LoadBalancing::UriHash
                | LoadBalancing::HeaderHash
                | LoadBalancing::CookieHash
        )
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
/// fit's default `response_timeout`.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3600);
/// fit's default `response_header_timeout`.
pub const DEFAULT_RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(60);
/// fit's default `response_idle_timeout`.
pub const DEFAULT_RESPONSE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// When not 0, the HTTPS port every request this listener in the clear takes is redirected
    /// to with a 308, but for ACME's HTTP-01 challenges; it serves no routes. Only the listener
    /// HTTPS by default makes (`auto_https`, I33) has one; 0 for every listener written.
    pub redirect_https: u16,
    /// `proxy_protocol` (I52): the proxies whose connections begin with a PROXY protocol header
    /// naming the client, for a listener of `tcp` or of HTTP over TCP.
    pub proxy_protocol: Option<ListenerProxyProtocol>,
    /// `socks` (I55): who may use the listener's `socks` protocol and where it connects them;
    /// set exactly when its protocols include `socks`.
    pub socks: Option<ForwardProxy>,
}

/// Who may use a forward proxy, SOCKS (a listener's `socks`) or HTTP CONNECT (the
/// `forward_proxy` handler), and where it may connect them to: fit's `config.ForwardProxy`
/// (I55).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardProxy {
    /// `clients`: the addresses and networks of the clients that may use the proxy.
    pub clients: Vec<Network>,
    /// `allow`: the destinations the proxy connects to. Left out, every destination `deny` does
    /// not hold, but for loopback, link-local, unspecified and multicast addresses and the
    /// host's own, which a network here must name.
    pub allow: Vec<Destination>,
    /// `deny`: destinations refused, whatever `allow` holds.
    pub deny: Vec<Destination>,
    /// `ports`: the destination ports allowed; any when empty.
    pub ports: Vec<u16>,
    /// `connect_timeout`: how long resolving and connecting to a destination may take;
    /// [`DEFAULT_FORWARD_CONNECT_TIMEOUT`] when left out.
    pub connect_timeout: Duration,
    /// `idle_timeout`: how long a proxied connection may carry nothing either way;
    /// [`DEFAULT_FORWARD_IDLE_TIMEOUT`] when left out.
    pub idle_timeout: Duration,
}

/// fit's `DefaultForwardConnectTimeout`.
pub const DEFAULT_FORWARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's `DefaultForwardIdleTimeout`.
pub const DEFAULT_FORWARD_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// A forward proxy's destination in `allow` or `deny`, as fit's `dests.add` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// An IP address or CIDR range, which also holds the address a host name resolves to.
    Network(Network),
    /// A host name, in lower case and without a trailing dot.
    Name(String),
    /// `*.example.com`: every name below `example.com`, kept as the suffix `.example.com`, in
    /// lower case.
    Suffix(String),
}

/// A listener's `proxy_protocol`, as fit's `config.ProxyProtocol`: a connection from one of
/// `trusted` must begin with a PROXY protocol header, of version 1 or 2, within 10s, and is
/// served as from the client it names; one from elsewhere is served as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerProxyProtocol {
    /// `trusted`: the addresses and networks of the proxies that send the header.
    pub trusted: Vec<Network>,
}

impl ListenerProxyProtocol {
    /// Whether a connection from `ip` is one of a trusted proxy's (an IPv4-mapped address taken
    /// as the IPv4 address it maps).
    #[must_use]
    pub fn trusts(&self, ip: IpAddr) -> bool {
        let ip = unmap(ip);
        self.trusted.iter().any(|n| n.contains(ip))
    }
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
    /// `acme`: host names whose certificates are obtained with ACME (I31), as the top-level
    /// `acme` says. A client asking for one is given its certificate once there is one; until
    /// then, and for other names, the certificates from files.
    pub acme: Vec<String>,
    /// `on_demand`: a certificate is obtained during the handshake for a name a client asks for
    /// that neither the files nor `acme` have, as the top-level `acme.on_demand` allows (I34).
    pub on_demand: bool,
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
    /// `tcp`, raw TCP, whose connections the service that lists the listener proxies (I51);
    /// beside HTTP, told apart from it by what a client sends first (I53).
    Tcp,
    /// `udp`, alone: each client address a session, whose datagrams the service that lists the
    /// listener proxies (I51).
    Udp,
    /// `socks`: a SOCKS5, SOCKS4 and SOCKS4a forward proxy, as the listener's `socks` allows
    /// (I55); beside HTTP and `tcp`, told apart from them by what a client sends first.
    Socks,
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
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
            Protocol::Socks => "socks",
        }
    }

    /// Whether it is one of HTTP's: HTTP/1.1, h2c, h2 or h3.
    #[must_use]
    pub fn is_http(self) -> bool {
        !matches!(self, Protocol::Tcp | Protocol::Udp | Protocol::Socks)
    }
}

/// A layer 4 service (I51): it proxies the connections of its `tcp` listeners, and the sessions
/// of its `udp` listeners, to an address of its upstream, as fit's `config.Service`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// `name`, unique among the services.
    pub name: String,
    /// `listeners`: the names of the `tcp` and `udp` listeners whose connections it takes; a
    /// listener is one service's.
    pub listeners: Vec<String>,
    /// `upstream`: the name of the upstream connections go to.
    pub upstream: String,
    /// `idle_timeout`, when given: how long a connection or session may carry nothing either
    /// way before both sides are closed ([`Service::idle`], [`Service::udp_idle`]).
    pub idle_timeout: Option<Duration>,
    /// `max_conns`: the most connections and sessions it proxies at once;
    /// [`DEFAULT_SERVICE_MAX_CONNS`] when left out or 0.
    pub max_conns: usize,
    /// `server_names` (I52): the server names of the TLS connections it takes from its `tcp`
    /// listeners, by their ClientHello, without terminating them; `*.example.com` takes the
    /// names one label below `example.com`. In lower case, without a trailing dot. Empty: the
    /// connections no other service's names take.
    pub server_names: Vec<String>,
    /// `proxy_protocol` (I52): the PROXY protocol header sent first on each upstream
    /// connection, naming the client and the address it connected to.
    pub proxy_protocol: Option<ProxyVersion>,
}

/// A version of the PROXY protocol: a service's `proxy_protocol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyVersion {
    /// `v1`, a line of text.
    V1,
    /// `v2`, binary.
    V2,
}

/// fit's default `idle_timeout` of a service's TCP connections.
pub const DEFAULT_SERVICE_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// fit's default `idle_timeout` of a service's UDP sessions.
pub const DEFAULT_SERVICE_UDP_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// fit's default `max_conns` of a service.
pub const DEFAULT_SERVICE_MAX_CONNS: usize = 10_000;

impl Service {
    /// The idle timeout of a TCP connection: `idle_timeout`, or 10 minutes.
    #[must_use]
    pub fn idle(&self) -> Duration {
        self.idle_timeout.unwrap_or(DEFAULT_SERVICE_IDLE_TIMEOUT)
    }

    /// The idle timeout of a UDP session: `idle_timeout`, or 1 minute.
    #[must_use]
    pub fn udp_idle(&self) -> Duration {
        self.idle_timeout
            .unwrap_or(DEFAULT_SERVICE_UDP_IDLE_TIMEOUT)
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
    /// `fastcgi`: requests answered by a FastCGI application, as PHP-FPM is (I75).
    FastCgi(crate::fastcgi::FastCgi),
    /// `forward_proxy`: HTTP CONNECT tunnels to where its rules allow (I55).
    ForwardProxy(ForwardProxy),
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
    /// The `basic_auth` middleware: a user name and password, checked against bcrypt hashes
    /// (I42).
    BasicAuth(BasicAuth),
    /// The `client_cert` middleware: a client certificate the listener verified (I42).
    ClientCert(ClientCert),
    /// The `forward_auth` middleware: an auth service asked whether a request may go on (I43).
    ForwardAuth(ForwardAuth),
    /// The `circuit_breaker` middleware: requests stopped while too many fail (I45).
    CircuitBreaker(CircuitBreaker),
    /// The `compress` middleware: responses compressed as they are written (I47).
    Compress(Compress),
    /// The `jwt` middleware: a JSON Web Token checked with keys from files or a JWK set (I44).
    Jwt(Jwt),
    /// The `cache` middleware: responses kept and answered from by RFC 9111's rules (I67).
    Cache(Cache),
}

impl Handler {
    /// The module's name, as the config writes it in `module`.
    #[must_use]
    pub fn module(&self) -> &'static str {
        match self {
            Handler::Respond(_) => "respond",
            Handler::Static(_) => "static",
            Handler::Proxy(_) => "proxy",
            Handler::FastCgi(_) => "fastcgi",
            Handler::ForwardProxy(_) => "forward_proxy",
            Handler::Rewrite(_) => "rewrite",
            Handler::Redirect(_) => "redirect",
            Handler::Headers(_) => "headers",
            Handler::RateLimit(_) => "rate_limit",
            Handler::ConcurrencyLimit(_) => "concurrency_limit",
            Handler::BodyLimit(_) => "body_limit",
            Handler::BasicAuth(_) => "basic_auth",
            Handler::ClientCert(_) => "client_cert",
            Handler::ForwardAuth(_) => "forward_auth",
            Handler::CircuitBreaker(_) => "circuit_breaker",
            Handler::Compress(_) => "compress",
            Handler::Jwt(_) => "jwt",
            Handler::Cache(_) => "cache",
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
                | Handler::BasicAuth(_)
                | Handler::ClientCert(_)
                | Handler::ForwardAuth(_)
                | Handler::CircuitBreaker(_)
                | Handler::Compress(_)
                | Handler::Jwt(_)
                | Handler::Cache(_)
        )
    }
}

/// The `rewrite` middleware's params, as fit's `middleware/rewrite`: a prefix stripped, each
/// match of `regexp` replaced, a prefix added, in that order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rewrite {
    /// `strip_prefix`: prefixes, each starting with `/`; the first the path starts with, at a
    /// segment boundary, is removed. None when left out.
    pub strip_prefix: Vec<String>,
    /// `regexp`: a regular expression (checked when the config is read); each match in the
    /// decoded path is replaced with `replacement`. Empty when left out.
    pub regexp: String,
    /// `replacement`: what replaces each match, with `$1`, `${name}` for the regexp's groups and
    /// `$$` for `$` (Go's `Regexp.Expand`). Needs a `regexp`.
    pub replacement: String,
    /// `add_prefix`: a prefix put before the path, starting with `/` and not ending with one;
    /// empty when left out.
    pub add_prefix: String,
}

/// The `redirect` middleware's params, as fit's `middleware/redirect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// `to`: where to, with `{scheme}`, `{host}`, `{path}`, `{query}` and `{uri}` standing for
    /// the request's and, with a `regexp`, `$1` or `${name}` for its groups (`$$` for `$`, Go's
    /// `Regexp.Expand`).
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

/// The `compress` middleware's params, as fit's `middleware/compress`, its levels worked out as
/// its `Provision` works them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compress {
    /// `encodings`: those offered, in order of preference among encodings the client prefers as
    /// much; zstd, gzip and br when left out.
    pub encodings: Vec<Encoding>,
    /// gzip's level, 1 to 9: `gzip_level`, else the one `level` names.
    pub gzip_level: u32,
    /// brotli's quality, 1 to 11: `brotli_level`, else the one `level` names.
    pub brotli_level: u32,
    /// zstd's level, 1 to 22: `zstd_level`, else the one `level` names.
    pub zstd_level: u32,
    /// `min_bytes` (once `min_length`): the shortest declared body compressed,
    /// [`DEFAULT_COMPRESS_MIN_BYTES`] when left out or 0.
    pub min_bytes: u64,
    /// `max_in_flight`: how many responses are compressed at once, past which they go
    /// uncompressed; [`DEFAULT_COMPRESS_MAX_IN_FLIGHT`] when left out or 0.
    pub max_in_flight: u64,
}

/// fit's default `compress` `min_bytes`: a body below it goes in one packet either way.
pub const DEFAULT_COMPRESS_MIN_BYTES: u64 = 1400;
/// fit's default `compress` `max_in_flight`.
pub const DEFAULT_COMPRESS_MAX_IN_FLIGHT: u64 = 1024;

/// The `cache` middleware's params, as fit's `middleware/cache`, worked out as its `Provision`
/// works them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    /// `zone`: the store responses are kept in, shared by every cache of the same zone and kept
    /// across reloads; [`DEFAULT_CACHE_ZONE`] when left out or empty.
    pub zone: String,
    /// `max_bytes` (once `max_size`): the most bytes the zone keeps, bodies, fields and keys
    /// counted; [`DEFAULT_CACHE_MAX_BYTES`] when left out or 0.
    pub max_bytes: u64,
    /// `max_entry_bytes` (once `max_entry_size`): the largest body kept, at most a sixteenth of
    /// `max_bytes`; [`DEFAULT_CACHE_MAX_ENTRY_BYTES`], or a sixteenth of `max_bytes` if less,
    /// when left out or 0.
    pub max_entry_bytes: u64,
    /// `default_ttl`: how long a response that gives no lifetime of its own is fresh for, when
    /// its status allows one to be assumed; a tenth of the time since its `Last-Modified`, at
    /// most a day, when left out.
    pub default_ttl: Option<Duration>,
    /// `disable_lock`: every miss fetches for itself, with no cache lock.
    pub disable_lock: bool,
    /// `lock_timeout`: how long a miss waits for another's fetch of its response;
    /// [`DEFAULT_CACHE_LOCK_TIMEOUT`] when left out or 0.
    pub lock_timeout: Duration,
    /// `stale_while_revalidate`: how long past its lifetime a response that says nothing of it
    /// may be answered while it is fetched again in the background (RFC 5861); `None` when left
    /// out or 0.
    pub stale_while_revalidate: Option<Duration>,
    /// `stale_if_error`: how long past its lifetime a response that says nothing of it may be
    /// answered when fetching it again fails (RFC 5861); `None` when left out or 0.
    pub stale_if_error: Option<Duration>,
    /// `disk`: the zone's disk tier, or `None`.
    pub disk: Option<CacheDisk>,
    /// Not a param: the route has an auth middleware (`basic_auth`, `client_cert`,
    /// `forward_auth`, `jwt`), so every request the cache keeps a response for was let in on
    /// credentials, which fit's auth middlewares record per request and RFC 9111 section 3.5
    /// treats as `Authorization`.
    pub authenticated: bool,
}

/// The `cache` middleware's `disk` param, as fit's `middleware/cache` `Disk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDisk {
    /// `path` (required): the directory the zone's responses are kept in, the zone's alone.
    pub path: String,
    /// `max_bytes` (once `max_size`): the most bytes the zone keeps on disk;
    /// [`DEFAULT_CACHE_DISK_MAX_BYTES`] when left out or 0.
    pub max_bytes: u64,
    /// `max_entries`: the most responses the zone keeps on disk;
    /// [`DEFAULT_CACHE_DISK_MAX_ENTRIES`] when left out or 0.
    pub max_entries: u64,
}

/// fit's default `cache` `disk` `max_bytes`: 1 GiB.
pub const DEFAULT_CACHE_DISK_MAX_BYTES: u64 = 1 << 30;
/// fit's default `cache` `disk` `max_entries`.
pub const DEFAULT_CACHE_DISK_MAX_ENTRIES: u64 = 100_000;

/// fit's default `cache` `zone`.
pub const DEFAULT_CACHE_ZONE: &str = "default";
/// fit's default `cache` `max_bytes`: 128 MiB.
pub const DEFAULT_CACHE_MAX_BYTES: u64 = 128 << 20;
/// fit's default `cache` `max_entry_bytes`: 1 MiB.
pub const DEFAULT_CACHE_MAX_ENTRY_BYTES: u64 = 1 << 20;
/// fit's default `cache` `lock_timeout`.
pub const DEFAULT_CACHE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// How many parts a zone's store is split into; `max_entry_bytes` is at most a part's share.
pub const CACHE_SHARDS: u64 = 16;

/// The `body_limit` middleware's params, as fit's `middleware/limit` `Body`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyLimit {
    /// `max_bytes`: the most bytes a request's body may have.
    pub max_bytes: u64,
}

/// The `basic_auth` middleware's params, as fit's `middleware/auth` `Basic`, checked as its
/// `Provision` checks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuth {
    /// `realm`: what the browser shows when it asks for a user and password; `fit` when left
    /// out. It holds no quote, backslash or control character.
    pub realm: String,
    /// `users`: at least one, each name once.
    pub users: Vec<BasicUser>,
    /// `user_header`: a request field set to the user's name for the handler; empty when left
    /// out. An HTTP token.
    pub user_header: String,
    /// `forward_authorization`: leave the `Authorization` field on the request the handler
    /// sees; it is removed when false.
    pub forward_authorization: bool,
}

/// One user `basic_auth` lets in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicUser {
    /// `username`: not empty, without a colon, NUL, CR or LF.
    pub username: String,
    /// `password_hash`: a bcrypt hash (`$2a$`, `$2b$` or `$2y$`, cost 4 to 31), as
    /// `htpasswd -B` or fit's `hash-password` makes it; checked with `akio_crypto::bcrypt`.
    pub password_hash: String,
}

/// fit's `realm` when left out.
pub const DEFAULT_REALM: &str = "fit";

/// The `client_cert` middleware's params, as fit's `middleware/auth` `ClientCert`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientCert {
    /// `optional`: let a request without a certificate through as well; refused 403 when false.
    pub optional: bool,
    /// `subject_header`: a request field set to the certificate's subject, as Go writes a
    /// distinguished name (`CN=alice,O=Example`); empty when left out. An HTTP token.
    pub subject_header: String,
}

/// The `forward_auth` middleware's params, as fit's `middleware/auth` `ForwardAuth`, checked as
/// its `Provision` checks them, but for the upstream's name, which the middleware looks up when
/// it is provisioned (as the `proxy` handler's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardAuth {
    /// `upstream`: the name of the upstream the auth service is.
    pub upstream: String,
    /// `uri`: the path, and query, of the request sent to the auth service; `/` when left out.
    /// It starts with `/`, holds no control character, and its path's `%` escapes are valid.
    pub uri: String,
    /// `request_headers`: the request's fields sent to the auth service; all but the hop-by-hop
    /// ones and those of the body when empty (left out). Each an HTTP token.
    pub request_headers: Vec<String>,
    /// `copy_headers`: fields copied from a 2xx answer onto the request. Each an HTTP token.
    pub copy_headers: Vec<String>,
    /// `timeout`: how long to wait for the auth service's answer;
    /// [`DEFAULT_FORWARD_AUTH_TIMEOUT`] when left out or 0.
    pub timeout: Duration,
}

/// fit's `forward_auth` `timeout` when left out (`DefaultForwardAuthTimeout`).
pub const DEFAULT_FORWARD_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// The `circuit_breaker` middleware's params, as fit's `middleware/breaker` `Breaker`, with its
/// defaults filled in: while it is closed it counts, over the last `window`, the route's
/// responses that were failures and those slower than `slow_threshold` to begin; once at least
/// `min_requests` were counted and the failures reach `error_ratio`, or the slow ones
/// `slow_ratio`, it opens, answering 503 for `open_duration`, then lets `half_open_requests`
/// through to try.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    /// `window`: how far back requests are counted, in ten steps; 10s when left out or 0.
    pub window: Duration,
    /// `min_requests`: how many requests the window must have counted before it can open; at
    /// least 1, 20 when left out.
    pub min_requests: u64,
    /// `error_ratio`: the share of failures in the window that opens it, from 0 to 1; 0.5 when
    /// left out or 0.
    pub error_ratio: f64,
    /// `statuses`: the statuses that count as failures, each from 100 to 599; 500 to 599 when
    /// empty (left out).
    pub statuses: Vec<u16>,
    /// `slow_threshold`: a response that takes longer than this to begin counts as slow; none
    /// does when left out or 0.
    pub slow_threshold: Option<Duration>,
    /// `slow_ratio`: with `slow_threshold`, the share of slow responses that opens it, from 0 to
    /// 1; 0.5 when left out or 0.
    pub slow_ratio: f64,
    /// `open_duration`: how long it stays open; 30s when left out or 0.
    pub open_duration: Duration,
    /// `half_open_requests`: how many requests it lets through half-open, which must all fare
    /// well for it to close; at least 1, 5 when left out.
    pub half_open_requests: u32,
}

impl Default for CircuitBreaker {
    fn default() -> CircuitBreaker {
        CircuitBreaker {
            window: Duration::from_secs(10),
            min_requests: 20,
            error_ratio: 0.5,
            statuses: Vec::new(),
            slow_threshold: None,
            slow_ratio: 0.5,
            open_duration: Duration::from_secs(30),
            half_open_requests: 5,
        }
    }
}

// The ratios are finite numbers (the decoder refuses others), so comparing their bits is an
// equivalence.
impl PartialEq for CircuitBreaker {
    fn eq(&self, o: &CircuitBreaker) -> bool {
        self.window == o.window
            && self.min_requests == o.min_requests
            && self.error_ratio.to_bits() == o.error_ratio.to_bits()
            && self.statuses == o.statuses
            && self.slow_threshold == o.slow_threshold
            && self.slow_ratio.to_bits() == o.slow_ratio.to_bits()
            && self.open_duration == o.open_duration
            && self.half_open_requests == o.half_open_requests
    }
}

impl Eq for CircuitBreaker {}

/// The `jwt` middleware's params, as fit's `middleware/auth` `JWT`, checked as its `Provision`
/// checks them, but for the key files, which the middleware reads when it is provisioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwt {
    /// `jwks_url`: an `http` or `https` URL of a JWK set (RFC 7517); empty when left out.
    pub jwks_url: String,
    /// `jwks_refresh_interval` (or its old name, `jwks_refresh`): how often the JWK set is
    /// fetched again; [`DEFAULT_JWKS_REFRESH_INTERVAL`] when left out or 0.
    pub jwks_refresh_interval: Duration,
    /// `public_key_files`: PEM files of public keys or certificates.
    pub public_key_files: Vec<String>,
    /// `hmac_secret_file`: a file holding the HMAC secret; empty when left out.
    pub hmac_secret_file: String,
    /// `algorithms`: those tokens may be signed with, each one of [`JWT_ALGORITHMS`]; any that
    /// fits the keys when empty.
    pub algorithms: Vec<String>,
    /// `issuer`: the `iss` a token must have; any when empty.
    pub issuer: String,
    /// `audience`: audiences of which a token's `aud` must name one; any when empty.
    pub audience: Vec<String>,
    /// `leeway`: how far `exp` and `nbf` may be off; none when left out.
    pub leeway: Duration,
    /// `token_from`: where the token is.
    pub token_from: TokenFrom,
    /// `claims_headers`: request fields set to claims, as (field, claim), in the order written.
    /// Each field an HTTP token, each claim not empty.
    pub claims_headers: Vec<(String, String)>,
}

/// Where the `jwt` middleware finds a request's token: its `token_from`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TokenFrom {
    /// `header` (the default): after `Bearer ` in the `Authorization` field.
    #[default]
    Authorization,
    /// `header:Name`: the whole of that field. The name as written, an HTTP token.
    Header(String),
    /// `cookie:name`: the cookie of that name. An HTTP token.
    Cookie(String),
    /// `query:name`: the query parameter of that name; not empty.
    Query(String),
}

/// fit's `jwks_refresh_interval` when left out (`defaultRefresh`).
pub const DEFAULT_JWKS_REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

/// The algorithms the `jwt` middleware verifies, as fit's `algorithms` enum lists them.
pub const JWT_ALGORITHMS: [&str; 13] = [
    "RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256", "ES384", "ES512", "EdDSA",
    "HS256", "HS384", "HS512",
];

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
/// `Files` can act on; the rest are refused as not supported yet (akio API requests A14 and A15
/// bring them).
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
    /// `compress`: the encodings a file kept in memory is also kept compressed with, made once
    /// and served to the clients that prefer one, in the order written. None when left out.
    pub compress: Vec<Encoding>,
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

/// A content coding: of a file's compressed twin (`precompressed`), a compressed copy
/// (`static`'s `compress`) or a response (the `compress` middleware's `encodings`).
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
    /// `upstream`: the name of the upstream requests go to; empty with `upstreams`. That it
    /// names one of the config's upstreams is checked when the handler is provisioned, once
    /// every file's are joined.
    pub upstream: String,
    /// `upstreams` (I46): the upstreams requests are spread over at random by weight, in place
    /// of `upstream`; empty when left out.
    pub upstreams: Vec<Split>,
    /// `failover` (I46): the upstream that takes the requests when none of the chosen
    /// upstream's addresses is in rotation and one of its own is; none when left out or empty.
    pub failover: Option<String>,
    /// `mirror` (I46): where copies of a share of the requests go; none when left out.
    pub mirror: Option<Mirror>,
    /// `trusted_proxies`: the proxies in front of this one, whose `X-Forwarded-*` and
    /// `Forwarded` fields are believed. None when left out.
    pub trusted_proxies: Vec<Network>,
    /// `host_header`: the `Host` sent upstream; the client's when empty.
    pub host_header: String,
    /// `timeout`: the most a request may take, from when the proxy takes it to the end of its
    /// response: one past it before its response began is answered 504, and one whose response
    /// is under way is cut short. None when left out or 0. With `retry`, it bounds the attempts
    /// together.
    pub timeout: Option<Duration>,
    /// `retry` (I45): when a request is sent again, to the address picked next; none when left
    /// out.
    pub retry: Option<Retry>,
    /// `tunnel_idle_timeout`: how long a tunnel (a WebSocket joined to its upstream, I49) may
    /// carry nothing either way before it is closed. [`DEFAULT_TUNNEL_IDLE_TIMEOUT`] when left
    /// out or 0.
    pub tunnel_idle_timeout: Duration,
}

/// The `proxy` handler's `retry`, as fit's `proxy.Retry`, with its defaults filled in: a request
/// whose connection could not be made (or whose address had `max_pending` requests waiting) is
/// sent again whatever its method; one of an idempotent method (GET, HEAD, OPTIONS, TRACE, PUT,
/// DELETE) also when it failed before its response began or was answered with one of
/// `statuses`. A request whose body streams is never sent again. Retries are held to a budget.
#[derive(Debug, Clone)]
pub struct Retry {
    /// `attempts`: how many times a request may be sent again after the first, from 1 to 10;
    /// 2 when left out.
    pub attempts: u32,
    /// `statuses`: the upstream's statuses, each 5xx, that have an idempotent request sent
    /// again; none when left out.
    pub statuses: Vec<u16>,
    /// `budget_ratio`: the share of the requests the proxy takes that retries may add, from 0 to
    /// 1; 0.2 when left out.
    pub budget_ratio: f64,
    /// `budget_min_per_second`: the retries allowed each second beyond `budget_ratio`, at least
    /// 0; 10 when left out.
    pub budget_min_per_second: f64,
}

impl Default for Retry {
    fn default() -> Retry {
        Retry {
            attempts: 2,
            statuses: Vec::new(),
            budget_ratio: 0.2,
            budget_min_per_second: 10.0,
        }
    }
}

// The budget's numbers are finite (the decoder refuses others), so comparing their bits is an
// equivalence.
impl PartialEq for Retry {
    fn eq(&self, o: &Retry) -> bool {
        self.attempts == o.attempts
            && self.statuses == o.statuses
            && self.budget_ratio.to_bits() == o.budget_ratio.to_bits()
            && self.budget_min_per_second.to_bits() == o.budget_min_per_second.to_bits()
    }
}

impl Eq for Retry {}

/// One of the `proxy` handler's `upstreams` (I46), as fit's `proxy.Split`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// `upstream`: the name of the upstream, checked when the handler is provisioned.
    pub upstream: String,
    /// `weight`: its share of the requests against the others' weights, from 1 to 1000.
    pub weight: u32,
}

/// The `proxy` handler's `mirror` (I46), as fit's `proxy.Mirror`: a copy of a share of the
/// requests whose bodies arrive whole goes to another upstream, its answer dropped. At most
/// [`MAX_MIRRORED`] copies are under way at once for a proxy; past that, requests are not
/// copied. The requests never wait for their copies.
#[derive(Debug, Clone)]
pub struct Mirror {
    /// `upstream`: the name of the upstream the copies go to, checked when the handler is
    /// provisioned.
    pub upstream: String,
    /// `percent`: the share of the requests copied, chosen at random, from 0 to 100; 100 when
    /// left out.
    pub percent: f64,
}

// The percent is finite (the decoder refuses others), so comparing its bits is an equivalence.
impl PartialEq for Mirror {
    fn eq(&self, o: &Mirror) -> bool {
        self.upstream == o.upstream && self.percent.to_bits() == o.percent.to_bits()
    }
}

impl Eq for Mirror {}

/// fit's `maxMirrored`: the most copies of a proxy's requests under way to its mirror at once.
pub const MAX_MIRRORED: usize = 256;

/// fit's `DefaultTunnelIdleTimeout`: a tunnel's silence, when `tunnel_idle_timeout` says none.
pub const DEFAULT_TUNNEL_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

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
    "acme",
    "auto_https",
    "services",
    "telemetry",
    "providers",
];
const CONFIG_FIT_ONLY: &[&str] = &[
    "log",
    "access",
    "engine",
    "pprof",
    "llm",
    "tunnel",
    "webrtc",
    "media_auth",
    "streaming",
    "media_library",
    "sync",
    "bittorrent",
];
// I26: the Admin API's address, history and limits; I27: its token; I63: its dashboard. Its TLS waits for the API to
// be served over TLS, its access lists for fit's access.
const ADMIN_FIELDS: &[&str] = &[
    "address",
    "token_file",
    "history",
    "max_config_bytes",
    "max_conns",
    "disable_dashboard",
];
const ADMIN_FIT_ONLY: &[&str] = &[
    "mptcp",
    "cert_file",
    "key_file",
    "client_ca_file",
    "node_id",
    "store",
    "console",
    "assistant",
    "cluster",
    "cluster_member",
    "access",
];
const LIMITS_FIELDS: &[&str] = &["max_body_bytes", "write_stall_timeout"];
const LIMITS_FIT_ONLY: &[&str] = &[
    "max_pending_bytes",
    "max_header_bytes",
    "max_conns",
    "max_write_queue_bytes",
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
// I64: OpenTelemetry, every field.
const TELEMETRY_FIELDS: &[&str] = &[
    "endpoint",
    "headers_file",
    "ca_file",
    "cert_file",
    "key_file",
    "service_name",
    "timeout",
    "traces",
    "metrics",
];
const TRACES_EXPORT_FIELDS: &[&str] = &["sample_ratio", "max_queue_bytes", "interval"];
const METRICS_EXPORT_FIELDS: &[&str] = &["interval"];
const LISTENER_FIELDS: &[&str] = &[
    "name",
    "address",
    "protocols",
    "tls",
    "proxy_protocol",
    "socks",
];
const LISTENER_FIT_ONLY: &[&str] = &["mptcp", "access"];
const FORWARD_PROXY_FIELDS: &[&str] = &[
    "clients",
    "allow",
    "deny",
    "ports",
    "connect_timeout",
    "idle_timeout",
];
const LISTENER_PROXY_PROTOCOL_FIELDS: &[&str] = &["trusted"];
// I51, I52; access comes with fit's access lists.
const SERVICE_FIELDS: &[&str] = &[
    "name",
    "listeners",
    "upstream",
    "idle_timeout",
    "max_conns",
    "server_names",
    "proxy_protocol",
];
const SERVICE_FIT_ONLY: &[&str] = &["access"];
const TLS_FIELDS: &[&str] = &[
    "certificates",
    "client_auth",
    "max_version",
    "acme",
    "on_demand",
];
// kernel_tls_offload comes with engine's.
const TLS_FIT_ONLY: &[&str] = &["kernel_tls_offload"];
// I31: shared_storage comes with fit's shared storage.
const ACME_FIELDS: &[&str] = &[
    "directory",
    "email",
    "ca_file",
    "storage",
    "max_concurrent",
    "timeout",
    "request_timeout",
    "challenges",
    "on_demand",
];
const ON_DEMAND_FIELDS: &[&str] = &[
    "ask",
    "rate",
    "interval",
    "max_names",
    "timeout",
    "idle_timeout",
];
// I33: HTTPS by default (crate::auto_https).
const AUTO_HTTPS_FIELDS: &[&str] = &["disable", "disable_redirects", "http_port", "https_port"];
const ACME_FIT_ONLY: &[&str] = &["shared_storage"];
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
const FIT_MIDDLEWARE: &[&str] = &["wasm"];
const CACHE_FIELDS: &[&str] = &[
    "zone",
    "max_bytes",
    "max_size",
    "max_entry_bytes",
    "max_entry_size",
    "default_ttl",
    "disable_lock",
    "lock_timeout",
    "stale_while_revalidate",
    "stale_if_error",
    "disk",
];
const CACHE_DISK_FIELDS: &[&str] = &["path", "max_bytes", "max_size", "max_entries"];
const REWRITE_FIELDS: &[&str] = &["strip_prefix", "regexp", "replacement", "add_prefix"];
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
const COMPRESS_FIELDS: &[&str] = &[
    "encodings",
    "level",
    "gzip_level",
    "brotli_level",
    "zstd_level",
    "min_bytes",
    "min_length",
    "max_in_flight",
];
const BASIC_AUTH_FIELDS: &[&str] = &["realm", "users", "user_header", "forward_authorization"];
const BASIC_USER_FIELDS: &[&str] = &["username", "password_hash"];
const CLIENT_CERT_FIELDS: &[&str] = &["optional", "subject_header"];
const FORWARD_AUTH_FIELDS: &[&str] = &[
    "upstream",
    "uri",
    "request_headers",
    "copy_headers",
    "timeout",
];
const CIRCUIT_BREAKER_FIELDS: &[&str] = &[
    "window",
    "min_requests",
    "error_ratio",
    "statuses",
    "slow_threshold",
    "slow_ratio",
    "open_duration",
    "half_open_requests",
];
const JWT_FIELDS: &[&str] = &[
    "jwks_url",
    "jwks_refresh_interval",
    "jwks_refresh",
    "public_key_files",
    "hmac_secret_file",
    "algorithms",
    "issuer",
    "audience",
    "leeway",
    "token_from",
    "claims_headers",
];
const RESPOND_FIELDS: &[&str] = &["status", "headers", "body"];
const STATIC_FIELDS: &[&str] = &[
    "root",
    "index",
    "max_file_bytes",
    "dotfiles",
    "disk_io",
    "check_interval",
    "precompressed",
    "compress",
    "browse",
    "browse_max_entries",
    "strip_prefix",
    "try_files",
];
// max_cache_bytes and default_type wait on akio API requests A15 and A14.
const STATIC_FIT_ONLY: &[&str] = &["max_cache_bytes", "default_type"];
/// The fields of a `fastcgi` handler's params (I75), read in `fastcgi.rs`.
pub(crate) const FASTCGI_FIELDS: &[&str] = &[
    "address",
    "root",
    "split_path",
    "index",
    "try_files",
    "dotfiles",
    "env",
    "max_conns",
    "max_idle_conns",
    "max_pending",
    "connect_timeout",
    "response_header_timeout",
    "response_idle_timeout",
    "timeout",
    "idle_timeout",
    "max_header_bytes",
];
const PROXY_FIELDS: &[&str] = &[
    "upstream",
    "trusted_proxies",
    "host_header",
    "timeout",
    "retry",
    "tunnel_idle_timeout",
    "upstreams",
    "failover",
    "mirror",
];
const SPLIT_FIELDS: &[&str] = &["upstream", "weight"];
const MIRROR_FIELDS: &[&str] = &["upstream", "percent"];
const RETRY_FIELDS: &[&str] = &[
    "attempts",
    "statuses",
    "budget_ratio",
    "budget_min_per_second",
];
// splice_uploads (splice(2), which needs unsafe or an akio API) is not planned yet.
const PROXY_FIT_ONLY: &[&str] = &["splice_uploads"];
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
    "hash_key",
    "sticky_cookie",
    "max_fails",
    "fail_timeout",
    "health_check",
    "response_timeout",
    "response_header_timeout",
    "response_idle_timeout",
    "tls_server_name",
    "tls_ca_file",
    "tls_cert_file",
    "tls_key_file",
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
const STICKY_COOKIE_FIELDS: &[&str] = &["name", "path", "max_age", "secure"];
const UPSTREAM_FIT_ONLY: &[&str] = &["deny_internal"];

/// One file's part of a config, before the parts are merged.
pub(crate) struct Part {
    /// `schema_version`, with its position, when the file gives it.
    schema_version: Option<(i64, usize)>,
    listeners: Vec<At<Listener>>,
    routes: Vec<At<Route>>,
    upstreams: Vec<At<Upstream>>,
    services: Vec<At<Service>>,
    /// `limits`, with its position, when the file gives it.
    limits: Option<(Limits, usize)>,
    /// `access_log`, with the position of its key, when the file gives it.
    access_log: Option<(AccessLog, usize)>,
    /// `admin`, with the position of its key, when the file gives it.
    admin: Option<(Admin, usize)>,
    /// `metrics`, with the position of its key, when the file gives it.
    metrics: Option<(Metrics, usize)>,
    /// `acme`, with the position of its key, when the file gives it.
    acme: Option<(Acme, usize)>,
    /// `auto_https`, with where a problem with its ports is reported, and the position of its
    /// key, when the file gives it.
    auto_https: Option<((crate::AutoHttps, usize), usize)>,
    /// `telemetry`, with the position of its key, when the file gives it.
    telemetry: Option<(Telemetry, usize)>,
    providers: Vec<At<crate::providers::Provider>>,
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
                let empty_tls = crate::auto_https::empty_tls(n);
                let mut l = At::new(file, p, n, listener(d, n, p)?);
                l.empty_tls = empty_tls;
                Some(l)
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
    let services = o
        .take("services")
        .map(|v| {
            d.array(v, "services", |d, n, p| {
                Some(At::new(file, p, n, service(d, n, p)?))
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
    let acme = o
        .take("acme")
        .and_then(|v| acme(d, v, "acme").map(|a| (a, v.off)));
    let auto_https = o.take("auto_https").and_then(|v| {
        crate::auto_https::decode(d, v, "auto_https", AUTO_HTTPS_FIELDS).map(|a| (a, v.off))
    });
    let telemetry = o
        .take("telemetry")
        .and_then(|v| telemetry(d, v, "telemetry").map(|t| (t, v.off)));
    let providers = o
        .take("providers")
        .map(|v| {
            d.array(v, "providers", |d, n, p| {
                Some(At::new(file, p, n, crate::providers::decode(d, n, p)?))
            })
        })
        .unwrap_or_default();
    let doc_off = o.off;
    o.finish(d, CONFIG_FIELDS, CONFIG_FIT_ONLY);
    Some(Part {
        schema_version,
        listeners,
        routes,
        upstreams,
        services,
        limits,
        access_log,
        admin,
        metrics,
        acme,
        auto_https,
        telemetry,
        providers,
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
    let mut services = Vec::new();
    let mut limits: Option<(Limits, usize)> = None;
    let mut first_doc: Option<(usize, usize)> = None;
    let mut access_log: Option<(AccessLog, usize)> = None;
    let mut admin: Option<(Admin, usize)> = None;
    let mut metrics: Option<(Metrics, usize, usize)> = None;
    let mut acme: Option<(Acme, usize)> = None;
    let mut auto_https: Option<((crate::AutoHttps, usize), usize)> = None;
    let mut telemetry: Option<(Telemetry, usize)> = None;
    let mut providers = Vec::new();
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
        if let Some((a, off)) = p.acme {
            once(ds, &mut acme, a, file, off, "acme");
        }
        if let Some((a, off)) = p.auto_https {
            once(ds, &mut auto_https, a, file, off, "auto_https");
        }
        if let Some((t, off)) = p.telemetry {
            once(ds, &mut telemetry, t, file, off, "telemetry");
        }
        listeners.extend(p.listeners);
        routes.extend(p.routes);
        upstreams.extend(p.upstreams);
        services.extend(p.services);
        providers.extend(p.providers);
    }
    crate::providers::check(
        ds,
        providers
            .iter()
            .map(|a: &At<_>| (a.file, a.off, a.path.as_str(), &a.v)),
    );
    check_listeners(ds, &listeners);
    let served = served_listeners(ds, &listeners, &routes, auto_https, first_doc);
    let names: Vec<&str> = served.iter().map(|l| l.name.as_str()).collect();
    check_routes(ds, &routes, &names, &listeners);
    check_upstreams(ds, &upstreams);
    check_services(ds, &services, &listeners, &upstreams);
    check_acme_names(ds, &listeners, acme.as_ref().map(|(a, _)| a));
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
        listeners: served
            .into_iter()
            .map(|l| Listener {
                max_body_bytes: limits.max_body_bytes,
                ..l
            })
            .collect(),
        routes: routes.into_iter().map(|a| a.v).collect(),
        upstreams: upstreams.into_iter().map(|a| a.v).collect(),
        limits,
        access_log: access_log.map(|(a, _)| a),
        admin: admin.map(|(a, _)| a),
        metrics: metrics.map(|(m, _, _)| m),
        acme: acme.map(|(a, _)| a),
        auto_https: auto_https.map(|((a, _), _)| a),
        services: services.into_iter().map(|a| a.v).collect(),
        telemetry: telemetry.map(|(t, _)| t),
        providers: providers.into_iter().map(|a| a.v).collect(),
    }
}

/// The listeners the config is served on: those written, or those HTTPS by default makes
/// ([`crate::auto_https`]), with fit's checks of them (config/validate.go). A config with no
/// listeners and no routes is let through, as akit's start with only the Admin API (fit's with
/// only its console or its roles).
fn served_listeners(
    ds: &mut [Decoder<'_>],
    listeners: &[At<Listener>],
    routes: &[At<Route>],
    auto_https: Option<((crate::AutoHttps, usize), usize)>,
    first_doc: Option<(usize, usize)>,
) -> Vec<Listener> {
    use crate::auto_https::{Effective, effective};
    let (auto, auto_at) = match auto_https {
        Some(((a, off), file)) => (a, Some((file, off))),
        None => (crate::AutoHttps::default(), None),
    };
    if listeners.is_empty() && routes.is_empty() {
        return Vec::new();
    }
    let written = listeners
        .iter()
        .map(|l| (l.v.clone(), l.empty_tls.is_some()))
        .collect();
    let routes: Vec<&Route> = routes.iter().map(|r| &r.v).collect();
    match effective(&auto, written, &routes) {
        Effective::Written(served, problems) => {
            for (i, msg) in problems {
                if let Some(l) = listeners.get(i) {
                    if let (Some(off), Some(d)) = (l.empty_tls, ds.get_mut(l.file)) {
                        d.error(off, &crate::error::join_field(&l.path, "tls"), msg);
                    }
                }
            }
            served
        }
        Effective::Made(served) => served,
        Effective::None(msg) => {
            // fit's, with no position: about the config as a whole.
            if let Some(d) = first_doc.and_then(|(f, _)| ds.get_mut(f)) {
                d.errors.push(crate::error::Error {
                    file: d.file().map(str::to_owned),
                    path: "listeners".to_owned(),
                    line: 0,
                    col: 0,
                    msg: msg.to_owned(),
                });
            }
            Vec::new()
        }
        Effective::OnePort(port) => {
            if let Some((file, off)) = auto_at {
                if let Some(d) = ds.get_mut(file) {
                    d.error(
                        off,
                        "auto_https.http_port",
                        format!("the HTTP port is the HTTPS port, {port}"),
                    );
                }
            }
            Vec::new()
        }
    }
}

/// Keeps `v`, a field at the top given by `file` at `off`, in `slot`, or reports it as given by
/// two files, as fit's adapter does: only the lists at the top may be split across files.
fn once<T>(
    ds: &mut [Decoder<'_>],
    slot: &mut Option<(T, usize)>,
    v: T,
    file: usize,
    off: usize,
    key: &str,
) {
    match slot {
        Some((_, first_file)) => {
            let name = ds
                .get(*first_file)
                .and_then(Decoder::file)
                .unwrap_or_default()
                .to_owned();
            if let Some(d) = ds.get_mut(file) {
                d.error(
                    off,
                    key,
                    format!(
                        "also given in {name}: only the lists at the top of a config (listeners, routes, upstreams, services, providers) may be split across files"
                    ),
                );
            }
        }
        None => *slot = Some((v, file)),
    }
}

/// `acme`, as fit's `config.ACME` and its checks (config/validate.go).
fn acme(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Acme> {
    let mut o = d.object(n, path)?;
    let mut a = Acme::default();
    let string = |o: &mut Obj<'_>, d: &mut Decoder<'_>, key: &str| {
        o.take(key)
            .and_then(|v| d.string(v, &o.at(key)).map(|s| (v.off, s)))
    };
    if let Some((off, s)) = string(&mut o, d, "directory").filter(|(_, s)| !s.is_empty()) {
        if !http_url(&s) {
            d.error(
                off,
                &o.at("directory"),
                format!("{s:?} is not an http or https URL"),
            );
        }
        a.directory = s;
    }
    if let Some((_, s)) = string(&mut o, d, "email") {
        a.email = s;
    }
    if let Some((_, s)) = string(&mut o, d, "ca_file") {
        a.ca_file = s;
    }
    if let Some((_, s)) = string(&mut o, d, "storage") {
        a.storage = s;
    }
    if let Some(v) = o.take("max_concurrent") {
        // fit's int, at least 0; 0 is the default.
        if let Some(n) = d
            .int(v, &o.at("max_concurrent"), 0, i64::from(u16::MAX))
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n != 0)
        {
            a.max_concurrent = n;
        }
    }
    for (key, slot) in [
        ("timeout", &mut a.timeout),
        ("request_timeout", &mut a.request_timeout),
    ] {
        if let Some(t) = o
            .take(key)
            .and_then(|v| d.duration(v, &o.at(key)))
            .filter(|t| !t.is_zero())
        {
            *slot = t;
        }
    }
    if let Some(v) = o.take("challenges") {
        let p = o.at("challenges");
        let mut list = Vec::new();
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            let c = match s.as_str() {
                "http-01" => Challenge::Http01,
                "tls-alpn-01" => Challenge::TlsAlpn01,
                _ => {
                    d.error(
                        off,
                        &at,
                        format!("{s:?} is not one of \"http-01\", \"tls-alpn-01\""),
                    );
                    continue;
                }
            };
            if list.contains(&c) {
                d.error(off, &at, format!("{s:?} is listed twice"));
            }
            list.push(c);
        }
        if !list.is_empty() {
            a.challenges = list;
        }
    }
    if let Some(v) = o.take("on_demand") {
        a.on_demand = on_demand(d, v, &o.at("on_demand"));
    }
    o.finish(d, ACME_FIELDS, ACME_FIT_ONLY);
    Some(a)
}

/// `acme.on_demand`, as fit's `config.OnDemand` and its checks.
fn on_demand(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<OnDemand> {
    let mut o = d.object(n, path)?;
    let mut od = OnDemand::new("");
    match o.take("ask") {
        None => d.error(n.off, &o.at("ask"), "missing required field"),
        Some(v) => {
            if let Some(s) = d.string(v, &o.at("ask")) {
                if !http_url(&s) {
                    d.error(
                        v.off,
                        &o.at("ask"),
                        format!("{s:?} is not an http or https URL"),
                    );
                }
                od.ask = s;
            }
        }
    }
    for (key, slot) in [("rate", &mut od.rate), ("max_names", &mut od.max_names)] {
        if let Some(v) = o.take(key) {
            // fit's int, at least 0; 0 is the default.
            if let Some(n) = d
                .int(v, &o.at(key), 0, i64::from(u32::MAX))
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n != 0)
            {
                *slot = n;
            }
        }
    }
    for (key, slot) in [
        ("interval", &mut od.interval),
        ("timeout", &mut od.timeout),
        ("idle_timeout", &mut od.idle_timeout),
    ] {
        if let Some(t) = o
            .take(key)
            .and_then(|v| d.duration(v, &o.at(key)))
            .filter(|t| !t.is_zero())
        {
            *slot = t;
        }
    }
    o.finish(d, ON_DEMAND_FIELDS, &[]);
    Some(od)
}

/// Whether `s` is an `http` or `https` URL with a host, as fit checks `acme.directory`.
fn http_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| !r.split(['/', '?', '#']).next().unwrap_or("").is_empty())
}

/// fit's checks of the names listeners list in `tls.acme` (config/validate.go): host names, no
/// wildcards (HTTP-01 cannot obtain them), no IP addresses, none twice in a listener; and a
/// listener requiring client certificates needs HTTP-01 answered; `on_demand` needs
/// `acme.on_demand`.
fn check_acme_names(ds: &mut [Decoder<'_>], listeners: &[At<Listener>], acme: Option<&Acme>) {
    let http01 = acme.is_none_or(|a| a.challenges.contains(&Challenge::Http01));
    for l in listeners {
        let Some(t) = &l.v.tls else { continue };
        let tls_path = crate::error::join_field(&l.path, "tls");
        if t.on_demand && acme.is_none_or(|a| a.on_demand.is_none()) {
            l.error(
                ds,
                &crate::error::join_field(&tls_path, "on_demand"),
                "on_demand needs acme.on_demand, which says whom to ask whether a name may have a certificate".to_owned(),
            );
        }
        let path = crate::error::join_field(&tls_path, "acme");
        let mut seen: Vec<String> = Vec::new();
        for (i, name) in t.acme.iter().enumerate() {
            let at = crate::error::join_index(&path, i);
            let lower = name.to_ascii_lowercase();
            let msg = match check_host(name) {
                Err(msg) => Some(msg),
                Ok(()) if name.starts_with("*.") || name.starts_with("**.") => Some(format!(
                    "{name:?} is a wildcard, which the HTTP-01 challenge cannot obtain"
                )),
                Ok(()) if name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() => {
                    Some(format!("{name:?} is an IP address; acme takes host names"))
                }
                Ok(()) if seen.contains(&lower) => Some(format!("{name:?} is listed twice")),
                Ok(()) => None,
            };
            if let Some(msg) = msg {
                l.error(ds, &at, msg);
            }
            seen.push(lower);
        }
        if let Some(ca) = &t.client_auth {
            if ca.mode == ClientAuthMode::Require && (!t.acme.is_empty() || t.on_demand) && !http01
            {
                l.error(
                    ds,
                    &crate::error::join_field(&tls_path, "client_auth"),
                    "a listener that requires client certificates cannot answer TLS-ALPN-01, the only challenge acme.challenges allows; set mode verify_if_given or allow http-01".to_owned(),
                );
            }
        }
    }
}

/// A decoded element with its file, path and the position of its node.
struct At<T> {
    file: usize,
    path: String,
    off: usize,
    v: T,
    /// For a listener whose `tls` asks for nothing, the position of its `tls`
    /// ([`crate::auto_https::empty_tls`]).
    empty_tls: Option<usize>,
}

impl<T> At<T> {
    fn new(file: usize, path: &str, n: &Node, v: T) -> At<T> {
        At {
            file,
            path: path.to_owned(),
            off: n.off,
            v,
            empty_tls: None,
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

/// `telemetry`, as fit's `config.Telemetry` and its checks (`checkTelemetry`).
fn telemetry(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Telemetry> {
    let mut o = d.object(n, path)?;
    let string = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        o.take(key)
            .and_then(|v| Some((d.string(v, &o.at(key))?, v.off)))
    };
    let endpoint = string(d, &mut o, "endpoint");
    if let Some((ep, off)) = &endpoint {
        if let Err(msg) = check_endpoint(ep) {
            d.error(*off, &o.at("endpoint"), msg);
        }
    }
    let headers_file = string(d, &mut o, "headers_file").map(|s| s.0);
    let ca_file = string(d, &mut o, "ca_file").map(|s| s.0);
    let cert_file = string(d, &mut o, "cert_file");
    let key_file = string(d, &mut o, "key_file");
    let service_name = string(d, &mut o, "service_name").map(|s| s.0);
    let timeout = o
        .take("timeout")
        .and_then(|v| d.duration(v, &o.at("timeout")))
        .filter(|t| !t.is_zero())
        .unwrap_or(DEFAULT_TELEMETRY_TIMEOUT);
    let traces = o.take("traces").and_then(|v| {
        let at = o.at("traces");
        let mut t = d.object(v, &at)?;
        let sample_ratio = t
            .take("sample_ratio")
            .and_then(|v| ratio(d, v, &t.at("sample_ratio")))
            .unwrap_or(0.0);
        let max_queue_bytes = t
            .take("max_queue_bytes")
            .and_then(|v| d.int(v, &t.at("max_queue_bytes"), 0, i64::MAX))
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n != 0)
            .unwrap_or(DEFAULT_TRACES_MAX_QUEUE_BYTES);
        let interval = t
            .take("interval")
            .and_then(|v| d.duration(v, &t.at("interval")))
            .filter(|i| !i.is_zero())
            .unwrap_or(DEFAULT_TRACES_INTERVAL);
        t.finish(d, TRACES_EXPORT_FIELDS, &[]);
        Some(TracesExport {
            sample_ratio,
            max_queue_bytes,
            interval,
        })
    });
    let metrics = o.take("metrics").and_then(|v| {
        let at = o.at("metrics");
        let mut m = d.object(v, &at)?;
        let interval = m
            .take("interval")
            .and_then(|v| d.duration(v, &m.at("interval")))
            .filter(|i| !i.is_zero())
            .unwrap_or(DEFAULT_METRICS_INTERVAL);
        m.finish(d, METRICS_EXPORT_FIELDS, &[]);
        Some(MetricsExport { interval })
    });
    // fit's checkTLSFiles.
    match (&cert_file, &key_file) {
        (Some((c, off)), k) if !c.is_empty() && k.as_ref().is_none_or(|k| k.0.is_empty()) => {
            d.error(*off, &o.at("cert_file"), "cert_file needs key_file");
        }
        (c, Some((k, off))) if !k.is_empty() && c.as_ref().is_none_or(|c| c.0.is_empty()) => {
            d.error(*off, &o.at("key_file"), "key_file needs cert_file");
        }
        _ => {}
    }
    if traces.is_none() && metrics.is_none() {
        d.error(
            o.off,
            path,
            "set traces, metrics or both: telemetry exports nothing without them",
        );
    }
    o.finish(d, TELEMETRY_FIELDS, &[]);
    Some(Telemetry {
        endpoint: endpoint.map(|e| e.0).filter(|e| !e.is_empty()).map_or_else(
            || DEFAULT_TELEMETRY_ENDPOINT.to_owned(),
            |e| e.strip_suffix('/').unwrap_or(&e).to_owned(),
        ),
        headers_file: headers_file.unwrap_or_default(),
        ca_file: ca_file.unwrap_or_default(),
        cert_file: cert_file.map(|c| c.0).unwrap_or_default(),
        key_file: key_file.map(|k| k.0).unwrap_or_default(),
        service_name: service_name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_TELEMETRY_SERVICE_NAME.to_owned()),
        timeout,
        traces,
        metrics,
    })
}

/// fit's `checkEndpoint`: `http://` or `https://` and a host and port alone. An empty one is
/// the default.
pub(crate) fn check_endpoint(ep: &str) -> Result<(), String> {
    if ep.is_empty() {
        return Ok(());
    }
    let Some(rest) = ep
        .strip_prefix("http://")
        .or_else(|| ep.strip_prefix("https://"))
    else {
        return Err(format!(
            "{ep:?} is neither http://<host>:<port> nor https://<host>:<port>"
        ));
    };
    // As Go's net.SplitHostPort: a bracketed IPv6 host, or a host without a colon.
    let port = match rest.strip_prefix('[') {
        Some(r) => r.split_once(']').and_then(|(_, p)| p.strip_prefix(':')),
        None => rest
            .rsplit_once(':')
            .filter(|(h, _)| !h.contains(':'))
            .map(|(_, p)| p),
    };
    let ok = port
        .and_then(|p| {
            p.parse::<u32>()
                .ok()
                .filter(|_| p.bytes().all(|b| b.is_ascii_digit()))
        })
        .is_some_and(|n| (1..=65535).contains(&n))
        && !rest.contains(['/', '?', '#']);
    if ok {
        Ok(())
    } else {
        Err(format!("{ep:?} is not a host and port alone"))
    }
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
                "tcp" => Protocol::Tcp,
                "udp" => Protocol::Udp,
                "socks" => Protocol::Socks,
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
    let tls_at = o.take("tls").map(|v| (v, o.at("tls")));
    let tls = tls_at.as_ref().and_then(|(v, p)| tls(d, v, p));
    let pp_at = o
        .take("proxy_protocol")
        .map(|v| (v, o.at("proxy_protocol")));
    let proxy_protocol = pp_at
        .as_ref()
        .and_then(|(v, p)| listener_proxy_protocol(d, v, p));
    let socks_at = o.take("socks").map(|v| (v.off, o.at("socks"), v));
    // The defaults never hold socks: only the protocols written may.
    if let Some((off, p, _)) = &socks_at {
        if !protocols.iter().any(|(_, _, p)| *p == Protocol::Socks) {
            d.error(
                *off,
                p,
                "socks is for a listener whose protocols include socks",
            );
        }
    }
    let socks = socks_at
        .as_ref()
        .and_then(|(_, p, v)| forward_proxy(d, v, p));
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
            Protocol::Udp if protocols.len() > 1 => d.error(
                *off,
                at,
                "a udp listener serves UDP alone: list udp without other protocols",
            ),
            Protocol::Socks if socks_at.is_none() => d.error(
                *off,
                at,
                "socks needs the listener's socks, which says who may use it",
            ),
            // tcp beside HTTP: told apart by what a client sends first (I53, akit-listener).
            _ => {}
        }
    }
    if let Some((v, p)) = tls_at.filter(|_| {
        tls.is_some() && !protocols.is_empty() && protocols.iter().all(|(_, _, p)| !p.is_http())
    }) {
        d.error(
            v.off,
            &p,
            "tls serves the listener's HTTP, and this one serves none: tcp and udp are proxied as they are, TLS and all",
        );
    }
    let mut protocols: Vec<Protocol> = protocols.into_iter().map(|(_, _, p)| p).collect();
    if protocols.is_empty() {
        protocols = if tls.is_some() {
            DEFAULT_TLS_PROTOCOLS.to_vec()
        } else {
            DEFAULT_PROTOCOLS.to_vec()
        };
    }
    // The header comes at the start of a TCP connection: neither udp nor HTTP/3 takes one.
    if let Some((v, p)) = &pp_at {
        if protocols
            .iter()
            .all(|p| matches!(p, Protocol::Udp | Protocol::H3))
        {
            d.error(
                v.off,
                p,
                "a PROXY protocol header comes at the start of a TCP connection; this listener \
                 takes UDP alone",
            );
        }
    }
    Some(Listener {
        name: name?,
        address: address?,
        protocols,
        tls,
        // Set from the config's limits once the files are merged.
        max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        redirect_https: 0,
        proxy_protocol,
        socks,
    })
}

/// A forward proxy's rules, a listener's `socks` or the `forward_proxy` handler's params, as
/// fit's `config.ForwardProxy` and its `checkForwardProxy` (I55).
fn forward_proxy(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<ForwardProxy> {
    let mut o = d.object(n, path)?;
    let clients = o.require(d, "clients").map(|v| {
        let p = o.at("clients");
        let given = d.strings(v, &p);
        if given.is_empty() && matches!(v.kind, crate::json::Kind::Array(_)) {
            d.error(v.off, &p, "at least one address is needed");
        }
        let mut nets = Vec::new();
        for (i, (off, s)) in given.into_iter().enumerate() {
            match Network::parse(&s) {
                Some(net) => nets.push(net),
                None => d.error(
                    off,
                    &crate::error::join_index(&p, i),
                    format!("{s:?} is not an IP address or CIDR range"),
                ),
            }
        }
        nets
    });
    let dests = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        let mut out = Vec::new();
        let Some(v) = o.take(key) else {
            return out;
        };
        let p = o.at(key);
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            match destination(&s) {
                Some(dest) => out.push(dest),
                None => d.error(
                    off,
                    &crate::error::join_index(&p, i),
                    format!("{s:?} is not a host name, an IP address or a CIDR range"),
                ),
            }
        }
        out
    };
    let allow = dests(d, &mut o, "allow");
    let deny = dests(d, &mut o, "deny");
    let ports = o.take("ports").map_or_else(Vec::new, |v| {
        d.array(v, &o.at("ports"), |d, item, at| {
            let n = d.int(item, at, i64::MIN, i64::MAX)?;
            match u16::try_from(n) {
                Ok(port) if port > 0 => Some(port),
                _ => {
                    d.error(item.off, at, format!("{n} is not a port"));
                    None
                }
            }
        })
    });
    let timeout = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, default: Duration| {
        o.take(key)
            .and_then(|v| d.duration(v, &o.at(key)))
            .filter(|t| !t.is_zero())
            .unwrap_or(default)
    };
    let connect_timeout = timeout(
        d,
        &mut o,
        "connect_timeout",
        DEFAULT_FORWARD_CONNECT_TIMEOUT,
    );
    let idle_timeout = timeout(d, &mut o, "idle_timeout", DEFAULT_FORWARD_IDLE_TIMEOUT);
    o.finish(d, FORWARD_PROXY_FIELDS, &[]);
    Some(ForwardProxy {
        clients: clients?,
        allow,
        deny,
        ports,
        connect_timeout,
        idle_timeout,
    })
}

/// A destination of `allow` or `deny`, as fit's `checkForwardProxy` takes it and `dests.add`
/// reads it: an IP address or CIDR range, a host name, or a name under `*.`.
fn destination(s: &str) -> Option<Destination> {
    if let Some(net) = Network::parse(s) {
        return Some(Destination::Network(net));
    }
    // A host as fit's checkHost takes it, but no IP address (a bracketed one) and no `**.`.
    if s.starts_with("**.")
        || s.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
        || check_host(s).is_err()
    {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    Some(match lower.strip_prefix('*') {
        Some(rest) => Destination::Suffix(rest.to_owned()),
        None => Destination::Name(lower),
    })
}

/// A listener's `proxy_protocol`, as fit's `config.ProxyProtocol` and its checks.
fn listener_proxy_protocol(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
) -> Option<ListenerProxyProtocol> {
    let mut o = d.object(n, path)?;
    let trusted = o.require(d, "trusted").map(|v| {
        let p = o.at("trusted");
        let mut nets = Vec::new();
        let given = d.strings(v, &p);
        if given.is_empty() && matches!(v.kind, crate::json::Kind::Array(_)) {
            d.error(v.off, &p, "at least one address is needed");
        }
        for (i, (off, s)) in given.into_iter().enumerate() {
            match Network::parse(&s) {
                Some(net) => nets.push(net),
                None => d.error(
                    off,
                    &crate::error::join_index(&p, i),
                    format!("{s:?} is not an IP address or CIDR range"),
                ),
            }
        }
        nets
    });
    o.finish(d, LISTENER_PROXY_PROTOCOL_FIELDS, &[]);
    Some(ListenerProxyProtocol { trusted: trusted? })
}

/// `admin`, as fit's `config.Admin` and its `checkAdminAddress`.
fn admin(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Admin> {
    let mut o = d.object(n, path)?;
    let mut a = Admin::default();
    if let Some(v) = o.take("address") {
        let p = o.at("address");
        if let Some(s) = d.string(v, &p).filter(|s| !s.is_empty()) {
            if let Some(path) = s.strip_prefix("unix/") {
                if path.is_empty() {
                    d.error(v.off, &p, format!("{s:?} has no path after \"unix/\""));
                }
            } else if let Err(msg) = check_address(&s) {
                d.error(v.off, &p, msg);
            } else if !loopback_host(&s) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{s:?} is not a loopback address: the Admin API can change everything \
                         akit serves, so listen on localhost, 127.0.0.1 or [::1]; off loopback it \
                         needs a token (token_file) and TLS (cert_file and key_file), and akit \
                         does not serve the Admin API over TLS yet"
                    ),
                );
            }
            a.address = s;
        }
    }
    if let Some(v) = o.take("token_file") {
        a.token_file = d.string(v, &o.at("token_file")).filter(|s| !s.is_empty());
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
    let mut ok = true;
    a.disable_dashboard = flag(d, &mut o, "disable_dashboard", &mut ok);
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
    if let Some(v) = o.take("write_stall_timeout") {
        let p = o.at("write_stall_timeout");
        if let Some(t) = d.duration(v, &p).filter(|t| !t.is_zero()) {
            l.write_stall_timeout = t;
        }
    }
    o.finish(d, LIMITS_FIELDS, LIMITS_FIT_ONLY);
    Some(l)
}

fn tls(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Tls> {
    let mut o = d.object(n, path)?;
    let certificates = o
        .take("certificates")
        .map(|v| d.array(v, &o.at("certificates"), certificate))
        .unwrap_or_default();
    let client_auth = o
        .take("client_auth")
        .and_then(|v| client_auth(d, v, &o.at("client_auth")));
    let max_version = match o.take("max_version") {
        Some(v) => tls_version(d, v, &o.at("max_version")),
        None => TlsVersion::Tls13,
    };
    let acme = o
        .take("acme")
        .map(|v| {
            d.strings(v, &o.at("acme"))
                .into_iter()
                .map(|(_, s)| s)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut ok = true;
    let on_demand = flag(d, &mut o, "on_demand", &mut ok);
    // One with no certificates, no names and no on_demand gets its routes' host names
    // (auto_https), or is refused once the routes are read (`served_listeners`).
    o.finish(d, TLS_FIELDS, TLS_FIT_ONLY);
    Some(Tls {
        certificates,
        client_auth,
        max_version,
        acme,
        on_demand,
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
        // TLS 1.2 alone over TCP (akio's `tls12_only`, ADR-121); QUIC stays TLS 1.3.
        "1.2" => TlsVersion::Tls12,
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
    let mut middleware: Vec<Handler> = o
        .take("middleware")
        .map(|v| d.array(v, &o.at("middleware"), middleware))
        .unwrap_or_default();
    // A route that lets requests in on credentials has its cache treat every one as authorized.
    let authenticated = middleware.iter().any(|m| {
        matches!(
            m,
            Handler::BasicAuth(_)
                | Handler::ClientCert(_)
                | Handler::ForwardAuth(_)
                | Handler::Jwt(_)
        )
    });
    for m in &mut middleware {
        if let Handler::Cache(c) = m {
            c.authenticated = authenticated;
        }
    }
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
        "fastcgi" => crate::fastcgi::fastcgi(d, params, off, &params_path).map(Handler::FastCgi),
        "forward_proxy" => {
            let Some(n) = params else {
                d.error(
                    off,
                    &crate::error::join_field(&params_path, "clients"),
                    "missing required field",
                );
                return None;
            };
            forward_proxy(d, n, &params_path).map(Handler::ForwardProxy)
        }
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
        "basic_auth" => basic_auth(d, params, off, &params_path).map(Handler::BasicAuth),
        "client_cert" => client_cert(d, params, &params_path).map(Handler::ClientCert),
        "forward_auth" => forward_auth(d, params, off, &params_path).map(Handler::ForwardAuth),
        "circuit_breaker" => circuit_breaker(d, params, &params_path).map(Handler::CircuitBreaker),
        "compress" => compress(d, params, &params_path).map(Handler::Compress),
        "jwt" => jwt(d, params, off, &params_path).map(Handler::Jwt),
        "cache" => cache(d, params, &params_path).map(Handler::Cache),
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
        let replacement = o.take("replacement");
        if let Some(v) = replacement {
            r.replacement = d.string(v, &o.at("replacement")).unwrap_or_default();
        }
        if let Some(v) = o.take("regexp") {
            let p = o.at("regexp");
            r.regexp = d.string(v, &p).unwrap_or_default();
            if !r.regexp.is_empty() {
                if let Err(e) = akit_router::regexp(r.regexp.as_bytes()) {
                    d.error(v.off, &p, e.to_string());
                    ok = false;
                }
            }
        }
        if r.regexp.is_empty() && !r.replacement.is_empty() {
            let off = replacement.map_or(at, |v| v.off);
            d.error(off, &o.at("replacement"), "a replacement needs a regexp");
            ok = false;
        }
        o.finish(d, REWRITE_FIELDS, &[]);
    }
    if ok && r.strip_prefix.is_empty() && r.regexp.is_empty() && r.add_prefix.is_empty() {
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
    }
    ok.then_some(Redirect { to, status, regexp })
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

/// A list of encodings at `p`, each one of `br`, `zstd` and `gzip`; `once` refuses one listed
/// twice, as fit's `compress` does.
fn encodings(d: &mut Decoder<'_>, v: &Node, p: &str, once: bool) -> Vec<Encoding> {
    let mut list: Vec<Encoding> = Vec::new();
    for (i, (off, name)) in d.strings(v, p).into_iter().enumerate() {
        let at = crate::error::join_index(p, i);
        let e = match name.as_str() {
            "br" => Encoding::Br,
            "zstd" => Encoding::Zstd,
            "gzip" => Encoding::Gzip,
            _ => {
                d.error(
                    off,
                    &at,
                    format!("{name:?} is not one of \"br\", \"zstd\", \"gzip\""),
                );
                continue;
            }
        };
        if once && list.contains(&e) {
            d.error(off, &at, format!("{name:?} is listed twice"));
            continue;
        }
        list.push(e);
    }
    list
}

/// The `cache` middleware's params, checked and worked out as fit's `Cache.Provision` does.
fn cache(d: &mut Decoder<'_>, params: Option<&Node>, path: &str) -> Option<Cache> {
    let mut c = Cache {
        zone: DEFAULT_CACHE_ZONE.to_owned(),
        max_bytes: DEFAULT_CACHE_MAX_BYTES,
        max_entry_bytes: 0,
        default_ttl: None,
        disable_lock: false,
        lock_timeout: DEFAULT_CACHE_LOCK_TIMEOUT,
        stale_while_revalidate: None,
        stale_if_error: None,
        disk: None,
        authenticated: false,
    };
    let mut ok = true;
    let mut entry_at = (0, String::new());
    if let Some(n) = params {
        let mut o = d.object(n, path)?;
        if let Some(v) = o.take("zone") {
            match d.string(v, &o.at("zone")) {
                Some(z) if !z.is_empty() => c.zone = z,
                Some(_) => {}
                None => ok = false,
            }
        }
        if let Some(m) = size_field(d, &mut o, "max_bytes", "max_size", &mut ok) {
            c.max_bytes = m;
        }
        if let Some((m, off, at)) =
            size_field_at(d, &mut o, "max_entry_bytes", "max_entry_size", &mut ok)
        {
            c.max_entry_bytes = m;
            entry_at = (off, at);
        }
        if let Some(v) = o.take("default_ttl") {
            match d.duration(v, &o.at("default_ttl")) {
                Some(t) => c.default_ttl = Some(t).filter(|t| !t.is_zero()),
                None => ok = false,
            }
        }
        if let Some(v) = o.take("disable_lock") {
            match &v.kind {
                crate::json::Kind::Bool(b) => c.disable_lock = *b,
                other => {
                    d.error(
                        v.off,
                        &o.at("disable_lock"),
                        format!("expected a boolean, got {}", other.name()),
                    );
                    ok = false;
                }
            }
        }
        if let Some(v) = o.take("lock_timeout") {
            match d.duration(v, &o.at("lock_timeout")) {
                Some(t) if !t.is_zero() => c.lock_timeout = t,
                Some(_) => {}
                None => ok = false,
            }
        }
        for (key, slot) in [
            ("stale_while_revalidate", &mut c.stale_while_revalidate),
            ("stale_if_error", &mut c.stale_if_error),
        ] {
            if let Some(v) = o.take(key) {
                match d.duration(v, &o.at(key)) {
                    Some(t) => *slot = Some(t).filter(|t| !t.is_zero()),
                    None => ok = false,
                }
            }
        }
        if let Some(v) = o.take("disk") {
            c.disk = cache_disk(d, v, &o.at("disk"));
            ok &= c.disk.is_some();
        }
        o.finish(d, CACHE_FIELDS, &[]);
    }
    let share = c.max_bytes / CACHE_SHARDS;
    if c.max_entry_bytes == 0 {
        c.max_entry_bytes = DEFAULT_CACHE_MAX_ENTRY_BYTES.min(share);
    } else if c.max_entry_bytes > share {
        d.error(
            entry_at.0,
            &entry_at.1,
            format!("is more than a sixteenth of max_bytes ({share})"),
        );
        ok = false;
    }
    ok.then_some(c)
}

/// The `cache` middleware's `disk` param.
fn cache_disk(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<CacheDisk> {
    let mut o = d.object(n, path)?;
    let mut ok = true;
    let mut c = CacheDisk {
        path: String::new(),
        max_bytes: DEFAULT_CACHE_DISK_MAX_BYTES,
        max_entries: DEFAULT_CACHE_DISK_MAX_ENTRIES,
    };
    match o.require(d, "path") {
        Some(v) => match d.string(v, &o.at("path")) {
            Some(p) if !p.is_empty() => c.path = p,
            Some(_) => {
                d.error(v.off, &o.at("path"), "must not be empty");
                ok = false;
            }
            None => ok = false,
        },
        None => ok = false,
    }
    if let Some(m) = size_field(d, &mut o, "max_bytes", "max_size", &mut ok) {
        c.max_bytes = m;
    }
    if let Some(v) = o.take("max_entries") {
        match d
            .int(v, &o.at("max_entries"), 0, i64::MAX)
            .and_then(|n| u64::try_from(n).ok())
        {
            Some(0) => {}
            Some(n) => c.max_entries = n,
            None => ok = false,
        }
    }
    o.finish(d, CACHE_DISK_FIELDS, &[]);
    ok.then_some(c)
}

/// A size given as `key` or by its old name `old` (not both), at least 0; `None` when left out
/// or 0. Its offset and path, for an error about it, come along.
fn size_field_at(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    key: &str,
    old: &str,
    ok: &mut bool,
) -> Option<(u64, usize, String)> {
    let new = o.take(key).map(|v| (v, o.at(key)));
    let was = o.take(old).map(|v| (v, o.at(old)));
    if let (Some(_), Some((v, at))) = (&new, &was) {
        d.error(v.off, at, format!("renamed {key:?}, which is also given"));
        *ok = false;
        return None;
    }
    let (v, at) = new.or(was)?;
    match d
        .int(v, &at, 0, i64::MAX)
        .and_then(|n| u64::try_from(n).ok())
    {
        Some(0) => None,
        Some(n) => Some((n, v.off, at)),
        None => {
            *ok = false;
            None
        }
    }
}

/// [`size_field_at`] without where.
fn size_field(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    key: &str,
    old: &str,
    ok: &mut bool,
) -> Option<u64> {
    size_field_at(d, o, key, old, ok).map(|(n, _, _)| n)
}

/// The `compress` middleware's params, checked and its levels worked out as fit's
/// `Compress.Provision` does.
fn compress(d: &mut Decoder<'_>, params: Option<&Node>, path: &str) -> Option<Compress> {
    let mut c = Compress {
        encodings: vec![Encoding::Zstd, Encoding::Gzip, Encoding::Br],
        gzip_level: 1,
        brotli_level: 1,
        zstd_level: 1,
        min_bytes: DEFAULT_COMPRESS_MIN_BYTES,
        max_in_flight: DEFAULT_COMPRESS_MAX_IN_FLIGHT,
    };
    let Some(n) = params else {
        return Some(c);
    };
    let mut o = d.object(n, path)?;
    let mut ok = true;
    if let Some(v) = o.take("encodings") {
        let list = encodings(d, v, &o.at("encodings"), true);
        if !list.is_empty() {
            c.encodings = list;
        }
    }
    // fastest, default and better, as gzip's, brotli's and zstd's levels.
    let mut named = [1, 1, 1];
    if let Some(v) = o.take("level") {
        match d.string(v, &o.at("level")).as_deref() {
            None => ok = false,
            Some("fastest") => {}
            Some("default") => named = [5, 4, 3],
            Some("better") => named = [7, 6, 6],
            Some(other) => {
                d.error(
                    v.off,
                    &o.at("level"),
                    format!("{other:?} is not one of \"fastest\", \"default\", \"better\""),
                );
                ok = false;
            }
        }
    }
    c.gzip_level = compress_level(d, &mut o, "gzip_level", 9, named[0], &mut ok);
    c.brotli_level = compress_level(d, &mut o, "brotli_level", 11, named[1], &mut ok);
    c.zstd_level = compress_level(d, &mut o, "zstd_level", 22, named[2], &mut ok);
    let min_bytes = positive(d, &mut o, "min_bytes", 0, &mut ok);
    let min_length = positive(d, &mut o, "min_length", 0, &mut ok);
    if let (Some(_), Some(_)) = (min_bytes, min_length) {
        d.error(
            n.off,
            &o.at("min_length"),
            "renamed \"min_bytes\", which is also given",
        );
        ok = false;
    }
    if let Some(m) = min_bytes.or(min_length) {
        c.min_bytes = m;
    }
    if let Some(m) = positive(d, &mut o, "max_in_flight", 0, &mut ok) {
        c.max_in_flight = m;
    }
    o.finish(d, COMPRESS_FIELDS, &[]);
    ok.then_some(c)
}

/// One of the `compress` middleware's levels by number, 1 to `max`: `named` (the one `level`
/// gives it) when left out or 0.
fn compress_level(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    key: &str,
    max: i64,
    named: u32,
    ok: &mut bool,
) -> u32 {
    let Some(v) = o.take(key) else {
        return named;
    };
    match d
        .int(v, &o.at(key), 0, max)
        .and_then(|n| u32::try_from(n).ok())
    {
        Some(0) => named,
        Some(n) => n,
        None => {
            *ok = false;
            named
        }
    }
}

/// A count of at least `min`; `None` when left out or 0, for the default.
fn positive(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    key: &str,
    min: i64,
    ok: &mut bool,
) -> Option<u64> {
    let v = o.take(key)?;
    let n = d
        .int(v, &o.at(key), min, i64::MAX)
        .and_then(|n| u64::try_from(n).ok());
    if n.is_none() {
        *ok = false;
    }
    n.filter(|n| *n != 0)
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

/// A boolean member of `o`, false when left out.
fn flag(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, ok: &mut bool) -> bool {
    match o.take(key).map(|v| (v.off, &v.kind)) {
        None => false,
        Some((_, crate::json::Kind::Bool(b))) => *b,
        Some((off, other)) => {
            d.error(
                off,
                &o.at(key),
                format!("expected a boolean, got {}", other.name()),
            );
            *ok = false;
            false
        }
    }
}

/// The `basic_auth` middleware's params, checked as fit's `Basic.Provision` checks them.
fn basic_auth(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<BasicAuth> {
    let mut o = limit_params(d, params, off, path, "users")?;
    let mut ok = true;
    let mut realm = DEFAULT_REALM.to_owned();
    if let Some(v) = o.take("realm") {
        let p = o.at("realm");
        match d.string(v, &p) {
            Some(r)
                if r.bytes()
                    .any(|c| c < 0x20 || c == 0x7f || c == b'"' || c == b'\\') =>
            {
                d.error(
                    v.off,
                    &p,
                    format!("{r:?} has a quote, a backslash or a control character"),
                );
                ok = false;
            }
            Some(r) if !r.is_empty() => realm = r,
            Some(_) => {}
            None => ok = false,
        }
    }
    let mut users: Vec<BasicUser> = Vec::new();
    if let Some(v) = o.require(d, "users") {
        let at = o.at("users");
        let list = d.array(v, &at, |d, n, p| basic_user(d, n, p).map(|u| (n.off, u)));
        if list.is_empty() && ok {
            d.error(v.off, &at, "at least one user is needed");
        }
        for (i, (uoff, u)) in list.into_iter().enumerate() {
            if users.iter().any(|x| x.username == u.username) {
                d.error(
                    uoff,
                    &at,
                    format!("user {i}: {:?} is listed twice", u.username),
                );
                ok = false;
            }
            users.push(u);
        }
    } else {
        ok = false;
    }
    let mut user_header = String::new();
    if let Some(v) = o.take("user_header") {
        let p = o.at("user_header");
        match d.string(v, &p) {
            Some(h) if !h.is_empty() && !is_token(&h) => {
                d.error(v.off, &p, format!("{h:?} is not a field name"));
                ok = false;
            }
            Some(h) => user_header = h,
            None => ok = false,
        }
    }
    let forward_authorization = flag(d, &mut o, "forward_authorization", &mut ok);
    o.finish(d, BASIC_AUTH_FIELDS, &[]);
    (ok && !users.is_empty()).then_some(BasicAuth {
        realm,
        users,
        user_header,
        forward_authorization,
    })
}

/// One of `basic_auth`'s `users`.
fn basic_user(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<BasicUser> {
    let mut o = d.object(n, path)?;
    let mut ok = true;
    let username = o.require(d, "username").and_then(|v| {
        let p = o.at("username");
        let u = d.string(v, &p)?;
        if u.is_empty() || u.contains([':', '\0', '\r', '\n']) {
            d.error(
                v.off,
                &p,
                format!(
                    "{u:?} is not a user name: it is empty, or has a colon or a control character"
                ),
            );
            return None;
        }
        Some(u)
    });
    let password_hash = o.require(d, "password_hash").and_then(|v| {
        let p = o.at("password_hash");
        let h = d.string(v, &p)?;
        if let Err(why) = akio_crypto::bcrypt::Hash::parse(&h) {
            d.error(
                v.off,
                &p,
                format!("password_hash is not a bcrypt hash: {why}"),
            );
            return None;
        }
        Some(h)
    });
    o.finish(d, BASIC_USER_FIELDS, &[]);
    ok &= username.is_some() && password_hash.is_some();
    ok.then(|| BasicUser {
        username: username.unwrap_or_default(),
        password_hash: password_hash.unwrap_or_default(),
    })
}

/// The `client_cert` middleware's params.
fn client_cert(d: &mut Decoder<'_>, params: Option<&Node>, path: &str) -> Option<ClientCert> {
    let mut c = ClientCert::default();
    let Some(n) = params else {
        return Some(c);
    };
    let mut o = d.object(n, path)?;
    let mut ok = true;
    c.optional = flag(d, &mut o, "optional", &mut ok);
    if let Some(v) = o.take("subject_header") {
        let p = o.at("subject_header");
        match d.string(v, &p) {
            Some(h) if !h.is_empty() && !is_token(&h) => {
                d.error(v.off, &p, format!("{h:?} is not a field name"));
                ok = false;
            }
            Some(h) => c.subject_header = h,
            None => ok = false,
        }
    }
    o.finish(d, CLIENT_CERT_FIELDS, &[]);
    ok.then_some(c)
}

/// The `forward_auth` middleware's params, checked as fit's `ForwardAuth.Provision` checks them.
fn forward_auth(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<ForwardAuth> {
    let mut o = limit_params(d, params, off, path, "upstream")?;
    let mut ok = true;
    let upstream = match o.require(d, "upstream") {
        Some(v) => {
            let p = o.at("upstream");
            match d.string(v, &p) {
                Some(u) if u.is_empty() => {
                    d.error(v.off, &p, "the upstream's name is empty");
                    None
                }
                u => u,
            }
        }
        None => None,
    };
    ok &= upstream.is_some();
    let mut uri = "/".to_owned();
    if let Some(v) = o.take("uri") {
        let p = o.at("uri");
        match d.string(v, &p) {
            Some(u) if u.is_empty() => {}
            Some(u) if !is_request_path(&u) => {
                d.error(
                    v.off,
                    &p,
                    format!("{u:?} is not a path, with a query or not"),
                );
                ok = false;
            }
            Some(u) => uri = u,
            None => ok = false,
        }
    }
    let names = |d: &mut Decoder<'_>, o: &mut Obj<'_>, field: &str, ok: &mut bool| {
        let mut out = Vec::new();
        if let Some(v) = o.take(field) {
            let at = o.at(field);
            for (i, (off, h)) in d.strings(v, &at).into_iter().enumerate() {
                if is_token(&h) {
                    out.push(h);
                } else {
                    d.error(
                        off,
                        &crate::error::join_index(&at, i),
                        format!("{h:?} is not a field name"),
                    );
                    *ok = false;
                }
            }
        }
        out
    };
    let request_headers = names(d, &mut o, "request_headers", &mut ok);
    let copy_headers = names(d, &mut o, "copy_headers", &mut ok);
    let mut timeout = DEFAULT_FORWARD_AUTH_TIMEOUT;
    if let Some(v) = o.take("timeout") {
        match d.duration(v, &o.at("timeout")) {
            Some(t) if !t.is_zero() => timeout = t,
            Some(_) => {}
            None => ok = false,
        }
    }
    o.finish(d, FORWARD_AUTH_FIELDS, &[]);
    ok.then(|| ForwardAuth {
        upstream: upstream.unwrap_or_default(),
        uri,
        request_headers,
        copy_headers,
        timeout,
    })
}

/// The `jwt` middleware's params, checked as fit's `JWT.Provision` checks them, but for the key
/// files, which the middleware reads.
fn jwt(d: &mut Decoder<'_>, params: Option<&Node>, off: usize, path: &str) -> Option<Jwt> {
    let Some(n) = params else {
        d.error(
            off,
            path,
            "a key is needed: jwks_url, public_key_files or hmac_secret_file",
        );
        return None;
    };
    let mut o = d.object(n, path)?;
    let mut ok = true;
    let string = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, ok: &mut bool| {
        o.take(key).map_or_else(String::new, |v| {
            d.string(v, &o.at(key)).unwrap_or_else(|| {
                *ok = false;
                String::new()
            })
        })
    };
    let url_off = o.take("jwks_url").map_or(n.off, |v| v.off);
    let jwks_url = string(d, &mut o, "jwks_url", &mut ok);
    if !jwks_url.is_empty() && !is_http_url(&jwks_url) {
        d.error(
            url_off,
            &o.at("jwks_url"),
            format!("{jwks_url:?} is not an http or https URL"),
        );
        ok = false;
    }
    let mut jwks_refresh_interval = DEFAULT_JWKS_REFRESH_INTERVAL;
    let new_name = o.has("jwks_refresh_interval");
    for key in ["jwks_refresh_interval", "jwks_refresh"] {
        let Some(v) = o.take(key) else { continue };
        if key == "jwks_refresh" && new_name {
            d.error(
                v.off,
                &o.at(key),
                "\"jwks_refresh\" is the old name of \"jwks_refresh_interval\"; give only \"jwks_refresh_interval\"",
            );
            ok = false;
            continue;
        }
        match d.duration(v, &o.at(key)) {
            Some(t) if !t.is_zero() => jwks_refresh_interval = t,
            Some(_) => {}
            None => ok = false,
        }
    }
    let strings = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| -> Vec<(usize, String)> {
        o.take(key)
            .map_or_else(Vec::new, |v| d.strings(v, &o.at(key)))
    };
    let public_key_files: Vec<String> = strings(d, &mut o, "public_key_files")
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    let hmac_secret_file = string(d, &mut o, "hmac_secret_file", &mut ok);
    let mut algorithms = Vec::new();
    let at = o.at("algorithms");
    for (i, (aoff, a)) in strings(d, &mut o, "algorithms").into_iter().enumerate() {
        if JWT_ALGORITHMS.contains(&a.as_str()) {
            algorithms.push(a);
        } else {
            let list = JWT_ALGORITHMS
                .iter()
                .map(|a| format!("{a:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            d.error(
                aoff,
                &crate::error::join_index(&at, i),
                format!("{a:?} is not one of {list}"),
            );
            ok = false;
        }
    }
    let issuer = string(d, &mut o, "issuer", &mut ok);
    let audience = strings(d, &mut o, "audience")
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    let mut leeway = Duration::ZERO;
    if let Some(v) = o.take("leeway") {
        match d.duration(v, &o.at("leeway")) {
            Some(t) => leeway = t,
            None => ok = false,
        }
    }
    let mut token_from = TokenFrom::Authorization;
    if let Some(v) = o.take("token_from") {
        let p = o.at("token_from");
        match d.string(v, &p) {
            Some(f) => match parse_token_from(&f) {
                Some(t) => token_from = t,
                None => {
                    d.error(
                        v.off,
                        &p,
                        format!("{f:?} is not header, header:Name, cookie:name or query:name"),
                    );
                    ok = false;
                }
            },
            None => ok = false,
        }
    }
    let mut claims_headers = Vec::new();
    if let Some(v) = o.take("claims_headers") {
        let p = o.at("claims_headers");
        for (koff, field, claim) in d.string_map(v, &p) {
            if !is_token(&field) {
                d.error(koff, &p, format!("{field:?} is not a field name"));
                ok = false;
            } else if claim.is_empty() {
                d.error(koff, &p, format!("{field:?} names no claim"));
                ok = false;
            } else {
                claims_headers.push((field, claim));
            }
        }
    }
    o.finish(d, JWT_FIELDS, &[]);
    if ok && jwks_url.is_empty() && public_key_files.is_empty() && hmac_secret_file.is_empty() {
        d.error(
            n.off,
            path,
            "a key is needed: jwks_url, public_key_files or hmac_secret_file",
        );
        ok = false;
    }
    ok.then_some(Jwt {
        jwks_url,
        jwks_refresh_interval,
        public_key_files,
        hmac_secret_file,
        algorithms,
        issuer,
        audience,
        leeway,
        token_from,
        claims_headers,
    })
}

/// `jwt`'s `token_from`, as fit's `Provision` reads it.
fn parse_token_from(f: &str) -> Option<TokenFrom> {
    if f.is_empty() || f == "header" {
        return Some(TokenFrom::Authorization);
    }
    if let Some(n) = f.strip_prefix("header:") {
        return is_token(n).then(|| TokenFrom::Header(n.to_owned()));
    }
    if let Some(n) = f.strip_prefix("cookie:") {
        return is_token(n).then(|| TokenFrom::Cookie(n.to_owned()));
    }
    if let Some(n) = f.strip_prefix("query:") {
        return (!n.is_empty()).then(|| TokenFrom::Query(n.to_owned()));
    }
    None
}

/// Whether `u` is an absolute `http` or `https` URL with a host, as fit's `jwt` checks its
/// `jwks_url` (Go's `url.Parse`, its scheme in any case).
fn is_http_url(u: &str) -> bool {
    if u.bytes().any(|c| c < 0x20 || c == 0x7f || c == b' ') {
        return false;
    }
    let Some((scheme, rest)) = u.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    !host.is_empty() && !host.starts_with(':')
}

/// Whether `u` is a path, with a query or not, as fit's `forward_auth` takes its `uri` (Go's
/// `url.ParseRequestURI`, without a scheme or host): it starts with `/`, holds no control
/// character, and the `%` escapes of its path are valid.
fn is_request_path(u: &str) -> bool {
    if !u.starts_with('/') || u.bytes().any(|c| c < 0x20 || c == 0x7f) {
        return false;
    }
    let path = u.split_once('?').map_or(u, |(p, _)| p).as_bytes();
    let mut i = 0;
    while let Some(&c) = path.get(i) {
        if c == b'%' {
            let hex = |j: usize| path.get(j).is_some_and(u8::is_ascii_hexdigit);
            if !(hex(i.saturating_add(1)) && hex(i.saturating_add(2))) {
                return false;
            }
            i = i.saturating_add(3);
        } else {
            i = i.saturating_add(1);
        }
    }
    true
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
        .map(|v| encodings(d, v, &o.at("precompressed"), false))
        .unwrap_or_default();
    let compress = o
        .take("compress")
        .map(|v| encodings(d, v, &o.at("compress"), true))
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
        compress,
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
        upstreams: Vec::new(),
        failover: None,
        mirror: None,
        trusted_proxies: Vec::new(),
        host_header: String::new(),
        timeout: None,
        retry: None,
        tunnel_idle_timeout: DEFAULT_TUNNEL_IDLE_TIMEOUT,
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
    if let Some(v) = o.take("timeout") {
        // 0, as leaving it out, is no timeout.
        p.timeout = d.duration(v, &o.at("timeout")).filter(|t| !t.is_zero());
    }
    if let Some(v) = o.take("retry") {
        match retry(d, v, &o.at("retry")) {
            Some(r) => p.retry = Some(r),
            None => ok = false,
        }
    }
    if let Some(v) = o.take("tunnel_idle_timeout") {
        // 0, as leaving it out, is fit's default.
        if let Some(t) = d
            .duration(v, &o.at("tunnel_idle_timeout"))
            .filter(|t| !t.is_zero())
        {
            p.tunnel_idle_timeout = t;
        }
    }
    let mut upstreams_off = None;
    if let Some(v) = o.take("upstreams") {
        upstreams_off = Some(v.off);
        let at = o.at("upstreams");
        let given = match &v.kind {
            crate::json::Kind::Array(items) => items.len(),
            _ => usize::MAX,
        };
        p.upstreams = d.array(v, &at, split);
        ok &= p.upstreams.len() == given;
    }
    if let Some(v) = o.take("failover") {
        p.failover = d.string(v, &o.at("failover")).filter(|s| !s.is_empty());
    }
    if let Some(v) = o.take("mirror") {
        match mirror(d, v, &o.at("mirror")) {
            Some(m) => p.mirror = Some(m),
            None => ok = false,
        }
    }
    o.finish(d, PROXY_FIELDS, PROXY_FIT_ONLY);
    // fit's checks of upstream and upstreams, which are each other's alternatives.
    match (p.upstream.is_empty(), p.upstreams.is_empty(), upstreams_off) {
        (false, false, Some(off)) => {
            d.error(
                off,
                &crate::error::join_field(path, "upstreams"),
                "upstream and upstreams are each other's alternatives: set one",
            );
            ok = false;
        }
        // An upstreams some of whose items were refused is reported already.
        (true, true, _) if ok => {
            d.error(
                upstream_off,
                &upstream_path,
                "upstream or upstreams is needed",
            );
            ok = false;
        }
        _ => {}
    }
    ok.then_some(p)
}

/// One of the `proxy` handler's `upstreams` (I46), checked as fit's tags check a `proxy.Split`.
fn split(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Split> {
    let mut o = d.object(n, path)?;
    let upstream = o
        .require(d, "upstream")
        .and_then(|v| d.string(v, &o.at("upstream")));
    let weight = o
        .require(d, "weight")
        .and_then(|v| d.int(v, &o.at("weight"), 1, 1000));
    o.finish(d, SPLIT_FIELDS, &[]);
    Some(Split {
        upstream: upstream?,
        weight: u32::try_from(weight?).ok()?,
    })
}

/// The `proxy` handler's `mirror` (I46), checked as fit's tags check a `proxy.Mirror`.
fn mirror(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Mirror> {
    let mut o = d.object(n, path)?;
    let upstream = o
        .require(d, "upstream")
        .and_then(|v| d.string(v, &o.at("upstream")));
    let mut percent = Some(100.0);
    if let Some(v) = o.take("percent") {
        let at = o.at("percent");
        percent = match d.number(v, &at) {
            Some(x) if x < 0.0 => {
                d.error(v.off, &at, format!("{x} is less than the minimum 0"));
                None
            }
            Some(x) if x > 100.0 => {
                d.error(v.off, &at, format!("{x} is greater than the maximum 100"));
                None
            }
            x => x,
        };
    }
    o.finish(d, MIRROR_FIELDS, &[]);
    Some(Mirror {
        upstream: upstream?,
        percent: percent?,
    })
}

/// A ratio from 0 to 1, as fit's `min:"0" max:"1"` bound a float.
fn ratio(d: &mut Decoder<'_>, v: &Node, at: &str) -> Option<f64> {
    let r = d.number(v, at)?;
    if r < 0.0 {
        d.error(v.off, at, format!("{r} is less than the minimum 0"));
        return None;
    }
    if r > 1.0 {
        d.error(v.off, at, format!("{r} is greater than the maximum 1"));
        return None;
    }
    Some(r)
}

/// A list of statuses each within `min..=max`, or `None` (an error given) when one is not:
/// `what` names what each must be, as fit's messages do ("a 5xx status").
fn statuses(
    d: &mut Decoder<'_>,
    v: &Node,
    at: &str,
    range: std::ops::RangeInclusive<i64>,
    what: &str,
) -> Option<Vec<u16>> {
    let given = match &v.kind {
        crate::json::Kind::Array(items) => items.len(),
        _ => usize::MAX,
    };
    let list = d.array(v, at, |d, item, p| {
        let s = d.int(item, p, i64::MIN, i64::MAX)?;
        if !range.contains(&s) {
            d.error(item.off, p, format!("{s} is not {what}"));
            return None;
        }
        u16::try_from(s).ok()
    });
    (list.len() == given).then_some(list)
}

/// The `proxy` handler's `retry`, checked as fit's `Proxy.Provision` and its tags check it.
fn retry(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Retry> {
    let mut o = d.object(n, path)?;
    let mut r = Retry::default();
    let mut ok = true;
    if let Some(v) = o.take("attempts") {
        match d.int(v, &o.at("attempts"), 1, 10) {
            Some(a) => r.attempts = u32::try_from(a).unwrap_or(2),
            None => ok = false,
        }
    }
    if let Some(v) = o.take("statuses") {
        match statuses(d, v, &o.at("statuses"), 500..=599, "a 5xx status") {
            Some(l) => r.statuses = l,
            None => ok = false,
        }
    }
    if let Some(v) = o.take("budget_ratio") {
        match ratio(d, v, &o.at("budget_ratio")) {
            Some(x) => r.budget_ratio = x,
            None => ok = false,
        }
    }
    if let Some(v) = o.take("budget_min_per_second") {
        let at = o.at("budget_min_per_second");
        match d.number(v, &at) {
            Some(x) if x < 0.0 => {
                d.error(v.off, &at, format!("{x} is less than the minimum 0"));
                ok = false;
            }
            Some(x) => r.budget_min_per_second = x,
            None => ok = false,
        }
    }
    o.finish(d, RETRY_FIELDS, &[]);
    ok.then_some(r)
}

/// The `circuit_breaker` middleware's params, checked as fit's `Breaker.Provision` and its tags
/// check them; every one has a default, so it may have none.
fn circuit_breaker(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    path: &str,
) -> Option<CircuitBreaker> {
    let mut b = CircuitBreaker::default();
    let Some(n) = params else {
        return Some(b);
    };
    let mut o = d.object(n, path)?;
    let mut ok = true;
    // A duration, 0 (as leaving it out) keeping the default.
    let mut duration = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        let v = o.take(key)?;
        let t = d.duration(v, &o.at(key));
        ok &= t.is_some();
        t.filter(|t| !t.is_zero())
    };
    if let Some(t) = duration(d, &mut o, "window") {
        b.window = t;
    }
    b.slow_threshold = duration(d, &mut o, "slow_threshold");
    if let Some(t) = duration(d, &mut o, "open_duration") {
        b.open_duration = t;
    }
    if let Some(v) = o.take("min_requests") {
        match d.int(v, &o.at("min_requests"), 1, i64::MAX) {
            Some(m) => b.min_requests = u64::try_from(m).unwrap_or(20),
            None => ok = false,
        }
    }
    if let Some(v) = o.take("half_open_requests") {
        match d.int(v, &o.at("half_open_requests"), 1, i64::from(u32::MAX)) {
            Some(m) => b.half_open_requests = u32::try_from(m).unwrap_or(5),
            None => ok = false,
        }
    }
    for (key, slot) in [
        ("error_ratio", &mut b.error_ratio),
        ("slow_ratio", &mut b.slow_ratio),
    ] {
        if let Some(v) = o.take(key) {
            match ratio(d, v, &o.at(key)) {
                // 0, as leaving it out, keeps the default.
                Some(r) if r > 0.0 => *slot = r,
                Some(_) => {}
                None => ok = false,
            }
        }
    }
    if let Some(v) = o.take("statuses") {
        match statuses(d, v, &o.at("statuses"), 100..=599, "a status") {
            Some(l) => b.statuses = l,
            None => ok = false,
        }
    }
    o.finish(d, CIRCUIT_BREAKER_FIELDS, &[]);
    ok.then_some(b)
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
    let mut protocol = UpstreamProtocol::Http11;
    if let Some(v) = o.take("protocol") {
        let p = o.at("protocol");
        match d.string(v, &p).as_deref() {
            Some("http/1.1") | None => {}
            Some("h2c") => protocol = UpstreamProtocol::H2c,
            Some("h2") => protocol = UpstreamProtocol::H2,
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
    let response_timeout = duration(d, "response_timeout", DEFAULT_RESPONSE_TIMEOUT);
    let response_header_timeout = duration(
        d,
        "response_header_timeout",
        DEFAULT_RESPONSE_HEADER_TIMEOUT,
    );
    let response_idle_timeout = duration(d, "response_idle_timeout", DEFAULT_RESPONSE_IDLE_TIMEOUT);
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
    // fit's policies, as its `load_balancing` enum lists them.
    let policy = o.take("load_balancing").map(|v| {
        let p = o.at("load_balancing");
        let lb = match d.string(v, &p).as_deref() {
            Some("round_robin") => Some(LoadBalancing::RoundRobin),
            Some("weighted_round_robin") => Some(LoadBalancing::WeightedRoundRobin),
            Some("least_conn") => Some(LoadBalancing::LeastConn),
            Some("random") => Some(LoadBalancing::Random),
            Some("p2c") => Some(LoadBalancing::P2c),
            Some("ip_hash") => Some(LoadBalancing::IpHash),
            Some("uri_hash") => Some(LoadBalancing::UriHash),
            Some("header_hash") => Some(LoadBalancing::HeaderHash),
            Some("cookie_hash") => Some(LoadBalancing::CookieHash),
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
    let hash_key_at = o.take("hash_key").map(|v| {
        let p = o.at("hash_key");
        (v.off, p.clone(), d.string(v, &p).unwrap_or_default())
    });
    let sticky_cookie = o
        .take("sticky_cookie")
        .and_then(|v| sticky_cookie(d, v, &o.at("sticky_cookie")));
    let health_check = o
        .take("health_check")
        .and_then(|v| health_check(d, v, &o.at("health_check")));
    // TLS to an h2 upstream (I54): each with the position of its value, where fit reports it.
    let mut tls_field = |d: &mut Decoder<'_>, key: &str| {
        let v = o.take(key)?;
        let p = o.at(key);
        let s = d.string(v, &p)?;
        (!s.is_empty()).then_some((v.off, p, s))
    };
    let tls_server_name = tls_field(d, "tls_server_name");
    let tls_ca_file = tls_field(d, "tls_ca_file");
    let tls_cert_file = tls_field(d, "tls_cert_file");
    let tls_key_file = tls_field(d, "tls_key_file");
    let obj_off = o.off;
    let hash_key_path = o.at("hash_key");
    o.finish(d, UPSTREAM_FIELDS, UPSTREAM_FIT_ONLY);
    // fit's checks of the TLS fields (config.validate): with h2 only, and a certificate with its
    // key.
    if protocol != UpstreamProtocol::H2 {
        for (f, key) in [
            (&tls_server_name, "tls_server_name"),
            (&tls_ca_file, "tls_ca_file"),
            (&tls_cert_file, "tls_cert_file"),
        ] {
            if let Some((off, p, _)) = f {
                d.error(*off, p, format!("{key} needs protocol h2"));
            }
        }
    }
    match (&tls_cert_file, &tls_key_file) {
        (Some((off, p, _)), None) => d.error(*off, p, "tls_cert_file needs tls_key_file"),
        (None, Some((off, p, _))) => d.error(*off, p, "tls_key_file needs tls_cert_file"),
        _ => {}
    }
    let value = |f: Option<(usize, String, String)>| f.map(|(_, _, s)| s).unwrap_or_default();
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
    // fit's checks of hash_key against the policy; one left out is reported where the upstream
    // starts, as fit reports it.
    let keyed = matches!(
        load_balancing,
        LoadBalancing::HeaderHash | LoadBalancing::CookieHash
    );
    let what = if load_balancing == LoadBalancing::HeaderHash {
        "header"
    } else {
        "cookie"
    };
    let mut hash_key = String::new();
    match hash_key_at {
        Some((off, p, k)) if !k.is_empty() => {
            if !keyed {
                d.error(off, &p, "hash_key is for header_hash and cookie_hash");
            } else if !is_token(&k) {
                d.error(off, &p, format!("{k:?} is not a valid {what} name"));
            } else {
                hash_key = k;
            }
        }
        at if keyed => d.error(
            at.map_or(obj_off, |(off, _, _)| off),
            &hash_key_path,
            format!(
                "{} needs hash_key, the name of the {what} to hash",
                load_balancing.name()
            ),
        ),
        _ => {}
    }
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
        hash_key,
        sticky_cookie,
        max_fails,
        fail_timeout,
        health_check,
        response_timeout,
        response_header_timeout,
        response_idle_timeout,
        protocol,
        tls_server_name: value(tls_server_name),
        tls_ca_file: value(tls_ca_file),
        tls_cert_file: value(tls_cert_file),
        tls_key_file: value(tls_key_file),
    })
}

/// An upstream's `sticky_cookie`, with fit's checks (config.validate): a name that is a token and
/// a path that starts with `/` and holds no `;`.
fn sticky_cookie(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<StickyCookie> {
    let mut o = d.object(n, path)?;
    let mut ok = true;
    let mut name = DEFAULT_STICKY_COOKIE_NAME.to_owned();
    if let Some(v) = o.take("name") {
        let p = o.at("name");
        match d.string(v, &p) {
            Some(s) if s.is_empty() => {}
            Some(s) if is_token(&s) => name = s,
            Some(s) => {
                d.error(v.off, &p, format!("{s:?} is not a valid cookie name"));
                ok = false;
            }
            None => ok = false,
        }
    }
    let mut cookie_path = "/".to_owned();
    if let Some(v) = o.take("path") {
        let p = o.at("path");
        match d.string(v, &p) {
            Some(s) if s.is_empty() => {}
            Some(s) if s.starts_with('/') && !s.contains([';', '\0', '\r', '\n']) => {
                cookie_path = s;
            }
            Some(s) => {
                d.error(
                    v.off,
                    &p,
                    format!("{s:?} is not a cookie path: it must start with '/' and hold no ';'"),
                );
                ok = false;
            }
            None => ok = false,
        }
    }
    let max_age = match o.take("max_age") {
        Some(v) => match d.duration(v, &o.at("max_age")) {
            Some(t) => Some(t).filter(|t| !t.is_zero()),
            None => {
                ok = false;
                None
            }
        },
        None => None,
    };
    let secure = match o.take("secure") {
        None => false,
        Some(v) => match &v.kind {
            crate::json::Kind::Bool(b) => *b,
            other => {
                d.error(
                    v.off,
                    &o.at("secure"),
                    format!("expected a boolean, got {}", other.name()),
                );
                ok = false;
                false
            }
        },
    };
    o.finish(d, STICKY_COOKIE_FIELDS, &[]);
    ok.then_some(StickyCookie {
        name,
        path: cookie_path,
        max_age,
        secure,
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
/// `services[]`, as fit's `config.Service`; checked across the config by [`check_services`].
fn service(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Service> {
    let mut o = d.object(n, path)?;
    let name = o
        .require(d, "name")
        .and_then(|v| d.string(v, &o.at("name")));
    let listeners = o
        .require(d, "listeners")
        .map(|v| {
            d.strings(v, &o.at("listeners"))
                .into_iter()
                .map(|(_, s)| s)
                .collect()
        })
        .unwrap_or_default();
    let upstream = o
        .require(d, "upstream")
        .and_then(|v| d.string(v, &o.at("upstream")));
    // 0 means the defaults, as fit's `Idle` and `UDPIdle` read it.
    let idle_timeout = o
        .take("idle_timeout")
        .and_then(|v| d.duration(v, &o.at("idle_timeout")))
        .filter(|t| !t.is_zero());
    let max_conns = o
        .take("max_conns")
        .and_then(|v| d.int(v, &o.at("max_conns"), 0, i64::MAX))
        .map_or(DEFAULT_SERVICE_MAX_CONNS, |n| match usize::try_from(n) {
            Ok(0) => DEFAULT_SERVICE_MAX_CONNS,
            Ok(n) => n,
            Err(_) => usize::MAX,
        });
    let mut server_names: Vec<String> = Vec::new();
    if let Some(v) = o.take("server_names") {
        let p = o.at("server_names");
        for (i, (off, sn)) in d.strings(v, &p).into_iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            let lower = sn.to_ascii_lowercase();
            if server_names.contains(&lower) {
                d.error(off, &at, format!("{sn:?} is listed twice"));
                continue;
            }
            if let Err(msg) = check_server_name(&sn) {
                d.error(off, &at, msg);
            }
            // Kept when refused too: the service still shares its listener by name.
            server_names.push(lower);
        }
    }
    let proxy_protocol = o.take("proxy_protocol").and_then(|v| {
        let p = o.at("proxy_protocol");
        match d.string(v, &p).as_deref() {
            Some("v1") => Some(ProxyVersion::V1),
            Some("v2") => Some(ProxyVersion::V2),
            // fit's enum takes "" as left out.
            Some("") | None => None,
            Some(other) => {
                d.error(v.off, &p, format!("{other:?} is not one of \"v1\", \"v2\""));
                None
            }
        }
    });
    o.finish(d, SERVICE_FIELDS, SERVICE_FIT_ONLY);
    Some(Service {
        name: name?,
        listeners,
        upstream: upstream?,
        idle_timeout,
        max_conns,
        server_names,
        proxy_protocol,
    })
}

/// What is wrong with `name` as one of a service's `server_names`, as fit checks it: a host name,
/// or one under a `*.` wildcard, and not an IP address, which a ClientHello does not name.
fn check_server_name(name: &str) -> Result<(), String> {
    if name.starts_with("**.") {
        return Err(format!(
            "{name:?}: a server name's wildcard is \"*.\", one label, as in certificates"
        ));
    }
    check_host(name)?;
    if name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() {
        return Err(format!(
            "{name:?} is an IP address; a ClientHello names host names"
        ));
    }
    Ok(())
}

/// The layer 4 protocol a listener serves, `tcp` or `udp`, if any.
fn l4_protocol(l: &Listener) -> Option<Protocol> {
    l.protocols
        .iter()
        .copied()
        .find(|p| matches!(p, Protocol::Tcp | Protocol::Udp))
}

/// The checks of fit's `config.Validate` across services, listeners and upstreams: names
/// unique, each service's listeners `tcp` or `udp` ones, each listener one service's, and every
/// `tcp` or `udp` listener taken by one.
fn check_services(
    ds: &mut [Decoder<'_>],
    services: &[At<Service>],
    listeners: &[At<Listener>],
    upstreams: &[At<Upstream>],
) {
    let mut seen: Vec<&str> = Vec::new();
    // Each listener a service took, that service's name, and whether it took it without
    // server_names.
    let mut served_by: Vec<(&str, &str, bool)> = Vec::new();
    // Each tcp listener's server names, with the service that takes each.
    let mut by_name: Vec<(&str, &str, &str)> = Vec::new();
    for svc in services {
        let s = &svc.v;
        let named = !s.server_names.is_empty();
        let name = &s.name;
        let s_path = &svc.path;
        let path = crate::error::join_field(s_path, "name");
        if !valid_name(name) {
            svc.error(ds, &path, bad_name(name));
        } else if seen.contains(&name.as_str()) {
            svc.error(
                ds,
                &path,
                format!("another service is already named {name:?}"),
            );
        }
        seen.push(name);
        let lpath = crate::error::join_field(s_path, "listeners");
        if s.listeners.is_empty() {
            svc.error(
                ds,
                &lpath,
                "a service needs at least one tcp or udp listener".to_owned(),
            );
        }
        for (i, l) in s.listeners.iter().enumerate() {
            let at = crate::error::join_index(&lpath, i);
            let Some(listener) = listeners.iter().find(|x| &x.v.name == l) else {
                svc.error(ds, &at, format!("there is no listener named {l:?}"));
                continue;
            };
            let proto = l4_protocol(&listener.v);
            // A tcp listener's services with server_names share it (I52): one may have none.
            let taken = served_by.iter().find(|(x, _, fallback)| {
                x == l && (proto == Some(Protocol::Udp) || (*fallback && !named))
            });
            match taken {
                _ if proto.is_none() => svc.error(
                    ds,
                    &at,
                    format!(
                        "{l:?} is not a tcp or udp listener: a service takes the connections of \
                         listeners whose protocols include tcp, or are [\"udp\"]"
                    ),
                ),
                // Services that share a tcp listener pick by SNI, I52: one without
                // server_names takes them all.
                Some((_, other, _)) if proto == Some(Protocol::Udp) => svc.error(
                    ds,
                    &at,
                    format!("listener {l:?} is already service {other:?}'s"),
                ),
                Some((_, other, _)) => svc.error(
                    ds,
                    &at,
                    format!(
                        "listener {l:?} is already service {other:?}'s: services that share a \
                         tcp listener take the connections of their server_names, and only one \
                         of them may have none"
                    ),
                ),
                None => {
                    if proto == Some(Protocol::Tcp) {
                        // Two services of one listener take no name both.
                        for (j, sn) in s.server_names.iter().enumerate() {
                            match by_name
                                .iter()
                                .find(|(x, n, other)| x == l && n == sn && other != name)
                            {
                                Some((_, _, other)) => svc.error(
                                    ds,
                                    &crate::error::join_index(
                                        &crate::error::join_field(s_path, "server_names"),
                                        j,
                                    ),
                                    format!(
                                        "{sn:?} is already service {other:?}'s on listener {l:?}"
                                    ),
                                ),
                                None => by_name.push((l, sn, name)),
                            }
                        }
                    }
                    served_by.push((l, name, !named));
                }
            }
        }
        let udp = s.listeners.iter().any(|l| {
            listeners
                .iter()
                .any(|x| &x.v.name == l && l4_protocol(&x.v) == Some(Protocol::Udp))
        });
        if udp && named {
            svc.error(
                ds,
                &crate::error::join_field(s_path, "server_names"),
                "server_names pick a service by the TLS ClientHello of a tcp connection; a \
                 service with a udp listener takes none"
                    .to_owned(),
            );
        }
        if udp && s.proxy_protocol.is_some() {
            svc.error(
                ds,
                &crate::error::join_field(s_path, "proxy_protocol"),
                "the PROXY protocol is sent on tcp connections; a service with a udp listener \
                 sends none"
                    .to_owned(),
            );
        }
        if !upstreams.iter().any(|u| u.v.name == s.upstream) {
            svc.error(
                ds,
                &crate::error::join_field(s_path, "upstream"),
                format!("there is no upstream named {:?}", s.upstream),
            );
        }
    }
    for l in listeners {
        if let Some(p) = l4_protocol(&l.v) {
            if !served_by.iter().any(|(x, _, _)| *x == l.v.name) {
                l.error(
                    ds,
                    &crate::error::join_field(&l.path, "protocols"),
                    format!(
                        "no service takes this {} listener's connections: list it in a \
                         service's listeners",
                        p.as_str()
                    ),
                );
            }
        }
    }
}

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

fn check_routes(
    ds: &mut [Decoder<'_>],
    routes: &[At<Route>],
    listeners: &[&str],
    written: &[At<Listener>],
) {
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
            let path = crate::error::join_index(&crate::error::join_field(&r.path, "listeners"), j);
            if !listeners.contains(&l.as_str()) {
                r.error(ds, &path, format!("there is no listener named {l:?}"));
            } else if let Some(x) = written.iter().find(|x| &x.v.name == l) {
                // fit's message: such a listener is a service's.
                if !x.v.protocols.iter().any(|p| p.is_http()) {
                    if let Some(p) = l4_protocol(&x.v) {
                        r.error(
                            ds,
                            &path,
                            format!(
                                "{l:?} is a {} listener, which a service serves, not routes: it \
                                 serves no HTTP",
                                p.as_str()
                            ),
                        );
                    } else if x.v.protocols.contains(&Protocol::Socks) {
                        r.error(
                            ds,
                            &path,
                            format!("{l:?} is a socks listener, not routes: it serves no HTTP"),
                        );
                    }
                }
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
