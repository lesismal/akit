//! The Gateway API's objects, translated (fit's `gateway.go`, `grpc.go` and `l4.go`), on the
//! translator Ingresses share.
//!
//! The listeners of a Gateway whose GatewayClass names `controller_name` are listeners of akit's,
//! one for each port (shared by Gateways) and family: HTTP; HTTPS, with the certificates of the
//! Secrets it names, and TLS passed through, told apart by server name on one port; TCP; UDP.
//! Each HTTPRoute and GRPCRoute attached to one is a route of akit's for each of its rules'
//! matches, each TLSRoute, TCPRoute and UDPRoute a service, and each Service port they send to
//! an upstream, of its ready endpoints or its cluster IP. Which listener takes which route
//! follows the Gateway API: parentRefs' sections and ports, listeners' allowedRoutes (Same, All
//! and Selector namespaces, and kinds) and host names; a backend or certificate in another
//! namespace needs a ReferenceGrant. Routes keep the API's precedence as their priority. What
//! the API allows and akit does not serve yet (TLS terminated on a TLS listener, header and query
//! matches by regular expression, filters on backends, ExternalName Services) is left out,
//! reported, and said in the status ([`super::status`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use akit_config::value::write_string;

use super::translate::{
    Family, Index, OutCert, OutMatch, OutRoute, OutService, OutUpstream, RefError, Translator,
    by_creation, host_matches, join_host_port, respond, string_lists, strings,
};
use super::types::{
    Filter, Gateway, GrpcMatch, HeaderFilter, HttpMatch, LabelSelector, Listener, Object,
    ObjectRef, ParentRef, PathModifier, Redirect, Route, RouteGroupKind, Rule,
};

/// The Gateway API's group.
pub(crate) const GATEWAY_GROUP: &str = "gateway.networking.k8s.io";

/// A kind of route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteKind {
    Http,
    Grpc,
    Tls,
    Tcp,
    Udp,
}

impl RouteKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            RouteKind::Http => "HTTPRoute",
            RouteKind::Grpc => "GRPCRoute",
            RouteKind::Tls => "TLSRoute",
            RouteKind::Tcp => "TCPRoute",
            RouteKind::Udp => "UDPRoute",
        }
    }

    /// Its plural, which names its kind of the provider's.
    pub(crate) fn plural(self) -> &'static str {
        match self {
            RouteKind::Http => "httproutes",
            RouteKind::Grpc => "grpcroutes",
            RouteKind::Tls => "tlsroutes",
            RouteKind::Tcp => "tcproutes",
            RouteKind::Udp => "udproutes",
        }
    }

    fn l4(self) -> bool {
        !matches!(self, RouteKind::Http | RouteKind::Grpc)
    }
}

/// A reason and a message, of a condition that is not as it should be.
pub(crate) type Why = (&'static str, String);

/// One of akit's Gateways, and its listeners' states.
#[derive(Debug)]
pub(crate) struct GwState {
    pub(crate) obj: Arc<Object>,
    pub(crate) listeners: Vec<GwListener>,
}

impl GwState {
    pub(crate) fn gateway(&self) -> Option<&Gateway> {
        match &*self.obj {
            Object::Gateway(g) => Some(g),
            _ => None,
        }
    }
}

/// One of a Gateway's listeners, and its state.
#[derive(Debug)]
pub(crate) struct GwListener {
    pub(crate) l: Listener,
    /// The name of akit's listener serving it; `None` when none does.
    pub(crate) fit: Option<String>,
    /// The kinds of routes it takes.
    pub(crate) kinds: Vec<RouteGroupKind>,
    /// Routes attached.
    pub(crate) attached: i64,
    /// Its conditions: why it is not accepted, its conflict, and why its references are not
    /// resolved.
    pub(crate) refused: Option<Why>,
    pub(crate) conflict: Option<Why>,
    pub(crate) unresolved: Option<Why>,
}

/// A route being or having been translated, and what its status says.
#[derive(Debug)]
pub(crate) struct RouteState {
    pub(crate) kind: RouteKind,
    pub(crate) obj: Arc<Object>,
    /// One for each parentRef to one of akit's Gateways.
    pub(crate) parents: Vec<ParentState>,
    /// Why a backend is not resolved, and the first part of the route left out.
    pub(crate) unresolved: Option<Why>,
    pub(crate) partial: Option<String>,
}

impl RouteState {
    pub(crate) fn route(&self) -> Option<&Route> {
        match &*self.obj {
            Object::Route(r) => Some(r),
            _ => None,
        }
    }

    /// Whether a listener of one of akit's Gateways takes the route.
    pub(crate) fn accepted(&self) -> bool {
        self.parents.iter().any(|p| p.accepted)
    }
}

/// Whether one of akit's Gateways takes a route, and why not.
#[derive(Debug)]
pub(crate) struct ParentState {
    pub(crate) r: ParentRef,
    pub(crate) accepted: bool,
    pub(crate) why: Option<Why>,
}

/// Where a route's rules are served: akit's listeners, for the host names `hosts`, or any host
/// when `None`.
#[derive(Debug)]
struct Attachment {
    listeners: Vec<String>,
    hosts: Option<Vec<String>>,
}

/// Where a rule's requests go.
#[derive(Debug)]
struct RuleBackends {
    /// Upstreams and their weights.
    splits: Vec<(String, i64)>,
    /// What answers when `splits` is empty: 500 for backends that are not valid, 503 for valid
    /// ones with no endpoints ready.
    status: u16,
}

/// The route kinds of each protocol.
fn protocol_kinds(protocol: &str) -> Option<&'static [RouteKind]> {
    match protocol {
        "HTTP" | "HTTPS" => Some(&[RouteKind::Http, RouteKind::Grpc]),
        "TLS" => Some(&[RouteKind::Tls]),
        "TCP" => Some(&[RouteKind::Tcp]),
        "UDP" => Some(&[RouteKind::Udp]),
        _ => None,
    }
}

fn or<'a>(v: Option<&'a str>, def: &'a str) -> &'a str {
    v.unwrap_or(def)
}

