//! What the objects held become: a part, as fit's `translator` makes it from Ingresses
//! (`ingress.go`), with the listeners, upstreams and certificate files of `gateway.go` that they
//! use. The Gateway API's objects come with I60, on the same translator.
//!
//! The part is the same bytes for the same objects, so that one unchanged is not sent again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use akit_config::value::write_string;
use akit_config::{KubernetesBackends, KubernetesIngress, KubernetesProvider};

use super::types::{
    Backend, EndpointSlice, Ingress, IngressClass, Meta, Object, Secret, Service, ServicePort,
};

/// The prefix of the annotations of Ingresses read: fit's, so that the same Ingresses serve
/// both.
pub(crate) const ANNOTATION_PREFIX: &str = "fit.lesismal.github.io/";

/// The largest certificate file written, as fit's TLS reads one (`tlsmgr.MaxCertFileBytes`).
pub(crate) const MAX_CERT_BYTES: usize = 1 << 20;

/// What the informers hold, at one moment.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    pub(crate) ingress_classes: Vec<Arc<Object>>,
    pub(crate) ingresses: Vec<Arc<Object>>,
    pub(crate) services: Vec<Arc<Object>>,
    pub(crate) slices: Vec<Arc<Object>>,
    pub(crate) secrets: Vec<Arc<Object>>,
}

/// The snapshot, indexed.
struct Index<'a> {
    classes: Vec<&'a IngressClass>,
    ingresses: Vec<&'a Ingress>,
    service: HashMap<String, &'a Service>,
    endpoints: HashMap<String, Vec<&'a EndpointSlice>>,
    secret: HashMap<String, &'a Secret>,
}

impl<'a> Index<'a> {
    fn new(s: &'a Snapshot) -> Index<'a> {
        let mut ix = Index {
            classes: s
                .ingress_classes
                .iter()
                .filter_map(|o| match &**o {
                    Object::IngressClass(c) => Some(c),
                    _ => None,
                })
                .collect(),
            ingresses: s
                .ingresses
                .iter()
                .filter_map(|o| match &**o {
                    Object::Ingress(i) => Some(i),
                    _ => None,
                })
                .collect(),
            service: HashMap::new(),
            endpoints: HashMap::new(),
            secret: HashMap::new(),
        };
        for o in &s.services {
            if let Object::Service(sv) = &**o {
                ix.service.insert(sv.meta.key(), sv);
            }
        }
        for o in &s.slices {
            if let Object::Slice(es) = &**o {
                if let Some(svc) = es.meta.label("kubernetes.io/service-name") {
                    if !svc.is_empty() {
                        ix.endpoints
                            .entry(format!("{}/{svc}", es.meta.namespace))
                            .or_default()
                            .push(es);
                    }
                }
            }
        }
        for o in &s.secrets {
            if let Object::Secret(sec) = &**o {
                if sec.typ == "kubernetes.io/tls" {
                    ix.secret.insert(sec.meta.key(), sec);
                }
            }
        }
        // As fit orders them: oldest first, then by namespace and name.
        ix.ingresses.sort_by(|a, b| by_creation(&a.meta, &b.meta));
        ix
    }
}

fn by_creation(a: &Meta, b: &Meta) -> std::cmp::Ordering {
    a.creation_timestamp
        .cmp(&b.creation_timestamp)
        .then_with(|| a.namespace.cmp(&b.namespace))
        .then_with(|| a.name.cmp(&b.name))
}

/// The protocol families of a port: what one listener of akit's serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Http,
    Tls,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::Http => "http",
            Family::Tls => "tls",
        }
    }
}

/// A certificate file a listener names: a Secret's chain and key, in one file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OutCert(String);

/// A listener of the part: the port's.
#[derive(Debug)]
struct OutListener {
    name: String,
    address: String,
    port: u16,
    family: Family,
    certs: Vec<OutCert>,
}

#[derive(Debug)]
struct OutUpstream {
    addresses: Vec<String>,
    h2c: bool,
}

#[derive(Debug, Clone, Default)]
struct OutMatch {
    host: Vec<String>,
    path: Vec<String>,
    remote_ip: Vec<String>,
}

impl OutMatch {
    fn is_empty(&self) -> bool {
        self.host.is_empty() && self.path.is_empty() && self.remote_ip.is_empty()
    }
}

#[derive(Debug)]
struct OutRoute {
    name: String,
    listeners: Vec<String>,
    priority: i64,
    m: Option<OutMatch>,
    /// Each a module as JSON.
    middleware: Vec<String>,
    handler: String,
}

