//! fit's tests of the Gateway API's translation and statuses, which need no API server:
//! `gateway_test.go`, `routes_test.go`, `status_test.go` and `edges_test.go`. Messages that
//! name fit name akit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akit_config::KubernetesProvider;
use akit_config::value::{Value, parse};

use super::gateway::{intersect, redirect, rewrite, selects};
use super::status::{MAX_PARENTS, StatusUpdate};
use super::tests::{Part, base, compact, endpoints, obj, opts, secret_doc, service, test_cert};
use super::translate::{Snapshot, Translator};
use super::types::{LabelSelector, Object, PathModifier, Redirect, Requirement, Shape};

fn route(shape: Shape, ns: &str, name: &str, created: &str, rest: &str) -> Arc<Object> {
    obj(
        shape,
        &format!(
            r#"{{"metadata": {{"namespace": "{ns}", "name": "{name}", "creationTimestamp": "{created}"}}, {rest}}}"#
        ),
    )
}

fn http(ns: &str, name: &str, created: &str, spec: &str) -> Arc<Object> {
    route(
        Shape::HttpRoute,
        ns,
        name,
        created,
        &format!(r#""spec": {spec}"#),
    )
}

fn gateway(doc: &str) -> Arc<Object> {
    obj(Shape::Gateway, doc)
}

fn grant(doc: &str) -> Arc<Object> {
    obj(Shape::Grant, doc)
}

fn plain() -> KubernetesProvider {
    opts(None)
}

/// A translator of `s`, run, with certificates in `/certs` (or nowhere).
fn translated<'o>(
    s: &Snapshot,
    o: &'o KubernetesProvider,
    cert_dir: Option<&str>,
) -> (Translator<'o>, Part) {
    let mut t = Translator::new("k8s", o, cert_dir.map(str::to_owned));
    let data = String::from_utf8(t.run(s)).unwrap();
    if data != "{}" {
        if let Err(e) = akit_config::from_json(&data, None) {
            panic!("the part is not a valid config: {e}\n{data}");
        }
    }
    let p = Part {
        v: parse(&data).unwrap(),
        problems: t.problems.clone(),
    };
    (t, p)
}

fn field(v: &Value, k: &str) -> String {
    v.get(k).map_or_else(|| "null".to_owned(), compact)
}

