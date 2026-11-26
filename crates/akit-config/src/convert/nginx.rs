//! nginx configs to fit's native JSON, as fit's `adapter/nginx` converts them: the http block's
//! servers (listen, server_name, ssl_certificate), their locations (exact, prefix, `^~` and
//! regular expressions, nested), and in them proxy_pass, upstream blocks, root, alias, index,
//! try_files, return, rewrite, limit_req with limit_req_zone, client_max_body_size, add_header,
//! proxy_set_header with literal values, gzip, and error_log and access_log, to fit's log and
//! access log. Every directive it does not convert is a warning naming its line.

use std::collections::{HashMap, HashSet};

use super::gostd::{
    self, Fields, J, Params, atoi, atoi_ok, join_host_port, parse_float, quote, split_host_port,
};
use super::nginx_parse::{Directive, parse};
use super::{Converted, goregexp};

#[derive(Debug, Clone, Default)]
struct OutListener {
    name: String,
    address: String,
    protocols: Vec<String>,
    /// The certificates of a TLS listener.
    tls: Option<Vec<OutCert>>,
    line: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutCert {
    cert_file: String,
    key_file: String,
}

#[derive(Debug, Clone, Default)]
struct OutUpstream {
    name: String,
    addresses: Vec<String>,
    weights: Vec<i64>,
    load_balancing: String,
    hash_key: String,
    max_conns: i64,
    max_idle_conns: Option<i64>,
    connect_timeout: String,
    protocol: String,
    max_fails: Option<i64>,
    fail_timeout: String,
    response_idle_timeout: String,
    line: i64,
    /// The upstream of the servers marked backup, for proxies to fail over to.
    backup: String,
}

#[derive(Debug, Clone, Default)]
struct OutRoute {
    name: String,
    listeners: Vec<String>,
    priority: i64,
    matcher: Option<OutMatch>,
    middleware: Vec<OutRef>,
    handler: OutRef,
    line: i64,
}

#[derive(Debug, Clone, Default)]
struct OutMatch {
    host: Vec<String>,
    path: Vec<String>,
    path_regexp: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct OutRef {
    module: String,
    params: Params,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutLog {
    level: String,
    output: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutAccessLog {
    output: String,
    flush_interval: String,
    buffer_bytes: i64,
}

fn list(v: &[String]) -> J {
    if v.is_empty() { J::Null } else { J::strs(v) }
}

impl OutListener {
    fn json(&self) -> J {
        Fields::new()
            .put("name", J::str(&self.name))
            .put("address", J::str(&self.address))
            .strs("protocols", &self.protocols)
            .opt(
                "tls",
                self.tls.as_ref().map(|certs| {
                    let certs = if certs.is_empty() {
                        J::Null
                    } else {
                        J::Arr(
                            certs
                                .iter()
                                .map(|c| {
                                    Fields::new()
                                        .put("cert_file", J::str(&c.cert_file))
                                        .put("key_file", J::str(&c.key_file))
                                        .done()
                                })
                                .collect(),
                        )
                    };
                    Fields::new().put("certificates", certs).done()
                }),
            )
            .done()
    }
}

impl OutUpstream {
    fn json(&self) -> J {
        Fields::new()
            .put("name", J::str(&self.name))
            .put("addresses", list(&self.addresses))
            .opt(
                "weights",
                (!self.weights.is_empty())
                    .then(|| J::Arr(self.weights.iter().map(|w| J::Int(*w)).collect())),
            )
            .str("load_balancing", &self.load_balancing)
            .str("hash_key", &self.hash_key)
            .int("max_conns", self.max_conns)
            .opt("max_idle_conns", self.max_idle_conns.map(J::Int))
            .str("connect_timeout", &self.connect_timeout)
            .str("protocol", &self.protocol)
            .opt("max_fails", self.max_fails.map(J::Int))
            .str("fail_timeout", &self.fail_timeout)
            .str("response_idle_timeout", &self.response_idle_timeout)
            .done()
    }
}

impl OutRef {
    fn new(module: &str, params: Params) -> OutRef {
        OutRef {
            module: module.into(),
            params,
        }
    }

    fn json(&self) -> J {
        Fields::new()
            .put("module", J::str(&self.module))
            .params("params", &self.params)
            .done()
    }
}

impl OutRoute {
    fn json(&self) -> J {
        Fields::new()
            .str("name", &self.name)
            .strs("listeners", &self.listeners)
            .int("priority", self.priority)
            .opt(
                "match",
                self.matcher.as_ref().map(|m| {
                    Fields::new()
                        .strs("host", &m.host)
                        .strs("path", &m.path)
                        .strs("path_regexp", &m.path_regexp)
                        .done()
                }),
            )
            .opt(
                "middleware",
                (!self.middleware.is_empty())
                    .then(|| J::Arr(self.middleware.iter().map(OutRef::json).collect())),
            )
            .put("handler", self.handler.json())
            .done()
    }
}

/// `{key: value, ...}`.
fn params<const N: usize>(kv: [(&str, J); N]) -> Params {
    kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// A `limit_req_zone`: what requests are counted by, and the rate.
#[derive(Debug, Clone)]
struct Zone {
    key: String,
    rate: f64,
}

/// What a context's directives set, as nginx inherits them: from http to server to location,
/// and for add_header, limit_req and proxy_set_header only when the inner context sets none of
/// its own.
#[derive(Debug, Clone, Default)]
struct Settings {
    root: String,
    alias: String,
    index: Vec<String>,
    try_files: Vec<String>,
    body_limit: i64,
    gzip: bool,
    gzip_level: i64,
    gzip_min_length: i64,
    add_headers: Vec<(String, String)>,
    limit_reqs: Vec<Params>,
    proxy_headers: Vec<(String, String)>,
    host_header: String,
    /// The line of the auth_basic this context is under, 0 for none: its requests answer 401,
    /// since its users are not converted.
    auth_basic: i64,
    set_add_headers: bool,
    set_limit_reqs: bool,
    set_proxy_headers: bool,
}

/// A location's settings, starting from its parent's.
fn inherit(p: &Settings) -> Settings {
    let mut s = p.clone();
    s.set_add_headers = false;
    s.set_limit_reqs = false;
    s.set_proxy_headers = false;
    s
}

/// What a server's locations share.
#[derive(Debug, Default)]
struct ServerInfo {
    name: String,
    hosts: Vec<String>,
    listeners: Vec<String>,
    rewrites: Vec<OutRef>,
    server_name: Option<String>,
}

#[derive(Default)]
struct Converter {
    listeners: Vec<OutListener>,
    upstreams: Vec<OutUpstream>,
    routes: Vec<OutRoute>,
    log: Option<OutLog>,
    access_log: Option<OutAccessLog>,
    warnings: Vec<String>,
    listener_at: HashMap<String, usize>,
    upstream_at: HashMap<String, usize>,
    zones: HashMap<String, Zone>,
    servers: i64,
    names: HashSet<String>,
    log_line: i64,
    access_log_line: i64,
}

/// Converts an nginx config to fit's native JSON.
pub(crate) fn convert(src: &str) -> Result<Converted, String> {
    convert_parsed(&parse(src)?)
}

/// Converts an nginx config read into its directives.
pub(crate) fn convert_parsed(ds: &[Directive]) -> Result<Converted, String> {
    let mut c = Converter::default();
    for d in ds {
        match d.name.as_str() {
            "http" => c.http(d),
            "error_log" => c.error_log(d, "main"),
            "events"
            | "user"
            | "worker_processes"
            | "worker_rlimit_nofile"
            | "worker_cpu_affinity"
            | "pid"
            | "load_module"
            | "daemon"
            | "master_process"
            | "thread_pool"
            | "env"
            | "worker_shutdown_timeout" => {
                c.skip(d, "akit runs as one process with its own workers");
            }
            "stream" => c.skip(
                d,
                "TCP and UDP proxying is akit's tcp and udp listeners, written by hand",
            ),
            _ => c.skip(d, "not converted"),
        }
    }
    if c.routes.is_empty() {
        return Err("the config has no server to convert".into());
    }
    gostd::sort_warnings(&mut c.warnings);
    let mut top = Fields::new()
        .put(
            "listeners",
            if c.listeners.is_empty() {
                J::Null
            } else {
                J::Arr(c.listeners.iter().map(OutListener::json).collect())
            },
        )
        .opt(
            "upstreams",
            (!c.upstreams.is_empty())
                .then(|| J::Arr(c.upstreams.iter().map(OutUpstream::json).collect())),
        )
        .put(
            "routes",
            J::Arr(c.routes.iter().map(OutRoute::json).collect()),
        );
    if let Some(l) = &c.log {
        top = top.put(
            "log",
            Fields::new()
                .str("level", &l.level)
                .str("output", &l.output)
                .done(),
        );
    }
    if let Some(l) = &c.access_log {
        top = top.put(
            "access_log",
            Fields::new()
                .str("output", &l.output)
                .str("flush_interval", &l.flush_interval)
                .int("buffer_bytes", l.buffer_bytes)
                .done(),
        );
    }
    let json = top.done().marshal()?;
    let mut positions = HashMap::new();
    for (i, l) in c.listeners.iter().enumerate() {
        positions.insert(format!("listeners[{i}]"), l.line);
    }
    for (i, u) in c.upstreams.iter().enumerate() {
        positions.insert(format!("upstreams[{i}]"), u.line);
    }
    for (i, r) in c.routes.iter().enumerate() {
        positions.insert(format!("routes[{i}]"), r.line);
    }
    if c.log_line > 0 {
        positions.insert("log".into(), c.log_line);
    }
    if c.access_log_line > 0 {
        positions.insert("access_log".into(), c.access_log_line);
    }
    Ok(Converted {
        json,
        warnings: c.warnings,
        positions,
    })
}

/// Whether `s` holds an nginx variable: `$name` or `${name}`.
fn has_var(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        if c != b'$' {
            return false;
        }
        let ident = |j: usize| {
            b.get(j)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        };
        let i1 = i.saturating_add(1);
        if ident(i1) {
            return true;
        }
        if b.get(i1) == Some(&b'{') && ident(i1.saturating_add(1)) {
            let mut j = i1.saturating_add(2);
            while b
                .get(j)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                j = j.saturating_add(1);
            }
            return b.get(j) == Some(&b'}');
        }
        false
    })
}

impl Converter {
    fn warn(&mut self, line: i64, msg: impl AsRef<str>) {
        self.warnings.push(format!("line {line}: {}", msg.as_ref()));
    }

