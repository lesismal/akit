//! The fields the provider reads of the kinds it watches, as the API server writes them in JSON;
//! everything else is left out (fit's `types.go`): Ingresses and their classes (I59), the Gateway
//! API's kinds and the statuses akit writes of them (I60), and the core kinds both use. An object
//! is decoded from its JSON once, when listed or when its watch event comes, and kept so.

use akit_config::value::Value;

/// An object's metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub(crate) name: String,
    pub(crate) namespace: String,
    pub(crate) resource_version: String,
    pub(crate) generation: i64,
    pub(crate) creation_timestamp: String,
    pub(crate) labels: Vec<(String, String)>,
    pub(crate) annotations: Vec<(String, String)>,
}

impl Meta {
    /// How a store keys an object: its namespace and name.
    pub(crate) fn key(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.namespace, self.name)
        }
    }

    pub(crate) fn label(&self, k: &str) -> Option<&str> {
        find(&self.labels, k)
    }

    pub(crate) fn annotation(&self, k: &str) -> Option<&str> {
        find(&self.annotations, k)
    }
}

fn find<'a>(kv: &'a [(String, String)], k: &str) -> Option<&'a str> {
    kv.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// A networking.k8s.io/v1 IngressClass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressClass {
    pub(crate) meta: Meta,
    pub(crate) controller: String,
}

/// A networking.k8s.io/v1 Ingress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ingress {
    pub(crate) meta: Meta,
    pub(crate) class_name: Option<String>,
    pub(crate) default_backend: Option<Backend>,
    pub(crate) tls: Vec<IngressTls>,
    pub(crate) rules: Vec<IngressRule>,
}

/// One of an Ingress's `tls`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressTls {
    pub(crate) hosts: Vec<String>,
    pub(crate) secret_name: String,
}

/// An Ingress's paths for a host, or for any; `paths` is `None` without `http`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressRule {
    pub(crate) host: String,
    pub(crate) paths: Option<Vec<IngressPath>>,
}

/// A path of a rule and its backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressPath {
    pub(crate) path: String,
    pub(crate) path_type: String,
    pub(crate) backend: Backend,
}

/// An Ingress's backend: a Service's port, by number or name, or another resource (`service`
/// `None`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Backend {
    pub(crate) service: Option<ServiceRef>,
}

/// A backend's Service and port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServiceRef {
    pub(crate) name: String,
    pub(crate) port_name: String,
    /// 0 when given by name.
    pub(crate) port_number: i64,
}

/// A core/v1 Service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) meta: Meta,
    pub(crate) typ: String,
    pub(crate) cluster_ip: String,
    pub(crate) external_name: String,
    pub(crate) ports: Vec<ServicePort>,
}

/// One of a Service's ports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServicePort {
    pub(crate) name: String,
    pub(crate) protocol: String,
    pub(crate) port: i64,
    pub(crate) app_protocol: Option<String>,
}

/// A discovery.k8s.io/v1 EndpointSlice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EndpointSlice {
    pub(crate) meta: Meta,
    pub(crate) endpoints: Vec<Endpoint>,
    pub(crate) ports: Vec<SlicePort>,
}

/// One of a slice's endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) addresses: Vec<String>,
    pub(crate) ready: Option<bool>,
}

/// One of a slice's ports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SlicePort {
    pub(crate) name: Option<String>,
    pub(crate) protocol: Option<String>,
    pub(crate) port: Option<i64>,
}

/// A core/v1 Secret, of type `kubernetes.io/tls`: its `tls.crt` and `tls.key`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Secret {
    pub(crate) meta: Meta,
    pub(crate) typ: String,
    pub(crate) crt: Vec<u8>,
    pub(crate) key: Vec<u8>,
}

/// A core/v1 Namespace, for its labels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Namespace {
    pub(crate) meta: Meta,
}