/// fit's `TestTranslateGateway`.
#[test]
fn translate_gateway() {
    let mut w = base();
    w.http_routes = vec![
        // Attached to every listener that takes it: http (All) and web (Same, with its host
        // name).
        http(
            "default",
            "main",
            "2026-01-01T00:00:00Z",
            r#"{"parentRefs": [{"name": "gw"}],
  "rules": [{
    "matches": [{"path": {"type": "PathPrefix", "value": "/app/"}, "headers": [{"name": "X-Env", "value": "prod"}], "method": "GET"},
                {"path": {"type": "Exact", "value": "/health"}, "queryParams": [{"name": "full", "value": "1"}]}],
    "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "X-A", "value": "1"}], "add": [{"name": "X-B", "value": "2"}], "remove": ["X-C"]}},
                {"type": "ResponseHeaderModifier", "responseHeaderModifier": {"add": [{"name": "X-D", "value": "3"}]}}],
    "backendRefs": [{"name": "web", "port": 80, "weight": 3}, {"name": "api", "port": 8080}, {"name": "gone", "port": 1, "weight": 0}]
  }]}"#,
        ),
        // From another namespace, by section, with its own host names.
        http(
            "team",
            "shop",
            "2026-01-02T00:00:00Z",
            r#"{"parentRefs": [{"namespace": "default", "name": "gw", "sectionName": "http"}, {"namespace": "default", "name": "gw", "sectionName": "picky"}],
  "hostnames": ["shop.example.com"],
  "rules": [{"matches": [{"path": {"value": "/cart"}}],
             "filters": [{"type": "URLRewrite", "urlRewrite": {"hostname": "shop.internal", "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2/cart"}}}],
             "backendRefs": [{"name": "shop", "port": 80}]}]}"#,
        ),
        // Refused by web, which takes its own namespace's only, and by picky, whose selector
        // leaves out "outside".
        http(
            "outside",
            "denied",
            "",
            r#"{"parentRefs": [{"namespace": "default", "name": "gw", "sectionName": "web"}, {"namespace": "default", "name": "gw", "sectionName": "picky"}],
  "rules": [{"backendRefs": [{"name": "x", "port": 80}]}]}"#,
        ),
        // Another controller's.
        http(
            "default",
            "theirs",
            "",
            r#"{"parentRefs": [{"name": "theirs"}], "rules": [{}]}"#,
        ),
        // Backends that cannot be served: another namespace's without a grant, no endpoints
        // ready, and none at all; a redirect.
        http(
            "default",
            "broken",
            "2026-01-03T00:00:00Z",
            r#"{"parentRefs": [{"name": "gw", "port": 80}],
  "rules": [{"matches": [{"path": {"value": "/cross"}}], "backendRefs": [{"name": "shop", "namespace": "team", "port": 80}]},
            {"matches": [{"path": {"value": "/empty"}}], "backendRefs": [{"name": "empty", "port": 80}]},
            {"matches": [{"path": {"value": "/none"}}]},
            {"matches": [{"path": {"value": "/old"}}], "filters": [{"type": "RequestRedirect", "requestRedirect": {"scheme": "https", "hostname": "new.example", "statusCode": 301}}]},
            {"matches": [{"path": {"value": "/re"}, "headers": [{"type": "RegularExpression", "name": "X", "value": ".*"}]}], "backendRefs": [{"name": "web", "port": 80}]},
            {"matches": [{"path": {"value": "/ext"}}], "filters": [{"type": "ExtensionRef"}], "backendRefs": [{"name": "web", "port": 80}]}]}"#,
        ),
    ];
    let o = plain();
    let p = Part::of(&w, &o, None);
    assert_eq!(
        field(&p.v, "listeners"),
        r#"[{"name":"k8s-80","address":":80","protocols":["http/1.1","h2c"]},{"name":"k8s-8080","address":":8080","protocols":["http/1.1","h2c"]},{"name":"k8s-8081","address":":8081","protocols":["http/1.1","h2c"]}]"#
    );
    assert_eq!(
        field(&p.v, "upstreams"),
        r#"[{"name":"default.api.8080","addresses":["10.0.1.1:9000"]},{"name":"default.web.80","addresses":["10.0.0.1:8000","10.0.0.2:8000"],"protocol":"h2c"},{"name":"team.shop.80","addresses":["10.0.3.1:3000"]}]"#
    );
    assert_eq!(
        p.names(),
        [
            "default.main.r0.m0.g0",
            "default.main.r0.m0.g1",
            "default.main.r0.m1.g0",
            "default.main.r0.m1.g1",
            "team.shop.r0.m0",
            "default.broken.r0.m0",
            "default.broken.r1.m0",
            "default.broken.r2.m0",
            "default.broken.r3.m0",
            "default.broken.r5.m0",
        ]
    );
    let pri = |r: &BTreeMap<&str, String>| r["priority"].parse::<i64>().unwrap();

    // main: on http for any host, on web for *.example.com.
    let r = p.route("default.main.r0.m0.g0");
    assert_eq!(r["listeners"], r#"["k8s-80"]"#);
    assert_eq!(
        r["match"],
        r#"{"path":["/app","/app/*"],"method":["GET"],"header":{"X-Env":["prod"]}}"#
    );
    assert_eq!(
        r["handler"],
        r#"{"module":"proxy","params":{"upstreams":[{"upstream":"default.web.80","weight":3},{"upstream":"default.api.8080","weight":1}]}}"#
    );
    assert_eq!(
        r["middleware"],
        r#"[{"module":"headers","params":{"request":{"add":{"X-B":["2"]},"delete":["X-C"],"set":{"X-A":"1"}},"response":{"add":{"X-D":["3"]}}}}]"#
    );
    let r1 = p.route("default.main.r0.m0.g1");
    assert_eq!(r1["listeners"], r#"["k8s-8080"]"#);
    assert!(
        r1["match"].starts_with(r#"{"host":["**.example.com"]"#),
        "{}",
        r1["match"]
    );
    assert!(pri(&r1) > pri(&r));
    let m1 = p.route("default.main.r0.m1.g0");
    assert_eq!(
        m1["match"],
        r#"{"path":["/health"],"query":{"full":["1"]}}"#
    );
    assert!(pri(&m1) > pri(&r), "exact beats prefix");

    // shop: on http and picky, rewritten.
    let s = p.route("team.shop.r0.m0");
    assert_eq!(s["listeners"], r#"["k8s-80","k8s-8081"]"#);
    assert_eq!(
        s["match"],
        r#"{"host":["shop.example.com"],"path":["/cart","/cart/*"]}"#
    );
    assert_eq!(
        s["middleware"],
        r#"[{"module":"rewrite","params":{"add_prefix":"/v2/cart","strip_prefix":["/cart"]}}]"#
    );
    assert_eq!(
        s["handler"],
        r#"{"module":"proxy","params":{"host_header":"shop.internal","upstream":"team.shop.80"}}"#
    );

    // broken: 500, 503, 500, a redirect, and 500 for an unknown filter.
    for (name, want) in [
        ("default.broken.r0.m0", 500),
        ("default.broken.r1.m0", 503),
        ("default.broken.r2.m0", 500),
        ("default.broken.r5.m0", 500),
    ] {
        assert_eq!(
            p.route(name)["handler"],
            format!(r#"{{"module":"respond","params":{{"status":{want}}}}}"#),
            "{name}"
        );
    }
    assert_eq!(
        p.route("default.broken.r3.m0")["middleware"],
        r#"[{"module":"redirect","params":{"status":301,"to":"https://new.example{uri}"}}]"#
    );
    assert_eq!(
        p.problems,
        [
            "gateway default/gw: listener tls: an HTTPS listener needs a certificateRef",
            "httproute outside/denied: no listener of its Gateways takes it: their sections, ports, allowed routes or host names leave it out",
            "httproute default/broken: rule 0: backend 0: service team/shop is in another namespace, and no ReferenceGrant there allows it",
            "httproute default/broken: rule 4: match 0: header X: match type RegularExpression is not supported yet",
            "httproute default/broken: rule 5: match 0: filter ExtensionRef is not supported; the rule answers 500",
        ]
    );

    // The same objects in another order give the same part.
    let data = Translator::new("k8s", &o, None).run(&w);
    let mut shuffled = Snapshot {
        http_routes: w.http_routes.clone(),
        gateways: w.gateways.clone(),
        services: w.services.clone(),
        slices: w.slices.clone(),
        gateway_classes: w.gateway_classes.clone(),
        namespaces: w.namespaces.clone(),
        ..Snapshot::default()
    };
    for i in 0..5 {
        for v in [
            &mut shuffled.http_routes,
            &mut shuffled.gateways,
            &mut shuffled.services,
            &mut shuffled.slices,
            &mut shuffled.gateway_classes,
        ] {
            let n = v.len();
            v.rotate_left(1 + i % n.max(1));
            v.reverse();
        }
        assert_eq!(Translator::new("k8s", &o, None).run(&shuffled), data);
    }

    // A ReferenceGrant lets the cross-namespace backend through.
    w.grants = vec![grant(
        r#"{"metadata": {"namespace": "team", "name": "g"}, "spec": {
  "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "default"}],
  "to": [{"group": "", "kind": "Service", "name": "shop"}]}}"#,
    )];
    let p = Part::of(&w, &o, None);
    assert_eq!(
        p.route("default.broken.r0.m0")["handler"],
        r#"{"module":"proxy","params":{"upstream":"team.shop.80"}}"#
    );
}

/// fit's `TestTranslateOptions`.
#[test]
fn translate_options() {
    let mut w = base();
    w.http_routes = vec![
        http(
            "default",
            "a",
            "",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}], "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}"#,
        ),
        http(
            "team",
            "b",
            "",
            r#"{"parentRefs": [{"namespace": "default", "name": "gw", "sectionName": "http"}], "rules": [{"backendRefs": [{"name": "shop", "port": 80}]}]}"#,
        ),
    ];
    // cluster_ip sends to the Service's address, listen_host binds there.
    let mut o = plain();
    o.backends = akit_config::KubernetesBackends::ClusterIp;
    o.listen_host = "127.0.0.1".to_owned();
    let p = Part::of(&w, &o, None);
    assert_eq!(
        field(&p.v, "upstreams"),
        r#"[{"name":"default.web.80","addresses":["10.96.0.80:80"],"protocol":"h2c"},{"name":"team.shop.80","addresses":["10.96.0.80:80"]}]"#
    );
    assert_eq!(
        field(&p.list("listeners")[0], "address"),
        r#""127.0.0.1:80""#
    );
    // namespaces leaves out the others' routes.
    let mut o = plain();
    o.namespaces = vec!["default".to_owned()];
    assert_eq!(Part::of(&w, &o, None).names(), ["default.a.r0.m0"]);
    // Another controller name serves the other class's Gateway.
    w.http_routes.push(http(
        "default",
        "theirs",
        "",
        r#"{"parentRefs": [{"name": "theirs"}], "rules": [{}]}"#,
    ));
    let mut o = plain();
    "example.net/other".clone_into(&mut o.controller_name);
    let p = Part::of(&w, &o, None);
    assert_eq!(p.names(), ["default.theirs.r0.m0"]);
    assert_eq!(field(&p.list("listeners")[0], "name"), r#""k8s-9999""#);
    // Nothing of akit's is an empty part.
    assert_eq!(
        Translator::new("k8s", &plain(), None).run(&Snapshot::default()),
        b"{}"
    );
}

/// fit's `TestPrecedence`.
#[test]
fn precedence() {
    let mut w = base();
    w.http_routes = vec![http(
        "default",
        "p",
        "",
        r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}], "hostnames": ["a.example.com"],
  "rules": [{"matches": [
    {"path": {"type": "Exact", "value": "/a"}},
    {"path": {"type": "PathPrefix", "value": "/abcdef"}},
    {"path": {"type": "PathPrefix", "value": "/abc"}, "method": "GET"},
    {"path": {"type": "PathPrefix", "value": "/abc"}, "headers": [{"name": "A", "value": "1"}, {"name": "B", "value": "1"}]},
    {"path": {"type": "PathPrefix", "value": "/abc"}, "headers": [{"name": "A", "value": "1"}], "queryParams": [{"name": "q", "value": "1"}, {"name": "r", "value": "1"}]},
    {"path": {"type": "PathPrefix", "value": "/abc"}, "queryParams": [{"name": "q", "value": "1"}]},
    {"path": {"type": "RegularExpression", "value": "/a.*"}}
  ], "backendRefs": [{"name": "web", "port": 80}]}]}"#,
    )];
    let p = Part::of(&w, &plain(), None);
    let pris: Vec<i64> = p
        .list("routes")
        .iter()
        .map(|r| field(r, "priority").parse().unwrap())
        .collect();
    for i in 1..pris.len() {
        assert!(
            pris[i - 1] > pris[i],
            "match {} comes before match {i}: {pris:?}",
            i - 1
        );
    }
    // A route for a host name comes before one for a wildcard, which comes before one for any
    // host, whatever their paths.
    w.http_routes = vec![
        http(
            "default",
            "exact",
            "",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}], "hostnames": ["a.example.com"], "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}"#,
        ),
        http(
            "default",
            "wild",
            "",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}], "hostnames": ["*.example.com"], "rules": [{"matches": [{"path": {"type": "Exact", "value": "/x"}}], "backendRefs": [{"name": "web", "port": 80}]}]}"#,
        ),
        http(
            "default",
            "any",
            "",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}], "rules": [{"matches": [{"path": {"type": "Exact", "value": "/xyzzy"}}], "backendRefs": [{"name": "web", "port": 80}]}]}"#,
        ),
    ];
    let p = Part::of(&w, &plain(), None);
    let pri = |n: &str| p.route(n)["priority"].parse::<i64>().unwrap();
    assert!(pri("default.exact.r0.m0") > pri("default.wild.r0.m0"));
    assert!(pri("default.wild.r0.m0") > pri("default.any.r0.m0"));
    // The Gateway API's wildcard stands for one label or more: akit's "**.".
    assert!(p.route("default.wild.r0.m0")["match"].starts_with(r#"{"host":["**.example.com"]"#));
}

/// fit's `TestIntersect`.
#[test]
fn intersects() {
    let v = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    for (l, r, want) in [
        (None, v(&[]), Some(None)),
        (None, v(&["a.com"]), Some(Some(v(&["a.com"])))),
        (Some("a.com"), v(&[]), Some(Some(v(&["a.com"])))),
        (
            Some("*.a.com"),
            v(&["x.a.com", "b.com", "y.z.a.com"]),
            Some(Some(v(&["x.a.com", "y.z.a.com"]))),
        ),
        (
            Some("x.a.com"),
            v(&["*.a.com"]),
            Some(Some(v(&["x.a.com"]))),
        ),
        (Some("a.com"), v(&["*.a.com"]), None),
        (
            Some("*.a.com"),
            v(&["*.a.com"]),
            Some(Some(v(&["*.a.com"]))),
        ),
        (Some("a.com"), v(&["b.com"]), None),
    ] {
        assert_eq!(intersect(l, &r), want, "{l:?} {r:?}");
    }
}

/// fit's `TestSelects`.
#[test]
fn selectors() {
    let req = |k: &str, op: &str, vs: &[&str]| Requirement {
        key: k.to_owned(),
        operator: op.to_owned(),
        values: vs.iter().map(|x| (*x).to_owned()).collect(),
    };
    let sel = LabelSelector {
        match_labels: vec![("a".to_owned(), "1".to_owned())],
        match_expressions: vec![
            req("b", "In", &["x", "y"]),
            req("c", "NotIn", &["z"]),
            req("d", "Exists", &[]),
            req("e", "DoesNotExist", &[]),
        ],
    };
    let labels = |kv: &[(&str, &str)]| {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<Vec<_>>()
    };
    for (l, want) in [
        (labels(&[("a", "1"), ("b", "x"), ("d", "")]), true),
        (
            labels(&[("a", "1"), ("b", "y"), ("c", "w"), ("d", "1")]),
            true,
        ),
        (labels(&[("a", "2"), ("b", "x"), ("d", "")]), false),
        (labels(&[("a", "1"), ("b", "q"), ("d", "")]), false),
        (
            labels(&[("a", "1"), ("b", "x"), ("c", "z"), ("d", "")]),
            false,
        ),
        (labels(&[("a", "1"), ("b", "x")]), false),
        (
            labels(&[("a", "1"), ("b", "x"), ("d", ""), ("e", "")]),
            false,
        ),
    ] {
        assert_eq!(selects(&sel, &l), want, "{l:?}");
    }
    let bad = LabelSelector {
        match_labels: Vec::new(),
        match_expressions: vec![req("a", "Gt", &[])],
    };
    assert!(!selects(&bad, &labels(&[("a", "1")])));
}

/// fit's `TestFilters`.
#[test]
fn filters() {
    let pm = |typ: &str, full: Option<&str>, prefix: Option<&str>| PathModifier {
        typ: typ.to_owned(),
        replace_full_path: full.map(str::to_owned),
        replace_prefix_match: prefix.map(str::to_owned),
    };
    for (p, path_type, prefix, want) in [
        (
            Some(pm("ReplaceFullPath", Some("/new$1"), None)),
            "Exact",
            "",
            Some(r#"{"module":"rewrite","params":{"regexp":"^.*$","replacement":"/new$$1"}}"#),
        ),
        (
            Some(pm("ReplacePrefixMatch", None, Some("/"))),
            "PathPrefix",
            "/api",
            Some(r#"{"module":"rewrite","params":{"strip_prefix":["/api"]}}"#),
        ),
        (
            Some(pm("ReplacePrefixMatch", None, Some("/v2/"))),
            "PathPrefix",
            "",
            Some(r#"{"module":"rewrite","params":{"add_prefix":"/v2"}}"#),
        ),
        (
            Some(pm("ReplacePrefixMatch", None, Some("/"))),
            "PathPrefix",
            "",
            None,
        ),
        (None, "PathPrefix", "", None),
    ] {
        assert_eq!(
            rewrite(p.as_ref(), path_type, prefix),
            Ok(want.map(str::to_owned)),
            "{p:?}"
        );
    }
    assert!(
        rewrite(
            Some(&pm("ReplacePrefixMatch", None, Some("/x"))),
            "Exact",
            ""
        )
        .is_err()
    );
    for (rd, path_type, prefix, want) in [
        (
            Redirect::default(),
            "PathPrefix",
            "",
            r#"{"module":"redirect","params":{"status":302,"to":"{scheme}://{host}{uri}"}}"#,
        ),
        (
            Redirect {
                hostname: Some("b.example".to_owned()),
                port: Some(8443),
                path: Some(pm("ReplaceFullPath", Some("/x"), None)),
                ..Redirect::default()
            },
            "Exact",
            "",
            r#"{"module":"redirect","params":{"status":302,"to":"{scheme}://b.example:8443/x"}}"#,
        ),
        (
            Redirect {
                path: Some(pm("ReplacePrefixMatch", None, Some("/new/"))),
                ..Redirect::default()
            },
            "PathPrefix",
            "/old",
            r#"{"module":"redirect","params":{"regexp":"^/old(/.*)?$","status":302,"to":"{scheme}://{host}/new$1"}}"#,
        ),
    ] {
        assert_eq!(redirect(&rd, path_type, prefix), Ok(want.to_owned()));
    }
    assert!(
        redirect(
            &Redirect {
                port: Some(1),
                ..Redirect::default()
            },
            "PathPrefix",
            ""
        )
        .is_err()
    );
}

/// fit's `TestMirror`.
#[test]
fn mirror() {
    let mut w = base();
    w.http_routes = vec![http(
        "default",
        "m",
        "",
        r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}],
  "rules": [{"filters": [{"type": "RequestMirror", "requestMirror": {"backendRef": {"name": "api", "port": 8080}, "fraction": {"numerator": 1, "denominator": 4}}},
                         {"type": "RequestMirror", "requestMirror": {"backendRef": {"name": "api", "port": 8080}}}],
             "backendRefs": [{"name": "web", "port": 80}]}]}"#,
    )];
    let p = Part::of(&w, &plain(), None);
    assert_eq!(
        p.route("default.m.r0.m0")["handler"],
        r#"{"module":"proxy","params":{"mirror":{"percent":25,"upstream":"default.api.8080"},"upstream":"default.web.80"}}"#
    );
    assert_eq!(
        p.problems,
        [
            "gateway default/gw: listener tls: an HTTPS listener needs a certificateRef",
            "httproute default/m: rule 0: match 0: only one RequestMirror is supported; the others are left out",
        ]
    );
}

fn cert_file(dir: &str, ns: &str, name: &str, data: &[u8]) -> String {
    let sum = akio_crypto::hash::Sha256::digest(data);
    let hex: String = sum.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{dir}/{ns}.{name}.{hex}.pem")
}

/// fit's `TestHTTPS`.
#[test]
fn https() {
    let (crt_a, key_a) = test_cert(&["a.example.com"]);
    let (crt_b, key_b) = test_cert(&["*.b.example.com"]);
    let mut w = base();
    w.secrets = vec![
        obj(Shape::Secret, &secret_doc("default", "a", &crt_a, &key_a)),
        obj(Shape::Secret, &secret_doc("certs", "b", &crt_b, &key_b)),
        obj(Shape::Secret, &secret_doc("default", "bad", &crt_a, &key_b)),
    ];
    w.gateways = vec![
        gateway(
            r#"{"metadata": {"namespace": "default", "name": "secure", "creationTimestamp": "2026-01-01T00:00:00Z"}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "a", "port": 443, "protocol": "HTTPS", "hostname": "a.example.com", "tls": {"certificateRefs": [{"name": "a"}]}},
  {"name": "b", "port": 443, "protocol": "HTTPS", "hostname": "*.b.example.com", "tls": {"certificateRefs": [{"name": "b", "namespace": "certs"}]}},
  {"name": "pass", "port": 443, "protocol": "TLS", "hostname": "db.example.com", "tls": {"mode": "Passthrough"}},
  {"name": "bad", "port": 8443, "protocol": "HTTPS", "tls": {"certificateRefs": [{"name": "bad"}, {"name": "missing"}, {"kind": "ConfigMap", "name": "x"}]}},
  {"name": "term", "port": 9443, "protocol": "TLS"}]}}"#,
        ),
        gateway(
            r#"{"metadata": {"namespace": "default", "name": "late", "creationTimestamp": "2026-01-02T00:00:00Z"}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "plain", "port": 443, "protocol": "HTTP"}]}}"#,
        ),
    ];
    w.http_routes = vec![http(
        "default",
        "site",
        "",
        r#"{"parentRefs": [{"name": "secure", "sectionName": "a"}], "rules": [{"backendRefs": [{"name": "api", "port": 8080}]}]}"#,
    )];
    w.tls_routes = vec![route(
        Shape::TlsRoute,
        "default",
        "db",
        "",
        r#""spec": {"parentRefs": [{"name": "secure"}], "rules": [{"backendRefs": [{"name": "api", "port": 8080}]}]}"#,
    )];
    let o = plain();
    let (t, p) = translated(&w, &o, Some("/certs"));
    let pem_a = [&crt_a[..], b"\n", &key_a[..]].concat();
    let file_a = cert_file("/certs", "default", "a", &pem_a);
    assert_eq!(
        field(&p.v, "listeners"),
        format!(
            r#"[{{"name":"k8s-443","address":":443","protocols":["http/1.1","h2","tcp"],"tls":{{"certificates":[{{"cert_file":"{file_a}","key_file":"{file_a}"}}]}}}}]"#
        )
    );
    assert_eq!(t.certs.len(), 1);
    assert_eq!(t.certs[&file_a], pem_a);
    let r = p.route("default.site.r0.m0");
    assert_eq!(r["listeners"], r#"["k8s-443"]"#);
    assert_eq!(r["match"], r#"{"host":["a.example.com"]}"#);
    // The TLS route takes the passthrough listener alone, for its host name.
    assert_eq!(
        field(&p.v, "services"),
        r#"[{"name":"default.db.tls","listeners":["k8s-443"],"upstream":"default.api.8080","server_names":["db.example.com"]}]"#
    );
    p.has_problems(&[
        "gateway default/secure: listener b: certificateRef 0: secret certs/b is in another namespace, and no ReferenceGrant there allows it",
        "gateway default/secure: listener bad: certificateRef 0: secret default/bad: ",
        "gateway default/secure: listener bad: certificateRef 1: there is no secret default/missing of type kubernetes.io/tls",
        "gateway default/secure: listener bad: certificateRef 2: /ConfigMap is not a Secret",
        "gateway default/secure: listener term: akit passes the connections of TLS listeners through",
        "gateway default/late: listener plain: port 443 serves tls already",
    ]);
    let late = &t.gws[t.by_key["default/late"]].listeners[0];
    assert_eq!(late.conflict.as_ref().unwrap().0, "ProtocolConflict");
    assert!(late.fit.is_none());
    let secure = &t.gws[t.by_key["default/secure"]].listeners;
    assert_eq!(secure[1].unresolved.as_ref().unwrap().0, "RefNotPermitted");
    assert!(secure[1].fit.is_none());
    assert_eq!(
        secure[3].unresolved.as_ref().unwrap().0,
        "InvalidCertificateRef"
    );
    assert!(secure[3].fit.is_none());

    // A grant lets the Gateway use the other namespace's Secret.
    w.grants = vec![grant(
        r#"{"metadata": {"namespace": "certs", "name": "g"}, "spec": {
  "from": [{"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default"}], "to": [{"group": "", "kind": "Secret"}]}}"#,
    )];
    let (t, p) = translated(&w, &o, Some("/certs"));
    let ls = p.list("listeners");
    assert_eq!(ls.len(), 1);
    assert_eq!(
        match ls[0].get("tls").and_then(|t| t.get("certificates")) {
            Some(Value::Array(a)) => a.len(),
            _ => 0,
        },
        2
    );
    assert_eq!(t.certs.len(), 2);

    // With nowhere to write certificates, HTTPS is not served.
    let (_, p) = translated(&w, &o, None);
    assert_eq!(
        field(&p.v, "listeners"),
        r#"[{"name":"k8s-443","address":":443","protocols":["tcp"]}]"#
    );
    p.has_problems(&[
        "gateway default/secure: listener a: there is no directory to write certificates to",
    ]);
}