    /// Reports that `d` is not converted, and why.
    fn skip(&mut self, d: &Directive, why: &str) {
        self.warn(d.line, format!("{}: skipped: {why}", d.name));
    }

    fn route_name(&mut self, server: &str, path: &str) -> String {
        gostd::route_name(&mut self.names, server, path, "/")
    }

    fn http(&mut self, h: &Directive) {
        let mut base = Settings {
            body_limit: -1,
            ..Settings::default()
        };
        // Upstreams and zones first, which servers refer to wherever they are.
        for d in &h.block {
            match d.name.as_str() {
                "upstream" => self.upstream(d),
                "limit_req_zone" => self.limit_req_zone(d),
                _ => {}
            }
        }
        for d in &h.block {
            if !matches!(d.name.as_str(), "upstream" | "limit_req_zone" | "server") {
                self.setting(&mut base, d, "http");
            }
        }
        for d in &h.block {
            if d.name == "server" {
                self.server(d, &base);
            }
        }
    }

    /// Reads a directive that sets something servers and locations inherit, into `s`, and
    /// reports the rest as skipped.
    #[allow(clippy::too_many_lines)]
    fn setting(&mut self, s: &mut Settings, d: &Directive, place: &str) {
        let one = d.args.len() == 1;
        let arg0 = d.args.first().map(String::as_str).unwrap_or_default();
        match d.name.as_str() {
            "root" if one => {
                s.root = arg0.into();
                s.alias.clear();
                return;
            }
            "alias" if place == "location" && one => {
                s.alias = arg0.into();
                return;
            }
            "index" if !d.args.is_empty() => {
                s.index.clone_from(&d.args);
                return;
            }
            "try_files" => {
                if let Some(t) = self.try_files(d) {
                    s.try_files = t;
                }
                return;
            }
            "client_max_body_size" if one => {
                if let Some(n) = size(arg0) {
                    s.body_limit = n;
                    return;
                }
            }
            "gzip" if one => {
                s.gzip = arg0 == "on";
                return;
            }
            "auth_basic" if one => {
                if arg0 == "off" {
                    s.auth_basic = 0;
                    return;
                }
                s.auth_basic = d.line;
                self.warn(d.line, "auth_basic: not converted: what it guards answers 401 until a basic_auth middleware takes its place, with its users' bcrypt hashes (akit hash-password makes one)");
                return;
            }
            "auth_basic_user_file" => {
                self.skip(d, "akit does not read htpasswd files: basic_auth's users hold the bcrypt hashes, written in the config");
                return;
            }
            "add_header" if (2..=3).contains(&d.args.len()) => {
                let (name, value) = (arg0, d.args.get(1).map(String::as_str).unwrap_or_default());
                if value.contains('$') {
                    self.skip(d, "its value has variables, which akit does not expand");
                    return;
                }
                if !s.set_add_headers {
                    s.add_headers.clear();
                    s.set_add_headers = true;
                }
                s.add_headers.push((name.into(), value.into()));
                return;
            }
            "limit_req" => {
                if let Some(r) = self.limit_req(d) {
                    if !s.set_limit_reqs {
                        s.limit_reqs.clear();
                        s.set_limit_reqs = true;
                    }
                    s.limit_reqs.push(r);
                }
                return;
            }
            "proxy_set_header" => {
                self.proxy_set_header(s, d);
                return;
            }
            // fit's proxy speaks HTTP/1.1 to upstreams and keeps their Location.
            "proxy_http_version" | "proxy_redirect" => return,
            "gzip_comp_level" => {
                if one && let Some(n) = atoi_ok(arg0).filter(|n| (1..=9).contains(n)) {
                    s.gzip_level = n;
                    return;
                }
                self.skip(d, "gzip_comp_level takes a level of 1 to 9");
                return;
            }
            "gzip_min_length" => {
                if one && let Some(n) = size(arg0).filter(|n| *n > 0) {
                    s.gzip_min_length = n;
                    return;
                }
                self.skip(d, "gzip_min_length takes a length of 1 byte or more");
                return;
            }
            "gzip_types" | "gzip_vary" | "gzip_proxied" | "gzip_http_version" | "gzip_disable"
            | "gzip_buffers" => {
                self.skip(
                    d,
                    "akit's compress middleware compresses what compresses well, with zstd, brotli or gzip",
                );
                return;
            }
            "ssl_protocols"
            | "ssl_ciphers"
            | "ssl_prefer_server_ciphers"
            | "ssl_session_cache"
            | "ssl_session_timeout"
            | "ssl_session_tickets"
            | "ssl_ecdh_curve"
            | "ssl_dhparam"
            | "ssl_stapling"
            | "ssl_stapling_verify" => {
                self.skip(
                    d,
                    "akit picks TLS versions and ciphers, and staples OCSP, itself",
                );
                return;
            }
            "ssl_conf_command" => {
                if d.args.len() == 2
                    && arg0.eq_ignore_ascii_case("Options")
                    && d.args.get(1).is_some_and(|a| go_upper(a).contains("KTLS"))
                {
                    // fit hands a connection that sends a large file to the kernel's TLS
                    // anyway; every connection, as Options KTLS does, is a setting of the whole
                    // engine.
                    self.skip(d, "akit uses kTLS for large files by default; for every connection, as for a network card that encrypts TLS, set engine.kernel_tls_offload");
                    return;
                }
                self.skip(
                    d,
                    "akit picks TLS versions and ciphers, and staples OCSP, itself",
                );
                return;
            }
            "error_log" => {
                self.error_log(d, place);
                return;
            }
            "access_log" => {
                self.access_log(d, place);
                return;
            }
            "log_format" => {
                self.skip(
                    d,
                    "akit writes its access log lines as JSON, with fields of its own",
                );
                return;
            }
            "sendfile"
            | "tcp_nopush"
            | "tcp_nodelay"
            | "keepalive_timeout"
            | "keepalive_requests"
            | "types_hash_max_size"
            | "server_tokens"
            | "default_type"
            | "include"
            | "types"
            | "charset"
            | "client_body_buffer_size"
            | "client_header_buffer_size"
            | "large_client_header_buffers"
            | "proxy_buffering"
            | "proxy_buffers"
            | "proxy_buffer_size"
            | "proxy_busy_buffers_size"
            | "proxy_send_timeout"
            | "send_timeout"
            | "client_body_timeout"
            | "client_header_timeout"
            | "resolver"
            | "resolver_timeout" => {
                self.skip(d, "not converted: akit's defaults stand in for it");
                return;
            }
            _ => {}
        }
        if d.name.contains("_by_lua") {
            self.skip(d, "OpenResty's Lua is not converted: akit's plugins are WebAssembly, run by the wasm middleware");
            return;
        }
        self.skip(d, "not converted");
    }

