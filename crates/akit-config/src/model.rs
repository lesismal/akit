//! The config model, v0: the parts of fit's config akit reads so far, with fit's field names,
//! defaults and checks. Every other field fit reads is refused as "not supported yet" by the
//! object that holds it (decision 002), and shrinks from the lists below as iterations land.

use std::net::IpAddr;

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
}

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
}

impl Protocol {
    /// The name the config writes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http11 => "http/1.1",
            Protocol::H2c => "h2c",
            Protocol::H2 => "h2",
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
}

/// A route's handler module and its params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// `respond`: a fixed status, header fields and body.
    Respond(Respond),
}

impl Handler {
    /// The handler module's name, as the config writes it in `module`.
    #[must_use]
    pub fn module(&self) -> &'static str {
        match self {
            Handler::Respond(_) => "respond",
        }
    }
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

// What akit reads, and what fit reads that akit does not yet, object by object.

const CONFIG_FIELDS: &[&str] = &["schema_version", "listeners", "routes"];
const CONFIG_FIT_ONLY: &[&str] = &[
    "upstreams",
    "services",
    "log",
    "access_log",
    "telemetry",
    "limits",
    "access",
    "engine",
    "admin",
    "auto_https",
    "metrics",
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
const LISTENER_FIELDS: &[&str] = &["name", "address", "protocols", "tls"];
const LISTENER_FIT_ONLY: &[&str] = &["mptcp", "socks", "access", "proxy_protocol"];
const FIT_PROTOCOLS: &[&str] = &["h3", "tcp", "udp", "socks"];
const TLS_FIELDS: &[&str] = &["certificates", "client_auth", "max_version"];
// acme and on_demand come with I31 and I34; kernel_tls_offload with engine's.
const TLS_FIT_ONLY: &[&str] = &["acme", "on_demand", "kernel_tls_offload"];
const CERTIFICATE_FIELDS: &[&str] = &["cert_file", "key_file"];
const CLIENT_AUTH_FIELDS: &[&str] = &["ca_files", "mode"];
const ROUTE_FIELDS: &[&str] = &["name", "listeners", "priority", "match", "handler"];
const ROUTE_FIT_ONLY: &[&str] = &["access", "middleware"];
const MATCH_FIELDS: &[&str] = &["host", "path"];
const MATCH_FIT_ONLY: &[&str] = &[
    "path_regexp",
    "method",
    "header",
    "query",
    "remote_ip",
    "expr",
];
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
    "proxy",
    "static",
    "stream_whep",
    "stream_whip",
    "sync_ingest",
    "vod",
    "webrtc_proxy",
    "websockify",
    "whep",
    "whip",
];
const RESPOND_FIELDS: &[&str] = &["status", "headers", "body"];

/// One file's part of a config, before the parts are merged.
pub(crate) struct Part {
    /// `schema_version`, with its position, when the file gives it.
    schema_version: Option<(i64, usize)>,
    listeners: Vec<At<Listener>>,
    routes: Vec<At<Route>>,
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
    let doc_off = o.off;
    o.finish(d, CONFIG_FIELDS, CONFIG_FIT_ONLY);
    Some(Part {
        schema_version,
        listeners,
        routes,
        doc_off,
    })
}

/// Merges the files' parts, `ds` their decoders in the same order, as fit merges a config's
/// files: the lists at the top (`listeners`, `routes`) are joined in the files' order, and
/// `schema_version` may be given by each file if they agree. The checks across elements (names
/// unique, routes naming listeners that exist) run on the merged lists; each problem is reported
/// in the file and at the position of the element it is about, with the element's path in that
/// file.
pub(crate) fn merge(ds: &mut [Decoder<'_>], parts: Vec<Part>) -> Config {
    let mut schema: Option<(i64, usize)> = None;
    let mut listeners = Vec::new();
    let mut routes = Vec::new();
    let mut first_doc: Option<(usize, usize)> = None;
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
        listeners.extend(p.listeners);
        routes.extend(p.routes);
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
    Config {
        schema_version: schema.map_or(SCHEMA_VERSION, |(v, _)| v),
        listeners: listeners.into_iter().map(|a| a.v).collect(),
        routes: routes.into_iter().map(|a| a.v).collect(),
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
    })
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
    let handler = o
        .require(d, "handler")
        .and_then(|v| handler(d, v, &o.at("handler")));
    o.finish(d, ROUTE_FIELDS, ROUTE_FIT_ONLY);
    Some(Route {
        name,
        listeners,
        priority,
        matcher,
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
    o.finish(d, MATCH_FIELDS, MATCH_FIT_ONLY);
    Some(m)
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