/// fit's `TestGRPCRoute`.
#[test]
fn grpc_route() {
    let mut w = base();
    w.gateways = vec![gateway(
        r#"{"metadata": {"namespace": "default", "name": "g"}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "http", "port": 80, "protocol": "HTTP"},
  {"name": "httponly", "port": 81, "protocol": "HTTP", "allowedRoutes": {"kinds": [{"kind": "HTTPRoute"}]}}]}}"#,
    )];
    w.grpc_routes = vec![
        route(
            Shape::GrpcRoute,
            "default",
            "echo",
            "2026-01-01T00:00:00Z",
            r#""spec": {"parentRefs": [{"name": "g"}], "hostnames": ["grpc.example.com"],
  "rules": [
    {"matches": [{"method": {"service": "echo.Echo", "method": "Say"}},
                 {"method": {"service": "echo.Echo"}},
                 {"method": {"method": "Ping"}},
                 {"method": {"type": "RegularExpression", "service": "echo\\..*"}, "headers": [{"name": "x-v", "value": "2"}]}],
     "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {"set": [{"name": "x-a", "value": "1"}]}}],
     "backendRefs": [{"name": "api", "port": 8080}]},
    {"backendRefs": [{"name": "web", "port": 80}]},
    {"matches": [{"method": {"type": "RegularExpression", "service": "("}}], "backendRefs": [{"name": "web", "port": 80}]}]}"#,
        ),
        route(
            Shape::GrpcRoute,
            "default",
            "only",
            "2026-01-02T00:00:00Z",
            r#""spec": {"parentRefs": [{"name": "g", "sectionName": "httponly"}], "rules": [{"backendRefs": [{"name": "api", "port": 8080}]}]}"#,
        ),
    ];
    let o = plain();
    let (t, p) = translated(&w, &o, Some("/certs"));
    assert_eq!(
        p.names(),
        [
            "default.echo.grpc.r0.m0",
            "default.echo.grpc.r0.m1",
            "default.echo.grpc.r0.m2",
            "default.echo.grpc.r0.m3",
            "default.echo.grpc.r1.m0",
        ]
    );
    let want = [
        r#"{"host":["grpc.example.com"],"path":["/echo.Echo/Say"]}"#,
        r#"{"host":["grpc.example.com"],"path":["/echo.Echo/*"]}"#,
        r#"{"host":["grpc.example.com"],"path_regexp":["^/[^/]+/Ping$"]}"#,
        r#"{"host":["grpc.example.com"],"path_regexp":["^/(?:echo\\..*)/[^/]+$"],"header":{"x-v":["2"]}}"#,
        r#"{"host":["grpc.example.com"]}"#,
    ];
    let mut pri = Vec::new();
    for (r, want) in p.list("routes").iter().zip(want) {
        assert_eq!(field(r, "match"), want);
        assert_eq!(field(r, "listeners"), r#"["k8s-80"]"#);
        pri.push(field(r, "priority").parse::<i64>().unwrap_or(0));
    }
    assert!(
        pri[0] > pri[1] && pri[1] > pri[2] && pri[2] > pri[4],
        "{pri:?}"
    );
    let r0 = p.route("default.echo.grpc.r0.m0");
    assert_eq!(
        r0["handler"],
        r#"{"module":"proxy","params":{"upstream":"default.api.8080.h2c"}}"#
    );
    assert_eq!(
        r0["middleware"],
        r#"[{"module":"headers","params":{"request":{"set":{"x-a":"1"}}}}]"#
    );
    // web's port is h2c already: its upstream serves HTTPRoutes and GRPCRoutes alike.
    assert_eq!(
        field(&p.v, "upstreams"),
        r#"[{"name":"default.api.8080.h2c","addresses":["10.0.1.1:9000"],"protocol":"h2c"},{"name":"default.web.80","addresses":["10.0.0.1:8000","10.0.0.2:8000"],"protocol":"h2c"}]"#
    );
    p.has_problems(&[
        r#"grpcroute default/echo: rule 2: match 0: method "" of service "(" is not a regular expression akit takes"#,
        "grpcroute default/only: no listener of its Gateways takes it",
    ]);
    let only = &t.states[1].parents[0];
    assert!(!only.accepted);
    assert_eq!(only.why.as_ref().unwrap().0, "NotAllowedByListeners");
}

