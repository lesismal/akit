//! fit's tests of the Kubernetes provider's parts that need no API server: `ingress_test.go`'s
//! and `edges_test.go`'s Ingresses, `kubernetes_test.go`'s and `runtime_test.go`'s kubeconfigs,
//! tokens, watch events and certificate files. The provider against a fake API server is in
//! `tests/kubernetes.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akit_config::value::{Value, parse};
use akit_config::{KubernetesIngress, KubernetesProvider};

use super::client::{Failure, Stream};
use super::kubeconfig::{Env, Token, from_kubeconfig, load};
use super::translate::{Snapshot, Translator};
use super::types::{Object, Shape, decode};
use super::watch::{Signal, Store, Watch, lock};
use super::{Msg, is_cert_file, remove_certs, write_certs};

/// A self-signed certificate for `names`, and its key, in PEM.
pub(crate) fn test_cert(names: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let key =
        akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let cert = akio_x509::CertificateParams::new(names)
        .self_signed(&key)
        .unwrap();
    (cert.to_pem().into_bytes(), key.to_pkcs8_pem().into_bytes())
}

fn b64(b: &[u8]) -> String {
    akio_crypto::base64::STANDARD.encode(b)
}

/// A Secret of type kubernetes.io/tls, as the API server writes it.
fn secret_doc(ns: &str, name: &str, crt: &[u8], key: &[u8]) -> String {
    format!(
        r#"{{"metadata": {{"namespace": "{ns}", "name": "{name}"}}, "type": "kubernetes.io/tls", "data": {{"tls.crt": "{}", "tls.key": "{}"}}}}"#,
        b64(crt),
        b64(key)
    )
}

fn obj(shape: Shape, doc: &str) -> Arc<Object> {
    Arc::new(decode(shape, &parse(doc).unwrap()).unwrap())
}

fn service(ns: &str, name: &str, port: u16, port_name: &str, extra: &str) -> Arc<Object> {
    obj(
        Shape::Service,
        &format!(
            r#"{{"metadata": {{"namespace": "{ns}", "name": "{name}"}}, "spec": {{"clusterIP": "10.96.0.{}", "ports": [{{"name": "{port_name}", "port": {port}{extra}}}]}}}}"#,
            port % 250
        ),
    )
}

fn endpoints(
    ns: &str,
    svc: &str,
    port_name: &str,
    port: u16,
    ready: &[(&str, bool)],
) -> Arc<Object> {
    let mut ready = ready.to_vec();
    ready.sort();
    let eps: Vec<String> = ready
        .iter()
        .map(|(ip, r)| format!(r#"{{"addresses": ["{ip}"], "conditions": {{"ready": {r}}}}}"#))
        .collect();
    obj(
        Shape::Slice,
        &format!(
            r#"{{"metadata": {{"namespace": "{ns}", "name": "{svc}-abc", "labels": {{"kubernetes.io/service-name": "{svc}"}}}},
  "addressType": "IPv4", "endpoints": [{}], "ports": [{{"name": "{port_name}", "protocol": "TCP", "port": {port}}}]}}"#,
            eps.join(",")
        ),
    )
}

/// fit's `base`, its Services and their endpoints, with fit's IngressClass and another's
/// (`ingressWorld`).
fn ingress_world(default_class: bool) -> Snapshot {
    let ann = if default_class {
        r#", "annotations": {"ingressclass.kubernetes.io/is-default-class": "true"}"#
    } else {
        ""
    };
    Snapshot {
        ingress_classes: vec![
            obj(
                Shape::IngressClass,
                &format!(
                    r#"{{"metadata": {{"name": "fit"{ann}}}, "spec": {{"controller": "lesismal.github.io/fit"}}}}"#
                ),
            ),
            obj(
                Shape::IngressClass,
                r#"{"metadata": {"name": "nginx"}, "spec": {"controller": "k8s.io/ingress-nginx"}}"#,
            ),
        ],
        ingresses: Vec::new(),
        services: vec![
            service(
                "default",
                "web",
                80,
                "http",
                r#", "appProtocol": "kubernetes.io/h2c""#,
            ),
            service("default", "api", 8080, "", ""),
            service("default", "empty", 80, "", ""),
            service("team", "shop", 80, "http", ""),
        ],
        slices: vec![
            endpoints(
                "default",
                "web",
                "http",
                8000,
                &[("10.0.0.2", true), ("10.0.0.1", true), ("10.0.0.9", false)],
            ),
            endpoints("default", "api", "", 9000, &[("10.0.1.1", true)]),
            endpoints("default", "empty", "", 80, &[("10.0.2.1", false)]),
            endpoints("team", "shop", "http", 3000, &[("10.0.3.1", true)]),
        ],
        secrets: Vec::new(),
    }
}

fn ingress(name: &str, created: &str, annotations: &str, spec: &str) -> Arc<Object> {
    obj(
        Shape::Ingress,
        &format!(
            r#"{{"metadata": {{"namespace": "default", "name": "{name}", "creationTimestamp": "{created}", "annotations": {{{annotations}}}}}, "spec": {spec}}}"#
        ),
    )
}

fn opts(ingress: Option<KubernetesIngress>) -> KubernetesProvider {
    KubernetesProvider {
        kubeconfig: String::new(),
        context: String::new(),
        controller_name: akit_config::DEFAULT_KUBERNETES_CONTROLLER_NAME.to_owned(),
        namespaces: Vec::new(),
        listen_host: String::new(),
        backends: akit_config::KubernetesBackends::Endpoints,
        max_objects: 10_000,
        request_timeout: Duration::from_secs(10),
        ingress,
        certificates_dir: String::new(),
    }
}

