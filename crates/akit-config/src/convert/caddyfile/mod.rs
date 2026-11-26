//! Caddyfiles to fit's native JSON, as fit's `adapter/caddyfile` converts them: the global
//! options fit has a place for, snippets and their imports, sites by their addresses, named
//! matchers, handle, handle_path and route blocks, and in them reverse_proxy, file_server,
//! respond, php_fastcgi, root, try_files, redir, rewrite, uri, header, request_header, encode,
//! basic_auth, forward_auth, request_body, tls and log. Every directive it does not convert is
//! a warning naming its line.

mod directives;
mod matcher;
mod parse;
mod routes;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use matcher::OutMatch;
use parse::{Directive, ServerBlock};

use super::Converted;
use super::gostd::{self, Fields, J, Params, atoi_ok, join_host_port, split_host_port};
use crate::auto_https::{DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, auto_https_name};

#[derive(Debug, Clone, Default)]
struct OutListener {
    name: String,
    address: String,
    protocols: Vec<String>,
    tls: Option<OutTls>,
    line: i64,
}

#[derive(Debug, Clone, Default)]
struct OutTls {
    certificates: Vec<OutCert>,
    acme: Vec<String>,
    on_demand: bool,
    client_auth: Option<OutClientAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutCert {
    cert_file: String,
    key_file: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutClientAuth {
    ca_files: Vec<String>,
    mode: String,
}

#[derive(Debug, Clone, Default)]
struct OutUpstream {
    name: String,
    addresses: Vec<String>,
    weights: Vec<i64>,
    load_balancing: String,
    hash_key: String,
    /// A sticky cookie's name; `Some("")` for one with no name.
    sticky_cookie: Option<String>,
    max_conns: i64,
    max_idle_conns: Option<i64>,
    idle_timeout: String,
    connect_timeout: String,
    protocol: String,
    tls_server_name: String,
    tls_ca_file: String,
    tls_cert_file: String,
    tls_key_file: String,
    max_fails: Option<i64>,
    fail_timeout: String,
    response_header_timeout: String,
    response_idle_timeout: String,
    health_check: Option<OutHealth>,
    line: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutHealth {
    path: String,
    host: String,
    port: i64,
    expect_status: Vec<i64>,
    interval: String,
    timeout: String,
    fails: i64,
    passes: i64,
}

#[derive(Debug, Clone, Default)]
struct OutRoute {
    name: String,
    listeners: Vec<String>,
    matcher: Option<OutMatch>,
    middleware: Vec<OutRef>,
    handler: OutRef,
    line: i64,
    mw_lines: Vec<i64>,
    h_line: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OutRef {
    pub(crate) module: String,
    pub(crate) params: Params,
}

impl OutRef {
    pub(crate) fn new(module: &str, params: Params) -> OutRef {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutLog {
    level: String,
    output: String,
    format: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutAccessLog {
    output: String,
    level: String,
    format: String,
}

#[derive(Debug, Clone, Default)]
struct OutAutoHttps {
    disable: bool,
    disable_redirects: bool,
    http_port: i64,
    https_port: i64,
}

#[derive(Debug, Clone, Default)]
struct OutAcme {
    directory: String,
    email: String,
    ca_file: String,
    storage: String,
    on_demand: Option<OutOnDemand>,
}

#[derive(Debug, Clone, Default)]
struct OutOnDemand {
    ask: String,
    rate: i64,
    interval: String,
}

/// `{key: value, ...}`.
pub(crate) fn params<const N: usize>(kv: [(&str, J); N]) -> Params {
    kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
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
                self.tls.as_ref().map(|t| {
                    Fields::new()
                        .opt(
                            "certificates",
                            (!t.certificates.is_empty()).then(|| {
                                J::Arr(
                                    t.certificates
                                        .iter()
                                        .map(|c| {
                                            Fields::new()
                                                .put("cert_file", J::str(&c.cert_file))
                                                .put("key_file", J::str(&c.key_file))
                                                .done()
                                        })
                                        .collect(),
                                )
                            }),
                        )
                        .strs("acme", &t.acme)
                        .bool("on_demand", t.on_demand)
                        .opt(
                            "client_auth",
                            t.client_auth.as_ref().map(|c| {
                                Fields::new()
                                    .put("ca_files", list(&c.ca_files))
                                    .str("mode", &c.mode)
                                    .done()
                            }),
                        )
                        .done()
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
            .opt(
                "sticky_cookie",
                self.sticky_cookie
                    .as_ref()
                    .map(|n| Fields::new().str("name", n).done()),
            )
            .int("max_conns", self.max_conns)
            .opt("max_idle_conns", self.max_idle_conns.map(J::Int))
            .str("idle_timeout", &self.idle_timeout)
            .str("connect_timeout", &self.connect_timeout)
            .str("protocol", &self.protocol)
            .str("tls_server_name", &self.tls_server_name)
            .str("tls_ca_file", &self.tls_ca_file)
            .str("tls_cert_file", &self.tls_cert_file)
            .str("tls_key_file", &self.tls_key_file)
            .opt("max_fails", self.max_fails.map(J::Int))
            .str("fail_timeout", &self.fail_timeout)
            .str("response_header_timeout", &self.response_header_timeout)
            .str("response_idle_timeout", &self.response_idle_timeout)
            .opt(
                "health_check",
                self.health_check.as_ref().map(|h| {
                    Fields::new()
                        .str("path", &h.path)
                        .str("host", &h.host)
                        .int("port", h.port)
                        .opt(
                            "expect_status",
                            (!h.expect_status.is_empty()).then(|| {
                                J::Arr(h.expect_status.iter().map(|s| J::Int(*s)).collect())
                            }),
                        )
                        .str("interval", &h.interval)
                        .str("timeout", &h.timeout)
                        .int("fails", h.fails)
                        .int("passes", h.passes)
                        .done()
                }),
            )
            .done()
    }
}

impl OutRoute {
    fn json(&self) -> J {
        Fields::new()
            .str("name", &self.name)
            .strs("listeners", &self.listeners)
            .opt("match", self.matcher.as_ref().map(OutMatch::json))
            .opt(
                "middleware",
                (!self.middleware.is_empty())
                    .then(|| J::Arr(self.middleware.iter().map(OutRef::json).collect())),
            )
            .put("handler", self.handler.json())
            .done()
    }
}

/// What the conversion has made so far.
#[derive(Default)]
struct Converter {
    listeners: Vec<OutListener>,
    routes: Vec<OutRoute>,
    upstreams: Vec<OutUpstream>,
    log: Option<OutLog>,
    access_log: Option<OutAccessLog>,
    admin: Option<String>,
    auto_https: Option<OutAutoHttps>,
    metrics: bool,
    acme: Option<OutAcme>,
    warnings: Vec<String>,
    names: HashSet<String>,
    /// The upstreams made, by what they are written as with no name.
    upstream_by_sig: HashMap<String, usize>,
    listener_at: HashMap<String, usize>,
    http_port: i64,
    https_port: i64,
    auto_off: bool,
    no_redir: bool,
    no_certs: bool,
    protocols: Vec<String>,
    admin_off: bool,
    log_line: i64,
    /// The line of the global log fit's log is made from.
    global_log_line: i64,
    scopes: i64,
}

/// Converts a Caddyfile to fit's native JSON.
pub(crate) fn convert(
    src: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Converted, String> {
    let (blocks, warnings) = parse::parse(src, env)?;
    let mut c = Converter {
        warnings,
        http_port: i64::from(DEFAULT_HTTP_PORT),
        https_port: i64::from(DEFAULT_HTTPS_PORT),
        ..Converter::default()
    };
    let mut sites = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        if b.keys.is_empty() {
            if i == 0 {
                c.global(&b.dirs);
            }
            continue;
        }
        if let Some(s) = c.site(b) {
            sites.push(s);
        }
    }
    if sites.is_empty() {
        return Err("the Caddyfile has no site to convert".into());
    }
    c.serve(&sites);
    if c.routes.is_empty() {
        return Err("none of the Caddyfile's sites converted".into());
    }
    if !c.admin_off && c.admin.is_none() {
        // Caddy serves its admin endpoint unless told not to; fit's Admin API takes its place.
        c.admin = Some(String::new());
    }
    c.result()
}

impl Converter {
    fn result(self) -> Result<Converted, String> {
        // Warnings once each, in the order of the Caddyfile's lines.
        let mut ws: Vec<String> = Vec::new();
        for w in self.warnings {
            if !ws.contains(&w) {
                ws.push(w);
            }
        }
        gostd::sort_warnings(&mut ws);
        let mut top = Fields::new()
            .opt(
                "listeners",
                (!self.listeners.is_empty())
                    .then(|| J::Arr(self.listeners.iter().map(OutListener::json).collect())),
            )
            .put(
                "routes",
                J::Arr(self.routes.iter().map(OutRoute::json).collect()),
            )
            .opt(
                "upstreams",
                (!self.upstreams.is_empty())
                    .then(|| J::Arr(self.upstreams.iter().map(OutUpstream::json).collect())),
            );
        if let Some(l) = &self.log {
            top = top.put(
                "log",
                Fields::new()
                    .str("level", &l.level)
                    .str("output", &l.output)
                    .str("format", &l.format)
                    .done(),
            );
        }
        if let Some(l) = &self.access_log {
            top = top.put(
                "access_log",
                Fields::new()
                    .str("output", &l.output)
                    .str("level", &l.level)
                    .str("format", &l.format)
                    .done(),
            );
        }
        if let Some(a) = &self.admin {
            top = top.put("admin", Fields::new().str("address", a).done());
        }
        if let Some(a) = &self.auto_https {
            top = top.put(
                "auto_https",
                Fields::new()
                    .bool("disable", a.disable)
                    .bool("disable_redirects", a.disable_redirects)
                    .int("http_port", a.http_port)
                    .int("https_port", a.https_port)
                    .done(),
            );
        }
        if self.metrics {
            top = top.put("metrics", Fields::new().done());
        }
        if let Some(a) = &self.acme {
            top = top.put(
                "acme",
                Fields::new()
                    .str("directory", &a.directory)
                    .str("email", &a.email)
                    .str("ca_file", &a.ca_file)
                    .str("storage", &a.storage)
                    .opt(
                        "on_demand",
                        a.on_demand.as_ref().map(|o| {
                            Fields::new()
                                .put("ask", J::str(&o.ask))
                                .int("rate", o.rate)
                                .str("interval", &o.interval)
                                .done()
                        }),
                    )
                    .done(),
            );
        }
        let json = top.done().marshal()?;
        let mut pos = HashMap::new();
        for (i, l) in self.listeners.iter().enumerate() {
            pos.insert(format!("listeners[{i}]"), l.line);
        }
        for (i, u) in self.upstreams.iter().enumerate() {
            pos.insert(format!("upstreams[{i}]"), u.line);
        }
        for (i, r) in self.routes.iter().enumerate() {
            pos.insert(format!("routes[{i}]"), r.line);
            pos.insert(format!("routes[{i}].handler"), r.h_line);
            for (j, l) in r.mw_lines.iter().enumerate() {
                pos.insert(format!("routes[{i}].middleware[{j}]"), *l);
            }
        }
        if self.access_log.is_some() {
            pos.insert("access_log".into(), self.log_line);
        }
        if self.log.is_some() {
            pos.insert("log".into(), self.global_log_line);
        }
        Ok(Converted {
            json,
            warnings: ws,
            positions: pos,
        })
    }

    fn warn(&mut self, line: i64, msg: impl AsRef<str>) {
        self.warnings.push(format!("line {line}: {}", msg.as_ref()));
    }

    /// Reports that `d` is not converted, and why.
    fn skip(&mut self, d: &Directive, why: &str) {
        self.warn(d.line, format!("{}: skipped: {why}", d.name));
    }

    fn acme(&mut self) -> &mut OutAcme {
        self.acme.get_or_insert_with(OutAcme::default)
    }

    fn auto_https(&mut self) -> &mut OutAutoHttps {
        self.auto_https.get_or_insert_with(OutAutoHttps::default)
    }

    /// Converts the global options block.
    #[allow(clippy::too_many_lines)]
    fn global(&mut self, dirs: &[Directive]) {
        for d in dirs {
            let one = d.args.len() == 1;
            let arg0 = d.args.first().cloned().unwrap_or_default();
            match d.name.as_str() {
                "email" if one => {
                    self.acme().email = arg0;
                    continue;
                }
                "acme_ca" if one => {
                    self.acme().directory = arg0;
                    continue;
                }
                "acme_ca_root" if one => {
                    self.acme().ca_file = arg0;
                    continue;
                }
                "storage" => {
                    if arg0 == "file_system" {
                        let mut path = if d.args.len() == 2 {
                            d.args.get(1).cloned().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        for e in &d.block {
                            if e.name == "root" && e.args.len() == 1 {
                                path = e.args.first().cloned().unwrap_or_default();
                            }
                        }
                        if !path.is_empty() {
                            self.acme().storage = path;
                            continue;
                        }
                    }
                    self.skip(d, "akit keeps certificates in a directory, or in Redis, etcd or Consul through acme.shared_storage, written by hand");
                    continue;
                }
                "admin" => {
                    if one && arg0 == "off" {
                        self.admin_off = true;
                        continue;
                    } else if one {
                        self.admin = Some(arg0.strip_prefix("tcp/").unwrap_or(&arg0).to_owned());
                        if !d.block.is_empty() {
                            self.warn(d.line, "admin: its block is not converted");
                        }
                        continue;
                    } else if d.args.is_empty() {
                        self.admin = Some(String::new());
                        continue;
                    }
                }
                "auto_https" if one => match arg0.as_str() {
                    "off" => {
                        self.auto_off = true;
                        self.auto_https().disable = true;
                        continue;
                    }
                    "disable_redirects" => {
                        self.no_redir = true;
                        self.auto_https().disable_redirects = true;
                        continue;
                    }
                    "disable_certs" => {
                        self.no_certs = true;
                        continue;
                    }
                    "ignore_loaded_certs" | "prefer_wildcard" => continue,
                    _ => {}
                },
                "http_port" | "https_port" if one => {
                    if let Some(n) = atoi_ok(&arg0).filter(|n| *n > 0 && *n < 65536) {
                        if d.name == "http_port" {
                            self.http_port = n;
                            self.auto_https().http_port = n;
                        } else {
                            self.https_port = n;
                            self.auto_https().https_port = n;
                        }
                        continue;
                    }
                }
                "on_demand_tls" => {
                    self.on_demand_tls(d);
                    continue;
                }
                "servers" => {
                    self.servers(d);
                    continue;
                }
                "debug" if d.args.is_empty() => {
                    self.global_log(d.line).level = "debug".into();
                    continue;
                }
                "log" => {
                    self.global_log_directive(d);
                    continue;
                }
                "grace_period" | "shutdown_delay" | "order" | "persist_config"
                | "skip_install_trust" | "local_certs" | "key_type" | "renew_interval"
                | "ocsp_interval" | "ocsp_stapling" | "preferred_chains" | "cert_issuer"
                | "pki" | "events" | "filesystem" | "default_sni" | "fallback_sni"
                | "default_bind" | "cert_lifetime" | "metrics" => {
                    self.skip(d, "not converted: akit's defaults stand in for it");
                    continue;
                }
                _ => {}
            }
            self.skip(d, "not converted");
        }
    }

    fn on_demand_tls(&mut self, d: &Directive) {
        let mut od = OutOnDemand::default();
        for e in &d.block {
            let one = e.args.len() == 1;
            let arg0 = e.args.first().cloned().unwrap_or_default();
            if e.name == "ask" && one {
                od.ask = arg0;
            } else if e.name == "permission" && one && arg0 == "http" {
                for f in &e.block {
                    if f.name == "endpoint" && f.args.len() == 1 {
                        od.ask = f.args.first().cloned().unwrap_or_default();
                    }
                }
            } else if e.name == "interval" && one {
                if let Some(t) = directives::duration(&arg0) {
                    od.interval = t;
                }
            } else if e.name == "burst" && one {
                if let Some(n) = atoi_ok(&arg0).filter(|n| *n > 0) {
                    od.rate = n;
                }
            } else {
                self.skip(e, "not converted");
            }
        }
        if od.ask.is_empty() {
            self.skip(d, "on-demand TLS needs an ask URL");
            return;
        }
        self.acme().on_demand = Some(od);
    }

    fn servers(&mut self, d: &Directive) {
        for e in &d.block {
            if e.name == "protocols" {
                self.protocols.clear();
                for p in &e.args {
                    match p.as_str() {
                        "h1" => self.protocols.push("http/1.1".into()),
                        "h2" | "h2c" | "h3" => self.protocols.push(p.clone()),
                        _ => self.warn(e.line, format!("protocols {p}: skipped: not converted")),
                    }
                }
            } else {
                self.skip(
                    e,
                    "not converted: akit's limits and timeouts are set at the top of its config",
                );
            }
        }
        if !d.args.is_empty() {
            self.warn(
                d.line,
                format!(
                    "servers {}: its options apply to every listener",
                    d.args.join(" ")
                ),
            );
        }
    }

    fn site(&mut self, b: &ServerBlock) -> Option<Site> {
        let mut s = Site {
            block: b.clone(),
            ..Site::default()
        };
        for d in &b.dirs {
            match d.name.as_str() {
                "tls" => self.tls_directive(&mut s, d),
                "bind" => s.bind.extend(d.args.iter().cloned()),
                "log" => self.site_log(d),
                _ => {}
            }
        }
        self.collect_matchers(&mut s, &b.dirs);
        for k in &b.keys {
            if let Some(a) = self.address(b.line, k, &s) {
                s.addrs.push(a);
            }
        }
        let Some(first) = s.addrs.first() else {
            self.warn(
                b.line,
                format!(
                    "site {}: skipped: none of its addresses converted",
                    b.keys.join(", ")
                ),
            );
            return None;
        };
        s.name = if first.host.is_empty() {
            format!("port{}", first.port)
        } else {
            first.host.clone()
        };
        Some(s)
    }

    /// Converts the named matchers a site defines, in its block and the blocks in it.
    fn collect_matchers(&mut self, s: &mut Site, dirs: &[Directive]) {
        for d in dirs {
            if d.name.starts_with('@') {
                match matcher::named_matcher(d) {
                    Err(why) => {
                        self.warn(
                            d.line,
                            format!("{}: {why}; what uses it is skipped", d.name),
                        );
                        s.bad_match.insert(d.name.clone(), why);
                    }
                    Ok(m) => {
                        s.matchers.insert(d.name.clone(), m);
                    }
                }
                continue;
            }
            if matches!(d.name.as_str(), "handle" | "handle_path" | "route") {
                self.collect_matchers(s, &d.block);
            }
        }
    }

    /// Reads a site address: `[scheme://][host][:port][/path]`.
    fn address(&mut self, line: i64, raw: &str, s: &Site) -> Option<SiteAddr> {
        let mut a = SiteAddr {
            raw: raw.into(),
            ..SiteAddr::default()
        };
        let mut rest = raw;
        if let Some((scheme, r)) = rest.split_once("://") {
            if scheme != "http" && scheme != "https" {
                self.warn(
                    line,
                    format!("site address {raw}: skipped: akit serves http:// and https://"),
                );
                return None;
            }
            a.scheme = scheme.into();
            rest = r;
        }
        if rest.starts_with("unix/") || rest.contains('{') {
            self.warn(line, format!("site address {raw}: skipped: not converted"));
            return None;
        }
        if let Some(i) = rest.find('/') {
            a.path = rest.get(i..).unwrap_or_default().into();
            rest = rest.get(..i).unwrap_or_default();
            if !matcher::simple_path(&a.path) {
                self.warn(
                    line,
                    format!(
                        "site address {raw}: skipped: akit takes a path that is exact or ends in *"
                    ),
                );
                return None;
            }
        }
        if let Some((h, p)) = split_host_port(rest) {
            a.host = h.into();
            a.port = p.into();
        } else {
            a.host = gostd::trim(rest, "[]").into();
        }
        a.host = crate::convert::nginx::go_lower(&a.host);
        if !a.port.is_empty() && !atoi_ok(&a.port).is_some_and(|n| (0..=65535).contains(&n)) {
            self.warn(line, format!("site address {raw}: skipped: not a port"));
            return None;
        }
        if a.host == "*" {
            a.host.clear();
        }
        if a.host.matches('*').count() > 1 || a.host.contains('*') && !a.host.starts_with("*.") {
            self.warn(
                line,
                format!("site address {raw}: skipped: akit takes names and a leading \"*.\""),
            );
            return None;
        }
        let has_certs = !s.tls.certs.is_empty() || s.tls.on_demand;
        let public = !a.host.is_empty()
            && (auto_https_name(&a.host) || a.host.starts_with("*.") && has_certs);
        a.tls = if a.scheme == "http" {
            false
        } else if a.scheme == "https" {
            true
        } else if a.port == self.http_port.to_string() {
            false
        } else if public || has_certs {
            true
        } else {
            if !a.host.is_empty() {
                self.warn(line, format!("site address {raw}: served over HTTP: Caddy serves it over HTTPS with a certificate of its internal CA, which akit has not"));
            }
            false
        };
        if a.port.is_empty() {
            a.port = if a.tls {
                self.https_port
            } else {
                self.http_port
            }
            .to_string();
        }
        Some(a)
    }

    fn tls_directive(&mut self, s: &mut Site, d: &Directive) {
        s.tls.line = d.line;
        match d.args.as_slice() {
            [a] => {
                if a == "internal" {
                    s.tls.internal = true;
                    self.warn(d.line, "tls internal: akit has no internal CA: write certificate files, or let ACME obtain certificates for public names");
                } else if a.contains('@') {
                    self.email(d.line, a);
                } else {
                    self.skip(
                        d,
                        "tls takes a certificate file and a key file, an email address or internal",
                    );
                }
            }
            [c, k] => s.tls.certs.push(OutCert {
                cert_file: c.clone(),
                key_file: k.clone(),
            }),
            _ => {}
        }
        for e in &d.block {
            match e.name.as_str() {
                "on_demand" => s.tls.on_demand = true,
                "client_auth" => {
                    let mut ca = OutClientAuth::default();
                    for f in &e.block {
                        let one = f.args.len() == 1;
                        let arg0 = f.args.first().cloned().unwrap_or_default();
                        if f.name == "mode" && one {
                            ca.mode = match arg0.as_str() {
                                "require_and_verify" => "require".into(),
                                "verify_if_given" => "verify_if_given".into(),
                                _ => {
                                    self.warn(f.line, format!("client_auth mode {arg0}: akit requires and verifies certificates, or verifies them if given"));
                                    "verify_if_given".into()
                                }
                            };
                        } else if f.name == "trusted_ca_cert_file" && one {
                            ca.ca_files.push(arg0);
                        } else if f.name == "trust_pool" && f.args.len() >= 2 && arg0 == "file" {
                            ca.ca_files.extend(f.args.iter().skip(1).cloned());
                        } else if f.name == "trust_pool" && one && arg0 == "file" {
                            for g in &f.block {
                                if g.name == "pem_file" {
                                    ca.ca_files.extend(g.args.iter().cloned());
                                }
                            }
                        } else {
                            self.skip(f, "not converted");
                        }
                    }
                    if ca.ca_files.is_empty() {
                        self.skip(e, "client_auth needs CA files");
                        continue;
                    }
                    if ca.mode.is_empty() {
                        // Caddy requires and verifies certificates once CAs are set.
                        ca.mode = "require".into();
                    }
                    s.tls.client_auth = Some(ca);
                }
                "protocols" | "ciphers" | "curves" | "alpn" | "key_type" => {
                    self.skip(e, "akit picks TLS versions, ciphers and curves itself");
                }
                "issuer" => {
                    if e.args.first().is_some_and(|a| a == "internal") {
                        s.tls.internal = true;
                        self.warn(e.line, "issuer internal: akit has no internal CA");
                        continue;
                    }
                    for f in &e.block {
                        let arg0 = f.args.first().cloned().unwrap_or_default();
                        if f.name == "email" && f.args.len() == 1 {
                            self.email(f.line, &arg0);
                        } else if f.name == "dir" && f.args.len() == 1 {
                            self.acme().directory = arg0;
                        } else {
                            self.skip(f, "not converted");
                        }
                    }
                }
                "ca" => {
                    if let [dir] = e.args.as_slice() {
                        self.acme().directory.clone_from(dir);
                        continue;
                    }
                    self.skip(e, "not converted");
                }
                "dns" => self.skip(
                    e,
                    "akit answers ACME's HTTP-01 and TLS-ALPN-01 challenges, not DNS-01",
                ),
                _ => self.skip(e, "not converted"),
            }
        }
    }

    fn email(&mut self, line: i64, e: &str) {
        let have = self.acme().email.clone();
        if !have.is_empty() && have != e {
            self.warn(line, format!("tls {e}: akit has one ACME email, {have}"));
            return;
        }
        self.acme().email = e.into();
    }

    /// Converts a site's log to the access log, which fit has one of.
    fn site_log(&mut self, d: &Directive) {
        let (mut output, mut level, mut format) =
            ("stdout".to_owned(), String::new(), String::new());
        for e in &d.block {
            match e.name.as_str() {
                "output" => match e.args.as_slice() {
                    [o] if o == "stdout" || o == "stderr" => output.clone_from(o),
                    [f, path] if f == "file" => {
                        output.clone_from(path);
                        if !e.block.is_empty() {
                            self.warn(e.line, "output file: rolling is not converted: akit appends to the file, for logrotate to rotate");
                        }
                    }
                    [o] if o == "discard" => return,
                    _ => self.skip(e, "akit writes to stdout, stderr or a file"),
                },
                "format" => match e.args.first().map(String::as_str) {
                    Some("json") => format.clear(),
                    Some("console") => format = "line".into(),
                    _ => self.skip(e, "akit writes access log lines as JSON, text or line"),
                },
                "level" => {
                    if let Some(lv) = self.log_level(e) {
                        level = access_level(lv).into();
                    }
                }
                _ => self.skip(e, "not converted"),
            }
        }
        if let Some(l) = &self.access_log
            && (l.output != output || l.level != level || l.format != format)
        {
            let out = l.output.clone();
            self.warn(
                d.line,
                format!("log: akit has one access log, written to {out}"),
            );
            return;
        }
        self.access_log = Some(OutAccessLog {
            output,
            level,
            format,
        });
        self.log_line = d.line;
    }

    /// A level directive's Caddy level as fit's: DEBUG, INFO, WARN and ERROR, and PANIC and
    /// FATAL as error, fit's highest.
    fn log_level(&mut self, e: &Directive) -> Option<&'static str> {
        let [l] = e.args.as_slice() else {
            self.skip(e, "not converted");
            return None;
        };
        match crate::convert::nginx::go_upper(l).as_str() {
            "DEBUG" => Some("debug"),
            "INFO" => Some("info"),
            "WARN" | "WARNING" => Some("warn"),
            "ERROR" => Some("error"),
            "PANIC" | "FATAL" | "DPANIC" => {
                self.warn(e.line, format!("level {l} is akit's error, its highest"));
                Some("error")
            }
            _ => {
                self.skip(e, "not a level");
                None
            }
        }
    }

    /// fit's log, made from the global options, which `line` sets first.
    fn global_log(&mut self, line: i64) -> &mut OutLog {
        if self.log.is_none() {
            self.global_log_line = line;
        }
        // Caddy writes its log to stderr unless told otherwise.
        self.log.get_or_insert_with(|| OutLog {
            output: "stderr".into(),
            ..OutLog::default()
        })
    }

    /// Converts the global log option: the default logger, or one named, is fit's log, unless
    /// it includes only the access logs (http.log.access), when it is fit's access log.
    fn global_log_directive(&mut self, d: &Directive) {
        if d.args.len() > 1 {
            self.skip(d, "not converted");
            return;
        }
        let mut access = false;
        for e in &d.block {
            if e.name == "include" {
                access =
                    !e.args.is_empty() && e.args.iter().all(|a| a.starts_with("http.log.access"));
            }
        }
        if access {
            let mut site = d.clone();
            site.block.retain(|e| e.name != "include");
            self.site_log(&site);
            return;
        }
        if d.args.len() == 1 && d.args.first().is_some_and(|a| a != "default") {
            self.skip(d, "akit has one log: the default logger's is converted");
            return;
        }
        let mut out = self.log.clone().unwrap_or_else(|| OutLog {
            output: "stderr".into(),
            ..OutLog::default()
        });
        for e in &d.block {
            match e.name.as_str() {
                "output" => match e.args.as_slice() {
                    [o] if o == "stdout" || o == "stderr" => out.output.clone_from(o),
                    [f, path] if f == "file" => {
                        out.output.clone_from(path);
                        if !e.block.is_empty() {
                            self.warn(e.line, "output file: rolling is not converted: akit appends to the file, for logrotate to rotate");
                        }
                    }
                    _ => self.skip(e, "akit writes its log to stdout, stderr or a file"),
                },
                "format" => match e.args.first().map(String::as_str) {
                    Some("json") => out.format = "json".into(),
                    Some("console") => out.format.clear(),
                    _ => self.skip(e, "akit writes its log as line, text or JSON"),
                },
                "level" => {
                    if let Some(lv) = self.log_level(e) {
                        out.level = lv.into();
                    }
                }
                "exclude" => self.skip(
                    e,
                    "akit's log.levels sets the level of each of its loggers, written by hand",
                ),
                "include" => self.skip(e, "akit's log takes every logger's messages"),
                _ => self.skip(e, "not converted"),
            }
        }
        if out.level == "info" {
            out.level.clear();
        }
        *self.global_log(d.line) = out;
    }

    /// Makes the listeners the sites need, unless HTTPS by default makes them, and the sites'
    /// routes.
    fn serve(&mut self, sites: &[Site]) {
        let https_port = self.https_port.to_string();
        let http_port = self.http_port.to_string();
        let mut auto = !self.auto_off && self.protocols.is_empty();
        for s in sites {
            if !s.tls.certs.is_empty()
                || s.tls.on_demand
                || s.tls.client_auth.is_some()
                || !s.bind.is_empty()
            {
                auto = false;
            }
            if s.addrs.iter().any(|a| !a.tls || a.port != https_port) {
                auto = false;
            }
        }
        if auto {
            // fit's HTTPS by default serves the routes on :443, with certificates for their
            // names, and redirects :80 to it, as Caddy does.
            for s in sites {
                let g = Group {
                    listeners: Vec::new(),
                    hosts: s.addrs.iter().map(|a| a.host.clone()).collect(),
                    paths: s.addrs.iter().map(|a| a.path.clone()).collect(),
                };
                self.site_routes(s, &g);
            }
            return;
        }
        let mut redirect: HashSet<String> = HashSet::new();
        let mut plain: HashSet<String> = HashSet::new();
        for s in sites {
            let mut groups: Vec<Group> = Vec::new();
            let mut by_listener: HashMap<String, Vec<String>> = HashMap::new();
            let mut order: Vec<String> = Vec::new();
            let mut paths: HashMap<String, Vec<String>> = HashMap::new();
            for a in &s.addrs {
                let binds = if s.bind.is_empty() {
                    vec![String::new()]
                } else {
                    s.bind.clone()
                };
                for b in &binds {
                    let Some(name) = self.listener(s, a, b) else {
                        continue;
                    };
                    if !by_listener.contains_key(&name) {
                        order.push(name.clone());
                    }
                    by_listener
                        .entry(name.clone())
                        .or_default()
                        .push(a.host.clone());
                    paths.entry(name).or_default().push(a.path.clone());
                }
                if a.tls && a.port == https_port && !a.host.is_empty() && !s.tls.internal {
                    redirect.insert(a.host.clone());
                }
                if !a.tls && a.port == http_port {
                    plain.insert(a.host.clone());
                }
            }
            for name in order {
                let hosts = by_listener.get(&name).cloned().unwrap_or_default();
                let p = paths.get(&name).cloned().unwrap_or_default();
                let mut found = false;
                for g in &mut groups {
                    if matcher::same_set(&g.hosts, &hosts) && g.paths == p {
                        g.listeners.push(name.clone());
                        found = true;
                    }
                }
                if !found {
                    groups.push(Group {
                        listeners: vec![name],
                        hosts,
                        paths: p,
                    });
                }
            }
            for g in &groups {
                self.site_routes(s, g);
            }
        }
        self.tls_names(sites);
        if !self.auto_off && !self.no_redir {
            self.redirects(&redirect, &plain);
        }
    }

    /// The name of the listener a site's address is served on, made when it is the first
    /// there.
    fn listener(&mut self, s: &Site, a: &SiteAddr, bind: &str) -> Option<String> {
        let address = if bind.is_empty() {
            format!(":{}", a.port)
        } else {
            join_host_port(
                gostd::trim(bind.strip_prefix("tcp/").unwrap_or(bind), "[]"),
                &a.port,
            )
        };
        let i = if let Some(&i) = self.listener_at.get(&address) {
            let has_tls = self.listeners.get(i).is_some_and(|l| l.tls.is_some());
            if has_tls != a.tls {
                let with = if a.tls { "out" } else { "" };
                self.warn(
                    s.block.line,
                    format!(
                        "site address {}: skipped: another site serves {address} with{with} TLS",
                        a.raw
                    ),
                );
                return None;
            }
            i
        } else {
            let scheme = if a.tls { "https" } else { "http" };
            let name = format!(
                "{scheme}_{}",
                gostd::addr_name(address.strip_prefix(':').unwrap_or(&address), false)
            );
            let mut l = OutListener {
                name,
                address: address.clone(),
                line: s.block.line,
                tls: a.tls.then(OutTls::default),
                ..OutListener::default()
            };
            for p in &self.protocols {
                if (p == "h2" || p == "h3") && !a.tls || p == "h2c" && a.tls {
                    continue;
                }
                l.protocols.push(p.clone());
            }
            if l.protocols.is_empty() && !self.protocols.is_empty() {
                l.protocols = vec!["http/1.1".into()];
            }
            self.listeners.push(l);
            let i = self.listeners.len() - 1;
            self.listener_at.insert(address.clone(), i);
            i
        };
        let has_on_demand = self.acme.as_ref().is_some_and(|a| a.on_demand.is_some());
        let mut warns = Vec::new();
        let l = self.listeners.get_mut(i)?;
        if a.tls
            && let Some(t) = &mut l.tls
        {
            for cert in &s.tls.certs {
                if !t.certificates.contains(cert) {
                    t.certificates.push(cert.clone());
                }
            }
            if s.tls.on_demand {
                if has_on_demand {
                    t.on_demand = true;
                } else {
                    warns.push((
                        s.tls.line,
                        "tls on_demand: skipped: it needs the global on_demand_tls with an ask URL"
                            .to_owned(),
                    ));
                }
            }
            if let Some(ca) = &s.tls.client_auth {
                if t.client_auth
                    .as_ref()
                    .is_some_and(|have| have.ca_files != ca.ca_files)
                {
                    warns.push((
                        s.tls.line,
                        format!(
                            "tls client_auth: {address} already verifies clients with other CAs"
                        ),
                    ));
                } else {
                    t.client_auth = Some(ca.clone());
                }
            }
        }
        let name = l.name.clone();
        for (line, w) in warns {
            self.warn(line, w);
        }
        Some(name)
    }

    /// Lists, on a TLS listener that has certificate files or on-demand TLS, the names of the
    /// sites without files of their own, whose certificates ACME obtains. A listener with
    /// neither gets them from HTTPS by default.
    fn tls_names(&mut self, sites: &[Site]) {
        if self.auto_off || self.no_certs {
            return;
        }
        for l in &mut self.listeners {
            let Some(t) = &mut l.tls else { continue };
            if t.certificates.is_empty() && !t.on_demand {
                continue;
            }
            for s in sites {
                if !s.tls.certs.is_empty() || s.tls.internal {
                    continue;
                }
                for a in &s.addrs {
                    let port = format!(":{}", a.port);
                    if a.tls
                        && (l.address == port || l.address.ends_with(&port) && !s.bind.is_empty())
                        && auto_https_name(&a.host)
                        && !t.acme.contains(&a.host)
                    {
                        t.acme.push(a.host.clone());
                    }
                }
            }
        }
    }

    /// Redirects HTTP to HTTPS for the names served over HTTPS on its port, but those a site
    /// serves over HTTP itself, as Caddy does.
    fn redirects(&mut self, names: &HashSet<String>, plain: &HashSet<String>) {
        let mut hosts: Vec<String> = names
            .iter()
            .filter(|h| !plain.contains(*h))
            .cloned()
            .collect();
        if hosts.is_empty() {
            return;
        }
        hosts.sort();
        let address = format!(":{}", self.http_port);
        let lname = if let Some(&i) = self.listener_at.get(&address) {
            let Some(l) = self.listeners.get(i) else {
                return;
            };
            if l.tls.is_some() {
                self.warn(
                    0,
                    format!("redirects to HTTPS: skipped: {address} serves TLS"),
                );
                return;
            }
            l.name.clone()
        } else {
            let name = format!("http_{}", self.http_port);
            self.listeners.push(OutListener {
                name: name.clone(),
                address: address.clone(),
                ..OutListener::default()
            });
            self.listener_at.insert(address, self.listeners.len() - 1);
            name
        };
        let to = if self.https_port == i64::from(DEFAULT_HTTPS_PORT) {
            "https://{host}{uri}".to_owned()
        } else {
            format!("https://{{host}}:{}{{uri}}", self.https_port)
        };
        let name = self.route_name("redirect", "https");
        self.routes.push(OutRoute {
            name,
            listeners: vec![lname],
            matcher: Some(OutMatch {
                host: hosts,
                ..OutMatch::default()
            }),
            middleware: vec![OutRef::new(
                "redirect",
                params([("to", J::Str(to)), ("status", J::Int(308))]),
            )],
            handler: OutRef::new("respond", Params::new()),
            mw_lines: vec![0],
            ..OutRoute::default()
        });
    }

    /// A route name, unique among the routes, made of the site's name and a hint.
    fn route_name(&mut self, site: &str, hint: &str) -> String {
        gostd::route_name(&mut self.names, site, hint, "/*")
    }
}

/// The access log's level for a log's: info writes every request, as debug does.
fn access_level(level: &str) -> &str {
    if level == "debug" || level == "info" {
        ""
    } else {
        level
    }
}

/// A site block: its addresses, and what its directives say of how it is served.
#[derive(Debug, Clone, Default)]
struct Site {
    block: ServerBlock,
    addrs: Vec<SiteAddr>,
    name: String,
    tls: SiteTls,
    bind: Vec<String>,
    matchers: HashMap<String, OutMatch>,
    bad_match: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct SiteAddr {
    raw: String,
    host: String,
    port: String,
    path: String,
    scheme: String,
    tls: bool,
}

/// What a site's tls directive says.
#[derive(Debug, Clone, Default)]
struct SiteTls {
    certs: Vec<OutCert>,
    on_demand: bool,
    internal: bool,
    client_auth: Option<OutClientAuth>,
    line: i64,
}

/// The addresses of a site served alike: on the same listeners, for the same hosts.
#[derive(Debug, Clone, Default)]
struct Group {
    listeners: Vec<String>,
    hosts: Vec<String>,
    paths: Vec<String>,
}