/// fit's `TestL4Routes`.
#[test]
fn l4_routes() {
    let mut w = base();
    w.gateways = vec![gateway(
        r#"{"metadata": {"namespace": "default", "name": "l4"}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "tcp", "port": 5432, "protocol": "TCP"},
  {"name": "udp", "port": 53, "protocol": "UDP"},
  {"name": "udp2", "port": 5353, "protocol": "UDP"},
  {"name": "tcp53", "port": 53, "protocol": "TCP"},
  {"name": "pass", "port": 443, "protocol": "TLS", "tls": {"mode": "Passthrough"}}]}}"#,
    )];
    w.services.extend([
        obj(
            Shape::Service,
            r#"{"metadata": {"namespace": "default", "name": "dns"}, "spec": {"ports": [
  {"name": "dns", "port": 53, "protocol": "UDP"}, {"name": "dns-tcp", "port": 53, "protocol": "TCP"}]}}"#,
        ),
        service("default", "db", 5432, "", ""),
        service("default", "db2", 5432, "", ""),
    ]);
    w.slices.extend([
        obj(
            Shape::Slice,
            r#"{"metadata": {"namespace": "default", "name": "dns-x", "labels": {"kubernetes.io/service-name": "dns"}},
  "endpoints": [{"addresses": ["10.0.5.1"]}], "ports": [{"name": "dns", "protocol": "UDP", "port": 5353}, {"name": "dns-tcp", "protocol": "TCP", "port": 5353}]}"#,
        ),
        endpoints(
            "default",
            "db",
            "",
            5432,
            &[("10.0.6.1", true), ("10.0.6.2", true)],
        ),
        endpoints("default", "db2", "", 5432, &[("10.0.7.1", true)]),
    ]);
    let l4 = |shape: Shape, name: &str, created: &str, spec: &str| {
        route(
            shape,
            "default",
            name,
            created,
            &format!(r#""spec": {spec}"#),
        )
    };
    w.tcp_routes = vec![
        l4(
            Shape::TcpRoute,
            "db",
            "2026-01-01T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4", "sectionName": "tcp"}],
  "rules": [{"backendRefs": [{"name": "db", "port": 5432, "weight": 3}, {"name": "db2", "port": 5432}]}]}"#,
        ),
        l4(
            Shape::TcpRoute,
            "late",
            "2026-01-02T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4", "sectionName": "tcp"}], "rules": [{"backendRefs": [{"name": "db", "port": 5432}]}]}"#,
        ),
        l4(
            Shape::TcpRoute,
            "dnstcp",
            "2026-01-03T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4", "port": 53}], "rules": [{"backendRefs": [{"name": "dns", "port": 53}]}]}"#,
        ),
    ];
    w.udp_routes = vec![
        l4(
            Shape::UdpRoute,
            "dns",
            "2026-01-01T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4", "sectionName": "udp"}], "rules": [{"backendRefs": [{"name": "dns", "port": 53}]}]}"#,
        ),
        l4(
            Shape::UdpRoute,
            "noudp",
            "2026-01-02T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4", "sectionName": "udp2"}], "rules": [{"backendRefs": [{"name": "db", "port": 5432}]}]}"#,
        ),
    ];
    w.tls_routes = vec![
        l4(
            Shape::TlsRoute,
            "pass",
            "2026-01-01T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4"}], "hostnames": ["db.example.com", "*.db.example.com"],
  "rules": [{"backendRefs": [{"name": "db", "port": 5432}]}, {"backendRefs": [{"name": "db2", "port": 5432}]}]}"#,
        ),
        l4(
            Shape::TlsRoute,
            "pass2",
            "2026-01-02T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4"}], "hostnames": ["db.example.com"], "rules": [{"backendRefs": [{"name": "db2", "port": 5432}]}]}"#,
        ),
        l4(
            Shape::TlsRoute,
            "rest",
            "2026-01-03T00:00:00Z",
            r#"{"parentRefs": [{"name": "l4"}], "rules": [{"backendRefs": [{"name": "db2", "port": 5432}]}]}"#,
        ),
    ];
    let o = plain();
    let (t, p) = translated(&w, &o, Some("/certs"));
    assert_eq!(
        field(&p.v, "listeners"),
        r#"[{"name":"k8s-53","address":":53","protocols":["tcp"]},{"name":"k8s-53-udp","address":":53","protocols":["udp"]},{"name":"k8s-443","address":":443","protocols":["tcp"]},{"name":"k8s-5432","address":":5432","protocols":["tcp"]}]"#
    );
    assert_eq!(
        field(&p.v, "services"),
        concat!(
            r#"[{"name":"default.pass.tls","listeners":["k8s-443"],"upstream":"default.db.5432","server_names":["*.db.example.com","db.example.com"]},"#,
            r#"{"name":"default.rest.tls","listeners":["k8s-443"],"upstream":"default.db2.5432"},"#,
            r#"{"name":"default.db.tcp","listeners":["k8s-5432"],"upstream":"default.db.tcp"},"#,
            r#"{"name":"default.dnstcp.tcp","listeners":["k8s-53"],"upstream":"default.dns.53"},"#,
            r#"{"name":"default.dns.udp","listeners":["k8s-53-udp"],"upstream":"default.dns.53.udp"}]"#
        )
    );
    // db's two endpoints share its weight of 3, db2's one has 1.
    assert_eq!(
        field(&p.v, "upstreams"),
        concat!(
            r#"[{"name":"default.db.5432","addresses":["10.0.6.1:5432","10.0.6.2:5432"]},"#,
            r#"{"name":"default.db.tcp","addresses":["10.0.6.1:5432","10.0.6.2:5432","10.0.7.1:5432"],"weights":[1000,1000,667]},"#,
            r#"{"name":"default.db2.5432","addresses":["10.0.7.1:5432"]},"#,
            r#"{"name":"default.dns.53","addresses":["10.0.5.1:5353"]},{"name":"default.dns.53.udp","addresses":["10.0.5.1:5353"]}]"#
        )
    );
    p.has_problems(&[
        "tlsroute default/pass: only its first rule is served; the others are left out",
        "tlsroute default/pass2: no listener of its Gateways takes it",
        "tcproute default/late: no listener of its Gateways takes it",
        "udproute default/noudp: rule 0: backend 0: service default/db has no UDP port 5432",
    ]);
    for st in &t.states {
        let k = st.obj.meta().key();
        if k == "default/late" || k == "default/pass2" {
            let ps = &st.parents[0];
            let (reason, msg) = ps.why.as_ref().unwrap();
            assert!(!ps.accepted && *reason == "NotAllowedByListeners", "{k}");
            assert!(msg.starts_with("the listener carries "), "{k}: {msg}");
        }
    }
}