fn ports(http_port: u16, https_port: u16) -> Option<KubernetesIngress> {
    Some(KubernetesIngress {
        http_port,
        https_port,
    })
}

/// The part's JSON written compactly, its members in their order.
pub(crate) fn compact(v: &Value) -> String {
    let mut b = String::new();
    write(&mut b, v);
    b
}

fn write(b: &mut String, v: &Value) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(x) => b.push_str(if *x { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => akit_config::value::write_string(b, s),
        Value::Array(a) => {
            b.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write(b, x);
            }
            b.push(']');
        }
        Value::Object(ms) => {
            b.push('{');
            for (i, (k, x)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                akit_config::value::write_string(b, k);
                b.push(':');
                write(b, x);
            }
            b.push('}');
        }
    }
}

/// A translated part, checked to be a config akit takes, and why what was left out was.
struct Part {
    v: Value,
    problems: Vec<String>,
}

impl Part {
    fn of(s: &Snapshot, o: &KubernetesProvider, cert_dir: Option<&str>) -> Part {
        let mut t = Translator::new("k8s", o, cert_dir.map(str::to_owned));
        let data = String::from_utf8(t.run(s)).unwrap();
        // fit's checkPart: akit takes the part as a config of its own; an empty one, which adds
        // nothing, is not one.
        if data != "{}" {
            if let Err(e) = akit_config::from_json(&data, None) {
                panic!("the part is not a valid config: {e}\n{data}");
            }
        }
        Part {
            v: parse(&data).unwrap(),
            problems: t.problems,
        }
    }

    fn list(&self, k: &str) -> &[Value] {
        match self.v.get(k) {
            Some(Value::Array(a)) => a,
            _ => &[],
        }
    }

    fn names(&self) -> Vec<String> {
        self.list("routes")
            .iter()
            .map(|r| match r.get("name") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            })
            .collect()
    }

    /// The route `name`'s fields, compact; "null" for one left out.
    fn route(&self, name: &str) -> BTreeMap<&'static str, String> {
        let r = self
            .list("routes")
            .iter()
            .find(|r| r.get("name") == Some(&Value::String(name.to_owned())))
            .unwrap_or_else(|| panic!("no route {name} in {:?}", self.names()));
        let mut out = BTreeMap::new();
        for k in ["listeners", "match", "middleware", "handler", "priority"] {
            out.insert(k, r.get(k).map_or_else(|| "null".to_owned(), compact));
        }
        out
    }

    fn has_problems(&self, want: &[&str]) {
        for w in want {
            assert!(
                self.problems.iter().any(|p| p.starts_with(w)),
                "no problem {w:?} in:\n{}",
                self.problems.join("\n")
            );
        }
    }
}