/// A metav1.Condition, as the Gateway API's statuses hold them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Condition {
    pub(crate) typ: String,
    pub(crate) status: String,
    /// 0 when left out.
    pub(crate) observed_generation: i64,
    pub(crate) last_transition_time: String,
    pub(crate) reason: String,
    pub(crate) message: String,
}

/// A gateway.networking.k8s.io/v1 GatewayClass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GatewayClass {
    pub(crate) meta: Meta,
    pub(crate) controller_name: String,
    pub(crate) conditions: Vec<Condition>,
}

/// A gateway.networking.k8s.io/v1 Gateway.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Gateway {
    pub(crate) meta: Meta,
    pub(crate) class_name: String,
    pub(crate) listeners: Vec<Listener>,
    pub(crate) status: GatewayStatus,
}

/// The part of a Gateway's status akit writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GatewayStatus {
    pub(crate) addresses: Vec<GatewayAddress>,
    pub(crate) conditions: Vec<Condition>,
    pub(crate) listeners: Vec<ListenerStatus>,
}

/// Where a Gateway is reached.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GatewayAddress {
    pub(crate) typ: Option<String>,
    pub(crate) value: String,
}

/// The status of one of a Gateway's listeners.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ListenerStatus {
    pub(crate) name: String,
    pub(crate) supported_kinds: Vec<RouteGroupKind>,
    pub(crate) attached_routes: i64,
    pub(crate) conditions: Vec<Condition>,
}

/// One of a Gateway's listeners.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Listener {
    pub(crate) name: String,
    pub(crate) hostname: Option<String>,
    /// 0 when left out or not a number.
    pub(crate) port: i64,
    pub(crate) protocol: String,
    pub(crate) tls: Option<ListenerTls>,
    pub(crate) allowed_routes: Option<AllowedRoutes>,
}

/// How a listener serves TLS: terminated, with the certificates of Secrets, or passed through.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ListenerTls {
    pub(crate) mode: Option<String>,
    pub(crate) certificate_refs: Vec<ObjectRef>,
}

/// A reference to an object, by default in the referrer's namespace: a certificate's Secret, or
/// a backend's Service and port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ObjectRef {
    pub(crate) group: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) name: String,
    pub(crate) namespace: Option<String>,
    /// A backend's port.
    pub(crate) port: Option<i64>,
}

/// A kind of route.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RouteGroupKind {
    pub(crate) group: Option<String>,
    pub(crate) kind: String,
}

/// Which routes may attach to a listener.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AllowedRoutes {
    /// `namespaces.from`, and its selector; `None` without `namespaces`.
    pub(crate) namespaces: Option<(String, Option<LabelSelector>)>,
    pub(crate) kinds: Vec<RouteGroupKind>,
}

/// A metav1.LabelSelector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LabelSelector {
    pub(crate) match_labels: Vec<(String, String)>,
    pub(crate) match_expressions: Vec<Requirement>,
}

/// One of a selector's expressions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Requirement {
    pub(crate) key: String,
    pub(crate) operator: String,
    pub(crate) values: Vec<String>,
}

/// The Gateway and maybe the listener a route attaches to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParentRef {
    pub(crate) group: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) namespace: Option<String>,
    pub(crate) name: String,
    pub(crate) section_name: Option<String>,
    pub(crate) port: Option<i64>,
}

/// A route's status on one of its parents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RouteParentStatus {
    pub(crate) parent_ref: ParentRef,
    pub(crate) controller_name: String,
    pub(crate) conditions: Vec<Condition>,
}

/// A gateway.networking.k8s.io HTTPRoute, GRPCRoute, TLSRoute, TCPRoute or UDPRoute: its
/// parents, host names and rules, and its status, an entry for each of its parents, each
/// written by the controller of that parent: akit reads its own, and keeps the others' as they
/// are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Route {
    pub(crate) meta: Meta,
    pub(crate) parent_refs: Vec<ParentRef>,
    pub(crate) hostnames: Vec<String>,
    pub(crate) rules: Vec<Rule>,
    pub(crate) parents: Vec<Value>,
}