/// fit's `TestTranslateEdges`, but for ExternalName Services, which
/// `allow_external_name_services` would serve and akit refuses as not supported yet.
#[test]
fn translate_edges() {
    let mut w = base();
    w.gateways.push(gateway(
        r#"{"metadata": {"namespace": "default", "name": "edge", "creationTimestamp": "2026-01-05T00:00:00Z"},
  "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "badport", "port": 0, "protocol": "HTTP"},
  {"name": "hpass", "port": 8443, "protocol": "HTTPS", "tls": {"mode": "Passthrough"}},
  {"name": "p1", "port": 9443, "protocol": "TLS", "hostname": "a.example.com", "tls": {"mode": "Passthrough"}},
  {"name": "p2", "port": 9444, "protocol": "TLS", "hostname": "b.example.com", "tls": {"mode": "Passthrough"}},
  {"name": "p3", "port": 9445, "protocol": "TLS", "hostname": "c.example.com", "tls": {"mode": "Passthrough"}}]}}"#,
    ));
    w.services.extend([
        obj(
            Shape::Service,
            r#"{"metadata": {"namespace": "default", "name": "ext"}, "spec": {"type": "ExternalName", "externalName": "db.example.net", "ports": [{"port": 5432}]}}"#,
        ),
        obj(
            Shape::Service,
            r#"{"metadata": {"namespace": "default", "name": "extnone"}, "spec": {"type": "ExternalName", "ports": [{"port": 5432}]}}"#,
        ),
        service("default", "mis", 80, "a", ""),
    ]);
    w.slices.push(endpoints(
        "default",
        "mis",
        "b",
        8000,
        &[("10.0.9.1", true)],
    ));
    w.grants = vec![grant(
        r#"{"metadata": {"namespace": "outside", "name": "g"}, "spec": {
  "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "default"}], "to": [{"group": "", "kind": "Service"}]}}"#,
    )];
    let web = r#""backendRefs": [{"name": "web", "port": 80}]"#;
    w.http_routes = vec![
        http(
            "default",
            "edge",
            "2026-01-01T00:00:00Z",
            &format!(
                r#"{{"parentRefs": [{{"name": "gw", "sectionName": "http"}}, {{"group": "other.example", "kind": "Gateway", "name": "gw"}}],
  "rules": [
    {{"backendRefs": [{{"name": "web", "port": 80, "filters": [{{"type": "RequestHeaderModifier", "requestHeaderModifier": {{"set": [{{"name": "a", "value": "b"}}]}}}}]}}]}},
    {{"backendRefs": [{{"group": "x.example", "kind": "Thing", "name": "web"}}]}},
    {{"backendRefs": [{{"name": "web"}}]}},
    {{"backendRefs": [{{"name": "ext", "port": 5432}}]}},
    {{"backendRefs": [{{"name": "extnone", "port": 5432}}]}},
    {{"backendRefs": [{{"name": "mis", "port": 80}}]}},
    {{"backendRefs": [{{"name": "shop", "namespace": "team", "port": 80}}]}},
    {{"matches": [{{"path": {{"type": "Exact", "value": "x"}}}}, {{"path": {{"type": "PathPrefix", "value": "x"}}}},
                 {{"path": {{"type": "RegularExpression", "value": "("}}}}, {{"path": {{"type": "Weird", "value": "/"}}}},
                 {{"queryParams": [{{"type": "RegularExpression", "name": "q", "value": "x"}}]}}], {web}}},
    {{"filters": [{{"type": "URLRewrite"}}, {{"type": "RequestRedirect"}}, {{"type": "RequestMirror"}}], {web}}},
    {{"filters": [{{"type": "URLRewrite", "urlRewrite": {{"path": {{"type": "Bad"}}}}}}], {web}}},
    {{"filters": [{{"type": "RequestRedirect", "requestRedirect": {{"port": 8080}}}}], {web}}},
    {{"filters": [{{"type": "RequestMirror", "requestMirror": {{"backendRef": {{"name": "web"}}}}}}], {web}}},
    {{"filters": [{{"type": "RequestMirror", "requestMirror": {{"backendRef": {{"name": "empty", "port": 80}}}}}}], {web}}},
    {{"filters": [{{"type": "RequestMirror", "requestMirror": {{"backendRef": {{"name": "api", "port": 8080}}, "percent": 50}}}}], {web}}}]}}"#
            ),
        ),
        // In a namespace there is no Namespace of, on a listener that selects namespaces by
        // their labels.
        http(
            "ghost",
            "far",
            "2026-01-02T00:00:00Z",
            &format!(
                r#"{{"parentRefs": [{{"namespace": "default", "name": "gw", "sectionName": "picky"}}], "rules": [{{{web}}}]}}"#
            ),
        ),
    ];
    w.grpc_routes = vec![route(
        Shape::GrpcRoute,
        "default",
        "g",
        "2026-01-01T00:00:00Z",
        r#""spec": {"parentRefs": [{"name": "gw", "sectionName": "http"}, {"name": "gw", "sectionName": "web"}],
  "rules": [{"matches": [{}, {"method": {"service": "a/b"}}, {"method": {"type": "Weird", "service": "x"}},
                         {"headers": [{"type": "RegularExpression", "name": "x", "value": "y"}]}],
             "backendRefs": [{"name": "api", "port": 8080}]}]}"#,
    )];
    let l4 = |name: &str, spec: &str| {
        route(
            Shape::TlsRoute,
            "default",
            name,
            "2026-01-01T00:00:00Z",
            &format!(r#""spec": {spec}"#),
        )
    };
    w.tls_routes = vec![
        l4(
            "multi",
            r#"{"parentRefs": [{"name": "edge", "sectionName": "p1"}, {"name": "edge", "sectionName": "p2"}],
  "rules": [{"backendRefs": [{"name": "web", "port": 80, "weight": 0}, {"name": "api", "port": 8080}, {"name": "api", "port": 8080}]}]}"#,
        ),
        l4(
            "norules",
            r#"{"parentRefs": [{"name": "edge", "sectionName": "p3"}]}"#,
        ),
    ];
    let p = Part::of(&w, &plain(), Some("/certs"));
    p.has_problems(&[
        "gateway default/edge: listener badport: 0 is not a port",
        "gateway default/edge: listener hpass: an HTTPS listener terminates TLS",
        "httproute default/edge: rule 0: backend 0: filters on a backend are not supported yet",
        "httproute default/edge: rule 1: backend 0: x.example/Thing is not a Service",
        "httproute default/edge: rule 2: backend 0: service default/web: a port is needed",
        "httproute default/edge: rule 3: backend 0: service default/ext is an ExternalName Service, which allow_external_name_services must allow",
        "httproute default/edge: rule 4: backend 0: service default/extnone is an ExternalName Service",
        "httproute default/edge: rule 6: backend 0: service team/shop is in another namespace",
        r#"httproute default/edge: rule 7: match 0: path "x" does not start with '/'"#,
        r#"httproute default/edge: rule 7: match 1: path "x" does not start with '/'"#,
        r#"httproute default/edge: rule 7: match 2: path "(" is not a regular expression akit takes"#,
        "httproute default/edge: rule 7: match 3: path type Weird is not supported",
        "httproute default/edge: rule 7: match 4: query parameter q: match type RegularExpression is not supported yet",
        r#"httproute default/edge: rule 9: match 0: URLRewrite: path type "Bad" is not supported"#,
        "httproute default/edge: rule 10: match 0: RequestRedirect: a port without a hostname is not supported yet",
        "httproute default/edge: rule 11: match 0: RequestMirror: service default/web: a port is needed; no copies are sent",
        "httproute default/edge: rule 12: match 0: RequestMirror: it has no endpoints ready; no copies are sent",
        "httproute ghost/far: no listener of its Gateways takes it",
        "grpcroute default/g: rule 0: match 1: a service or method holds '/'",
        "grpcroute default/g: rule 0: match 2: method match type Weird is not supported",
        "grpcroute default/g: rule 0: match 3: header x: match type RegularExpression is not supported yet",
        "tlsroute default/norules: it has no rules, and so no backends",
    ]);
    let ups: BTreeMap<String, &Value> = p
        .list("upstreams")
        .iter()
        .map(|u| (field(u, "name"), u))
        .collect();
    for name in ["default.ext.5432", "default.extnone.5432", "default.mis.80"] {
        assert!(!ups.contains_key(&format!("\"{name}\"")), "upstream {name}");
    }
    // One TLS service on each host name's listeners, to api's endpoint, named twice, once.
    let multi = ups[r#""default.multi.tls""#];
    assert_eq!(field(multi, "addresses"), r#"["10.0.1.1:9000"]"#);
    assert_eq!(field(multi, "weights"), "[1000]");
    let services: Vec<String> = p
        .list("services")
        .iter()
        .map(|s| field(s, "name"))
        .collect();
    assert_eq!(
        services,
        [r#""default.multi.tls.g0""#, r#""default.multi.tls.g1""#]
    );
    let grpc: Vec<String> = p
        .names()
        .into_iter()
        .filter(|n| n.starts_with("default.g."))
        .collect();
    assert_eq!(grpc, ["default.g.grpc.r0.m0.g0", "default.g.grpc.r0.m0.g1"]);
    assert_eq!(
        p.route("default.edge.r13.m0")["handler"],
        r#"{"module":"proxy","params":{"mirror":{"percent":50,"upstream":"default.api.8080"},"upstream":"default.web.80"}}"#
    );
    let r8 = p.route("default.edge.r8.m0");
    assert_eq!(
        r8["handler"],
        r#"{"module":"proxy","params":{"upstream":"default.web.80"}}"#
    );
    assert_eq!(r8["middleware"], "null");
}

/// fit's `TestSecretTooLarge`.
#[test]
fn secret_too_large() {
    let (crt, key) = test_cert(&["a.example.com"]);
    let big = [&crt[..], &vec![b'#'; 1 << 20]].concat();
    let mut w = base();
    w.secrets = vec![obj(
        Shape::Secret,
        &secret_doc("default", "big", &big, &key),
    )];
    w.gateways = vec![gateway(
        r#"{"metadata": {"namespace": "default", "name": "gw"}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "https", "port": 443, "protocol": "HTTPS", "tls": {"certificateRefs": [{"name": "big"}]}}]}}"#,
    )];
    Part::of(&w, &plain(), Some("/certs")).has_problems(&[
        "gateway default/gw: listener https: certificateRef 0: secret default/big is larger than",
    ]);
}

/// fit's `statusWorld`: a Gateway of fit's with listeners valid and not, and routes it takes
/// and refuses.
fn status_world() -> Snapshot {
    let mut w = base();
    w.gateways = vec![gateway(
        r#"{"metadata": {"namespace": "default", "name": "gw", "generation": 3, "resourceVersion": "7"},
  "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "http", "port": 80, "protocol": "HTTP", "allowedRoutes": {"namespaces": {"from": "All"}}},
  {"name": "web", "port": 8080, "protocol": "HTTP", "hostname": "*.example.com"},
  {"name": "tls", "port": 443, "protocol": "HTTPS"},
  {"name": "odd", "port": 9, "protocol": "SCTP"},
  {"name": "kinds", "port": 81, "protocol": "HTTP", "allowedRoutes": {"kinds": [{"kind": "TCPRoute"}]}}]},
  "status": {"conditions": [{"type": "Accepted", "status": "Unknown", "reason": "Pending", "message": "", "lastTransitionTime": "1970-01-01T00:00:00Z"}]}}"#,
    )];
    let r = |ns: &str, name: &str, spec: &str, extra: &str| {
        obj(
            Shape::HttpRoute,
            &format!(
                r#"{{"metadata": {{"namespace": "{ns}", "name": "{name}", "generation": 1, "resourceVersion": "1"}}, "spec": {spec}{extra}}}"#
            ),
        )
    };
    w.http_routes = vec![
        r(
            "default",
            "ok",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "http"}], "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}"#,
            r#", "status": {"parents": [{"parentRef": {"name": "elsewhere"}, "controllerName": "example.net/other", "conditions": []}]}"#,
        ),
        r(
            "default",
            "nosection",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "nope"}], "rules": [{}]}"#,
            "",
        ),
        r(
            "default",
            "host",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "web"}], "hostnames": ["x.example.org"], "rules": [{}]}"#,
            "",
        ),
        r(
            "outside",
            "far",
            r#"{"parentRefs": [{"namespace": "default", "name": "gw", "sectionName": "web"}], "rules": [{}]}"#,
            "",
        ),
        r(
            "default",
            "broken",
            r#"{"parentRefs": [{"name": "gw", "port": 80}], "rules": [
  {"matches": [{"headers": [{"type": "RegularExpression", "name": "X", "value": ".*"}]}], "backendRefs": [{"name": "nothere", "port": 80}]}]}"#,
            "",
        ),
        // On a listener accepted but not served, its certificate missing: attached and
        // accepted all the same.
        r(
            "default",
            "secure",
            r#"{"parentRefs": [{"name": "gw", "sectionName": "tls"}], "rules": [{"backendRefs": [{"name": "nothere", "port": 80}]}]}"#,
            "",
        ),
        // No longer on akit's Gateway: its entry of akit's goes, the other's stays.
        r(
            "default",
            "stale",
            r#"{"parentRefs": [{"name": "theirs"}], "rules": [{}]}"#,
            r#", "status": {"parents": [{"parentRef": {"group": "gateway.networking.k8s.io", "kind": "Gateway", "name": "gw"}, "controllerName": "lesismal.github.io/akit", "conditions": []},
  {"parentRef": {"name": "theirs"}, "controllerName": "example.net/other", "conditions": []}]}"#,
        ),
        // Not on akit's Gateway, and never was: no status of akit's.
        r(
            "default",
            "theirs",
            r#"{"parentRefs": [{"name": "theirs"}], "rules": [{}]}"#,
            "",
        ),
    ];
    w
}