    #[allow(clippy::too_many_lines)]
    fn server(&mut self, sd: &Directive, base: &Settings) {
        self.servers = self.servers.saturating_add(1);
        let mut s = inherit(base);
        let mut info = ServerInfo {
            name: format!("server{}", self.servers),
            ..ServerInfo::default()
        };
        let mut listens: Vec<Directive> = Vec::new();
        let mut certs = Vec::new();
        let (mut cert, mut key) = (String::new(), String::new());
        let mut locations: Vec<&Directive> = Vec::new();
        let mut ret: Option<&Directive> = None;
        let mut http2 = false;
        for d in &sd.block {
            let one = d.args.len() == 1;
            let arg0 = d.args.first().map(String::as_str).unwrap_or_default();
            match d.name.as_str() {
                "listen" => listens.push(d.clone()),
                "server_name" => self.server_name(&mut info, d),
                "ssl_certificate" => {
                    if one {
                        cert = arg0.into();
                    }
                }
                "ssl_certificate_key" => {
                    if one {
                        key = arg0.into();
                    }
                }
                "http2" => http2 = one && arg0 == "on",
                "location" => locations.push(d),
                "return" => ret = Some(d),
                "rewrite" => {
                    if let Some(r) = self.rewrite(d, &info) {
                        info.rewrites.push(r);
                    }
                }
                "ssl" => self.skip(d, "write ssl on listen"),
                _ => self.setting(&mut s, d, "server"),
            }
        }
        if !cert.is_empty() && !key.is_empty() {
            certs.push(OutCert {
                cert_file: cert,
                key_file: key,
            });
        }
        if listens.is_empty() {
            listens.push(Directive {
                name: "listen".into(),
                args: vec!["80".into()],
                line: sd.line,
                block: Vec::new(),
            });
        }
        for d in &listens {
            if let Some(name) = self.listen(d, &certs, http2)
                && !info.listeners.contains(&name)
            {
                info.listeners.push(name);
            }
        }
        if info.listeners.is_empty() {
            self.warn(
                sd.line,
                "server: skipped: none of its listen directives converted",
            );
            return;
        }
        if let Some(ret) = ret {
            // A server's return answers every request before any location is looked at.
            let Some((h, mw)) = self.ret(ret, &info) else {
                return;
            };
            for l in &locations {
                self.warn(
                    l.line,
                    format!(
                        "location {}: skipped: the server's return answers every request first",
                        l.args.join(" ")
                    ),
                );
            }
            let name = self.route_name(&info.name.clone(), "");
            let mut middleware = info.rewrites.clone();
            middleware.extend(mw);
            self.routes.push(OutRoute {
                name,
                listeners: info.listeners.clone(),
                priority: 0,
                matcher: host_match(&info.hosts),
                middleware,
                handler: h,
                line: ret.line,
            });
            return;
        }
        let mut has_root = false;
        let mut regexps = 0i64;
        for l in &locations {
            self.location(l, &s, &info, &mut regexps);
            if l.args.len() == 1 && l.args.first().is_some_and(|a| a == "/")
                || l.args.len() == 2
                    && l.args.first().is_some_and(|a| a == "^~")
                    && l.args.get(1).is_some_and(|a| a == "/")
            {
                has_root = true;
            }
        }
        if !has_root {
            // What no location takes, the server answers as nginx does: from its root, or
            // with 404.
            let name = self.route_name(&info.name.clone(), "default");
            let m = OutMatch {
                host: info.hosts.clone(),
                ..OutMatch::default()
            };
            self.add_route(sd, &s, &info, Some(m), -1, name, "");
        }
    }

    fn server_name(&mut self, info: &mut ServerInfo, d: &Directive) {
        for n in &d.args {
            if n == "_" || n.is_empty() || n == "localhost" && d.args.len() == 1 {
                // A catch-all: no host to match.
            } else if n.starts_with('~') {
                self.warn(d.line, format!("server_name {n}: skipped: a regular expression of names is not converted; the server matches its other names"));
            } else if let Some(rest) = n.strip_prefix("*.") {
                info.hosts.push(format!("**.{rest}"));
            } else if let Some(rest) = n.strip_prefix('.') {
                info.hosts.push(rest.into());
                info.hosts.push(format!("**{n}"));
            } else if n.ends_with(".*") {
                self.warn(
                    d.line,
                    format!(
                        "server_name {n}: skipped: a wildcard at the end of a name is not converted"
                    ),
                );
            } else {
                info.hosts.push(go_lower(n));
            }
            if info.server_name.as_deref().unwrap_or_default().is_empty()
                && let Some(h) = info.hosts.first()
            {
                info.server_name = Some(h.clone());
            }
        }
        if let Some(first) = d.args.first()
            && !first.starts_with('~')
            && first != "_"
        {
            info.name.clone_from(first);
        }
    }