/// One of a route's rules: an HTTPRoute's, a GRPCRoute's (its matches in `grpc_matches`, and
/// those of its filters gRPC has), or a TLSRoute's, TCPRoute's or UDPRoute's (backends alone).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) matches: Vec<HttpMatch>,
    pub(crate) grpc_matches: Vec<GrpcMatch>,
    pub(crate) filters: Vec<Filter>,
    pub(crate) backend_refs: Vec<BackendRef>,
}

/// What a request must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HttpMatch {
    /// `path`'s type and value; `None` without `path`.
    pub(crate) path: Option<(Option<String>, Option<String>)>,
    pub(crate) headers: Vec<MatchValue>,
    pub(crate) query_params: Vec<MatchValue>,
    pub(crate) method: Option<String>,
}

/// What a gRPC request must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GrpcMatch {
    /// `method`'s type, service and method; `None` without `method`.
    pub(crate) method: Option<(Option<String>, Option<String>, Option<String>)>,
    pub(crate) headers: Vec<MatchValue>,
}

/// A header's or query parameter's match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MatchValue {
    pub(crate) typ: Option<String>,
    pub(crate) name: String,
    pub(crate) value: String,
}

/// A filter, changing requests or responses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) typ: String,
    pub(crate) request_header_modifier: Option<HeaderFilter>,
    pub(crate) response_header_modifier: Option<HeaderFilter>,
    pub(crate) request_redirect: Option<Redirect>,
    pub(crate) url_rewrite: Option<UrlRewrite>,
    pub(crate) request_mirror: Option<Mirror>,
}

/// Header fields set, added and removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HeaderFilter {
    pub(crate) set: Vec<(String, String)>,
    pub(crate) add: Vec<(String, String)>,
    pub(crate) remove: Vec<String>,
}

/// How a request's path is replaced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PathModifier {
    pub(crate) typ: String,
    pub(crate) replace_full_path: Option<String>,
    pub(crate) replace_prefix_match: Option<String>,
}

/// An answer with a redirect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Redirect {
    pub(crate) scheme: Option<String>,
    pub(crate) hostname: Option<String>,
    pub(crate) path: Option<PathModifier>,
    pub(crate) port: Option<i64>,
    pub(crate) status_code: Option<i64>,
}

/// A change of the request's host and path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UrlRewrite {
    pub(crate) hostname: Option<String>,
    pub(crate) path: Option<PathModifier>,
}

/// Requests copied to a backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Mirror {
    pub(crate) backend_ref: ObjectRef,
    pub(crate) percent: Option<i64>,
    /// `fraction`'s numerator and denominator.
    pub(crate) fraction: Option<(i64, Option<i64>)>,
}

/// A rule's backend, with its weight and, for HTTP, its own filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BackendRef {
    pub(crate) r: ObjectRef,
    pub(crate) weight: Option<i64>,
    pub(crate) filters: Vec<Filter>,
}

/// A gateway.networking.k8s.io ReferenceGrant: what in its namespace may be referred to from
/// others.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReferenceGrant {
    pub(crate) meta: Meta,
    /// Each `from`'s group, kind and namespace.
    pub(crate) from: Vec<(String, String, String)>,
    /// Each `to`'s group, kind and name.
    pub(crate) to: Vec<(String, String, Option<String>)>,
}

/// An object of one of the kinds watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Object {
    IngressClass(IngressClass),
    Ingress(Ingress),
    Service(Service),
    Slice(EndpointSlice),
    Secret(Secret),
    Namespace(Namespace),
    GatewayClass(GatewayClass),
    Gateway(Gateway),
    Route(Route),
    Grant(ReferenceGrant),
}

impl Object {
    pub(crate) fn meta(&self) -> &Meta {
        match self {
            Object::IngressClass(o) => &o.meta,
            Object::Ingress(o) => &o.meta,
            Object::Service(o) => &o.meta,
            Object::Slice(o) => &o.meta,
            Object::Secret(o) => &o.meta,
            Object::Namespace(o) => &o.meta,
            Object::GatewayClass(o) => &o.meta,
            Object::Gateway(o) => &o.meta,
            Object::Route(o) => &o.meta,
            Object::Grant(o) => &o.meta,
        }
    }
}