/// The statuses of `w` after a translation, by kind and key, decoded.
fn patches(w: &Snapshot, o: &KubernetesProvider, now: SystemTime) -> BTreeMap<String, Value> {
    let mut t = Translator::new("k8s", o, None);
    t.run(w);
    t.statuses(now)
        .into_iter()
        .map(
            |StatusUpdate {
                 kind,
                 ns,
                 name,
                 patch,
             }| {
                (
                    format!("{kind} {ns}/{name}"),
                    parse(std::str::from_utf8(&patch).unwrap()).unwrap(),
                )
            },
        )
        .collect()
}

/// Gives `w`'s objects the statuses `ps`, as the API server would.
fn apply(w: &mut Snapshot, ps: &BTreeMap<String, Value>) {
    for (key, p) in ps {
        let status = p.get("status").unwrap();
        let (kind, name) = key.split_once(' ').unwrap();
        let list = match kind {
            "gatewayclasses" => &mut w.gateway_classes,
            "gateways" => &mut w.gateways,
            "httproutes" => &mut w.http_routes,
            _ => panic!("{key}"),
        };
        let o = list
            .iter_mut()
            .find(|o| format!("{}/{}", o.meta().namespace, o.meta().name) == name)
            .unwrap_or_else(|| panic!("no object for {key}"));
        match Arc::make_mut(o) {
            Object::GatewayClass(c) => {
                c.conditions = super::types::conditions(status, "conditions");
            }
            Object::Gateway(g) => g.status = super::types::gateway_status(status),
            Object::Route(r) => {
                r.parents = match status.get("parents") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
            }
            _ => panic!("{key}"),
        }
    }
}