    /// Converts a listen directive to a listener, shared with the other servers on the same
    /// address, and returns its name.
    #[allow(clippy::too_many_lines)]
    fn listen(&mut self, d: &Directive, certs: &[OutCert], mut http2: bool) -> Option<String> {
        let Some(addr) = d.args.first() else {
            self.skip(d, "no address");
            return None;
        };
        if addr.starts_with("unix:") {
            self.skip(d, "akit listens on TCP and UDP addresses");
            return None;
        }
        let (mut host, port) = match split_host_port(addr) {
            Some((h, p)) => (h.to_owned(), p.to_owned()),
            None if atoi(addr).is_ok() => (String::new(), addr.clone()),
            None => (addr.clone(), "80".to_owned()),
        };
        if host == "*" || host == "::" || host == "0.0.0.0" {
            // fit's ":port" takes every address, IPv4 and IPv6.
            host.clear();
        }
        let (mut tls, mut quic) = (false, false);
        for a in d.args.iter().skip(1) {
            match a.as_str() {
                "ssl" => tls = true,
                "http2" => http2 = true,
                "quic" | "http3" => {
                    quic = true;
                    tls = true;
                }
                "default_server" | "default" | "reuseport" | "ipv6only=on" | "ipv6only=off"
                | "deferred" => {}
                a if a.starts_with("backlog=")
                    || a.starts_with("rcvbuf=")
                    || a.starts_with("sndbuf=") => {}
                "proxy_protocol" => self.warn(d.line, format!("listen {addr} proxy_protocol: the PROXY protocol is not converted: set the listener's proxy_protocol and the proxies it trusts")),
                a => self.warn(d.line, format!("listen {addr} {a}: skipped: not converted")),
            }
        }
        if tls && certs.is_empty() {
            self.warn(d.line, format!("listen {addr}: skipped: ssl with no ssl_certificate and ssl_certificate_key in its server"));
            return None;
        }
        let address = if host.is_empty() {
            format!(":{port}")
        } else {
            join_host_port(&host, &port)
        };
        let i = if let Some(&i) = self.listener_at.get(&address) {
            let has_tls = self.listeners.get(i).is_some_and(|l| l.tls.is_some());
            if has_tls != tls && !quic {
                let with = if tls { "out" } else { "" };
                self.warn(
                    d.line,
                    format!("listen {addr}: skipped: another server listens there with{with} ssl"),
                );
                return None;
            }
            i
        } else {
            let scheme = if tls { "https" } else { "http" };
            let name = format!(
                "{scheme}_{}",
                gostd::addr_name(address.strip_prefix(':').unwrap_or(&address), false)
            );
            self.listeners.push(OutListener {
                name,
                address: address.clone(),
                protocols: vec!["http/1.1".into()],
                tls: tls.then(Vec::new),
                line: d.line,
            });
            let i = self.listeners.len().saturating_sub(1);
            self.listener_at.insert(address, i);
            i
        };
        let l = self.listeners.get_mut(i)?;
        if tls && let Some(have) = &mut l.tls {
            for cert in certs {
                if !have.contains(cert) {
                    have.push(cert.clone());
                }
            }
        }
        let mut add = |p: &str| {
            if !l.protocols.iter().any(|x| x == p) {
                l.protocols.push(p.into());
            }
        };
        if http2 {
            add(if tls { "h2" } else { "h2c" });
        }
        if quic {
            add("h3");
        }
        Some(l.name.clone())
    }

    /// Converts a location block, and those nested in it, to routes.
    #[allow(clippy::too_many_lines)]
    fn location(&mut self, d: &Directive, parent: &Settings, info: &ServerInfo, regexps: &mut i64) {
        if d.args.is_empty() || d.args.len() > 2 {
            self.skip(d, "a location needs a path");
            return;
        }
        let (modifier, p) = match d.args.as_slice() {
            [m, p] => (m.as_str(), p.as_str()),
            [p] => ("", p.as_str()),
            _ => return,
        };
        if p.starts_with('@') {
            self.skip(
                d,
                "named locations, for try_files and error_page, are not converted",
            );
            return;
        }
        let mut m = OutMatch {
            host: info.hosts.clone(),
            ..OutMatch::default()
        };
        let priority = match modifier {
            "=" => {
                m.path = vec![p.into()];
                30000
            }
            "" | "^~" => {
                m.path = vec![format!("{p}*")];
                if modifier == "^~" { 20000 } else { 0 }
            }
            "~" | "~*" => {
                let mut re = p.to_owned();
                if modifier == "~*" {
                    re = format!("(?i){re}");
                } else if re.contains(r"\.php") {
                    // A file system that folds case opens x.php for /x.PHP, which a
                    // case-sensitive route misses; a static handler beside it would serve the
                    // script's source.
                    re = format!("(?i){re}");
                    self.warn(d.line, format!("location ~ {p}: matched in any case, as (?i){p}, since a file system that folds case opens x.php for /x.PHP"));
                }
                if let Err(e) = goregexp::check(&re) {
                    self.warn(
                        d.line,
                        format!(
                            "location {modifier} {p}: skipped: Go's regular expressions do not take it: {e}"
                        ),
                    );
                    return;
                }
                m.path_regexp = vec![re];
                // nginx tries regular expressions in order: the first wins.
                *regexps = regexps.saturating_add(1);
                10000i64.saturating_sub(*regexps)
            }
            _ => {
                self.skip(d, &format!("an unknown location modifier {modifier}"));
                return;
            }
        };
        let mut s = inherit(parent);
        let mut nested = Vec::new();
        let mut ret = None;
        let mut proxy = None;
        let mut rewrites = Vec::new();
        for e in &d.block {
            match e.name.as_str() {
                "location" => nested.push(e),
                "return" => ret = Some(e),
                "proxy_pass" => proxy = Some(e),
                "rewrite" => {
                    if let Some(r) = self.rewrite(e, info) {
                        rewrites.push(r);
                    }
                }
                // Kept for the upstream, below.
                "proxy_connect_timeout" | "proxy_read_timeout" => {}
                "if" => self.skip(e, "if blocks are not converted"),
                _ => self.setting(&mut s, e, "location"),
            }
        }
        let name = self.route_name(&info.name, p);
        let mut r = OutRoute {
            name,
            listeners: info.listeners.clone(),
            priority,
            matcher: Some(m),
            line: d.line,
            ..OutRoute::default()
        };
        r.middleware = info.rewrites.clone();
        r.middleware.extend(rewrites);
        r.middleware.extend(limits(&s));
        if let Some(ret) = ret {
            let Some((h, mw)) = self.ret(ret, info) else {
                return;
            };
            r.middleware.extend(mw);
            r.handler = h;
        } else if let Some(proxy) = proxy {
            let Some((h, mw)) = self.proxy_pass(proxy, d, modifier, p, &s) else {
                return;
            };
            r.middleware.extend(mw);
            r.handler = h;
        } else {
            let Some(h) = self.static_handler(d, &s, modifier, p) else {
                return;
            };
            r.handler = h;
        }
        if ret.is_none() {
            guarded(&mut r, &s);
        }
        r.middleware.extend(response(&s));
        self.routes.push(r);
        for n in nested {
            self.location(n, &s, info, regexps);
        }
    }

    /// Adds a route for a server's requests no location takes.
    #[allow(clippy::too_many_arguments)]
    fn add_route(
        &mut self,
        d: &Directive,
        s: &Settings,
        info: &ServerInfo,
        mut m: Option<OutMatch>,
        priority: i64,
        name: String,
        prefix: &str,
    ) {
        let Some(h) = self.static_handler(d, s, "", prefix) else {
            return;
        };
        if m.as_ref()
            .is_some_and(|m| m.host.is_empty() && m.path.is_empty())
        {
            m = None;
        }
        let mut middleware = info.rewrites.clone();
        middleware.extend(limits(s));
        middleware.extend(response(s));
        let mut r = OutRoute {
            name,
            listeners: info.listeners.clone(),
            priority,
            matcher: m,
            middleware,
            handler: h,
            line: d.line,
        };
        guarded(&mut r, s);
        self.routes.push(r);
    }