impl Translator<'_> {
    /// Reports a backend of the route being translated that cannot be resolved, which its status
    /// says.
    fn bad_ref(&mut self, r#where: &str, e: &RefError, suffix: &str) {
        self.problem(format!("{where}: {}{suffix}", e.msg));
        if let Some(st) = self.cur.as_mut() {
            if st.unresolved.is_none() {
                st.unresolved = Some((e.reason, e.msg.clone()));
            }
        }
    }

    /// Reports a part of the route being translated that akit does not serve, which its status
    /// says.
    fn left_out(&mut self, msg: String) {
        if let Some(st) = self.cur.as_mut() {
            if st.partial.is_none() {
                st.partial = Some(msg.clone());
            }
        }
        self.problem(msg);
    }

    /// The Gateway API's objects: akit's Gateways, then the routes in the namespaces watched,
    /// each kind oldest first.
    pub(crate) fn gateway_api(&mut self, ix: &Index<'_>) {
        self.classes.clone_from(&ix.snap.gateway_classes);
        self.gateways(ix);
        for (kind, objs) in [
            (RouteKind::Http, &ix.snap.http_routes),
            (RouteKind::Grpc, &ix.snap.grpc_routes),
            (RouteKind::Tls, &ix.snap.tls_routes),
            (RouteKind::Tcp, &ix.snap.tcp_routes),
            (RouteKind::Udp, &ix.snap.udp_routes),
        ] {
            let mut rs: Vec<&Arc<Object>> = objs
                .iter()
                .filter(|o| matches!(&***o, Object::Route(_)))
                .collect();
            rs.sort_by(|a, b| by_creation(a.meta(), b.meta()));
            for o in rs {
                if !self.watched(&o.meta().namespace) {
                    continue;
                }
                let Object::Route(r) = &**o else {
                    continue;
                };
                self.cur = Some(RouteState {
                    kind,
                    obj: Arc::clone(o),
                    parents: Vec::new(),
                    unresolved: None,
                    partial: None,
                });
                match kind {
                    RouteKind::Http | RouteKind::Grpc => self.http_route(ix, kind, r),
                    _ => self.l4_route(ix, kind, r),
                }
                if let Some(st) = self.cur.take() {
                    self.states.push(st);
                }
            }
        }
    }

    /// akit's Gateways: those of GatewayClasses whose controllerName is `controller_name`, in
    /// the namespaces watched, oldest first. A listener of akit's for each port, and family, of
    /// their listeners.
    fn gateways(&mut self, ix: &Index<'_>) {
        let classes: BTreeSet<&str> = ix
            .snap
            .gateway_classes
            .iter()
            .filter_map(|o| match &**o {
                Object::GatewayClass(c) if c.controller_name == self.opts.controller_name => {
                    Some(c.meta.name.as_str())
                }
                _ => None,
            })
            .collect();
        let mut gws: Vec<&Arc<Object>> = ix
            .snap
            .gateways
            .iter()
            .filter(|o| matches!(&***o, Object::Gateway(_)))
            .collect();
        gws.sort_by(|a, b| by_creation(a.meta(), b.meta()));
        for o in gws {
            let Object::Gateway(g) = &**o else {
                continue;
            };
            if !classes.contains(g.class_name.as_str()) || !self.watched(&g.meta.namespace) {
                continue;
            }
            let listeners = g
                .listeners
                .iter()
                .map(|l| self.listener(ix, g, l))
                .collect();
            self.by_key.insert(g.meta.key(), self.gws.len());
            self.gws.push(GwState {
                obj: Arc::clone(o),
                listeners,
            });
        }
    }

    /// Takes listener `l` of Gateway `g`: on akit's listener of its port and family, unless it
    /// is not valid or conflicts with one before it.
    fn listener(&mut self, ix: &Index<'_>, g: &Gateway, l: &Listener) -> GwListener {
        let mut gl = GwListener {
            l: l.clone(),
            fit: None,
            kinds: Vec::new(),
            attached: 0,
            refused: None,
            conflict: None,
            unresolved: None,
        };
        let r#where = format!("gateway {}: listener {}", g.meta.key(), l.name);
        let refuse = |t: &mut Self, mut gl: GwListener, reason: &'static str, msg: String| {
            t.problem(format!("{where}: {msg}"));
            gl.refused = Some((reason, msg));
            gl
        };
        let Some(kinds) = protocol_kinds(&l.protocol) else {
            let msg = format!("protocol {} is not supported", l.protocol);
            return refuse(self, gl, "UnsupportedProtocol", msg);
        };
        let allowed = l.allowed_routes.as_ref().map_or(&[][..], |a| &a.kinds[..]);
        for k in kinds {
            if allowed.is_empty()
                || allowed.iter().any(|rk| {
                    or(rk.group.as_deref(), GATEWAY_GROUP) == GATEWAY_GROUP && rk.kind == k.name()
                })
            {
                gl.kinds.push(RouteGroupKind {
                    group: Some(GATEWAY_GROUP.to_owned()),
                    kind: k.name().to_owned(),
                });
            }
        }
        for rk in allowed {
            if or(rk.group.as_deref(), GATEWAY_GROUP) != GATEWAY_GROUP
                || !kinds.iter().any(|k| k.name() == rk.kind)
            {
                let msg = format!("a {} listener takes no {}", l.protocol, rk.kind);
                self.problem(format!("{where}: {msg}"));
                gl.unresolved = Some(("InvalidRouteKinds", msg));
                break;
            }
        }
        let Some(port) = u16::try_from(l.port).ok().filter(|p| *p != 0) else {
            let msg = format!("{} is not a port", l.port);
            return refuse(self, gl, "PortUnavailable", msg);
        };
        let mode = l
            .tls
            .as_ref()
            .map(|t| or(t.mode.as_deref(), "Terminate"))
            .unwrap_or("");
        let family = match l.protocol.as_str() {
            "HTTP" => Family::Http,
            "HTTPS" => {
                if mode == "Passthrough" {
                    let msg = "an HTTPS listener terminates TLS; a TLS listener passes it through"
                        .to_owned();
                    return refuse(self, gl, "UnsupportedValue", msg);
                }
                Family::Tls
            }
            "TLS" => {
                if mode != "Passthrough" {
                    let msg = "akit passes the connections of TLS listeners through (tls.mode Passthrough), and does not terminate them yet".to_owned();
                    return refuse(self, gl, "UnsupportedValue", msg);
                }
                Family::Tls
            }
            "TCP" => Family::Tcp,
            _ => Family::Udp,
        };
        let mut certs = Vec::new();
        if l.protocol == "HTTPS" {
            certs = self.certificates(ix, g, &mut gl, &r#where);
            if certs.is_empty() {
                return gl;
            }
        }
        match self.port_listener(port, family) {
            Ok(name) => {
                if let Some(fl) = self.listeners.get_mut(&name) {
                    fl.add_certs(certs);
                }
                gl.fit = Some(name);
            }
            Err(other) => {
                let msg = format!("port {port} serves {} already", other.name());
                self.problem(format!("{where}: {msg}"));
                gl.conflict = Some(("ProtocolConflict", msg));
            }
        }
        gl
    }

    /// The certificate files of HTTPS listener `gl` of Gateway `g`, from the Secrets it names,
    /// and gives the files to write. Says why it leaves out those it does.
    fn certificates(
        &mut self,
        ix: &Index<'_>,
        g: &Gateway,
        gl: &mut GwListener,
        r#where: &str,
    ) -> Vec<OutCert> {
        let bad = |t: &mut Self, gl: &mut GwListener, reason: &'static str, msg: String| {
            t.problem(format!("{where}: {msg}"));
            if gl.unresolved.is_none() {
                gl.unresolved = Some((reason, msg));
            }
        };
        let refs = match &gl.l.tls {
            Some(t) if !t.certificate_refs.is_empty() => t.certificate_refs.clone(),
            _ => {
                let msg = "an HTTPS listener needs a certificateRef".to_owned();
                bad(self, gl, "InvalidCertificateRef", msg);
                return Vec::new();
            }
        };
        if self.cert_dir.is_none() {
            let msg = "there is no directory to write certificates to".to_owned();
            bad(self, gl, "InvalidCertificateRef", msg);
            return Vec::new();
        }
        let gns = &g.meta.namespace;
        let mut out = Vec::new();
        for (i, r) in refs.iter().enumerate() {
            let (group, kind) = (or(r.group.as_deref(), ""), or(r.kind.as_deref(), "Secret"));
            if !group.is_empty() || kind != "Secret" {
                let msg = format!("certificateRef {i}: {group}/{kind} is not a Secret");
                bad(self, gl, "InvalidCertificateRef", msg);
                continue;
            }
            let ns = or(r.namespace.as_deref(), gns);
            if ns != gns && !granted(ix, "Gateway", gns, ns, "Secret", &r.name) {
                let msg = format!(
                    "certificateRef {i}: secret {ns}/{} is in another namespace, and no ReferenceGrant there allows it",
                    r.name
                );
                bad(self, gl, "RefNotPermitted", msg);
                continue;
            }
            match self.secret_cert(ix, ns, &r.name) {
                Ok(c) => out.push(c),
                Err(e) => {
                    let msg = format!("certificateRef {i}: {e}");
                    bad(self, gl, "InvalidCertificateRef", msg);
                }
            }
        }
        out
    }

    /// Attaches the route being translated, `r` of `kind`, of the host names `hostnames`, to
    /// the listeners of akit's Gateways its parentRefs name and that take it, and records
    /// whether each does. Returns where the route is served, listeners serving the same host
    /// names together, in a fixed order; none when it is not akit's or no listener takes it.
    fn attach(
        &mut self,
        ix: &Index<'_>,
        kind: RouteKind,
        r: &Route,
        hostnames: &[String],
    ) -> Vec<Attachment> {
        let key = r.meta.key();
        let owner = format!("{} {key}", kind.name());
        // The hosts each listener of akit's serves the route for, or any.
        let mut hosts: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut any_host: BTreeSet<String> = BTreeSet::new();
        let mut parents = Vec::new();
        for pr in &r.parent_refs {
            if or(pr.group.as_deref(), GATEWAY_GROUP) != GATEWAY_GROUP
                || or(pr.kind.as_deref(), "Gateway") != "Gateway"
            {
                continue;
            }
            let gk = format!(
                "{}/{}",
                or(pr.namespace.as_deref(), &r.meta.namespace),
                pr.name
            );
            let Some(gi) = self.by_key.get(&gk).copied() else {
                continue; // not akit's
            };
            let mut ps = ParentState {
                r: pr.clone(),
                accepted: false,
                why: None,
            };
            let (mut named, mut allowed, mut host_ok, mut taken) =
                (false, false, false, String::new());
            let n = self.gws.get(gi).map_or(0, |g| g.listeners.len());
            for li in 0..n {
                let Some(gw) = self.gws.get(gi) else {
                    break;
                };
                let Some(gl) = gw.listeners.get(li) else {
                    break;
                };
                let l = &gl.l;
                if pr.section_name.as_ref().is_some_and(|s| *s != l.name)
                    || pr.port.is_some_and(|p| p != l.port)
                {
                    continue;
                }
                named = true;
                // A listener accepted takes routes, and counts them, whether or not akit serves
                // it (its certificates may be missing).
                let gns = gw.gateway().map_or("", |g| g.meta.namespace.as_str());
                if gl.refused.is_some() || !allowed_by(ix, gl, gns, kind, &r.meta.namespace) {
                    continue;
                }
                allowed = true;
                let lh = if l.protocol != "TCP" && l.protocol != "UDP" {
                    l.hostname.as_deref()
                } else {
                    None
                };
                let Some(hs) = intersect(lh, hostnames) else {
                    continue;
                };
                host_ok = true;
                let fit = gl.fit.clone();
                if kind.l4() {
                    if let Some(f) = &fit {
                        if let Some(other) = self.claim(f, hs.as_deref(), &owner) {
                            taken = other;
                            continue;
                        }
                    }
                }
                if let Some(gl) = self.gws.get_mut(gi).and_then(|g| g.listeners.get_mut(li)) {
                    gl.attached = gl.attached.saturating_add(1);
                }
                ps.accepted = true;
                match (fit, hs) {
                    (None, _) => {}
                    (Some(f), None) => {
                        any_host.insert(f);
                    }
                    (Some(f), Some(v)) => hosts.entry(f).or_default().extend(v),
                }
            }
            if !ps.accepted {
                ps.why = Some(if !named {
                    (
                        "NoMatchingParent",
                        "the Gateway has no listener of the section name or port given".to_owned(),
                    )
                } else if !allowed {
                    ("NotAllowedByListeners", "no listener of the Gateway takes it: their protocols, states, or allowed routes' namespaces and kinds leave it out".to_owned())
                } else if !host_ok {
                    (
                        "NoMatchingListenerHostname",
                        "no listener's host name matches the route's".to_owned(),
                    )
                } else {
                    (
                        "NotAllowedByListeners",
                        format!("the listener carries {taken} already"),
                    )
                });
            }
            parents.push(ps);
        }
        let none = parents.is_empty();
        let accepted = parents.iter().any(|p| p.accepted);
        if let Some(st) = self.cur.as_mut() {
            st.parents = parents;
        }
        if none {
            return Vec::new();
        }
        let mut groups: BTreeMap<String, Attachment> = BTreeMap::new();
        let all: BTreeSet<&String> = hosts.keys().chain(any_host.iter()).collect();
        for fl in all {
            let hs = if any_host.contains(fl) {
                None
            } else {
                let mut v = hosts.get(fl).cloned().unwrap_or_default();
                v.sort();
                v.dedup();
                Some(v)
            };
            let k = hs.as_ref().map_or(String::new(), |v| v.join(","));
            groups
                .entry(k)
                .or_insert_with(|| Attachment {
                    listeners: Vec::new(),
                    hosts: hs,
                })
                .listeners
                .push(fl.clone());
        }
        if groups.is_empty() {
            let lower = kind.name().to_ascii_lowercase();
            self.problem(if accepted {
                format!(
                    "{lower} {key}: the listeners that take it are not served: see their status"
                )
            } else {
                format!(
                    "{lower} {key}: no listener of its Gateways takes it: their sections, ports, allowed routes or host names leave it out"
                )
            });
            return Vec::new();
        }
        groups.into_values().collect()
    }

    /// Gives akit's listener `name`'s connections for the server names `hosts`, or those naming
    /// none when `None`, to the route `owner`; or the route that has them already.
    fn claim(&mut self, name: &str, hosts: Option<&[String]>, owner: &str) -> Option<String> {
        let fl = self.listeners.get_mut(name)?;
        match hosts {
            None => {
                if !fl.fallback.is_empty() && fl.fallback != owner {
                    return Some(fl.fallback.clone());
                }
                owner.clone_into(&mut fl.fallback);
                None
            }
            Some(hs) => {
                for h in hs {
                    if let Some(other) = fl.names.get(&h.to_ascii_lowercase()) {
                        if other != owner {
                            return Some(other.clone());
                        }
                    }
                }
                for h in hs {
                    fl.names.insert(h.to_ascii_lowercase(), owner.to_owned());
                }
                None
            }
        }
    }

    /// Translates `r`, an HTTPRoute or GRPCRoute: each match of each rule is a route of akit's
    /// on the listeners it is attached to; a GRPCRoute's to its backends over h2c.
    fn http_route(&mut self, ix: &Index<'_>, kind: RouteKind, r: &Route) {
        let groups = self.attach(ix, kind, r, &r.hostnames);
        // A route taken only by listeners akit does not serve makes no routes of akit's, but
        // its backends are still resolved, for its status.
        if groups.is_empty() && !self.cur.as_ref().is_some_and(RouteState::accepted) {
            return;
        }
        let grpc = kind == RouteKind::Grpc;
        let (key, ns, name) = (r.meta.key(), &r.meta.namespace, &r.meta.name);
        let (lower, tag) = if grpc {
            ("grpcroute", ".grpc")
        } else {
            ("httproute", "")
        };
        let any_http = [HttpMatch::default()];
        let any_grpc = [GrpcMatch::default()];
        for (i, rule) in r.rules.iter().enumerate() {
            let r#where = format!("{lower} {key}: rule {i}");
            let b = self.backends(ix, &r#where, rule, grpc);
            let n = if grpc {
                rule.grpc_matches.len().max(1)
            } else {
                rule.matches.len().max(1)
            };
            for j in 0..n {
                for (gi, at) in groups.iter().enumerate() {
                    let mut rname = format!("{ns}.{name}{tag}.r{i}.m{j}");
                    if groups.len() > 1 {
                        let _ = write!(rname, ".g{gi}");
                    }
                    let w = format!("{where}: match {j}");
                    let out = if grpc {
                        let m = rule
                            .grpc_matches
                            .get(j)
                            .or_else(|| any_grpc.first())
                            .cloned()
                            .unwrap_or_default();
                        self.grpc_match(ix, &w, &m, rule, &b, at)
                    } else {
                        let m = rule
                            .matches
                            .get(j)
                            .or_else(|| any_http.first())
                            .cloned()
                            .unwrap_or_default();
                        self.http_match(ix, &w, &m, rule, &b, at)
                    };
                    if let Some(mut out) = out {
                        out.name = rname;
                        self.routes.push(out);
                    }
                }
            }
        }
    }

    /// Resolves the backendRefs of `rule`, at `where`, of the route being translated; `h2c`
    /// makes their upstreams speak HTTP/2 in the clear, as gRPC's do.
    fn backends(&mut self, ix: &Index<'_>, r#where: &str, rule: &Rule, h2c: bool) -> RuleBackends {
        let mut b = RuleBackends {
            splits: Vec::new(),
            status: 0,
        };
        let mut invalid = false;
        for (k, br) in rule.backend_refs.iter().enumerate() {
            let w = br.weight.unwrap_or(1);
            if w <= 0 {
                continue;
            }
            if !br.filters.is_empty() {
                self.left_out(format!(
                    "{where}: backend {k}: filters on a backend are not supported yet; they are left out"
                ));
            }
            match self.backend_upstream(ix, &br.r, "TCP", h2c) {
                Err(e) => {
                    self.bad_ref(&format!("{where}: backend {k}"), &e, "");
                    invalid = true;
                }
                Ok((name, true)) => b.splits.push((name, w.min(1000))),
                Ok((_, false)) => {}
            }
        }
        if b.splits.is_empty() {
            b.status = if invalid || rule.backend_refs.is_empty() {
                500
            } else {
                503
            };
        }
        b
    }

    /// Makes the upstream of the Service port `r` names, for the route being translated, and
    /// returns its name and whether it has endpoints ready. `proto` is the port's protocol, TCP
    /// or UDP; `h2c` makes the upstream speak HTTP/2 in the clear.
    fn backend_upstream(
        &mut self,
        ix: &Index<'_>,
        r: &ObjectRef,
        proto: &str,
        h2c: bool,
    ) -> Result<(String, bool), RefError> {
        let (from_kind, from) = match &self.cur {
            Some(st) => (st.kind.name(), st.obj.meta().namespace.clone()),
            None => ("", String::new()),
        };
        let (group, kind) = (or(r.group.as_deref(), ""), or(r.kind.as_deref(), "Service"));
        if !group.is_empty() || kind != "Service" {
            return Err(RefError::new(
                "InvalidKind",
                format!("{group}/{kind} is not a Service"),
            ));
        }
        let ns = or(r.namespace.as_deref(), &from).to_owned();
        if ns != from && !granted(ix, from_kind, &from, &ns, "Service", &r.name) {
            return Err(RefError::new(
                "RefNotPermitted",
                format!(
                    "service {ns}/{} is in another namespace, and no ReferenceGrant there allows it",
                    r.name
                ),
            ));
        }
        let Some(port) = r.port else {
            return Err(RefError::new(
                "UnsupportedValue",
                format!("service {ns}/{}: a port is needed", r.name),
            ));
        };
        self.service_upstream(ix, &ns, &r.name, port, proto, h2c)
    }

    /// Translates `m`, the match at `where` of an HTTPRoute's `rule`, into a route of akit's on
    /// attachment `at`; or `None`, saying why, when it cannot be.
    fn http_match(
        &mut self,
        ix: &Index<'_>,
        r#where: &str,
        m: &HttpMatch,
        rule: &Rule,
        b: &RuleBackends,
        at: &Attachment,
    ) -> Option<OutRoute> {
        let mut om = OutMatch {
            host: route_hosts(at.hosts.as_deref()),
            ..OutMatch::default()
        };
        let (path_type, path_value) = match &m.path {
            Some((t, v)) => (or(t.as_deref(), "PathPrefix"), or(v.as_deref(), "/")),
            None => ("PathPrefix", "/"),
        };
        let mut prefix = "";
        let path_class: i64 = match path_type {
            "Exact" | "PathPrefix" => {
                if !path_value.starts_with('/') {
                    self.left_out(format!(
                        "{where}: path {path_value:?} does not start with '/'"
                    ));
                    return None;
                }
                if path_type == "Exact" {
                    om.path = vec![path_value.to_owned()];
                    3
                } else {
                    prefix = path_value.trim_end_matches('/');
                    if !prefix.is_empty() {
                        om.path = vec![prefix.to_owned(), format!("{prefix}/*")];
                    }
                    2
                }
            }
            "RegularExpression" => {
                let re = format!("^(?:{path_value})$");
                if let Err(e) = akit_router::Matcher::path_regexp(&[&re]) {
                    self.left_out(format!(
                        "{where}: path {path_value:?} is not a regular expression akit takes: {e}"
                    ));
                    return None;
                }
                om.path_regexp = vec![re];
                1
            }
            other => {
                self.left_out(format!("{where}: path type {other} is not supported"));
                return None;
            }
        };
        for h in &m.headers {
            if or(h.typ.as_deref(), "Exact") != "Exact" {
                self.left_out(format!(
                    "{where}: header {}: match type {} is not supported yet",
                    h.name,
                    or(h.typ.as_deref(), "")
                ));
                return None;
            }
            om.header.insert(h.name.clone(), vec![h.value.clone()]);
        }
        for q in &m.query_params {
            if or(q.typ.as_deref(), "Exact") != "Exact" {
                self.left_out(format!(
                    "{where}: query parameter {}: match type {} is not supported yet",
                    q.name,
                    or(q.typ.as_deref(), "")
                ));
                return None;
            }
            om.query.insert(q.name.clone(), vec![q.value.clone()]);
        }
        if let Some(method) = &m.method {
            om.method = vec![method.clone()];
        }
        // The Gateway API's precedence, as one priority: the most specific host, then an exact
        // path, the longest prefix, a method, the most headers, the most query parameters.
        // Equal ones keep the order of the routes' creation, which the part is written in.
        let len = |n: usize, max: usize| i64::try_from(n.min(max)).unwrap_or(0);
        let mut priority = host_class(at)
            .saturating_mul(100_000_000)
            .saturating_add(path_class.saturating_mul(10_000_000))
            .saturating_add(len(path_value.len(), 9999).saturating_mul(1000))
            .saturating_add(len(m.headers.len(), 16).saturating_mul(30))
            .saturating_add(len(m.query_params.len(), 16));
        if m.method.is_some() {
            priority = priority.saturating_add(500);
        }
        let mut out = OutRoute {
            name: String::new(),
            listeners: at.listeners.clone(),
            priority,
            m: (!om.is_empty()).then_some(om),
            middleware: Vec::new(),
            handler: String::new(),
        };
        self.handle(ix, r#where, &mut out, &rule.filters, b, path_type, prefix);
        Some(out)
    }

    /// Translates `m`, the match at `where` of a GRPCRoute's `rule`, into a route of akit's on
    /// attachment `at`; or `None`, saying why, when it cannot be. A gRPC method is the path
    /// `/<service>/<method>`.
    fn grpc_match(
        &mut self,
        ix: &Index<'_>,
        r#where: &str,
        m: &GrpcMatch,
        rule: &Rule,
        b: &RuleBackends,
        at: &Attachment,
    ) -> Option<OutRoute> {
        let mut om = OutMatch {
            host: route_hosts(at.hosts.as_deref()),
            ..OutMatch::default()
        };
        let (mut service, mut method) = ("", "");
        if let Some((typ, svc, meth)) = &m.method {
            service = or(svc.as_deref(), "");
            method = or(meth.as_deref(), "");
            match or(typ.as_deref(), "Exact") {
                "Exact" => {
                    if service.contains('/') || method.contains('/') {
                        self.left_out(format!("{where}: a service or method holds '/'"));
                        return None;
                    }
                    if !service.is_empty() && !method.is_empty() {
                        om.path = vec![format!("/{service}/{method}")];
                    } else if !service.is_empty() {
                        om.path = vec![format!("/{service}/*")];
                    } else if !method.is_empty() {
                        om.path_regexp = vec![format!("^/[^/]+/{}$", quote_meta(method))];
                    }
                }
                "RegularExpression" => {
                    let part = |re: &str| {
                        if re.is_empty() {
                            "[^/]+".to_owned()
                        } else {
                            format!("(?:{re})")
                        }
                    };
                    let re = format!("^/{}/{}$", part(service), part(method));
                    if let Err(e) = akit_router::Matcher::path_regexp(&[&re]) {
                        self.left_out(format!(
                            "{where}: method {method:?} of service {service:?} is not a regular expression akit takes: {e}"
                        ));
                        return None;
                    }
                    om.path_regexp = vec![re];
                }
                other => {
                    self.left_out(format!(
                        "{where}: method match type {other} is not supported"
                    ));
                    return None;
                }
            }
        }
        for h in &m.headers {
            if or(h.typ.as_deref(), "Exact") != "Exact" {
                self.left_out(format!(
                    "{where}: header {}: match type {} is not supported yet",
                    h.name,
                    or(h.typ.as_deref(), "")
                ));
                return None;
            }
            om.header.insert(h.name.clone(), vec![h.value.clone()]);
        }
        // The Gateway API's precedence for GRPCRoutes, as one priority: the most specific host,
        // then the longest service, the longest method, the most headers.
        let len = |n: usize, max: usize| i64::try_from(n.min(max)).unwrap_or(0);
        let priority = host_class(at)
            .saturating_mul(100_000_000)
            .saturating_add(len(service.len(), 4999).saturating_mul(10_000))
            .saturating_add(len(method.len(), 99).saturating_mul(100))
            .saturating_add(len(m.headers.len(), 16).saturating_mul(6));
        let mut out = OutRoute {
            name: String::new(),
            listeners: at.listeners.clone(),
            priority,
            m: (!om.is_empty()).then_some(om),
            middleware: Vec::new(),
            handler: String::new(),
        };
        self.handle(ix, r#where, &mut out, &rule.filters, b, "", "");
        Some(out)
    }

    /// Gives `out` the middleware of `filters` and a handler sending to `b`, for a match of
    /// `path_type` whose prefix, for PathPrefix, is `prefix`.
    #[allow(clippy::too_many_arguments)]
    fn handle(
        &mut self,
        ix: &Index<'_>,
        r#where: &str,
        out: &mut OutRoute,
        filters: &[Filter],
        b: &RuleBackends,
        path_type: &str,
        prefix: &str,
    ) {
        let grpc = self
            .cur
            .as_ref()
            .is_some_and(|st| st.kind == RouteKind::Grpc);
        let (mut req, mut resp) = (HeaderOps::default(), HeaderOps::default());
        let mut host_header: Option<String> = None;
        // A mirror's upstream, and its percent as JSON.
        let mut mirror: Option<(String, Option<String>)> = None;
        let mut fail = 0;
        for f in filters {
            match f.typ.as_str() {
                "RequestHeaderModifier" => {
                    if let Some(h) = &f.request_header_modifier {
                        req.add(h);
                    }
                }
                "ResponseHeaderModifier" => {
                    if let Some(h) = &f.response_header_modifier {
                        resp.add(h);
                    }
                }
                "URLRewrite" => {
                    let Some(rw) = &f.url_rewrite else {
                        continue;
                    };
                    if let Some(h) = &rw.hostname {
                        host_header = Some(h.clone());
                    }
                    match rewrite(rw.path.as_ref(), path_type, prefix) {
                        Err(e) => {
                            self.left_out(format!("{where}: URLRewrite: {e}"));
                            fail = 500;
                        }
                        Ok(Some(mw)) => out.middleware.push(mw),
                        Ok(None) => {}
                    }
                }
                "RequestRedirect" => {
                    let Some(rd) = &f.request_redirect else {
                        continue;
                    };
                    match redirect(rd, path_type, prefix) {
                        Err(e) => {
                            self.left_out(format!("{where}: RequestRedirect: {e}"));
                            fail = 500;
                        }
                        Ok(mw) => out.middleware.push(mw),
                    }
                }
                "RequestMirror" => {
                    let Some(rm) = &f.request_mirror else {
                        continue;
                    };
                    if mirror.is_some() {
                        self.left_out(format!(
                            "{where}: only one RequestMirror is supported; the others are left out"
                        ));
                        continue;
                    }
                    match self.backend_upstream(ix, &rm.backend_ref, "TCP", grpc) {
                        Err(e) => {
                            self.bad_ref(
                                &format!("{where}: RequestMirror"),
                                &e,
                                "; no copies are sent",
                            );
                        }
                        Ok((_, false)) => self.problem(format!(
                            "{where}: RequestMirror: it has no endpoints ready; no copies are sent"
                        )),
                        Ok((name, true)) => {
                            let percent = match (rm.percent, rm.fraction) {
                                (Some(p), _) => Some(p.to_string()),
                                (None, Some((num, den))) => {
                                    let den = den.unwrap_or(100);
                                    #[allow(clippy::cast_precision_loss)]
                                    (den > 0)
                                        .then(|| format!("{}", 100.0 * num as f64 / den as f64))
                                }
                                (None, None) => None,
                            };
                            mirror = Some((name, percent));
                        }
                    }
                }
                other => {
                    self.left_out(format!(
                        "{where}: filter {other} is not supported; the rule answers 500"
                    ));
                    fail = 500;
                }
            }
        }
        if !req.is_empty() || !resp.is_empty() {
            let mut mw = String::from(r#"{"module":"headers","params":{"#);
            if !req.is_empty() {
                mw.push_str(r#""request":"#);
                req.write(&mut mw);
            }
            if !resp.is_empty() {
                if !req.is_empty() {
                    mw.push(',');
                }
                mw.push_str(r#""response":"#);
                resp.write(&mut mw);
            }
            mw.push_str("}}");
            out.middleware.insert(0, mw);
        }
        if fail != 0 {
            out.middleware.clear();
            out.handler = respond(fail);
            return;
        }
        if b.splits.is_empty() {
            out.handler = respond(b.status);
            return;
        }
        let mut h = String::from(r#"{"module":"proxy","params":{"#);
        if let Some(host) = &host_header {
            h.push_str(r#""host_header":"#);
            write_string(&mut h, host);
            h.push(',');
        }
        if let Some((up, percent)) = &mirror {
            h.push_str(r#""mirror":{"#);
            if let Some(p) = percent {
                let _ = write!(h, r#""percent":{p},"#);
            }
            h.push_str(r#""upstream":"#);
            write_string(&mut h, up);
            h.push_str("},");
        }
        match b.splits.as_slice() {
            [(up, _)] => {
                h.push_str(r#""upstream":"#);
                write_string(&mut h, up);
            }
            splits => {
                h.push_str(r#""upstreams":["#);
                for (i, (up, w)) in splits.iter().enumerate() {
                    if i > 0 {
                        h.push(',');
                    }
                    h.push_str(r#"{"upstream":"#);
                    write_string(&mut h, up);
                    let _ = write!(h, r#","weight":{w}}}"#);
                }
                h.push(']');
            }
        }
        h.push_str("}}");
        out.handler = h;
    }

    /// Translates `r`, a TLSRoute, TCPRoute or UDPRoute as `kind` says: a service of akit's on
    /// the listeners it is attached to, to its backends; for a TLSRoute, the connections whose
    /// ClientHello names its host names.
    fn l4_route(&mut self, ix: &Index<'_>, kind: RouteKind, r: &Route) {
        let hostnames: &[String] = if kind == RouteKind::Tls {
            &r.hostnames
        } else {
            &[]
        };
        let groups = self.attach(ix, kind, r, hostnames);
        if groups.is_empty() && !self.cur.as_ref().is_some_and(RouteState::accepted) {
            return;
        }
        let r#where = format!("{} {}", kind.name().to_ascii_lowercase(), r.meta.key());
        let Some(rule) = r.rules.first() else {
            let e = RefError::new(
                "BackendNotFound",
                "it has no rules, and so no backends".to_owned(),
            );
            self.bad_ref(&r#where, &e, "");
            return;
        };
        if r.rules.len() > 1 {
            self.left_out(format!(
                "{where}: only its first rule is served; the others are left out"
            ));
        }
        let (proto, tag) = match kind {
            RouteKind::Udp => ("UDP", "udp"),
            RouteKind::Tcp => ("TCP", "tcp"),
            _ => ("TCP", "tls"),
        };
        let base = format!("{}.{}.{tag}", r.meta.namespace, r.meta.name);
        let Some(upstream) = self.l4_upstream(ix, &format!("{where}: rule 0"), &base, rule, proto)
        else {
            return;
        };
        let many = groups.len() > 1;
        for (gi, at) in groups.into_iter().enumerate() {
            let mut name = base.clone();
            if many {
                let _ = write!(name, ".g{gi}");
            }
            for l in &at.listeners {
                if let Some(fl) = self.listeners.get_mut(l) {
                    fl.services = fl.services.saturating_add(1);
                }
            }
            self.services.push(OutService {
                name,
                listeners: at.listeners,
                upstream: upstream.clone(),
                server_names: at.hosts.unwrap_or_default(),
            });
        }
    }

    /// The upstream of the backends of `rule`, at `where`, whose ports are of protocol `proto`:
    /// one Service port's own, or one named `name` of several Services' endpoints, each
    /// weighted by its Service's weight shared among them; `None` when none has endpoints
    /// ready.
    fn l4_upstream(
        &mut self,
        ix: &Index<'_>,
        r#where: &str,
        name: &str,
        rule: &Rule,
        proto: &str,
    ) -> Option<String> {
        // Each backend's upstream and weight.
        let mut bs: Vec<(String, i64)> = Vec::new();
        for (k, br) in rule.backend_refs.iter().enumerate() {
            let w = br.weight.unwrap_or(1);
            if w <= 0 {
                continue;
            }
            match self.backend_upstream(ix, &br.r, proto, false) {
                Err(e) => self.bad_ref(&format!("{where}: backend {k}"), &e, ""),
                Ok((up, true)) => bs.push((up, w)),
                Ok((_, false)) => {}
            }
        }
        // Each backend's addresses, and its weight shared among them.
        let shares: Vec<(Vec<String>, f64)> = bs
            .iter()
            .filter_map(|(up, w)| {
                let addrs = self.upstreams.get(up)?.addresses.clone();
                #[allow(clippy::cast_precision_loss)]
                let share = *w as f64 / addrs.len().max(1) as f64;
                Some((addrs, share))
            })
            .collect();
        match bs.as_slice() {
            [] => {
                self.problem(format!(
                    "{where}: no backend has endpoints ready; its connections are closed"
                ));
                return None;
            }
            [(up, _)] => return Some(up.clone()),
            _ => {}
        }
        let most = shares.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
        let mut u = OutUpstream {
            addresses: Vec::new(),
            weights: Vec::new(),
            h2c: false,
        };
        let mut at: BTreeMap<String, usize> = BTreeMap::new();
        for (addrs, share) in &shares {
            #[allow(clippy::cast_possible_truncation)]
            let w = ((1000.0 * share / most).round() as i64).max(1);
            for a in addrs {
                if let Some(i) = at.get(a) {
                    if let Some(x) = u.weights.get_mut(*i) {
                        *x = x.saturating_add(w).min(1000);
                    }
                    continue;
                }
                at.insert(a.clone(), u.addresses.len());
                u.addresses.push(a.clone());
                u.weights.push(w);
            }
        }
        self.upstreams.insert(name.to_owned(), u);
        Some(name.to_owned())
    }
}

/// Whether listener `gl`, of a Gateway in namespace `gns`, takes a route of `kind` in namespace
/// `rns`: its kinds, and its allowedRoutes' namespaces.
fn allowed_by(ix: &Index<'_>, gl: &GwListener, gns: &str, kind: RouteKind, rns: &str) -> bool {
    if !gl.kinds.iter().any(|k| k.kind == kind.name()) {
        return false;
    }
    let (from, sel) = match gl
        .l
        .allowed_routes
        .as_ref()
        .and_then(|a| a.namespaces.as_ref())
    {
        Some((f, s)) => (if f.is_empty() { "Same" } else { f.as_str() }, s.as_ref()),
        None => ("Same", None),
    };
    match from {
        "All" => true,
        "Selector" => match (ix.namespace.get(rns), sel) {
            (Some(ns), Some(sel)) => selects(sel, &ns.meta.labels),
            _ => false,
        },
        _ => rns == gns,
    }
}

/// Whether `sel` selects `labels`.
pub(crate) fn selects(sel: &LabelSelector, labels: &[(String, String)]) -> bool {
    let get = |k: &str| labels.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    for (k, v) in &sel.match_labels {
        if get(k) != Some(v.as_str()) {
            return false;
        }
    }
    for e in &sel.match_expressions {
        let v = get(&e.key);
        let in_values = v.is_some_and(|v| e.values.iter().any(|x| x == v));
        let ok = match e.operator.as_str() {
            "In" => in_values,
            "NotIn" => !in_values,
            "Exists" => v.is_some(),
            "DoesNotExist" => v.is_none(),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// The host names a route of `hostnames` is served for on a listener of host name `listener`:
/// `Some(None)` for any, and `None` for none.
pub(crate) fn intersect(
    listener: Option<&str>,
    hostnames: &[String],
) -> Option<Option<Vec<String>>> {
    let l = listener.unwrap_or("");
    match (l.is_empty(), hostnames.is_empty()) {
        (true, true) => return Some(None),
        (true, false) => return Some(Some(hostnames.to_vec())),
        (false, true) => return Some(Some(vec![l.to_owned()])),
        (false, false) => {}
    }
    let mut out = Vec::new();
    for h in hostnames {
        if host_matches(l, h) {
            out.push(h.clone());
        } else if host_matches(h, l) {
            out.push(l.to_owned());
        }
    }
    (!out.is_empty()).then_some(Some(out))
}

/// The host names of a route of akit's for the Gateway API's `hosts`: its wildcards stand for
/// one label or more, as akit's `**.` wildcards do.
fn route_hosts(hosts: Option<&[String]>) -> Vec<String> {
    hosts.map_or_else(Vec::new, |hs| {
        hs.iter()
            .map(|h| {
                if h.starts_with("*.") {
                    format!("*{h}")
                } else {
                    h.clone()
                }
            })
            .collect()
    })
}

/// How specific an attachment's hosts are: 2 for a name, 1 for a wildcard, 0 for any.
fn host_class(at: &Attachment) -> i64 {
    at.hosts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|h| if h.starts_with('*') { 1 } else { 2 })
        .max()
        .unwrap_or(0)
}

/// Whether a ReferenceGrant in namespace `to` lets objects of the Gateway API's kind
/// `from_kind` in namespace `from` refer to the core object of kind `to_kind` named `name`.
fn granted(
    ix: &Index<'_>,
    from_kind: &str,
    from: &str,
    to: &str,
    to_kind: &str,
    name: &str,
) -> bool {
    ix.grants.iter().any(|g| {
        g.meta.namespace == to
            && g.from
                .iter()
                .any(|(gr, k, ns)| gr == GATEWAY_GROUP && k == from_kind && ns == from)
            && g.to.iter().any(|(gr, k, n)| {
                gr.is_empty() && k == to_kind && n.as_ref().is_none_or(|n| n == name)
            })
    })
}

/// Go's `regexp.QuoteMeta`.
fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The changes of one side of the headers middleware.
#[derive(Debug, Default)]
struct HeaderOps {
    set: BTreeMap<String, String>,
    add: BTreeMap<String, Vec<String>>,
    delete: Vec<String>,
}

impl HeaderOps {
    fn is_empty(&self) -> bool {
        self.set.is_empty() && self.add.is_empty() && self.delete.is_empty()
    }

    fn add(&mut self, f: &HeaderFilter) {
        for (n, v) in &f.set {
            self.set.insert(n.clone(), v.clone());
        }
        for (n, v) in &f.add {
            self.add.entry(n.clone()).or_default().push(v.clone());
        }
        self.delete.extend(f.remove.iter().cloned());
    }

    /// As JSON, its keys in order.
    fn write(&self, b: &mut String) {
        b.push('{');
        let mut first = true;
        let mut key = |b: &mut String, k: &str| {
            if !first {
                b.push(',');
            }
            first = false;
            let _ = write!(b, r#""{k}":"#);
        };
        if !self.add.is_empty() {
            key(b, "add");
            string_lists(b, &self.add);
        }
        if !self.delete.is_empty() {
            key(b, "delete");
            strings(b, &self.delete);
        }
        if !self.set.is_empty() {
            key(b, "set");
            b.push('{');
            for (i, (n, v)) in self.set.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_string(b, n);
                b.push(':');
                write_string(b, v);
            }
            b.push('}');
        }
        b.push('}');
    }
}

/// The rewrite middleware for a URLRewrite's path `p`, on a match of `path_type` whose prefix,
/// for PathPrefix, is `prefix`; `None` for no change.
pub(crate) fn rewrite(
    p: Option<&PathModifier>,
    path_type: &str,
    prefix: &str,
) -> Result<Option<String>, String> {
    let Some(p) = p else {
        return Ok(None);
    };
    let mut b = String::from(r#"{"module":"rewrite","params":{"#);
    match p.typ.as_str() {
        "ReplaceFullPath" => {
            b.push_str(r#""regexp":"^.*$","replacement":"#);
            write_string(
                &mut b,
                &or(p.replace_full_path.as_deref(), "/").replace('$', "$$"),
            );
        }
        "ReplacePrefixMatch" => {
            if path_type != "PathPrefix" {
                return Err("ReplacePrefixMatch needs a PathPrefix match".to_owned());
            }
            let add = or(p.replace_prefix_match.as_deref(), "/").trim_end_matches('/');
            if add.is_empty() && prefix.is_empty() {
                return Ok(None);
            }
            if !add.is_empty() {
                b.push_str(r#""add_prefix":"#);
                write_string(&mut b, add);
            }
            if !prefix.is_empty() {
                if !add.is_empty() {
                    b.push(',');
                }
                b.push_str(r#""strip_prefix":["#);
                write_string(&mut b, prefix);
                b.push(']');
            }
        }
        other => return Err(format!("path type {other:?} is not supported")),
    }
    b.push_str("}}");
    Ok(Some(b))
}

/// The redirect middleware for a RequestRedirect, on a match of `path_type` whose prefix, for
/// PathPrefix, is `prefix`.
pub(crate) fn redirect(rd: &Redirect, path_type: &str, prefix: &str) -> Result<String, String> {
    let scheme = or(rd.scheme.as_deref(), "{scheme}");
    let host = match (&rd.hostname, rd.port) {
        (Some(h), Some(p)) => join_host_port(h, p),
        (Some(h), None) => h.clone(),
        (None, Some(_)) => {
            return Err("a port without a hostname is not supported yet".to_owned());
        }
        (None, None) => "{host}".to_owned(),
    };
    let mut regexp = None;
    let mut path = "{uri}".to_owned();
    if let Some(p) = &rd.path {
        match p.typ.as_str() {
            "ReplaceFullPath" => or(p.replace_full_path.as_deref(), "/").clone_into(&mut path),
            "ReplacePrefixMatch" => {
                if path_type != "PathPrefix" {
                    return Err("ReplacePrefixMatch needs a PathPrefix match".to_owned());
                }
                regexp = Some(format!("^{}(/.*)?$", quote_meta(prefix)));
                path = format!(
                    "{}$1",
                    or(p.replace_prefix_match.as_deref(), "/").trim_end_matches('/')
                );
            }
            other => return Err(format!("path type {other:?} is not supported")),
        }
    }
    let mut b = String::from(r#"{"module":"redirect","params":{"#);
    if let Some(re) = &regexp {
        b.push_str(r#""regexp":"#);
        write_string(&mut b, re);
        b.push(',');
    }
    let _ = write!(b, r#""status":{},"to":"#, rd.status_code.unwrap_or(302));
    write_string(&mut b, &format!("{scheme}://{host}{path}"));
    b.push_str("}}");
    Ok(b)
}