/// The conditions in `v`, by type, as "status reason message lastTransitionTime" and
/// " gen<observedGeneration>" when it has one.
fn conds(v: Option<&Value>) -> BTreeMap<String, String> {
    let s = |c: &Value, k: &str| match c.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let mut out = BTreeMap::new();
    if let Some(Value::Array(cs)) = v {
        for c in cs {
            let mut line = [
                s(c, "status"),
                s(c, "reason"),
                s(c, "message"),
                s(c, "lastTransitionTime"),
            ]
            .join(" ");
            if let Some(g) = c.get("observedGeneration") {
                line.push_str(&format!(" gen{}", compact(g)));
            }
            out.insert(s(c, "type"), line);
        }
    }
    out
}

fn parents(p: &Value) -> Vec<Value> {
    match p.get("status").and_then(|s| s.get("parents")) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

const T0: u64 = 1_792_663_200; // 2026-10-22T10:00:00Z
const AT0: &str = "2026-10-22T10:00:00Z";

/// fit's `TestStatus`.
#[test]
fn statuses() {
    let mut w = status_world();
    let t0 = UNIX_EPOCH + Duration::from_secs(T0);
    let o = plain();
    let ps = patches(&w, &o, t0);

    // akit's class is accepted; the other's is not written.
    let c = conds(
        ps["gatewayclasses /akit"]
            .get("status")
            .and_then(|s| s.get("conditions")),
    );
    assert_eq!(
        c["Accepted"],
        format!("True Accepted akit serves this class's Gateways {AT0}")
    );
    assert!(!ps.contains_key("gatewayclasses /other"));

    let g = &ps["gateways default/gw"];
    assert_eq!(
        field(g.get("metadata").unwrap(), "resourceVersion"),
        r#""7""#
    );
    let gs = g.get("status").unwrap();
    let c = conds(gs.get("conditions"));
    assert_eq!(c["Accepted"], format!("True ListenersNotValid  {AT0} gen3"));
    assert_eq!(c["Programmed"], format!("True Programmed  {AT0} gen3"));
    let listeners: BTreeMap<String, &Value> = match gs.get("listeners") {
        Some(Value::Array(ls)) => ls.iter().map(|l| (field(l, "name"), l)).collect(),
        _ => panic!("no listeners"),
    };
    let http_kinds = r#"[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute"},{"group":"gateway.networking.k8s.io","kind":"GRPCRoute"}]"#;
    for (name, kinds, attached, want) in [
        (
            "http",
            http_kinds,
            "2",
            &[
                ("Accepted", "True Accepted"),
                ("Programmed", "True Programmed"),
                ("ResolvedRefs", "True ResolvedRefs"),
                ("Conflicted", "False NoConflicts"),
            ][..],
        ),
        (
            "web",
            http_kinds,
            "0",
            &[
                ("Accepted", "True Accepted"),
                ("Programmed", "True Programmed"),
            ][..],
        ),
        (
            "tls",
            http_kinds,
            "1",
            &[
                ("Accepted", "True Accepted"),
                ("Programmed", "False Invalid"),
                ("ResolvedRefs", "False InvalidCertificateRef"),
            ][..],
        ),
        (
            "odd",
            "[]",
            "0",
            &[
                ("Accepted", "False UnsupportedProtocol"),
                ("Programmed", "False Invalid"),
            ][..],
        ),
        (
            "kinds",
            "[]",
            "0",
            &[
                ("Accepted", "True Accepted"),
                ("ResolvedRefs", "False InvalidRouteKinds"),
            ][..],
        ),
    ] {
        let l = listeners[&format!("\"{name}\"")];
        assert_eq!(field(l, "supportedKinds"), kinds, "{name}");
        assert_eq!(field(l, "attachedRoutes"), attached, "{name}");
        let c = conds(l.get("conditions"));
        for (typ, prefix) in want {
            assert!(
                c[*typ].starts_with(&format!("{prefix} ")),
                "listener {name}: {typ} is {:?}, want {prefix}",
                c[*typ]
            );
        }
    }

    // Routes: on akit's Gateway, accepted or not and why; their backends.
    for (key, want) in [
        (
            "default/ok",
            &[
                ("Accepted", "True Accepted"),
                ("ResolvedRefs", "True ResolvedRefs"),
            ][..],
        ),
        (
            "default/secure",
            &[
                ("Accepted", "True Accepted"),
                ("ResolvedRefs", "False BackendNotFound"),
            ][..],
        ),
        (
            "default/nosection",
            &[("Accepted", "False NoMatchingParent")][..],
        ),
        (
            "default/host",
            &[("Accepted", "False NoMatchingListenerHostname")][..],
        ),
        (
            "outside/far",
            &[("Accepted", "False NotAllowedByListeners")][..],
        ),
        (
            "default/broken",
            &[
                ("Accepted", "True Accepted"),
                (
                    "ResolvedRefs",
                    "False BackendNotFound service default/nothere does not exist",
                ),
                ("PartiallyInvalid", "True UnsupportedValue"),
            ][..],
        ),
    ] {
        let p = ps
            .get(&format!("httproutes {key}"))
            .unwrap_or_else(|| panic!("{key}: no status"));
        let parents = parents(p);
        let mine = parents.last().unwrap();
        assert_eq!(
            field(mine, "controllerName"),
            r#""lesismal.github.io/akit""#
        );
        assert_eq!(
            field(mine.get("parentRef").unwrap(), "group"),
            r#""gateway.networking.k8s.io""#
        );
        let c = conds(mine.get("conditions"));
        for (typ, prefix) in want {
            let got = &c[*typ];
            assert!(
                got.starts_with(prefix) && got.ends_with(&format!("{AT0} gen1")),
                "{key}: {typ} is {got:?}, want {prefix}"
            );
        }
        assert_eq!(
            c.contains_key("PartiallyInvalid"),
            key == "default/broken",
            "{key}: {c:?}"
        );
    }
    // ok keeps the other controller's entry before its own.
    let ok = parents(&ps["httproutes default/ok"]);
    assert_eq!(ok.len(), 2);
    assert_eq!(field(&ok[0], "controllerName"), r#""example.net/other""#);
    let stale = parents(&ps["httproutes default/stale"]);
    assert_eq!(stale.len(), 1);
    assert_eq!(field(&stale[0], "controllerName"), r#""example.net/other""#);
    assert!(!ps.contains_key("httproutes default/theirs"));

    // Once written, the same objects need nothing more.
    apply(&mut w, &ps);
    let t1 = t0 + Duration::from_secs(3600);
    let again = patches(&w, &o, t1);
    assert!(again.is_empty(), "written again: {again:?}");

    // A backend gone: ResolvedRefs changes, and its time with it; Accepted keeps its own.
    let ok = w
        .http_routes
        .iter_mut()
        .find(|r| r.meta().name == "ok")
        .unwrap();
    if let Object::Route(r) = Arc::make_mut(ok) {
        "gone".clone_into(&mut r.rules[0].backend_refs[0].r.name);
    }
    let ps = patches(&w, &o, t1);
    assert_eq!(ps.len(), 1, "after a change: {ps:?}");
    let parents = parents(&ps["httproutes default/ok"]);
    let c = conds(parents[1].get("conditions"));
    assert_eq!(c["Accepted"], format!("True Accepted  {AT0} gen1"));
    assert!(c["ResolvedRefs"].starts_with("False BackendNotFound"));
    assert!(c["ResolvedRefs"].contains("2026-10-22T11:00:00Z"));
}

/// fit's `TestStatusEdges`.
#[test]
fn status_edges() {
    let mut w = status_world();
    let others = vec![
        r#"{"parentRef": {"name": "elsewhere"}, "controllerName": "example.net/other", "conditions": []}"#;
        MAX_PARENTS
    ]
    .join(",");
    w.http_routes = vec![
        // Its parents full with another controller's: none of akit's fits.
        obj(
            Shape::HttpRoute,
            &format!(
                r#"{{"metadata": {{"namespace": "default", "name": "full"}}, "spec": {{"parentRefs": [{{"name": "gw", "sectionName": "http"}}],
  "rules": [{{"backendRefs": [{{"name": "web", "port": 80}}]}}]}}, "status": {{"parents": [{others}]}}}}"#
            ),
        ),
        // No longer on akit's Gateway, and on no other's: its parents are emptied.
        obj(
            Shape::HttpRoute,
            r#"{"metadata": {"namespace": "default", "name": "gone"}, "spec": {"parentRefs": [{"name": "theirs"}], "rules": [{}]},
  "status": {"parents": [{"parentRef": {"group": "gateway.networking.k8s.io", "kind": "Gateway", "name": "gw"}, "controllerName": "lesismal.github.io/akit", "conditions": []}]}}"#,
        ),
    ];
    let mut o = plain();
    o.addresses = vec!["192.0.2.1".to_owned(), "lb.example.com".to_owned()];
    let out = patches(&w, &o, UNIX_EPOCH + Duration::from_secs(T0));
    assert!(!out.contains_key("httproutes default/full"), "{out:?}");
    assert_eq!(
        field(&out["httproutes default/gone"], "status"),
        r#"{"parents":[]}"#
    );
    assert_eq!(
        field(
            out["gateways default/gw"].get("status").unwrap(),
            "addresses"
        ),
        r#"[{"type":"IPAddress","value":"192.0.2.1"},{"type":"Hostname","value":"lb.example.com"}]"#
    );
}