    /// The handler of a location that serves files, or answers 404 when the location says
    /// nothing of what to serve.
    fn static_handler(
        &mut self,
        d: &Directive,
        s: &Settings,
        modifier: &str,
        prefix: &str,
    ) -> Option<OutRef> {
        if s.root.is_empty() && s.alias.is_empty() {
            if !s.try_files.is_empty() {
                self.warn(
                    d.line,
                    format!(
                        "{} {}: try_files with no root: answered 404",
                        d.name,
                        d.args.join(" ")
                    ),
                );
            }
            return Some(OutRef::new("respond", params([("status", J::Int(404))])));
        }
        let mut p = Params::new();
        if s.alias.is_empty() {
            p.insert("root".into(), J::str(&s.root));
        } else {
            if !modifier.is_empty() && modifier != "^~" {
                self.warn(d.line, format!("location {modifier} {prefix}: skipped: alias in a location of a regular expression is not converted"));
                return None;
            }
            p.insert(
                "root".into(),
                J::str(s.alias.strip_suffix('/').unwrap_or(&s.alias)),
            );
            let strip = prefix.strip_suffix('/').unwrap_or(prefix);
            if !strip.is_empty() {
                p.insert("strip_prefix".into(), J::str(strip));
            }
        }
        if !s.index.is_empty() && s.index != ["index.html"] {
            p.insert("index".into(), J::strs(&s.index));
        }
        if !s.try_files.is_empty() {
            p.insert("try_files".into(), J::strs(&s.try_files));
        }
        Some(OutRef::new("static", p))
    }

    /// Converts try_files, its variables to fit's placeholders.
    fn try_files(&mut self, d: &Directive) -> Option<Vec<String>> {
        let mut out = Vec::new();
        for (i, a) in d.args.iter().enumerate() {
            if a.starts_with('=') {
                if i.saturating_add(1) != d.args.len() {
                    break;
                }
                if a != "=404" {
                    self.warn(
                        d.line,
                        format!("try_files: the last {a} answers 404 in akit"),
                    );
                }
                continue;
            }
            if a.starts_with('@') {
                self.skip(d, "named locations are not converted");
                return None;
            }
            let t = a
                .replace("$uri", "{path}")
                .replace("$document_uri", "{path}");
            if t.contains('$') {
                self.skip(d, "it has variables other than $uri");
                return None;
            }
            out.push(t);
        }
        if out.is_empty() {
            self.skip(d, "nothing to try");
            return None;
        }
        Some(out)
    }

    /// Converts return to a handler, and a redirect in front of it.
    fn ret(&mut self, d: &Directive, info: &ServerInfo) -> Option<(OutRef, Vec<OutRef>)> {
        if d.args.is_empty() || d.args.len() > 2 {
            self.skip(d, "return takes a status and a text or URL");
            return None;
        }
        let arg0 = d.args.first().map(String::as_str).unwrap_or_default();
        let (code, args) = match atoi(arg0) {
            Ok(n) => (n, d.args.clone()),
            // return URL: a temporary redirect.
            Err(_) => (302, vec!["302".to_owned(), arg0.to_owned()]),
        };
        if !(100..=599).contains(&code) {
            self.skip(d, "a status out of range");
            return None;
        }
        let Some(text) = args.get(1) else {
            return Some((
                OutRef::new("respond", params([("status", J::Int(code))])),
                Vec::new(),
            ));
        };
        if matches!(code, 301 | 302 | 303 | 307 | 308) {
            let Some(to) = vars(text, info) else {
                self.skip(d, "its URL has variables akit has no placeholder for");
                return None;
            };
            return Some((
                OutRef::new("respond", params([("status", J::Int(404))])),
                vec![OutRef::new(
                    "redirect",
                    params([("to", J::Str(to)), ("status", J::Int(code))]),
                )],
            ));
        }
        if text.contains('$') {
            self.skip(d, "its text has variables, which akit does not expand");
            return None;
        }
        Some((
            OutRef::new(
                "respond",
                params([("status", J::Int(code)), ("body", J::str(text))]),
            ),
            Vec::new(),
        ))
    }

    /// Converts rewrite to the rewrite middleware, or to a redirect.
    fn rewrite(&mut self, d: &Directive, info: &ServerInfo) -> Option<OutRef> {
        if d.args.len() < 2 || d.args.len() > 3 {
            self.skip(
                d,
                "rewrite takes a regular expression, a replacement and a flag",
            );
            return None;
        }
        let re = d.args.first().map(String::as_str).unwrap_or_default();
        let to = d.args.get(1).map(String::as_str).unwrap_or_default();
        if let Err(e) = goregexp::check(re) {
            self.warn(
                d.line,
                format!(
                    "rewrite: skipped: Go's regular expressions do not take {}: {e}",
                    quote(re)
                ),
            );
            return None;
        }
        let flag = d.args.get(2).map(String::as_str).unwrap_or_default();
        if flag == "redirect"
            || flag == "permanent"
            || to.starts_with("http://")
            || to.starts_with("https://")
            || to.starts_with("$scheme")
        {
            let status = if flag == "permanent" { 301 } else { 302 };
            let Some(u) = vars(to, info) else {
                self.skip(
                    d,
                    "its replacement has variables akit has no placeholder for",
                );
                return None;
            };
            return Some(OutRef::new(
                "redirect",
                params([
                    ("regexp", J::str(re)),
                    ("to", J::Str(u)),
                    ("status", J::Int(status)),
                ]),
            ));
        }
        let dollar_var = to.as_bytes().windows(2).any(|w| {
            w.first() == Some(&b'$')
                && w.get(1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'_' | b'{'))
        });
        if to.contains('?') || to.contains('$') && dollar_var {
            self.skip(
                d,
                "a replacement with a query or with variables is not converted",
            );
            return None;
        }
        if flag == "last" {
            self.warn(
                d.line,
                "rewrite ... last: converted, but akit does not look for the location again after it",
            );
        }
        // nginx replaces the whole path when the expression matches anywhere in it; fit's
        // rewrite replaces each match, so the expression is made to cover the whole path.
        let mut whole = re.to_owned();
        if !re.starts_with('^') || !re.ends_with('$') {
            let inner = re.strip_prefix('^').unwrap_or(re);
            whole = format!("^.*?(?:{}).*$", inner.strip_suffix('$').unwrap_or(inner));
            if let Some(rest) = re.strip_prefix('^') {
                whole = format!("^(?:{rest}).*$");
            }
        }
        Some(OutRef::new(
            "rewrite",
            params([("regexp", J::Str(whole)), ("replacement", J::str(to))]),
        ))
    }

    fn proxy_set_header(&mut self, s: &mut Settings, d: &Directive) {
        let [field, value] = d.args.as_slice() else {
            self.skip(d, "proxy_set_header takes a field and a value");
            return;
        };
        let name = go_lower(field);
        let v = value.as_str();
        if name == "host" && (v == "$host" || v == "$http_host") {
            // fit's proxy sends the client's Host.
        } else if name == "host" && !v.contains('$') {
            s.host_header = v.into();
        } else if name == "connection" || name == "upgrade" {
            // fit's proxy joins WebSockets and other upgrades itself.
        } else if (name == "x-forwarded-for" && v == "$proxy_add_x_forwarded_for")
            || (name == "x-forwarded-proto" && v == "$scheme")
            || (name == "x-forwarded-host" && (v == "$host" || v == "$http_host"))
        {
            // fit's proxy sets X-Forwarded-For, -Proto and -Host itself.
        } else if v.contains('$') {
            self.skip(d, "its value has variables, which akit does not expand");
        } else {
            if !s.set_proxy_headers {
                s.proxy_headers.clear();
                s.set_proxy_headers = true;
            }
            s.proxy_headers.push((field.clone(), v.into()));
        }
    }