/// fit's `TestIngress`.
#[test]
fn ingresses() {
    let (crt, key) = test_cert(&["shop.example.com"]);
    let mut w = ingress_world(true);
    w.secrets = vec![obj(
        Shape::Secret,
        &secret_doc("default", "shop-tls", &crt, &key),
    )];
    w.ingresses = vec![
        ingress(
            "shop",
            "2026-01-01T00:00:00Z",
            r#""fit.lesismal.github.io/ssl-redirect": "true", "fit.lesismal.github.io/limit-rps": "10",
  "fit.lesismal.github.io/proxy-body-size": "1m", "fit.lesismal.github.io/request-timeout": "30s""#,
            r#"{"ingressClassName": "fit",
  "tls": [{"hosts": ["shop.example.com"], "secretName": "shop-tls"}],
  "rules": [{"host": "shop.example.com", "http": {"paths": [
    {"path": "/api", "pathType": "Prefix", "backend": {"service": {"name": "api", "port": {"number": 8080}}}},
    {"path": "/health", "pathType": "Exact", "backend": {"service": {"name": "web", "port": {"name": "http"}}}}]}}]}"#,
        ),
        ingress(
            "legacy",
            "2026-01-02T00:00:00Z",
            r#""kubernetes.io/ingress.class": "fit", "fit.lesismal.github.io/rewrite-target": "/v2",
  "fit.lesismal.github.io/backend-protocol": "GRPC", "fit.lesismal.github.io/allow-source-range": "10.0.0.0/8, 192.168.1.1""#,
            r#"{
  "rules": [{"host": "*.example.com", "http": {"paths": [{"path": "/", "pathType": "ImplementationSpecific", "backend": {"service": {"name": "api", "port": {"number": 8080}}}}]}}]}"#,
        ),
        // No class: fit's is the default.
        ingress(
            "defaulted",
            "2026-01-03T00:00:00Z",
            "",
            r#"{"defaultBackend": {"service": {"name": "web", "port": {"number": 80}}},
  "tls": [{"secretName": "missing"}]}"#,
        ),
        ingress(
            "theirs",
            "2026-01-04T00:00:00Z",
            "",
            r#"{"ingressClassName": "nginx", "defaultBackend": {"service": {"name": "web", "port": {"number": 80}}}}"#,
        ),
        ingress(
            "broken",
            "2026-01-05T00:00:00Z",
            r#""fit.lesismal.github.io/limit-rps": "fast", "fit.lesismal.github.io/unknown": "x",
  "fit.lesismal.github.io/proxy-body-size": "lots""#,
            r#"{"ingressClassName": "fit",
  "rules": [{"host": "b.example.com", "http": {"paths": [
    {"path": "/x", "pathType": "Exact", "backend": {"service": {"name": "missing", "port": {"number": 80}}}},
    {"path": "/r", "pathType": "Prefix", "backend": {"resource": {"kind": "Bucket", "name": "b"}}},
    {"path": "/p", "pathType": "Exact", "backend": {"service": {"name": "web", "port": {"name": "nope"}}}},
    {"path": "/q", "pathType": "Regex", "backend": {"service": {"name": "web", "port": {"number": 80}}}}]}}]}"#,
        ),
        ingress(
            "closed",
            "2026-01-06T00:00:00Z",
            r#""fit.lesismal.github.io/allow-source-range": "10.0.0.0/8, nope""#,
            r#"{"ingressClassName": "fit",
  "rules": [{"host": "c.example.com", "http": {"paths": [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "web", "port": {"number": 80}}}}]}}]}"#,
        ),
    ];
    let p = Part::of(&w, &opts(ports(80, 443)), Some("/certs"));
    let listeners = compact(p.v.get("listeners").unwrap());
    assert!(
        listeners.starts_with(r#"[{"name":"k8s-80","address":":80","protocols":["http/1.1","h2c"]},{"name":"k8s-443","address":":443","protocols":["http/1.1","h2"],"tls":{"certificates":[{"cert_file":"/certs/default.shop-tls."#),
        "{listeners}"
    );
    assert_eq!(
        p.names(),
        [
            "default.shop.ing.r0.p0",
            "default.shop.ing.r0.p0.redirect",
            "default.shop.ing.r0.p1",
            "default.shop.ing.r0.p1.redirect",
            "default.legacy.ing.r0.p0",
            "default.legacy.ing.r0.p0.deny",
            "default.defaulted.ing.default",
            "default.broken.ing.r0.p0",
            "default.broken.ing.r0.p1",
            "default.broken.ing.r0.p2",
            "default.closed.ing.r0.p0",
        ]
    );

    // shop: HTTPS for its TLS host, HTTP redirected there; limits, timeout.
    let r = p.route("default.shop.ing.r0.p0");
    assert_eq!(r["listeners"], r#"["k8s-443"]"#);
    assert_eq!(
        r["match"],
        r#"{"host":["shop.example.com"],"path":["/api","/api/*"]}"#
    );
    assert_eq!(
        r["middleware"],
        r#"[{"module":"rate_limit","params":{"rate":10}},{"module":"body_limit","params":{"max_bytes":1048576}}]"#
    );
    assert_eq!(
        r["handler"],
        r#"{"module":"proxy","params":{"timeout":"30s","upstream":"default.api.8080"}}"#
    );
    let rd = p.route("default.shop.ing.r0.p0.redirect");
    assert_eq!(rd["listeners"], r#"["k8s-80"]"#);
    assert_eq!(rd["match"], r["match"]);
    assert_eq!(
        rd["middleware"],
        r#"[{"module":"redirect","params":{"status":308,"to":"https://shop.example.com{uri}"}}]"#
    );
    let exact = p.route("default.shop.ing.r0.p1");
    assert!(exact["priority"].parse::<i64>().unwrap() > r["priority"].parse::<i64>().unwrap());
    assert_eq!(
        exact["handler"],
        r#"{"module":"proxy","params":{"timeout":"30s","upstream":"default.web.80"}}"#
    );

    // legacy: any path of a wildcard host, from the ranges allowed, the others refused;
    // rewritten, over h2c.
    let r = p.route("default.legacy.ing.r0.p0");
    let deny = p.route("default.legacy.ing.r0.p0.deny");
    assert_eq!(r["listeners"], r#"["k8s-80"]"#);
    assert_eq!(
        r["match"],
        r#"{"host":["*.example.com"],"remote_ip":["10.0.0.0/8","192.168.1.1"]}"#
    );
    assert_eq!(
        r["middleware"],
        r#"[{"module":"rewrite","params":{"add_prefix":"/v2"}}]"#
    );
    assert_eq!(
        r["handler"],
        r#"{"module":"proxy","params":{"upstream":"default.api.8080.h2c"}}"#
    );
    assert_eq!(deny["match"], r#"{"host":["*.example.com"]}"#);
    assert_eq!(
        deny["handler"],
        r#"{"module":"respond","params":{"status":403}}"#
    );
    assert!(deny["priority"].parse::<i64>().unwrap() < r["priority"].parse::<i64>().unwrap());

    // defaulted: what nothing else takes, on HTTP: its Secret is missing.
    let d = p.route("default.defaulted.ing.default");
    assert_eq!(
        (
            d["listeners"].as_str(),
            d["match"].as_str(),
            d["priority"].as_str()
        ),
        (r#"["k8s-80"]"#, "null", "null")
    );

    // broken: 503s; closed: 403 for all, its range not parsing.
    for name in [
        "default.broken.ing.r0.p0",
        "default.broken.ing.r0.p1",
        "default.broken.ing.r0.p2",
    ] {
        let r = p.route(name);
        assert_eq!(
            r["handler"], r#"{"module":"respond","params":{"status":503}}"#,
            "{name}"
        );
        assert_eq!(r["middleware"], "null", "{name}");
    }
    assert_eq!(
        p.route("default.closed.ing.r0.p0")["handler"],
        r#"{"module":"respond","params":{"status":403}}"#
    );
    p.has_problems(&[
        "ingress default/defaulted: tls 0: there is no secret default/missing of type kubernetes.io/tls",
        r#"ingress default/broken: annotation fit.lesismal.github.io/limit-rps: "fast" is not a rate of at least 0.001; it is left out"#,
        r#"ingress default/broken: annotation fit.lesismal.github.io/proxy-body-size: "lots" is not a size, as 8m; it is left out"#,
        "ingress default/broken: annotation fit.lesismal.github.io/unknown is not one akit knows",
        "ingress default/broken: rule 0: path 0: service default/missing does not exist; it answers 503",
        "ingress default/broken: rule 0: path 1: only a Service is supported as a backend; it answers 503",
        r#"ingress default/broken: rule 0: path 2: service default/web has no port named "nope"; it answers 503"#,
        r#"ingress default/broken: rule 0: path 3: path type "Regex" is not supported"#,
        r#"ingress default/closed: annotation fit.lesismal.github.io/allow-source-range: "nope" is not an address or CIDR range; every request is refused"#,
    ]);
    // The certificate file of shop-tls is given to write: its chain and key.
    let (path, data) = {
        let o = opts(ports(80, 443));
        let mut t = Translator::new("k8s", &o, Some("/certs".to_owned()));
        t.run(&w);
        t.certs.into_iter().next().unwrap()
    };
    assert!(
        path.starts_with("/certs/default.shop-tls.") && path.ends_with(".pem"),
        "{path}"
    );
    assert!(is_cert_file(
        Path::new(&path).file_name().unwrap().to_str().unwrap()
    ));
    assert_eq!(data, [&crt[..], b"\n", &key[..]].concat());

    // Without a default class, an Ingress naming none is not akit's; without ingress, none
    // is; only those of the namespaces watched are.
    w.ingress_classes = ingress_world(false).ingress_classes;
    let p = Part::of(&w, &opts(ports(80, 443)), Some("/certs"));
    assert!(
        !p.names().iter().any(|n| n.contains("defaulted")),
        "{:?}",
        p.names()
    );
    let p = Part::of(&w, &opts(None), Some("/certs"));
    assert!(p.list("routes").is_empty() && p.list("listeners").is_empty());
    let mut o = opts(ports(80, 443));
    o.namespaces = vec!["team".to_owned()];
    assert!(Part::of(&w, &o, Some("/certs")).names().is_empty());
}

/// fit's `TestIngressPorts`, but for the Gateway holding a port, which comes with I60: other
/// ports, and a wildcard host not redirected.
#[test]
fn ingress_ports() {
    let (crt, key) = test_cert(&["shop.example.com"]);
    let mut w = ingress_world(false);
    w.secrets = vec![obj(
        Shape::Secret,
        &secret_doc("default", "shop-tls", &crt, &key),
    )];
    w.ingresses = vec![ingress(
        "shop",
        "",
        r#""fit.lesismal.github.io/ssl-redirect": "true""#,
        r#"{"ingressClassName": "fit",
  "tls": [{"hosts": ["shop.example.com", "*.shop.example.com"], "secretName": "shop-tls"}],
  "rules": [{"host": "shop.example.com", "http": {"paths": [{"pathType": "Prefix", "backend": {"service": {"name": "api", "port": {"number": 8080}}}}]}},
            {"host": "*.shop.example.com", "http": {"paths": [{"pathType": "Prefix", "backend": {"service": {"name": "api", "port": {"number": 8080}}}}]}}]}"#,
    )];
    let mut o = opts(ports(8080, 8443));
    o.listen_host = "::1".to_owned();
    let p = Part::of(&w, &o, Some("/certs"));
    assert_eq!(
        p.route("default.shop.ing.r0.p0.redirect")["middleware"],
        r#"[{"module":"redirect","params":{"status":308,"to":"https://shop.example.com:8443{uri}"}}]"#
    );
    assert_eq!(
        p.route("default.shop.ing.r1.p0")["listeners"],
        r#"["k8s-8080","k8s-8443"]"#
    );
    p.has_problems(&["ingress default/shop: rule 1: path 0: ssl-redirect: a wildcard or any host is redirected only from port 80 to 443"]);
    assert!(compact(p.v.get("listeners").unwrap()).contains(r#""address":"[::1]:8080""#));
}

/// fit's `TestIngressEdges`, but for the Gateway holding a port (I60).
#[test]
fn ingress_edges() {
    let (crt, key) = test_cert(&["shop.example.com"]);
    let mut w = ingress_world(false);
    w.secrets = vec![obj(
        Shape::Secret,
        &secret_doc("default", "shop-tls", &crt, &key),
    )];
    w.ingresses = vec![
        ingress(
            "a",
            "2026-01-01T00:00:00Z",
            r#""fit.lesismal.github.io/ssl-redirect": "maybe", "fit.lesismal.github.io/rewrite-target": "x",
  "fit.lesismal.github.io/backend-protocol": "HTTP", "fit.lesismal.github.io/request-timeout": "soon",
  "fit.lesismal.github.io/allow-source-range": "10.0.0.0/8, ,", "other.example/ignored": "x""#,
            r#"{"ingressClassName": "fit",
  "tls": [{"secretName": ""}, {"secretName": "shop-tls"}],
  "rules": [{"host": "x.example.com"},
            {"http": {"paths": [{"path": "x", "pathType": "Prefix", "backend": {"service": {"name": "web", "port": {"number": 80}}}},
                                {"path": "/e", "pathType": "Exact", "backend": {"service": {"name": "web", "port": {"number": 80}}}}]}}]}"#,
        ),
        ingress(
            "b",
            "2026-01-02T00:00:00Z",
            r#""fit.lesismal.github.io/ssl-redirect": "true", "fit.lesismal.github.io/rewrite-target": "/v2",
  "fit.lesismal.github.io/backend-protocol": "SCTP""#,
            r#"{"ingressClassName": "fit",
  "tls": [{"secretName": "shop-tls"}],
  "rules": [{"http": {"paths": [{"path": "/x", "pathType": "Exact", "backend": {"service": {"name": "web", "port": {"number": 80}}}},
                                {"path": "/y", "pathType": "Prefix", "backend": {"service": {"name": "empty", "port": {"number": 80}}}}]}}]}"#,
        ),
    ];
    let p = Part::of(&w, &opts(ports(80, 443)), Some("/certs"));
    p.has_problems(&[
        r#"ingress default/a: annotation fit.lesismal.github.io/ssl-redirect: "maybe" is not true or false"#,
        r#"ingress default/a: annotation fit.lesismal.github.io/rewrite-target: "x" is not a path"#,
        r#"ingress default/a: annotation fit.lesismal.github.io/request-timeout: "soon" is not a duration"#,
        "ingress default/a: tls 0: no secretName",
        r#"ingress default/a: rule 1: path 0: path "x" does not start with '/'"#,
        r#"ingress default/b: annotation fit.lesismal.github.io/backend-protocol: "SCTP" is not HTTP, H2C or GRPC"#,
    ]);
    // a: TLS for any host, and its source range, its empty entries left out.
    let r = p.route("default.a.ing.r1.p1");
    assert_eq!(r["listeners"], r#"["k8s-80","k8s-443"]"#);
    assert!(
        r["match"].contains(r#""remote_ip":["10.0.0.0/8"]"#),
        "{}",
        r["match"]
    );
    // b: an exact path rewritten whole; any host redirected to HTTPS; a backend with no
    // endpoints ready answers 503.
    assert_eq!(
        p.route("default.b.ing.r0.p0")["middleware"],
        r#"[{"module":"rewrite","params":{"regexp":"^.*$","replacement":"/v2"}}]"#
    );
    assert_eq!(
        p.route("default.b.ing.r0.p0.redirect")["middleware"],
        r#"[{"module":"redirect","params":{"status":308,"to":"https://{host}{uri}"}}]"#
    );
    assert_eq!(
        p.route("default.b.ing.r0.p1")["handler"],
        r#"{"module":"respond","params":{"status":503}}"#
    );

    // Nowhere to write certificates: TLS is not served.
    let p = Part::of(&w, &opts(ports(80, 443)), None);
    p.has_problems(&["ingress default/a: tls 1: there is no directory to write certificates to"]);

    // The HTTP port is the HTTPS one: HTTP alone (the config refuses it, but a part is made).
    let o = opts(ports(80, 80));
    let mut t = Translator::new("k8s", &o, Some("/certs".to_owned()));
    t.run(&w);
    assert!(
        t.problems
            .iter()
            .any(|p| p == "ingress default/a: port 80 serves http already: its rules are not served over HTTPS"),
        "{:?}",
        t.problems
    );
}

/// Backends: cluster IPs, Services' ports by protocol, ExternalName Services (refused without
/// allow_external_name_services, which waits for deny_internal), a certificate that is not its
/// key's, a Secret too large.
#[test]
fn ingress_backends() {
    let (crt, key) = test_cert(&["a.example.com"]);
    let (_, other_key) = test_cert(&["b.example.com"]);
    let mut w = ingress_world(true);
    w.services.push(obj(
        Shape::Service,
        r#"{"metadata": {"namespace": "default", "name": "ext"}, "spec": {"type": "ExternalName", "externalName": "db.example.net", "ports": [{"port": 80}]}}"#,
    ));
    w.services.push(obj(
        Shape::Service,
        r#"{"metadata": {"namespace": "default", "name": "udp"}, "spec": {"ports": [{"port": 53, "protocol": "UDP"}]}}"#,
    ));
    w.secrets = vec![
        obj(
            Shape::Secret,
            &secret_doc("default", "mismatch", &crt, &other_key),
        ),
        obj(
            Shape::Secret,
            &secret_doc(
                "default",
                "huge",
                &[&crt[..], &vec![b' '; 1 << 20]].concat(),
                &key,
            ),
        ),
    ];
    let path = |p: &str, svc: &str, port: u16| {
        format!(
            r#"{{"path": "{p}", "pathType": "Exact", "backend": {{"service": {{"name": "{svc}", "port": {{"number": {port}}}}}}}}}"#
        )
    };
    w.ingresses = vec![ingress(
        "i",
        "",
        "",
        &format!(
            r#"{{"tls": [{{"secretName": "mismatch"}}, {{"secretName": "huge"}}], "rules": [{{"http": {{"paths": [{}, {}, {}, {}]}}}}]}}"#,
            path("/web", "web", 80),
            path("/ext", "ext", 80),
            path("/udp", "udp", 53),
            path("/api", "api", 8080)
        ),
    )];
    let mut o = opts(ports(80, 443));
    o.backends = akit_config::KubernetesBackends::ClusterIp;
    let p = Part::of(&w, &o, Some("/certs"));
    assert_eq!(
        compact(p.v.get("upstreams").unwrap()),
        r#"[{"name":"default.api.8080","addresses":["10.96.0.80:8080"]},{"name":"default.web.80","addresses":["10.96.0.80:80"],"protocol":"h2c"}]"#
    );
    p.has_problems(&[
        "ingress default/i: tls 0: secret default/mismatch: ",
        "ingress default/i: tls 1: secret default/huge is larger than 1048576 bytes",
        "ingress default/i: rule 0: path 1: service default/ext is an ExternalName Service, which allow_external_name_services must allow; it answers 503",
        "ingress default/i: rule 0: path 2: service default/udp has no TCP port 53; it answers 503",
    ]);
    // Endpoints: the ready ones of the port's name, sorted.
    let p = Part::of(&w, &opts(ports(80, 443)), Some("/certs"));
    assert!(
        compact(p.v.get("upstreams").unwrap())
            .contains(r#"{"name":"default.web.80","addresses":["10.0.0.1:8000","10.0.0.2:8000"],"protocol":"h2c"}"#),
        "{}",
        compact(p.v.get("upstreams").unwrap())
    );
    // The same objects make the same bytes.
    let a = Translator::new("k8s", &o, Some("/certs".to_owned())).run(&w);
    let b = Translator::new("k8s", &o, Some("/certs".to_owned())).run(&w);
    assert_eq!(a, b);
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("akit-k8s-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn file(d: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = d.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

fn no_env() -> Env {
    Env {
        service_account_token: PathBuf::from("/nonexistent/token"),
        service_account_ca: PathBuf::from("/nonexistent/ca.crt"),
        service_host: None,
        service_port: None,
        kubeconfig: None,
        home: None,
    }
}

/// fit's `TestKubeconfig` and `TestLoadConfigEdges`.
#[test]
fn kubeconfigs() {
    let d = dir("kubeconfig");
    let (ca, _) = test_cert(&["example.com"]);
    let (crt, key) = test_cert(&["client"]);
    file(&d, "ca.crt", &ca);
    file(&d, "token", b"test-token\n");
    let path = file(
        &d,
        "config",
        format!(
            r#"apiVersion: v1
current-context: files
clusters:
- {{name: c, cluster: {{server: "https://127.0.0.1:6443/", certificate-authority: ca.crt, tls-server-name: example.com}}}}
- {{name: insecure, cluster: {{server: "https://127.0.0.1:1", insecure-skip-tls-verify: true}}}}
- {{name: data, cluster: {{server: "https://127.0.0.1:1", certificate-authority-data: "{}"}}}}
- {{name: noserver, cluster: {{}}}}
- {{name: nocafile, cluster: {{server: "https://127.0.0.1:1", certificate-authority: missing.crt}}}}
- {{name: notca, cluster: {{server: "https://127.0.0.1:1", certificate-authority-data: "{}"}}}}
- {{name: plain, cluster: {{server: "http://127.0.0.1:8001"}}}}
users:
- {{name: files, user: {{tokenFile: token}}}}
- {{name: plugin, user: {{exec: {{command: aws}}}}}}
- {{name: badcert, user: {{client-certificate-data: "bm90IGEgY2VydA==", client-key-data: "bm90IGEga2V5"}}}}
- {{name: cert, user: {{client-certificate-data: "{}", client-key-data: "{}"}}}}
- {{name: nokey, user: {{client-certificate-data: "{}", client-key: missing.key}}}}
- {{name: notb64, user: {{client-certificate-data: "%%%"}}}}
contexts:
- {{name: files, context: {{cluster: c, user: files}}}}
- {{name: plugin, context: {{cluster: c, user: plugin}}}}
- {{name: insecure, context: {{cluster: insecure, user: files}}}}
- {{name: badcert, context: {{cluster: c, user: badcert}}}}
- {{name: nocluster, context: {{cluster: none, user: files}}}}
- {{name: cert, context: {{cluster: data, user: cert}}}}
- {{name: noserver, context: {{cluster: noserver, user: cert}}}}
- {{name: nocafile, context: {{cluster: nocafile, user: cert}}}}
- {{name: notca, context: {{cluster: notca, user: cert}}}}
- {{name: nokey, context: {{cluster: c, user: nokey}}}}
- {{name: notb64, context: {{cluster: c, user: notb64}}}}
- {{name: plain, context: {{cluster: plain, user: nobody}}}}
"#,
            b64(&ca),
            b64(b"not PEM"),
            b64(&crt),
            b64(&key),
            b64(&crt)
        )
        .as_bytes(),
    );
    let mut rc = from_kubeconfig(&path, "").unwrap();
    let tls = rc.tls.as_ref().unwrap();
    assert_eq!(rc.server, "https://127.0.0.1:6443");
    assert_eq!(tls.server_name, "example.com");
    assert_eq!(tls.ca.as_deref(), Some(&ca[..]));
    assert_eq!(rc.token.as_mut().unwrap().get().unwrap(), "test-token");
    assert!(
        from_kubeconfig(&path, "insecure")
            .unwrap()
            .tls
            .unwrap()
            .insecure
    );
    let rc = from_kubeconfig(&path, "cert").unwrap();
    assert!(rc.tls.unwrap().client.is_some() && rc.token.is_none());
    let rc = from_kubeconfig(&path, "plain").unwrap();
    assert!(rc.tls.is_none() && rc.token.is_none());
    for (ctx, want) in [
        ("plugin", "authenticates with a plugin"),
        ("badcert", r#"user "badcert": client-certificate"#),
        ("nocluster", r#"no cluster "none""#),
        ("absent", r#"no context "absent""#),
        ("noserver", r#"cluster "noserver" has no server"#),
        ("nocafile", r#"cluster "nocafile": certificate-authority: "#),
        (
            "notca",
            r#"cluster "notca": certificate-authority holds no certificate"#,
        ),
        ("nokey", r#"user "nokey": client-key: "#),
        ("notb64", r#"user "notb64": client-certificate: not base64"#),
    ] {
        let e = from_kubeconfig(&path, ctx).unwrap_err();
        assert!(e.contains(want), "context {ctx}: {e}, want {want:?}");
    }
    let e = from_kubeconfig(&file(&d, "nocurrent", b"clusters: []\n"), "").unwrap_err();
    assert!(e.contains("no current-context"), "{e}");
    assert!(from_kubeconfig(&file(&d, "bad", b"clusters: [\n"), "").is_err());
    let e = from_kubeconfig(&d.join("missing"), "").unwrap_err();
    assert!(e.starts_with("kubeconfig: "), "{e}");

    // load takes the kubeconfig named, $KUBECONFIG outside a cluster (its first file), and the
    // service account inside one.
    let o = opts(ports(80, 443));
    let mut env = no_env();
    env.kubeconfig = Some(format!("{}:/nonexistent", path.display()));
    assert_eq!(load(&o, &env).unwrap().server, "https://127.0.0.1:6443");
    env.kubeconfig = None;
    env.home = Some(d.clone());
    let e = load(&o, &env).unwrap_err();
    assert!(
        e.contains(&d.join(".kube").join("config").display().to_string()),
        "{e}"
    );
    env.home = None;
    assert!(load(&o, &env).unwrap_err().contains("no home directory"));
    env.service_host = Some("10.96.0.1".to_owned());
    env.service_account_ca = d.join("missing");
    assert!(load(&o, &env).unwrap_err().starts_with("in the cluster: "));
    env.service_account_ca = d.join("ca.crt");
    env.service_account_token = d.join("missing");
    assert!(load(&o, &env).unwrap_err().starts_with("in the cluster: "));
    env.service_account_token = d.join("token");
    let rc = load(&o, &env).unwrap();
    assert_eq!(
        (rc.server.as_str(), rc.r#where.as_str()),
        ("https://10.96.0.1:443", "the pod's service account")
    );
    env.service_port = Some("6443".to_owned());
    env.service_host = Some("fd00::1".to_owned());
    assert_eq!(load(&o, &env).unwrap().server, "https://[fd00::1]:6443");
    // A kubeconfig named wins over the service account.
    let mut named = o.clone();
    named.kubeconfig = path.display().to_string();
    assert_eq!(load(&named, &env).unwrap().server, "https://127.0.0.1:6443");
    env.service_account_ca = d.join("token");
    assert!(
        load(&o, &env).is_err(),
        "a service account CA with no certificate"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// fit's `TestTokenRotates`: a token file is read again once a minute has passed, and the last
/// token stays while the file is gone.
#[test]
fn token_rotates() {
    let d = dir("token");
    let path = file(&d, "token", b"one");
    let mut t = Token::file_for_tests(path.clone());
    assert_eq!(t.get().unwrap(), "one");
    std::fs::write(&path, "two").unwrap();
    assert_eq!(t.get().unwrap(), "one", "within a minute");
    t.age(Duration::from_secs(120));
    assert_eq!(t.get().unwrap(), "two", "after a minute");
    std::fs::remove_file(&path).unwrap();
    t.age(Duration::from_secs(120));
    assert_eq!(t.get().unwrap(), "two", "with the file gone");
    let mut gone = Token::file_for_tests(d.join("missing"));
    assert!(gone.get().is_err());
    let _ = std::fs::remove_dir_all(&d);
}

/// fit's `TestCertFiles`, and the files written and removed.
#[test]
fn cert_files() {
    let d = dir("certs");
    remove_certs(&d.join("missing"), &BTreeMap::new());
    let mut certs = BTreeMap::new();
    certs.insert(
        d.join("missing").join("a.pem").display().to_string(),
        b"x".to_vec(),
    );
    assert!(
        write_certs(&certs).is_err(),
        "written into a directory that is not there"
    );
    let keep = d.join("ns.keep.0123456789ab.pem").display().to_string();
    let old = d.join("ns.old.0123456789ab.pem");
    let other = d.join("not-ours.pem");
    std::fs::write(&old, "old").unwrap();
    std::fs::write(&other, "other").unwrap();
    let mut certs = BTreeMap::new();
    certs.insert(keep.clone(), b"chain and key".to_vec());
    write_certs(&certs).unwrap();
    // One there is the same: not written again.
    write_certs(&certs).unwrap();
    remove_certs(&d, &certs);
    assert_eq!(std::fs::read(&keep).unwrap(), b"chain and key");
    assert!(!old.exists() && other.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&keep).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    for (name, ok) in [
        ("default.shop-tls.0123456789ab.pem", true),
        ("a.b.c.0123456789ab.pem", true),
        ("a.b.0123456789a.pem", false),
        ("a.b.0123456789AB.pem", false),
        ("A.b.0123456789ab.pem", false),
        ("a.0123456789ab.pem", false),
        ("a.b.0123456789ab.crt", false),
    ] {
        assert_eq!(is_cert_file(name), ok, "{name}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

fn watch(max: usize) -> (Watch, Arc<Mutex<Store>>, mpsc::Receiver<Msg>) {
    let (tx, rx) = mpsc::sync_channel(8);
    let store = Arc::new(Mutex::new(Store::default()));
    let w = Watch::new(
        3,
        7,
        "things",
        "/a".to_owned(),
        Shape::Service,
        Arc::clone(&store),
        max,
        Arc::new(Signal {
            tx,
            changed: AtomicBool::new(false),
        }),
    );
    (w, store, rx)
}

fn ended(rx: &mpsc::Receiver<Msg>) -> Result<(), Failure> {
    loop {
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            Msg::WatchEnded { kind: 3, seq: 7, r } => return r,
            Msg::Changed => {}
            m => panic!("{m:?}"),
        }
    }
}

/// fit's `TestInformerWatch`: a watch's events, a bookmark, objects past max and an error, in
/// pieces of every size; an object that does not decode; an answer other than 200.
#[test]
fn watch_events() {
    let events = [
        r#"{"type": "BOOKMARK", "object": {"metadata": {"resourceVersion": "7"}}}"#,
        r#"{"type": "ADDED", "object": {"metadata": {"namespace": "n", "name": "a", "resourceVersion": "8", "annotations": {"x": "}{\"]["}}}}"#,
        r#"{"type": "ADDED", "object": {"metadata": {"namespace": "n", "name": "b", "resourceVersion": "9"}}}"#,
        r#"{"type": "MODIFIED", "object": {"metadata": {"namespace": "n", "name": "a"}}}"#,
        r#"{"type": "ERROR", "object": {"kind": "Status", "code": 500, "message": "boom"}}"#,
        r#"{"type": "ADDED", "object": {"metadata": {"namespace": "n", "name": "c"}}}"#,
    ]
    .join("\n");
    for size in [1, 2, 7, 64, events.len()] {
        let (mut w, store, rx) = watch(1);
        w.head(200, "OK");
        for piece in events.as_bytes().chunks(size) {
            w.data(piece);
        }
        w.end(Ok(()));
        assert_eq!(
            ended(&rx),
            Err(Failure::Other("the watch failed: 500 boom".to_owned())),
            "{size}"
        );
        let st = lock(&store);
        assert_eq!(st.rv, "9", "{size}");
        assert!(st.full, "{size}");
        assert_eq!(st.objs.keys().collect::<Vec<_>>(), ["n/a"], "{size}");
        assert_eq!(
            st.objs["n/a"].meta().annotation("x"),
            None,
            "the MODIFIED replaced it"
        );
    }

    let (mut w, store, rx) = watch(10);
    w.head(200, "");
    w.data(br#"{"type": "DELETED", "object": {"metadata": {"namespace": "n", "name": "x"}}} {"type": "ERROR", "object": {"code": 410}}"#);
    assert_eq!(ended(&rx), Err(Failure::Gone));
    assert!(lock(&store).objs.is_empty());

    let (mut w, _, rx) = watch(10);
    w.head(200, "");
    w.data(br#"{"type": "ADDED", "object": 1}"#);
    match ended(&rx) {
        Err(Failure::Other(e)) => assert!(e.starts_with("an event of things: "), "{e}"),
        r => panic!("{r:?}"),
    }

    let (mut w, _, rx) = watch(10);
    w.head(403, "Forbidden");
    w.data(br#"{"kind":"Status","message":"secrets is forbidden"}"#);
    w.end(Ok(()));
    assert_eq!(
        ended(&rx),
        Err(Failure::Other(
            "GET /a: 403 Forbidden: secrets is forbidden".to_owned()
        ))
    );

    let (mut w, _, rx) = watch(10);
    w.head(200, "");
    w.data(b"{\"type\": \"ADDED\", \"object\": {\"metadata\": {\"name\": \"");
    w.data(&vec![b'a'; super::watch::MAX_EVENT_BYTES]);
    match ended(&rx) {
        Err(Failure::Other(e)) => assert!(e.contains("is larger than"), "{e}"),
        r => panic!("{r:?}"),
    }
    // What comes after an end is not read, and the end is said once.
    w.end(Err("closed".to_owned()));
    assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());

    // The watch ends in time: Ok; a change wakes the provider once until it is taken.
    let (mut w, store, rx) = watch(1000);
    w.head(200, "");
    let start = Instant::now();
    for i in 0..100 {
        w.data(
            format!(r#"{{"type": "ADDED", "object": {{"metadata": {{"namespace": "n", "name": "o{i}", "resourceVersion": "{i}"}}}}}}"#)
                .as_bytes(),
        );
    }
    w.end(Ok(()));
    assert!(matches!(rx.recv().unwrap(), Msg::Changed));
    assert_eq!(ended(&rx), Ok(()));
    let st = lock(&store);
    assert_eq!((st.objs.len(), st.rv.as_str()), (100, "99"));
    drop(st);
    assert!(start.elapsed() < Duration::from_secs(5));
}