/// What an Ingress's annotations ask of its routes.
#[derive(Debug, Default, Clone)]
struct Annotations {
    /// `ssl-redirect`: HTTP requests for its TLS hosts are redirected to HTTPS.
    ssl_redirect: bool,
    /// `rewrite-target`: the path its paths' matches are replaced with.
    rewrite: Option<String>,
    /// `backend-protocol`: H2C or GRPC.
    h2c: bool,
    /// `limit-rps`: requests a second from each client's address; 0 for no limit.
    rate: f64,
    /// `proxy-body-size`: the most bytes of a request body; 0 for no limit.
    body_bytes: u64,
    /// `allow-source-range`.
    allow: Vec<String>,
    /// `allow-source-range` does not parse: every request is refused.
    deny_all: bool,
    /// `request-timeout`: the proxy's timeout.
    timeout: String,
}

/// Turns a snapshot into a part.
#[derive(Debug)]
pub(crate) struct Translator<'o> {
    opts: &'o KubernetesProvider,
    provider: String,
    /// Where certificate files are written; `None` when there is nowhere, and HTTPS is not
    /// served.
    cert_dir: Option<String>,
    /// Why what was left out was, in the order found.
    pub(crate) problems: Vec<String>,
    listeners: BTreeMap<String, OutListener>,
    /// The family each port serves.
    ports: HashMap<u16, Family>,
    upstreams: BTreeMap<String, OutUpstream>,
    routes: Vec<OutRoute>,
    /// The certificate files the part names, by path: each a Secret's chain and key.
    pub(crate) certs: BTreeMap<String, Vec<u8>>,
}