    /// Converts proxy_pass to the proxy handler, with the upstream it names or one made for
    /// its address, and the rewrite its URI asks for.
    #[allow(clippy::too_many_lines)]
    fn proxy_pass(
        &mut self,
        d: &Directive,
        loc: &Directive,
        modifier: &str,
        prefix: &str,
        s: &Settings,
    ) -> Option<(OutRef, Vec<OutRef>)> {
        let [target] = d.args.as_slice() else {
            self.skip(d, "proxy_pass takes one URL");
            return None;
        };
        if target.contains('$') {
            self.skip(d, "a URL with variables is not converted");
            return None;
        }
        let (scheme, rest) = match target.split_once("://") {
            Some((s, r)) if s == "http" || s == "https" => (s, r),
            _ => {
                self.skip(d, "akit proxies to http:// and https:// upstreams");
                return None;
            }
        };
        let (hostport, uri, has_uri) = match rest.split_once('/') {
            Some((h, u)) => (h, format!("/{u}"), true),
            None => (rest, "/".to_owned(), false),
        };
        let i = if let Some(&i) = self.upstream_at.get(hostport) {
            i
        } else {
            let mut addr = hostport.to_owned();
            if split_host_port(&addr).is_none() {
                let port = if scheme == "https" { "443" } else { "80" };
                addr = join_host_port(&addr, port);
            }
            let name = format!("proxy_{}", gostd::addr_name(hostport, false));
            if let Some(&i) = self.upstream_at.get(&name) {
                i
            } else {
                self.upstreams.push(OutUpstream {
                    name: name.clone(),
                    addresses: vec![addr],
                    line: d.line,
                    ..OutUpstream::default()
                });
                let i = self.upstreams.len().saturating_sub(1);
                self.upstream_at.insert(name, i);
                i
            }
        };
        let mut warn_h2 = false;
        let (name, backup) = {
            let u = self.upstreams.get_mut(i)?;
            if scheme == "https" && u.protocol.is_empty() {
                u.protocol = "h2".into();
                warn_h2 = true;
            }
            for e in &loc.block {
                match e.name.as_str() {
                    "proxy_connect_timeout" => {
                        if let Some(t) = duration(&e.args) {
                            u.connect_timeout = t;
                        }
                    }
                    "proxy_read_timeout" => {
                        if let Some(t) = duration(&e.args) {
                            u.response_idle_timeout = t;
                        }
                    }
                    _ => {}
                }
            }
            (u.name.clone(), u.backup.clone())
        };
        if warn_h2 {
            self.warn(
                d.line,
                format!("proxy_pass {target}: akit reaches TLS upstreams over HTTP/2"),
            );
        }
        let mut p = params([("upstream", J::Str(name))]);
        if !backup.is_empty() {
            p.insert("failover".into(), J::Str(backup));
        }
        if !s.host_header.is_empty() {
            p.insert("host_header".into(), J::str(&s.host_header));
        }
        let mut mw = Vec::new();
        if !s.proxy_headers.is_empty() {
            let set: Params = s
                .proxy_headers
                .iter()
                .map(|(k, v)| (k.clone(), J::str(v)))
                .collect();
            mw.push(OutRef::new(
                "headers",
                params([("request", J::Map(params([("set", J::Map(set))])))]),
            ));
        }
        if has_uri {
            if matches!(modifier, "~" | "~*" | "=") {
                self.warn(
                    d.line,
                    format!(
                        "proxy_pass {target}: the URI is not converted in a location of {modifier}"
                    ),
                );
            } else {
                // The location's prefix is replaced with the URI.
                let mut rw = Params::new();
                let strip = prefix.strip_suffix('/').unwrap_or(prefix);
                if !strip.is_empty() {
                    rw.insert("strip_prefix".into(), J::strs(&[strip]));
                }
                let add = uri.strip_suffix('/').unwrap_or(&uri);
                if !add.is_empty() {
                    rw.insert("add_prefix".into(), J::str(add));
                }
                if !rw.is_empty() {
                    mw.push(OutRef::new("rewrite", rw));
                }
            }
        }
        Some((OutRef::new("proxy", p), mw))
    }

    /// Converts an upstream block. Servers marked backup go to an upstream of their own, which
    /// proxies to it fail over to.
    #[allow(clippy::too_many_lines)]
    fn upstream(&mut self, d: &Directive) {
        let [uname] = d.args.as_slice() else {
            self.skip(d, "an upstream needs a name");
            return;
        };
        let mut u = OutUpstream {
            name: uname.clone(),
            line: d.line,
            ..OutUpstream::default()
        };
        let mut backup: Option<OutUpstream> = None;
        let mut weighted = false;
        let mut weights = Vec::new();
        let (mut max_fails, mut fail_timeout) = (String::new(), String::new());
        for e in &d.block {
            match e.name.as_str() {
                "server" => {
                    let Some(first) = e.args.first() else {
                        self.skip(e, "no address");
                        continue;
                    };
                    if first.starts_with("unix:") {
                        self.skip(e, "akit reaches upstreams over TCP");
                        continue;
                    }
                    let mut addr = first.clone();
                    if split_host_port(&addr).is_none() {
                        addr = join_host_port(&addr, "80");
                    }
                    let (mut weight, mut is_backup, mut down) = (1i64, false, false);
                    for a in e.args.iter().skip(1) {
                        let (k, v) = a.split_once('=').unwrap_or((a, ""));
                        match k {
                            "weight" => {
                                if let Some(n) = atoi_ok(v).filter(|n| *n > 0) {
                                    weight = n.min(1000);
                                    weighted = weighted || n != 1;
                                }
                            }
                            "max_fails" => max_fails = v.into(),
                            "fail_timeout" => fail_timeout = v.into(),
                            "backup" => is_backup = true,
                            "down" => down = true,
                            "max_conns" => {
                                if let Some(n) = atoi_ok(v).filter(|n| *n > 0) {
                                    u.max_conns = u.max_conns.max(n);
                                }
                            }
                            _ => self.warn(
                                e.line,
                                format!("server {first} {a}: skipped: not converted"),
                            ),
                        }
                    }
                    if down {
                        self.warn(e.line, format!("server {first} down: left out"));
                    } else if is_backup {
                        backup
                            .get_or_insert_with(|| OutUpstream {
                                name: format!("{}_backup", u.name),
                                line: e.line,
                                ..OutUpstream::default()
                            })
                            .addresses
                            .push(addr);
                    } else {
                        u.addresses.push(addr);
                        weights.push(weight);
                    }
                }
                "least_conn" => u.load_balancing = "least_conn".into(),
                "ip_hash" => u.load_balancing = "ip_hash".into(),
                "random" => {
                    u.load_balancing = if e.args.is_empty() { "random" } else { "p2c" }.into();
                }
                "hash" => {
                    let Some(key) = e.args.first() else {
                        self.skip(e, "hash takes a key");
                        continue;
                    };
                    if key == "$request_uri" || key == "$uri" {
                        u.load_balancing = "uri_hash".into();
                    } else if key == "$remote_addr" || key == "$binary_remote_addr" {
                        u.load_balancing = "ip_hash".into();
                    } else if let Some(h) = key.strip_prefix("$http_") {
                        u.load_balancing = "header_hash".into();
                        u.hash_key = h.replace('_', "-");
                    } else if let Some(c) = key.strip_prefix("$cookie_") {
                        u.load_balancing = "cookie_hash".into();
                        u.hash_key = c.into();
                    } else {
                        self.warn(e.line, format!("hash {key}: skipped: akit hashes on the client's address, the URI, a header or a cookie"));
                    }
                }
                "keepalive" => {
                    if let [n] = e.args.as_slice()
                        && let Some(n) = atoi_ok(n)
                    {
                        u.max_idle_conns = Some(n);
                        continue;
                    }
                    self.skip(e, "keepalive takes a number");
                }
                "keepalive_timeout" | "keepalive_requests" | "keepalive_time" => {
                    self.skip(e, "akit's upstream idle_timeout stands in for it");
                }
                _ => self.skip(e, "not converted"),
            }
        }
        if u.addresses.is_empty() {
            let Some(b) = backup.take() else {
                self.warn(
                    d.line,
                    format!("upstream {}: skipped: no server converted", u.name),
                );
                return;
            };
            u.addresses = b.addresses;
        }
        if weighted {
            u.weights = weights;
            if u.load_balancing.is_empty() {
                u.load_balancing = "weighted_round_robin".into();
            }
        }
        if !max_fails.is_empty()
            && let Some(n) = atoi_ok(&max_fails)
        {
            u.max_fails = Some(n);
        }
        if let Some(t) = duration(std::slice::from_ref(&fail_timeout))
            && !fail_timeout.is_empty()
        {
            u.fail_timeout = t;
        }
        if let Some(b) = &backup {
            u.backup.clone_from(&b.name);
        }
        self.upstream_at
            .insert(u.name.clone(), self.upstreams.len());
        self.upstreams.push(u);
        if let Some(b) = backup {
            self.upstream_at
                .insert(b.name.clone(), self.upstreams.len());
            self.upstreams.push(b);
        }
    }

