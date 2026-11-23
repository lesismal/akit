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
    /// `protocols`; fit's default, HTTP/1.1 alone, when left out.
    pub protocols: Vec<Protocol>,
}

/// A protocol a listener serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `http/1.1`.
    Http11,
    /// `h2c`, HTTP/2 in the clear.
    H2c,
}

impl Protocol {
    /// The name the config writes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http11 => "http/1.1",
            Protocol::H2c => "h2c",
        }
    }
}

/// fit's default protocols for a listener without TLS: HTTP/1.1 alone (fit's
/// `config.DefaultProtocols`).
pub const DEFAULT_PROTOCOLS: [Protocol; 1] = [Protocol::Http11];

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
const LISTENER_FIELDS: &[&str] = &["name", "address", "protocols"];
const LISTENER_FIT_ONLY: &[&str] = &["mptcp", "tls", "socks", "access", "proxy_protocol"];
const FIT_PROTOCOLS: &[&str] = &["h2", "h3", "tcp", "udp", "socks"];
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

pub(crate) fn config(d: &mut Decoder<'_>, n: &Node) -> Option<Config> {
    let mut o = d.object(n, "")?;
    let schema_version = match o.take("schema_version") {
        Some(v) => d.int(v, "schema_version", SCHEMA_VERSION, SCHEMA_VERSION)?,
        None => SCHEMA_VERSION,
    };
    // Each element keeps its path and position for the checks across elements below.
    let listeners = o
        .take("listeners")
        .map(|v| {
            d.array(v, "listeners", |d, n, p| {
                Some(At::new(p, n, listener(d, n, p)?))
            })
        })
        .unwrap_or_default();
    let routes = o
        .take("routes")
        .map(|v| d.array(v, "routes", |d, n, p| Some(At::new(p, n, route(d, n, p)?))))
        .unwrap_or_default();
    let doc_off = o.off;
    o.finish(d, CONFIG_FIELDS, CONFIG_FIT_ONLY);

    check_listeners(d, &listeners);
    if listeners.is_empty() && !routes.is_empty() {
        // fit serves such a config over HTTPS by default, with certificates from ACME.
        d.not_yet(
            doc_off,
            "listeners",
            "a config with no listeners, served over HTTPS by default (auto_https)",
        );
    }
    check_routes(d, &routes, &listeners);
    Some(Config {
        schema_version,
        listeners: listeners.into_iter().map(|a| a.v).collect(),
        routes: routes.into_iter().map(|a| a.v).collect(),
    })
}

/// A decoded element with its path and the position of its node.
struct At<T> {
    path: String,
    off: usize,
    v: T,
}

impl<T> At<T> {
    fn new(path: &str, n: &Node, v: T) -> At<T> {
        At {
            path: path.to_owned(),
            off: n.off,
            v,
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
    let mut protocols = Vec::new();
    if let Some(v) = o.take("protocols") {
        let p = o.at("protocols");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            let at = crate::error::join_index(&p, i);
            let proto = match s.as_str() {
                "http/1.1" => Protocol::Http11,
                "h2c" => Protocol::H2c,
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
            if protocols.contains(&proto) {
                d.error(off, &at, format!("{s:?} is listed twice"));
                continue;
            }
            protocols.push(proto);
        }
    }
    o.finish(d, LISTENER_FIELDS, LISTENER_FIT_ONLY);
    if protocols.is_empty() {
        protocols = DEFAULT_PROTOCOLS.to_vec();
    }
    Some(Listener {
        name: name?,
        address: address?,
        protocols,
    })
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

fn check_listeners(d: &mut Decoder<'_>, listeners: &[At<Listener>]) {
    let mut seen: Vec<&str> = Vec::new();
    for l in listeners {
        let path = crate::error::join_field(&l.path, "name");
        if !valid_name(&l.v.name) {
            d.error(l.off, &path, bad_name(&l.v.name));
        } else if seen.contains(&l.v.name.as_str()) {
            d.error(
                l.off,
                &path,
                format!("another listener is already named {:?}", l.v.name),
            );
        }
        seen.push(&l.v.name);
    }
}

fn check_routes(d: &mut Decoder<'_>, routes: &[At<Route>], listeners: &[At<Listener>]) {
    let mut seen: Vec<&str> = Vec::new();
    for r in routes {
        if let Some(name) = r.v.name.as_deref().filter(|s| !s.is_empty()) {
            let path = crate::error::join_field(&r.path, "name");
            if !valid_name(name) {
                d.error(r.off, &path, bad_name(name));
            } else if seen.contains(&name) {
                d.error(
                    r.off,
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
                d.error(r.off, &path, format!("there is no listener named {l:?}"));
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