/// The kinds of object, as [`decode`] reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    IngressClass,
    Ingress,
    Service,
    Slice,
    Secret,
    Namespace,
    GatewayClass,
    Gateway,
    HttpRoute,
    GrpcRoute,
    TlsRoute,
    TcpRoute,
    UdpRoute,
    Grant,
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn opt_s(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn int(v: &Value, k: &str) -> Option<i64> {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().ok(),
        _ => None,
    }
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    arr(v, k)
        .iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn map(v: &Value, k: &str) -> Vec<(String, String)> {
    match v.get(k) {
        Some(Value::Object(ms)) => ms
            .iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) => Some((k.clone(), s.clone())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

const NULL: Value = Value::Null;

fn obj<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&NULL)
}

pub(crate) fn meta(v: &Value) -> Meta {
    let m = obj(v, "metadata");
    Meta {
        name: s(m, "name"),
        namespace: s(m, "namespace"),
        resource_version: s(m, "resourceVersion"),
        generation: int(m, "generation").unwrap_or(0),
        creation_timestamp: s(m, "creationTimestamp"),
        labels: map(m, "labels"),
        annotations: map(m, "annotations"),
    }
}

fn backend(v: &Value) -> Backend {
    let svc = v.get("service").filter(|s| matches!(s, Value::Object(_)));
    Backend {
        service: svc.map(|svc| {
            let port = obj(svc, "port");
            ServiceRef {
                name: s(svc, "name"),
                port_name: s(port, "name"),
                port_number: int(port, "number").unwrap_or(0),
            }
        }),
    }
}

pub(crate) fn conditions(v: &Value, k: &str) -> Vec<Condition> {
    arr(v, k)
        .iter()
        .map(|c| Condition {
            typ: s(c, "type"),
            status: s(c, "status"),
            observed_generation: int(c, "observedGeneration").unwrap_or(0),
            last_transition_time: s(c, "lastTransitionTime"),
            reason: s(c, "reason"),
            message: s(c, "message"),
        })
        .collect()
}

fn group_kinds(v: &Value, k: &str) -> Vec<RouteGroupKind> {
    arr(v, k)
        .iter()
        .map(|g| RouteGroupKind {
            group: opt_s(g, "group"),
            kind: s(g, "kind"),
        })
        .collect()
}

pub(crate) fn gateway_status(st: &Value) -> GatewayStatus {
    GatewayStatus {
        addresses: arr(st, "addresses")
            .iter()
            .map(|a| GatewayAddress {
                typ: opt_s(a, "type"),
                value: s(a, "value"),
            })
            .collect(),
        conditions: conditions(st, "conditions"),
        listeners: arr(st, "listeners")
            .iter()
            .map(|l| ListenerStatus {
                name: s(l, "name"),
                supported_kinds: group_kinds(l, "supportedKinds"),
                attached_routes: int(l, "attachedRoutes").unwrap_or(0),
                conditions: conditions(l, "conditions"),
            })
            .collect(),
    }
}

fn selector(v: &Value) -> LabelSelector {
    LabelSelector {
        match_labels: map(v, "matchLabels"),
        match_expressions: arr(v, "matchExpressions")
            .iter()
            .map(|e| Requirement {
                key: s(e, "key"),
                operator: s(e, "operator"),
                values: strings(e, "values"),
            })
            .collect(),
    }
}

fn is_obj(v: &Value) -> bool {
    matches!(v, Value::Object(_))
}

fn listener(l: &Value) -> Listener {
    Listener {
        name: s(l, "name"),
        hostname: opt_s(l, "hostname"),
        port: int(l, "port").unwrap_or(0),
        protocol: s(l, "protocol"),
        tls: l.get("tls").filter(|t| is_obj(t)).map(|t| ListenerTls {
            mode: opt_s(t, "mode"),
            certificate_refs: arr(t, "certificateRefs").iter().map(object_ref).collect(),
        }),
        allowed_routes: l
            .get("allowedRoutes")
            .filter(|a| is_obj(a))
            .map(|a| AllowedRoutes {
                namespaces: a.get("namespaces").filter(|n| is_obj(n)).map(|n| {
                    (
                        s(n, "from"),
                        n.get("selector").filter(|x| is_obj(x)).map(selector),
                    )
                }),
                kinds: group_kinds(a, "kinds"),
            }),
    }
}

fn object_ref(r: &Value) -> ObjectRef {
    ObjectRef {
        group: opt_s(r, "group"),
        kind: opt_s(r, "kind"),
        name: s(r, "name"),
        namespace: opt_s(r, "namespace"),
        port: int(r, "port"),
    }
}

pub(crate) fn parent_ref(r: &Value) -> ParentRef {
    ParentRef {
        group: opt_s(r, "group"),
        kind: opt_s(r, "kind"),
        namespace: opt_s(r, "namespace"),
        name: s(r, "name"),
        section_name: opt_s(r, "sectionName"),
        port: int(r, "port"),
    }
}

/// A route's status on a parent, as a controller wrote it.
pub(crate) fn parent_status(p: &Value) -> RouteParentStatus {
    RouteParentStatus {
        parent_ref: parent_ref(obj(p, "parentRef")),
        controller_name: s(p, "controllerName"),
        conditions: conditions(p, "conditions"),
    }
}

fn match_values(v: &Value, k: &str) -> Vec<MatchValue> {
    arr(v, k)
        .iter()
        .map(|m| MatchValue {
            typ: opt_s(m, "type"),
            name: s(m, "name"),
            value: s(m, "value"),
        })
        .collect()
}

fn http_match(m: &Value) -> HttpMatch {
    HttpMatch {
        path: m
            .get("path")
            .filter(|p| is_obj(p))
            .map(|p| (opt_s(p, "type"), opt_s(p, "value"))),
        headers: match_values(m, "headers"),
        query_params: match_values(m, "queryParams"),
        method: opt_s(m, "method"),
    }
}

fn grpc_match(m: &Value) -> GrpcMatch {
    GrpcMatch {
        method: m
            .get("method")
            .filter(|x| is_obj(x))
            .map(|x| (opt_s(x, "type"), opt_s(x, "service"), opt_s(x, "method"))),
        headers: match_values(m, "headers"),
    }
}

fn pairs(v: &Value, k: &str) -> Vec<(String, String)> {
    arr(v, k)
        .iter()
        .map(|h| (s(h, "name"), s(h, "value")))
        .collect()
}

fn header_filter(v: &Value) -> HeaderFilter {
    HeaderFilter {
        set: pairs(v, "set"),
        add: pairs(v, "add"),
        remove: strings(v, "remove"),
    }
}

fn path_modifier(v: &Value) -> PathModifier {
    PathModifier {
        typ: s(v, "type"),
        replace_full_path: opt_s(v, "replaceFullPath"),
        replace_prefix_match: opt_s(v, "replacePrefixMatch"),
    }
}

fn filter(f: &Value) -> Filter {
    let sub = |k: &str| f.get(k).filter(|x| is_obj(x));
    Filter {
        typ: s(f, "type"),
        request_header_modifier: sub("requestHeaderModifier").map(header_filter),
        response_header_modifier: sub("responseHeaderModifier").map(header_filter),
        request_redirect: sub("requestRedirect").map(|r| Redirect {
            scheme: opt_s(r, "scheme"),
            hostname: opt_s(r, "hostname"),
            path: r.get("path").filter(|p| is_obj(p)).map(path_modifier),
            port: int(r, "port"),
            status_code: int(r, "statusCode"),
        }),
        url_rewrite: sub("urlRewrite").map(|r| UrlRewrite {
            hostname: opt_s(r, "hostname"),
            path: r.get("path").filter(|p| is_obj(p)).map(path_modifier),
        }),
        request_mirror: sub("requestMirror").map(|m| Mirror {
            backend_ref: object_ref(obj(m, "backendRef")),
            percent: int(m, "percent"),
            fraction: m
                .get("fraction")
                .filter(|x| is_obj(x))
                .map(|x| (int(x, "numerator").unwrap_or(0), int(x, "denominator"))),
        }),
    }
}

fn backend_ref(b: &Value) -> BackendRef {
    BackendRef {
        r: object_ref(b),
        weight: int(b, "weight"),
        filters: arr(b, "filters").iter().map(filter).collect(),
    }
}

/// Decodes `v`, an object of `shape`.
///
/// # Errors
///
/// When `v` is not an object with metadata naming it.
pub(crate) fn decode(shape: Shape, v: &Value) -> Result<Object, String> {
    if !matches!(v, Value::Object(_)) || !matches!(v.get("metadata"), Some(Value::Object(_))) {
        return Err("not an object with metadata".to_owned());
    }
    let meta = meta(v);
    if meta.name.is_empty() {
        return Err("an object without a name".to_owned());
    }
    let spec = obj(v, "spec");
    Ok(match shape {
        Shape::IngressClass => Object::IngressClass(IngressClass {
            meta,
            controller: s(spec, "controller"),
        }),
        Shape::Ingress => Object::Ingress(Ingress {
            meta,
            class_name: opt_s(spec, "ingressClassName"),
            default_backend: spec
                .get("defaultBackend")
                .filter(|b| matches!(b, Value::Object(_)))
                .map(backend),
            tls: arr(spec, "tls")
                .iter()
                .map(|t| IngressTls {
                    hosts: strings(t, "hosts"),
                    secret_name: s(t, "secretName"),
                })
                .collect(),
            rules: arr(spec, "rules")
                .iter()
                .map(|r| IngressRule {
                    host: s(r, "host"),
                    paths: r
                        .get("http")
                        .filter(|h| matches!(h, Value::Object(_)))
                        .map(|h| {
                            arr(h, "paths")
                                .iter()
                                .map(|p| IngressPath {
                                    path: s(p, "path"),
                                    path_type: s(p, "pathType"),
                                    backend: backend(obj(p, "backend")),
                                })
                                .collect()
                        }),
                })
                .collect(),
        }),
        Shape::Service => Object::Service(Service {
            meta,
            typ: s(spec, "type"),
            cluster_ip: s(spec, "clusterIP"),
            external_name: s(spec, "externalName"),
            ports: arr(spec, "ports")
                .iter()
                .map(|p| ServicePort {
                    name: s(p, "name"),
                    protocol: s(p, "protocol"),
                    port: int(p, "port").unwrap_or(0),
                    app_protocol: opt_s(p, "appProtocol"),
                })
                .collect(),
        }),
        Shape::Slice => Object::Slice(EndpointSlice {
            meta,
            endpoints: arr(v, "endpoints")
                .iter()
                .map(|e| Endpoint {
                    addresses: strings(e, "addresses"),
                    ready: match obj(e, "conditions").get("ready") {
                        Some(Value::Bool(b)) => Some(*b),
                        _ => None,
                    },
                })
                .collect(),
            ports: arr(v, "ports")
                .iter()
                .map(|p| SlicePort {
                    name: opt_s(p, "name"),
                    protocol: opt_s(p, "protocol"),
                    port: int(p, "port"),
                })
                .collect(),
        }),
        Shape::Namespace => Object::Namespace(Namespace { meta }),
        Shape::GatewayClass => Object::GatewayClass(GatewayClass {
            meta,
            controller_name: s(spec, "controllerName"),
            conditions: conditions(obj(v, "status"), "conditions"),
        }),
        Shape::Gateway => Object::Gateway(Gateway {
            meta,
            class_name: s(spec, "gatewayClassName"),
            listeners: arr(spec, "listeners").iter().map(listener).collect(),
            status: gateway_status(obj(v, "status")),
        }),
        Shape::HttpRoute
        | Shape::GrpcRoute
        | Shape::TlsRoute
        | Shape::TcpRoute
        | Shape::UdpRoute => Object::Route(Route {
            meta,
            parent_refs: arr(spec, "parentRefs").iter().map(parent_ref).collect(),
            hostnames: strings(spec, "hostnames"),
            rules: arr(spec, "rules")
                .iter()
                .map(|r| Rule {
                    matches: if shape == Shape::HttpRoute {
                        arr(r, "matches").iter().map(http_match).collect()
                    } else {
                        Vec::new()
                    },
                    grpc_matches: if shape == Shape::GrpcRoute {
                        arr(r, "matches").iter().map(grpc_match).collect()
                    } else {
                        Vec::new()
                    },
                    filters: arr(r, "filters").iter().map(filter).collect(),
                    backend_refs: arr(r, "backendRefs").iter().map(backend_ref).collect(),
                })
                .collect(),
            parents: arr(obj(v, "status"), "parents").to_vec(),
        }),
        Shape::Grant => Object::Grant(ReferenceGrant {
            meta,
            from: arr(spec, "from")
                .iter()
                .map(|f| (s(f, "group"), s(f, "kind"), s(f, "namespace")))
                .collect(),
            to: arr(spec, "to")
                .iter()
                .map(|t| (s(t, "group"), s(t, "kind"), opt_s(t, "name")))
                .collect(),
        }),
        Shape::Secret => {
            let data = obj(v, "data");
            let b64 = |k: &str| -> Result<Vec<u8>, String> {
                match data.get(k) {
                    Some(Value::String(s)) => akio_crypto::base64::STANDARD
                        .decode(s)
                        .map_err(|_| format!("secret {}: {k} is not base64", meta.key())),
                    _ => Ok(Vec::new()),
                }
            };
            let (crt, key) = (b64("tls.crt")?, b64("tls.key")?);
            Object::Secret(Secret {
                typ: s(v, "type"),
                crt,
                key,
                meta,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::{Object, Shape, decode};
    use akit_config::value::parse;

    /// fit's `TestDecodeKinds`: each kind's fields, and objects that are not.
    #[test]
    fn decode_kinds() {
        let v = parse(
            r#"{"metadata": {"namespace": "ns", "name": "i", "annotations": {"a": "b"}},
              "spec": {"ingressClassName": "akit", "rules": [{"host": "h", "http": {"paths":
              [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "s",
              "port": {"name": "http"}}}}]}}, {"host": "x"}]}}"#,
        )
        .unwrap();
        let Object::Ingress(i) = decode(Shape::Ingress, &v).unwrap() else {
            panic!()
        };
        assert_eq!(i.meta.key(), "ns/i");
        assert_eq!(i.meta.annotation("a"), Some("b"));
        assert_eq!(i.class_name.as_deref(), Some("akit"));
        let paths = i.rules[0].paths.as_ref().unwrap();
        assert_eq!(paths[0].backend.service.as_ref().unwrap().port_name, "http");
        assert!(i.rules[1].paths.is_none());
        let v = parse(r#"{"metadata": {"name": "s"}, "type": "kubernetes.io/tls", "data": {"tls.crt": "aGk="}}"#).unwrap();
        let Object::Secret(s) = decode(Shape::Secret, &v).unwrap() else {
            panic!()
        };
        assert_eq!((s.crt.as_slice(), s.key.len()), (&b"hi"[..], 0));
        for bad in [r#"[]"#, r#"{"spec": {}}"#, r#"{"metadata": {}}"#] {
            assert!(
                decode(Shape::Service, &parse(bad).unwrap()).is_err(),
                "{bad}"
            );
        }
        let v = parse(r#"{"metadata": {"name": "s"}, "data": {"tls.crt": "!!"}}"#).unwrap();
        assert!(decode(Shape::Secret, &v).is_err());
    }
}