    fn limit_req_zone(&mut self, d: &Directive) {
        let (mut key, mut name) = (String::new(), String::new());
        let mut rate = 0f64;
        for (i, a) in d.args.iter().enumerate() {
            if i == 0 {
                if a == "$binary_remote_addr" || a == "$remote_addr" {
                    key = "ip".into();
                } else if let Some(h) = a.strip_prefix("$http_") {
                    key = format!("header:{}", h.replace('_', "-"));
                } else if a == "$server_name" || a == "$host" {
                    key = "route".into();
                } else {
                    self.warn(d.line, format!("limit_req_zone {a}: skipped: akit counts requests by the client's address, a header, or the route"));
                    return;
                }
            } else if let Some(z) = a.strip_prefix("zone=") {
                name = z.split_once(':').map_or(z, |(n, _)| n).into();
            } else if let Some(r) = a.strip_prefix("rate=") {
                let (r, per) = if let Some(r) = r.strip_suffix("r/s") {
                    (r, 1.0)
                } else if let Some(r) = r.strip_suffix("r/m") {
                    (r, 60.0)
                } else {
                    (r, 1.0)
                };
                let Some(n) = parse_float(r) else {
                    self.skip(d, "a rate akit cannot read");
                    return;
                };
                rate = n / per;
            }
        }
        // A rate that is not a number passes, as in fit, whose JSON then fails to be written.
        if name.is_empty() || rate <= 0.0 {
            self.skip(d, "a zone needs a name and a rate");
            return;
        }
        self.zones.insert(name, Zone { key, rate });
    }

    fn limit_req(&mut self, d: &Directive) -> Option<Params> {
        let mut zone = None;
        let mut p = Params::new();
        let mut nodelay = false;
        for a in &d.args {
            let (k, v) = a.split_once('=').unwrap_or((a, ""));
            match k {
                "zone" => zone = self.zones.get(v).cloned(),
                "burst" => {
                    if let Some(n) = atoi_ok(v) {
                        p.insert("burst".into(), J::Int(n));
                    }
                }
                "nodelay" => nodelay = true,
                _ => {}
            }
        }
        let Some(z) = zone else {
            self.skip(d, "its zone was not converted");
            return None;
        };
        p.insert("rate".into(), J::Float(z.rate));
        p.insert("key".into(), J::Str(z.key));
        if !nodelay {
            self.warn(d.line, "limit_req without nodelay: nginx delays requests past the rate, akit refuses them with 429");
        }
        Some(p)
    }

    /// Converts an error_log, from the context `place`, to fit's log, which is the whole
    /// process's: the first one converted is it.
    fn error_log(&mut self, d: &Directive, place: &str) {
        if d.args.is_empty() || d.args.len() > 2 {
            self.skip(d, "not converted");
            return;
        }
        let Some(output) = log_output(d.args.first().map(String::as_str).unwrap_or_default())
        else {
            self.skip(d, "akit writes its log to stdout, stderr or a file");
            return;
        };
        // nginx's level when none is named.
        let mut level = "error";
        if let Some(l) = d.args.get(1) {
            let Some(lv) = log_level(l) else {
                self.skip(d, "not a level");
                return;
            };
            level = lv;
            if matches!(l.as_str(), "crit" | "alert" | "emerg") {
                self.warn(
                    d.line,
                    format!("error_log: {l} is akit's error, its highest level"),
                );
            }
        }
        if let Some(l) = &self.log {
            if l.output != output || l.level != level {
                let line = self.log_line;
                self.warn(d.line, format!("error_log: skipped: akit has one log, for the whole process, which line {line} sets"));
            }
            return;
        }
        if place == "server" || place == "location" {
            self.warn(
                d.line,
                format!(
                    "error_log: akit has one log, for the whole process, not one for each {place}"
                ),
            );
        }
        self.log = Some(OutLog {
            level: level.into(),
            output,
        });
        self.log_line = d.line;
    }