/// fit's `TestDecodeKinds`, for the Gateway API's kinds.
#[test]
fn decode_gateway_kinds() {
    let Object::Gateway(g) = &*gateway(
        r#"{"metadata": {"namespace": "d", "name": "g", "generation": 4}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "l", "port": 443, "protocol": "HTTPS", "hostname": "a.b", "tls": {"mode": "Terminate", "certificateRefs": [{"name": "s", "namespace": "n"}]},
   "allowedRoutes": {"namespaces": {"from": "Selector", "selector": {"matchExpressions": [{"key": "k", "operator": "In", "values": ["v"]}]}}, "kinds": [{"kind": "HTTPRoute"}]}}]},
  "status": {"addresses": [{"value": "1.2.3.4"}], "listeners": [{"name": "l", "attachedRoutes": 2, "supportedKinds": [], "conditions": []}]}}"#,
    ) else {
        panic!()
    };
    assert_eq!((g.meta.generation, g.class_name.as_str()), (4, "akit"));
    let l = &g.listeners[0];
    assert_eq!((l.port, l.hostname.as_deref()), (443, Some("a.b")));
    let tls = l.tls.as_ref().unwrap();
    assert_eq!(tls.certificate_refs[0].namespace.as_deref(), Some("n"));
    let a = l.allowed_routes.as_ref().unwrap();
    let (from, sel) = a.namespaces.as_ref().unwrap();
    assert_eq!(from, "Selector");
    assert_eq!(sel.as_ref().unwrap().match_expressions[0].values, ["v"]);
    assert_eq!(g.status.listeners[0].attached_routes, 2);
    assert_eq!(g.status.addresses[0].typ, None);
    let Object::Grant(gr) = &*grant(
        r#"{"metadata": {"namespace": "n", "name": "g"}, "spec": {"from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "d"}], "to": [{"group": "", "kind": "Service"}]}}"#,
    ) else {
        panic!()
    };
    assert_eq!(gr.to[0], (String::new(), "Service".to_owned(), None));
}