impl<'o> Translator<'o> {
    pub(crate) fn new(
        provider: &str,
        opts: &'o KubernetesProvider,
        cert_dir: Option<String>,
    ) -> Translator<'o> {
        Translator {
            opts,
            provider: provider.to_owned(),
            cert_dir,
            problems: Vec::new(),
            listeners: BTreeMap::new(),
            ports: HashMap::new(),
            upstreams: BTreeMap::new(),
            routes: Vec::new(),
            certs: BTreeMap::new(),
        }
    }

    fn problem(&mut self, msg: String) {
        self.problems.push(msg);
    }

    fn watched(&self, ns: &str) -> bool {
        self.opts.namespaces.is_empty() || self.opts.namespaces.iter().any(|n| n == ns)
    }

    /// Makes the part: a listener for each port, routes for the Ingresses' paths, upstreams
    /// for the Services they send to.
    pub(crate) fn run(&mut self, s: &Snapshot) -> Vec<u8> {
        let ix = Index::new(s);
        if let Some(ports) = self.opts.ingress {
            self.ingresses(&ix, ports);
        }
        self.part()
    }

    /// Ingresses naming an IngressClass whose controller is `controller_name`, by
    /// `ingressClassName` or the `kubernetes.io/ingress.class` annotation, and those naming none
    /// when such a class is the default. Ingresses have no status of akit's.
    fn ingresses(&mut self, ix: &Index<'_>, ports: KubernetesIngress) {
        let mut classes: BTreeSet<&str> = BTreeSet::new();
        let mut any_default = false;
        for c in &ix.classes {
            if c.controller == self.opts.controller_name {
                classes.insert(&c.meta.name);
                any_default = any_default
                    || c.meta
                        .annotation("ingressclass.kubernetes.io/is-default-class")
                        == Some("true");
            }
        }
        for ing in &ix.ingresses {
            let class = ing.class_name.as_deref().unwrap_or_else(|| {
                ing.meta
                    .annotation("kubernetes.io/ingress.class")
                    .unwrap_or("")
            });
            if self.watched(&ing.meta.namespace)
                && ((class.is_empty() && any_default) || classes.contains(class))
            {
                self.ingress(ix, ing, ports);
            }
        }
    }

    /// Each path of each rule of `ing`, and its default backend, is a route on the listener of
    /// the HTTP port and, for the hosts of its tls, on that of the HTTPS port, with the
    /// certificates of its Secrets.
    fn ingress(&mut self, ix: &Index<'_>, ing: &Ingress, ports: KubernetesIngress) {
        let ns = &ing.meta.namespace;
        let r#where = format!("ingress {}", ing.meta.key());
        let a = self.annotations(&r#where, &ing.meta.annotations);

        let http_l = match self.port_listener(ports.http_port, Family::Http) {
            Ok(n) => Some(n),
            Err(other) => {
                self.problem(format!(
                    "{where}: port {} serves {} already: its rules are not served over HTTP",
                    ports.http_port,
                    other.name()
                ));
                None
            }
        };
        let mut certs = Vec::new();
        let mut tls_hosts: Vec<&str> = Vec::new();
        let mut tls_any = false;
        for (i, tl) in ing.tls.iter().enumerate() {
            if tl.secret_name.is_empty() {
                self.problem(format!("{where}: tls {i}: no secretName"));
                continue;
            }
            if self.cert_dir.is_none() {
                self.problem(format!(
                    "{where}: tls {i}: there is no directory to write certificates to"
                ));
                continue;
            }
            match self.secret_cert(ix, ns, &tl.secret_name) {
                Ok(c) => certs.push(c),
                Err(e) => {
                    self.problem(format!("{where}: tls {i}: {e}"));
                    continue;
                }
            }
            if tl.hosts.is_empty() {
                tls_any = true;
            }
            tls_hosts.extend(tl.hosts.iter().map(String::as_str));
        }
        let mut https_l = None;
        if !certs.is_empty() {
            match self.port_listener(ports.https_port, Family::Tls) {
                Ok(n) => {
                    if let Some(l) = self.listeners.get_mut(&n) {
                        for c in certs {
                            if !l.certs.contains(&c) {
                                l.certs.push(c);
                            }
                        }
                    }
                    https_l = Some(n);
                }
                Err(other) => self.problem(format!(
                    "{where}: port {} serves {} already: its rules are not served over HTTPS",
                    ports.https_port,
                    other.name()
                )),
            }
        }
        // Whether host, "" for any, is served over HTTPS.
        let secure = |host: &str| {
            https_l.is_some()
                && (tls_any
                    || (!host.is_empty() && tls_hosts.iter().any(|h| host_matches(h, host))))
        };
        let ls = Listeners {
            http: http_l.as_deref(),
            https: https_l.as_deref(),
            ports,
        };

        for (i, rule) in ing.rules.iter().enumerate() {
            let Some(paths) = &rule.paths else {
                continue;
            };
            let (hosts, host_class): (Vec<String>, i64) = if rule.host.is_empty() {
                (Vec::new(), 0)
            } else if rule.host.starts_with('*') {
                (vec![rule.host.clone()], 1)
            } else {
                (vec![rule.host.clone()], 2)
            };
            for (j, p) in paths.iter().enumerate() {
                let w = format!("{where}: rule {i}: path {j}");
                let path = if p.path.is_empty() { "/" } else { &p.path };
                if !path.starts_with('/') {
                    self.problem(format!("{w}: path {path:?} does not start with '/'"));
                    continue;
                }
                let mut om = OutMatch {
                    host: hosts.clone(),
                    ..OutMatch::default()
                };
                let (path_class, exact, prefix): (i64, bool, String) = match p.path_type.as_str() {
                    "Exact" => {
                        om.path = vec![path.to_owned()];
                        (3, true, String::new())
                    }
                    // fit's own reading of ImplementationSpecific is Prefix.
                    "Prefix" | "ImplementationSpecific" => {
                        let prefix = path.trim_end_matches('/').to_owned();
                        if !prefix.is_empty() {
                            om.path = vec![prefix.clone(), format!("{prefix}/*")];
                        }
                        (2, false, prefix)
                    }
                    other => {
                        self.problem(format!("{w}: path type {other:?} is not supported"));
                        continue;
                    }
                };
                let len = i64::try_from(path.len().min(9999)).unwrap_or(9999);
                let priority = host_class
                    .saturating_mul(100_000_000)
                    .saturating_add(path_class.saturating_mul(10_000_000))
                    .saturating_add(len.saturating_mul(1000));
                let name = format!("{ns}.{}.ing.r{i}.p{j}", ing.meta.name);
                let handler = self.backend(ix, &w, ns, &p.backend, &a);
                let route = Route {
                    r#where: &w,
                    name,
                    m: om,
                    priority,
                    host: &rule.host,
                    secure: secure(&rule.host),
                    exact,
                    prefix: &prefix,
                };
                self.routes_of(route, &ls, &a, handler);
            }
        }
        if let Some(b) = &ing.default_backend {
            let w = format!("{where}: defaultBackend");
            let handler = self.backend(ix, &w, ns, b, &a);
            let route = Route {
                r#where: &w,
                name: format!("{ns}.{}.ing.default", ing.meta.name),
                m: OutMatch::default(),
                priority: 0,
                host: "",
                secure: secure(""),
                exact: false,
                prefix: "",
            };
            self.routes_of(route, &ls, &a, handler);
        }
    }

    /// The routes of a path of an Ingress, or of its default backend: on the listener of the
    /// HTTP port and, when secure, on that of the HTTPS port, unless `a` redirects its HTTP to
    /// HTTPS; with a source range, a route refusing the other clients beside it.
    fn routes_of(&mut self, r: Route<'_>, ls: &Listeners<'_>, a: &Annotations, handler: String) {
        let mut mws = Vec::new();
        if a.rate > 0.0 {
            mws.push(format!(
                r#"{{"module":"rate_limit","params":{{"rate":{}}}}}"#,
                a.rate
            ));
        }
        if a.body_bytes > 0 {
            mws.push(format!(
                r#"{{"module":"body_limit","params":{{"max_bytes":{}}}}}"#,
                a.body_bytes
            ));
        }
        if let Some(to) = &a.rewrite {
            if let Some(mw) = rewrite(to, r.exact, r.prefix) {
                mws.push(mw);
            }
        }

        let mut redirect = String::new();
        if a.ssl_redirect && r.secure && ls.http.is_some() {
            let port = if ls.ports.https_port == 443 {
                String::new()
            } else {
                format!(":{}", ls.ports.https_port)
            };
            if !r.host.is_empty() && !r.host.starts_with('*') {
                redirect = format!("https://{}{port}{{uri}}", r.host);
            } else if ls.ports.http_port == 80 && port.is_empty() {
                redirect = "https://{host}{uri}".to_owned();
            } else {
                self.problem(format!(
                    "{}: ssl-redirect: a wildcard or any host is redirected only from port 80 to 443; it is served over HTTP",
                    r.r#where
                ));
            }
        }
        let mut listeners = Vec::new();
        if let Some(h) = ls.http {
            if redirect.is_empty() {
                listeners.push(h.to_owned());
            }
        }
        if r.secure {
            if let Some(h) = ls.https {
                listeners.push(h.to_owned());
            }
        }
        if !listeners.is_empty() {
            self.route(&r.name, listeners, &r.m, r.priority, mws, handler, a);
        }
        if let (false, Some(h)) = (redirect.is_empty(), ls.http) {
            let mut mw = String::from(r#"{"module":"redirect","params":{"status":308,"to":"#);
            write_string(&mut mw, &redirect);
            mw.push_str("}}");
            self.route(
                &format!("{}.redirect", r.name),
                vec![h.to_owned()],
                &r.m,
                r.priority,
                vec![mw],
                respond(404),
                a,
            );
        }
    }

    /// Adds the route `name`, and the one refusing the clients `a`'s source ranges leave out.
    #[allow(clippy::too_many_arguments)]
    fn route(
        &mut self,
        name: &str,
        listeners: Vec<String>,
        m: &OutMatch,
        priority: i64,
        middleware: Vec<String>,
        handler: String,
        a: &Annotations,
    ) {
        let opt = |m: &OutMatch| (!m.is_empty()).then(|| m.clone());
        if a.deny_all {
            self.routes.push(OutRoute {
                name: name.to_owned(),
                listeners,
                priority,
                m: opt(m),
                middleware: Vec::new(),
                handler: respond(403),
            });
        } else if !a.allow.is_empty() {
            let mut allowed = m.clone();
            allowed.remote_ip.clone_from(&a.allow);
            self.routes.push(OutRoute {
                name: name.to_owned(),
                listeners: listeners.clone(),
                priority: priority.saturating_add(1),
                m: Some(allowed),
                middleware,
                handler,
            });
            self.routes.push(OutRoute {
                name: format!("{name}.deny"),
                listeners,
                priority,
                m: opt(m),
                middleware: Vec::new(),
                handler: respond(403),
            });
        } else {
            self.routes.push(OutRoute {
                name: name.to_owned(),
                listeners,
                priority,
                m: opt(m),
                middleware,
                handler,
            });
        }
    }

    /// The handler sending to backend `b`: the proxy, to the upstream of a Service's port, or
    /// 503 when it cannot be.
    fn backend(
        &mut self,
        ix: &Index<'_>,
        r#where: &str,
        ns: &str,
        b: &Backend,
        a: &Annotations,
    ) -> String {
        let Some(svc_ref) = &b.service else {
            self.problem(format!(
                "{where}: only a Service is supported as a backend; it answers 503"
            ));
            return respond(503);
        };
        let mut port = svc_ref.port_number;
        if port == 0 && !svc_ref.port_name.is_empty() {
            if let Some(svc) = ix.service.get(&format!("{ns}/{}", svc_ref.name)) {
                for sp in &svc.ports {
                    if sp.name == svc_ref.port_name {
                        port = sp.port;
                    }
                }
            }
            if port == 0 {
                self.problem(format!(
                    "{where}: service {ns}/{} has no port named {:?}; it answers 503",
                    svc_ref.name, svc_ref.port_name
                ));
                return respond(503);
            }
        }
        match self.upstream(ix, ns, &svc_ref.name, port, a.h2c) {
            Err(e) => {
                self.problem(format!("{where}: {e}; it answers 503"));
                respond(503)
            }
            Ok(None) => respond(503),
            Ok(Some(name)) => {
                let mut h = String::from(r#"{"module":"proxy","params":{"#);
                if !a.timeout.is_empty() {
                    h.push_str(r#""timeout":"#);
                    write_string(&mut h, &a.timeout);
                    h.push(',');
                }
                h.push_str(r#""upstream":"#);
                write_string(&mut h, &name);
                h.push_str("}}");
                h
            }
        }
    }

    /// Makes the upstream of the TCP port `port` of the Service `ns/name`, and returns its
    /// name; `None` when it has no endpoints ready. `h2c` makes it speak HTTP/2 in the clear.
    fn upstream(
        &mut self,
        ix: &Index<'_>,
        ns: &str,
        name: &str,
        port: i64,
        h2c: bool,
    ) -> Result<Option<String>, String> {
        let Some(svc) = ix.service.get(&format!("{ns}/{name}")) else {
            return Err(format!("service {ns}/{name} does not exist"));
        };
        let Some(sp) = svc
            .ports
            .iter()
            .rev()
            .find(|sp| sp.port == port && or(&sp.protocol, "TCP") == "TCP")
        else {
            return Err(format!("service {ns}/{name} has no TCP port {port}"));
        };
        if svc.typ == "ExternalName" {
            // fit's allow_external_name_services, which needs the upstreams' deny_internal.
            return Err(format!(
                "service {ns}/{name} is an ExternalName Service, which allow_external_name_services must allow"
            ));
        }
        let app_h2c = sp.app_protocol.as_deref() == Some("kubernetes.io/h2c");
        let mut up = format!("{ns}.{name}.{}", sp.port);
        if h2c && !app_h2c {
            up.push_str(".h2c");
        }
        if self.upstreams.contains_key(&up) {
            return Ok(Some(up));
        }
        let addrs = self.addresses(ix, svc, sp);
        if addrs.is_empty() {
            return Ok(None);
        }
        self.upstreams.insert(
            up.clone(),
            OutUpstream {
                addresses: addrs,
                h2c: h2c || app_h2c,
            },
        );
        Ok(Some(up))
    }

    /// Where `svc`'s TCP port `sp` is reached: its cluster IP, with backends `cluster_ip`, or
    /// else its ready endpoints.
    fn addresses(&self, ix: &Index<'_>, svc: &Service, sp: &ServicePort) -> Vec<String> {
        if self.opts.backends == KubernetesBackends::ClusterIp
            && !svc.cluster_ip.is_empty()
            && svc.cluster_ip != "None"
        {
            return vec![join_host_port(&svc.cluster_ip, sp.port)];
        }
        let mut out = Vec::new();
        for es in ix
            .endpoints
            .get(&svc.meta.key())
            .map_or(&[][..], Vec::as_slice)
        {
            let mut target = 0;
            for p in &es.ports {
                if p.name.as_deref().unwrap_or("") == sp.name
                    && p.protocol.as_deref().unwrap_or("TCP") == "TCP"
                {
                    if let Some(port) = p.port {
                        target = port;
                    }
                }
            }
            if target == 0 {
                continue;
            }
            for e in &es.endpoints {
                if !e.ready.unwrap_or(true) {
                    continue;
                }
                if let Some(addr) = e.addresses.first() {
                    out.push(join_host_port(addr, target));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// akit's listener of `family` on `port`, made if need be; or the family the port serves
    /// already, when it is another.
    fn port_listener(&mut self, port: u16, family: Family) -> Result<String, Family> {
        match self.ports.get(&port) {
            Some(f) if *f != family => return Err(*f),
            _ => {
                self.ports.insert(port, family);
            }
        }
        let name = format!("{}-{port}", self.provider);
        self.listeners
            .entry(name.clone())
            .or_insert_with(|| OutListener {
                name: name.clone(),
                address: join_host_port(&self.opts.listen_host, i64::from(port)),
                port,
                family,
                certs: Vec::new(),
            });
        Ok(name)
    }

    /// The certificate file of the Secret `ns/name`, of type `kubernetes.io/tls`, and gives the
    /// file to write. `cert_dir` must be set.
    fn secret_cert(&mut self, ix: &Index<'_>, ns: &str, name: &str) -> Result<OutCert, String> {
        let Some(sec) = ix.secret.get(&format!("{ns}/{name}")) else {
            return Err(format!(
                "there is no secret {ns}/{name} of type kubernetes.io/tls"
            ));
        };
        key_pair(&sec.crt, &sec.key).map_err(|e| format!("secret {ns}/{name}: {e}"))?;
        let mut data = Vec::with_capacity(
            sec.crt
                .len()
                .saturating_add(sec.key.len())
                .saturating_add(1),
        );
        data.extend_from_slice(&sec.crt);
        data.push(b'\n');
        data.extend_from_slice(&sec.key);
        if data.len() > MAX_CERT_BYTES {
            return Err(format!(
                "secret {ns}/{name} is larger than {MAX_CERT_BYTES} bytes"
            ));
        }
        let sum = akio_crypto::hash::Sha256::digest(&data);
        let mut hex = String::with_capacity(12);
        for b in sum.iter().take(6) {
            let _ = write!(hex, "{b:02x}");
        }
        let dir = self.cert_dir.as_deref().unwrap_or_default();
        let path = Path::new(dir)
            .join(format!("{ns}.{name}.{hex}.pem"))
            .display()
            .to_string();
        self.certs.insert(path.clone(), data);
        Ok(OutCert(path))
    }

    /// Reads the annotations of an Ingress under [`ANNOTATION_PREFIX`], saying which it leaves
    /// out and why.
    fn annotations(&mut self, r#where: &str, ann: &[(String, String)]) -> Annotations {
        let mut a = Annotations::default();
        let mut sorted: Vec<&(String, String)> = ann.iter().collect();
        sorted.sort();
        for (k, v) in sorted {
            let Some(name) = k.strip_prefix(ANNOTATION_PREFIX) else {
                continue;
            };
            let v = v.trim();
            let bad = |why: String| format!("{where}: annotation {k}: {why}; it is left out");
            match name {
                "ssl-redirect" => match parse_bool(v) {
                    Some(b) => a.ssl_redirect = b,
                    None => self.problem(bad(format!("{v:?} is not true or false"))),
                },
                "rewrite-target" => {
                    if v.starts_with('/') {
                        a.rewrite = Some(v.to_owned());
                    } else {
                        self.problem(bad(format!("{v:?} is not a path")));
                    }
                }
                "backend-protocol" => match v.to_ascii_uppercase().as_str() {
                    "HTTP" => {}
                    "H2C" | "GRPC" => a.h2c = true,
                    _ => self.problem(bad(format!("{v:?} is not HTTP, H2C or GRPC"))),
                },
                "limit-rps" => match v.parse::<f64>() {
                    Ok(r) if r.is_finite() && r >= 0.001 => a.rate = r,
                    _ => self.problem(bad(format!("{v:?} is not a rate of at least 0.001"))),
                },
                "proxy-body-size" => match parse_size(v) {
                    Ok(n) => a.body_bytes = n,
                    Err(e) => self.problem(bad(e)),
                },
                "allow-source-range" => {
                    for r in v.split(',') {
                        let r = r.trim();
                        if r.is_empty() {
                            continue;
                        }
                        if !is_prefix(r) && r.parse::<IpAddr>().is_err() {
                            // A range that does not parse must not open the Ingress to
                            // everyone: it is closed instead.
                            self.problem(format!(
                                "{where}: annotation {k}: {r:?} is not an address or CIDR range; every request is refused"
                            ));
                            a.deny_all = true;
                            continue;
                        }
                        a.allow.push(r.to_owned());
                    }
                }
                "request-timeout" => match akit_config::duration::parse(v) {
                    Some(Ok(d)) if !d.is_zero() => a.timeout = v.to_owned(),
                    _ => self.problem(bad(format!("{v:?} is not a duration, as 30s"))),
                },
                _ => self.problem(format!("{where}: annotation {k} is not one akit knows")),
            }
        }
        a
    }

    /// The part, as JSON: listeners by port, upstreams by name, routes in the order made.
    fn part(&self) -> Vec<u8> {
        let mut b = String::with_capacity(256);
        b.push('{');
        let mut first_field = true;
        let mut field = |b: &mut String, name: &str| {
            if !first_field {
                b.push(',');
            }
            first_field = false;
            b.push('"');
            b.push_str(name);
            b.push_str("\":[");
        };
        let mut ls: Vec<&OutListener> = self
            .listeners
            .values()
            .filter(|l| l.family == Family::Http || !l.certs.is_empty())
            .collect();
        ls.sort_by(|a, b| a.port.cmp(&b.port).then_with(|| a.name.cmp(&b.name)));
        if !ls.is_empty() {
            field(&mut b, "listeners");
            for (i, l) in ls.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push_str(r#"{"name":"#);
                write_string(&mut b, &l.name);
                b.push_str(r#","address":"#);
                write_string(&mut b, &l.address);
                match l.family {
                    Family::Http => b.push_str(r#","protocols":["http/1.1","h2c"]}"#),
                    Family::Tls => {
                        b.push_str(r#","protocols":["http/1.1","h2"],"tls":{"certificates":["#);
                        for (j, c) in l.certs.iter().enumerate() {
                            if j > 0 {
                                b.push(',');
                            }
                            b.push_str(r#"{"cert_file":"#);
                            write_string(&mut b, &c.0);
                            b.push_str(r#","key_file":"#);
                            write_string(&mut b, &c.0);
                            b.push('}');
                        }
                        b.push_str("]}}");
                    }
                }
            }
            b.push(']');
        }
        if !self.upstreams.is_empty() {
            field(&mut b, "upstreams");
            for (i, (name, u)) in self.upstreams.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push_str(r#"{"name":"#);
                write_string(&mut b, name);
                b.push_str(r#","addresses":"#);
                strings(&mut b, &u.addresses);
                if u.h2c {
                    b.push_str(r#","protocol":"h2c""#);
                }
                b.push('}');
            }
            b.push(']');
        }
        if !self.routes.is_empty() {
            field(&mut b, "routes");
            for (i, r) in self.routes.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push_str(r#"{"name":"#);
                write_string(&mut b, &r.name);
                b.push_str(r#","listeners":"#);
                strings(&mut b, &r.listeners);
                if r.priority != 0 {
                    let _ = write!(b, r#","priority":{}"#, r.priority);
                }
                if let Some(m) = &r.m {
                    b.push_str(r#","match":{"#);
                    let mut first = true;
                    for (k, v) in [
                        ("host", &m.host),
                        ("path", &m.path),
                        ("remote_ip", &m.remote_ip),
                    ] {
                        if v.is_empty() {
                            continue;
                        }
                        if !first {
                            b.push(',');
                        }
                        first = false;
                        let _ = write!(b, r#""{k}":"#);
                        strings(&mut b, v);
                    }
                    b.push('}');
                }
                if !r.middleware.is_empty() {
                    b.push_str(r#","middleware":["#);
                    b.push_str(&r.middleware.join(","));
                    b.push(']');
                }
                b.push_str(r#","handler":"#);
                b.push_str(&r.handler);
                b.push('}');
            }
            b.push(']');
        }
        b.push('}');
        b.into_bytes()
    }
}

/// A route of an Ingress being made.
struct Route<'a> {
    r#where: &'a str,
    name: String,
    m: OutMatch,
    priority: i64,
    /// The rule's host, "" for any.
    host: &'a str,
    secure: bool,
    /// Its path type is Exact; else Prefix, whose prefix is `prefix`.
    exact: bool,
    prefix: &'a str,
}

/// The listeners of an Ingress's routes.
struct Listeners<'a> {
    http: Option<&'a str>,
    https: Option<&'a str>,
    ports: KubernetesIngress,
}

fn strings(b: &mut String, v: &[String]) {
    b.push('[');
    for (i, s) in v.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(b, s);
    }
    b.push(']');
}

fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

/// The respond handler answering `status`.
fn respond(status: u16) -> String {
    format!(r#"{{"module":"respond","params":{{"status":{status}}}}}"#)
}

/// The rewrite middleware for `rewrite-target` `to`: the whole path of an Exact match replaced,
/// or a Prefix match's `prefix` replaced; `None` for no change.
fn rewrite(to: &str, exact: bool, prefix: &str) -> Option<String> {
    let mut b = String::from(r#"{"module":"rewrite","params":{"#);
    if exact {
        b.push_str(r#""regexp":"^.*$","replacement":"#);
        write_string(&mut b, &to.replace('$', "$$"));
    } else {
        let add = to.trim_end_matches('/');
        let mut first = true;
        if !add.is_empty() {
            b.push_str(r#""add_prefix":"#);
            write_string(&mut b, add);
            first = false;
        }
        if !prefix.is_empty() {
            if !first {
                b.push(',');
            }
            b.push_str(r#""strip_prefix":["#);
            write_string(&mut b, prefix);
            b.push(']');
            first = false;
        }
        if first {
            return None;
        }
    }
    b.push_str("}}");
    Some(b)
}

/// Whether the host name pattern `p`, maybe a wildcard, takes `h`: equal, or `h` below `p`'s
/// domain for `*.`.
pub(crate) fn host_matches(p: &str, h: &str) -> bool {
    if p == h {
        return true;
    }
    match p.strip_prefix('*') {
        Some(suffix) => h.ends_with(suffix) && h.len() > suffix.len() && !h.starts_with('*'),
        None => false,
    }
}

/// Go's `net.JoinHostPort`.
fn join_host_port(host: &str, port: i64) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Go's `strconv.ParseBool`.
fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// Go's `netip.ParsePrefix`: an address, `/` and as many bits as it has at most.
fn is_prefix(r: &str) -> bool {
    let Some((a, bits)) = r.split_once('/') else {
        return false;
    };
    let Ok(a) = a.parse::<IpAddr>() else {
        return false;
    };
    let max = if a.is_ipv4() { 32 } else { 128 };
    !bits.is_empty()
        && bits.bytes().all(|c| c.is_ascii_digit())
        && (bits == "0" || !bits.starts_with('0'))
        && bits.parse::<u32>().is_ok_and(|n| n <= max)
}

/// Parses a size in bytes, as nginx writes them: a number, maybe followed by k, m or g (case
/// aside), for KiB, MiB and GiB; 0 for no limit.
pub(crate) fn parse_size(size: &str) -> Result<u64, String> {
    let lower = size.to_ascii_lowercase();
    let (v, mult): (&str, u64) = match lower.as_bytes().last() {
        Some(b'k') => (
            size.get(..size.len().saturating_sub(1)).unwrap_or(""),
            1 << 10,
        ),
        Some(b'm') => (
            size.get(..size.len().saturating_sub(1)).unwrap_or(""),
            1 << 20,
        ),
        Some(b'g') => (
            size.get(..size.len().saturating_sub(1)).unwrap_or(""),
            1 << 30,
        ),
        _ => (size, 1),
    };
    let bad = || format!("{size:?} is not a size, as 8m");
    let n: i64 = v.parse().map_err(|_| bad())?;
    let n = u64::try_from(n).map_err(|_| bad())?;
    if n > (1u64 << 40).checked_div(mult).unwrap_or(0) {
        return Err(bad());
    }
    n.checked_mul(mult).ok_or_else(bad)
}

/// Whether `crt` and `key` are a certificate chain and its key, as Go's `tls.X509KeyPair`
/// checks them.
pub(crate) fn key_pair(crt: &[u8], key: &[u8]) -> Result<(), String> {
    use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
    let chain = Certificate::from_pem(crt).map_err(|e| format!("tls.crt: {e}"))?;
    if chain.is_empty() {
        return Err("tls.crt holds no certificate".to_owned());
    }
    let key = PrivateKey::from_pem(key).map_err(|e| format!("tls.key: {e}"))?;
    ClientConfig::builder()
        .roots(RootCerts::empty())
        .client_cert(chain, key)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{host_matches, is_prefix, parse_size};

    /// fit's `TestParseSize`.
    #[test]
    fn sizes() {
        for (s, want) in [
            ("0", 0),
            ("100", 100),
            ("8k", 8 << 10),
            ("1m", 1 << 20),
            ("2G", 2 << 30),
        ] {
            assert_eq!(parse_size(s), Ok(want), "{s}");
        }
        for s in ["", "-1", "1t", "m", "99999999999g"] {
            assert!(parse_size(s).is_err(), "{s}");
        }
    }

    #[test]
    fn hosts_and_prefixes() {
        assert!(host_matches("*.example.com", "a.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "*.example.com.x"));
        assert!(host_matches("a.b", "a.b"));
        for ok in ["10.0.0.0/8", "::/0", "1.2.3.4/32"] {
            assert!(is_prefix(ok), "{ok}");
        }
        for bad in ["10.0.0.0", "10.0.0.0/33", "10.0.0.0/08", "x/8", "1.2.3.4/"] {
            assert!(!is_prefix(bad), "{bad}");
        }
    }
}