    /// Converts an access_log, from the context `place`, to fit's access log, which writes a
    /// line for every request: the first one converted is it.
    fn access_log(&mut self, d: &Directive, place: &str) {
        if d.args.len() == 1 && d.args.first().is_some_and(|a| a == "off") {
            if self.access_log.is_some() || place == "location" {
                self.warn(d.line, "access_log off: skipped: akit's access log, when there is one, writes a line for every request");
            }
            return;
        }
        let Some(file) = d.args.first() else {
            self.skip(d, "not converted");
            return;
        };
        let Some(output) = log_output(file) else {
            self.skip(d, "akit writes its access log to stdout, stderr or a file");
            return;
        };
        let mut l = OutAccessLog {
            output,
            ..OutAccessLog::default()
        };
        for (i, a) in d.args.iter().skip(1).enumerate() {
            let (key, value) = a.split_once('=').unwrap_or((a, ""));
            if i == 0 && !a.contains('=') {
                self.warn(d.line, format!("access_log: the format {a} is not converted: akit writes its lines as JSON, text or line, with fields of its own"));
            } else if key == "buffer" {
                match size(value).filter(|n| *n > 0) {
                    Some(n) => l.buffer_bytes = n,
                    None => self.warn(d.line, format!("access_log: {a}: not a size")),
                }
            } else if key == "flush" {
                match duration(&[value.to_owned()]) {
                    Some(t) => l.flush_interval = t,
                    None => self.warn(d.line, format!("access_log: {a}: not a time")),
                }
            } else if key == "if" {
                self.warn(d.line, format!("access_log: {a} is not converted: akit writes a line for every request, or as its level says"));
            } else {
                self.warn(d.line, format!("access_log: {a} is not converted"));
            }
        }
        if let Some(prev) = &self.access_log {
            if *prev != l {
                let line = self.access_log_line;
                self.warn(
                    d.line,
                    format!("access_log: skipped: akit has one access log, which line {line} sets"),
                );
            }
            return;
        }
        if place == "server" || place == "location" {
            self.warn(d.line, format!("access_log: akit has one access log, for every request, not one for each {place}"));
        }
        self.access_log = Some(l);
        self.access_log_line = d.line;
    }
}

fn host_match(hosts: &[String]) -> Option<OutMatch> {
    (!hosts.is_empty()).then(|| OutMatch {
        host: hosts.to_vec(),
        ..OutMatch::default()
    })
}

/// Makes `r`, a route under an auth_basic not converted, answer 401 rather than serve what the
/// password guarded to anyone.
fn guarded(r: &mut OutRoute, s: &Settings) {
    if s.auth_basic > 0 {
        r.handler = OutRef::new("respond", params([("status", J::Int(401))]));
    }
}

/// The middleware of a context's limits, which nginx applies after its rewrites and before the
/// content.
fn limits(s: &Settings) -> Vec<OutRef> {
    let mut mw: Vec<OutRef> = s
        .limit_reqs
        .iter()
        .map(|r| OutRef::new("rate_limit", r.clone()))
        .collect();
    if s.body_limit > 0 {
        mw.push(OutRef::new(
            "body_limit",
            params([("max_bytes", J::Int(s.body_limit))]),
        ));
    }
    mw
}

/// The middleware that changes a context's responses: the fields add_header adds, and
/// compression.
fn response(s: &Settings) -> Vec<OutRef> {
    let mut mw = Vec::new();
    if !s.add_headers.is_empty() {
        let mut add: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for (k, v) in &s.add_headers {
            add.entry(k.clone()).or_default().push(v.clone());
        }
        let add: Params = add.into_iter().map(|(k, v)| (k, J::strs(&v))).collect();
        mw.push(OutRef::new(
            "headers",
            params([("response", J::Map(params([("add", J::Map(add))])))]),
        ));
    }
    if s.gzip {
        let mut p = Params::new();
        if s.gzip_level != 0 {
            p.insert("gzip_level".into(), J::Int(s.gzip_level));
        }
        if s.gzip_min_length != 0 {
            p.insert("min_bytes".into(), J::Int(s.gzip_min_length));
        }
        mw.push(OutRef::new("compress", p));
    }
    mw
}

/// Replaces the nginx variables fit has placeholders for; `None` for any other.
fn vars(s: &str, info: &ServerInfo) -> Option<String> {
    let server_name = info.server_name.clone().unwrap_or_default();
    let out = gostd::replace(
        s,
        &[
            ("$request_uri", "{uri}"),
            ("$scheme", "{scheme}"),
            ("$http_host", "{host}"),
            ("$host", "{host}"),
            ("$uri", "{path}"),
            ("$document_uri", "{path}"),
            ("$query_string", "{query}"),
            ("$args", "{query}"),
            ("$server_name", &server_name),
        ],
    );
    (!has_var(&out)).then_some(out)
}

/// Converts an nginx time, as 60s, 1m or 500ms, to a Go one.
fn duration(args: &[String]) -> Option<String> {
    let [s] = args else { return None };
    if s.is_empty() {
        return None;
    }
    if atoi(s).is_ok() {
        return Some(format!("{s}s"));
    }
    // ^(\d+(ms|s|m|h|d|w))+$
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i = i.saturating_add(1);
        }
        if i == start {
            return None;
        }
        match b.get(i) {
            Some(b'm') if b.get(i.saturating_add(1)) == Some(&b's') => i = i.saturating_add(2),
            Some(b's' | b'm' | b'h' | b'd' | b'w') => i = i.saturating_add(1),
            _ => return None,
        }
    }
    // Go has no days or weeks.
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < b.len() {
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i = i.saturating_add(1);
        }
        let digits = s.get(start..i).unwrap_or_default();
        match b.get(i) {
            Some(&u @ (b'd' | b'w')) => {
                let mut n = match atoi(digits) {
                    Ok(n) | Err(Some(n)) => n,
                    Err(None) => 0,
                };
                if u == b'w' {
                    n = n.wrapping_mul(7);
                }
                out.push_str(&24i64.wrapping_mul(n).to_string());
                out.push('h');
                i = i.saturating_add(1);
            }
            _ => {
                out.push_str(digits);
                // The unit: ms, or one letter.
                let len = if b.get(i) == Some(&b'm') && b.get(i.saturating_add(1)) == Some(&b's') {
                    2
                } else {
                    1
                };
                out.push_str(s.get(i..i.saturating_add(len)).unwrap_or_default());
                i = i.saturating_add(len);
            }
        }
    }
    Some(out)
}

/// Converts an nginx size, as 10m, to bytes.
fn size(s: &str) -> Option<i64> {
    let (num, mult) = match s.as_bytes().last()? {
        b'k' | b'K' => (s.get(..s.len().saturating_sub(1))?, 1i64 << 10),
        b'm' | b'M' => (s.get(..s.len().saturating_sub(1))?, 1 << 20),
        b'g' | b'G' => (s.get(..s.len().saturating_sub(1))?, 1 << 30),
        _ => (s, 1),
    };
    let n = atoi(num).ok()?;
    (n >= 0).then(|| n.wrapping_mul(mult))
}

/// fit's log level for an nginx error_log level; crit, alert and emerg are error, fit's
/// highest.
fn log_level(l: &str) -> Option<&'static str> {
    Some(match l {
        "debug" => "debug",
        "info" | "notice" => "info",
        "warn" => "warn",
        "error" | "crit" | "alert" | "emerg" => "error",
        _ => return None,
    })
}

/// fit's output for an nginx log's file: stdout or stderr for theirs, or the path. Syslog and
/// memory buffers have none.
fn log_output(file: &str) -> Option<String> {
    if file == "stderr" || file == "/dev/stderr" {
        Some("stderr".into())
    } else if file == "/dev/stdout" {
        Some("stdout".into())
    } else if file.starts_with("syslog:") || file.starts_with("memory:") {
        None
    } else {
        Some(file.into())
    }
}

/// Go's `strings.ToLower`: each character's simple lower case.
pub(crate) fn go_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) => x,
                // U+0130, whose full lower case is two characters; its simple one is "i".
                (Some(x), Some(_)) if c == '\u{130}' => x,
                _ => c,
            }
        })
        .collect()
}

/// Go's `strings.ToUpper`: each character's simple upper case.
pub(crate) fn go_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_as_fit() {
        for (input, want) in [
            ("60", "60s"),
            ("2m", "2m"),
            ("500ms", "500ms"),
            ("1d", "24h"),
            ("1h30m", "1h30m"),
            ("1w", "168h"),
        ] {
            assert_eq!(duration(&[input.into()]).as_deref(), Some(want), "{input}");
        }
        assert_eq!(duration(&["soon".into()]), None);
        for (input, want) in [
            ("10m", 10 << 20),
            ("512k", 512 << 10),
            ("1G", 1 << 30),
            ("100", 100),
        ] {
            assert_eq!(size(input), Some(want), "{input}");
        }
        for input in ["", "m", "-1k", "big"] {
            assert_eq!(size(input), None, "{input:?}");
        }
    }
}
