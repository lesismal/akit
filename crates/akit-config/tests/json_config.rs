//! Reading configs in JSON: fit's examples, every kind of problem with its path and position,
//! and the line between "not supported yet" and wrong.

// Test helpers outside #[test] functions may panic like the tests themselves.
#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

use akit_config::{
    Certificate, ClientAuth, ClientAuthMode, Config, Errors, Handler, Protocol, TlsVersion,
    from_json,
};

fn ok(src: &str) -> Config {
    match from_json(src, Some("t.json")) {
        Ok(c) => c,
        Err(e) => panic!("unexpected errors:\n{e}"),
    }
}

fn errs(src: &str) -> Errors {
    from_json(src, Some("t.json")).expect_err("expected errors")
}

/// Each problem as `line:col: path: msg` without the file name.
fn lines(src: &str) -> Vec<String> {
    errs(src)
        .as_slice()
        .iter()
        .map(|e| e.to_string().trim_start_matches("t.json:").to_owned())
        .collect()
}

/// fit-examples' `hello`, in JSON.
const HELLO: &str = r#"{
  "listeners": [
    {"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2c"]},
  ],
  "routes": [
    {
      "name": "hello",
      "match": {"path": ["/hello"]},
      "handler": {"module": "respond", "params": {"body": "hello from fit\n"}},
    },
    // The fallback.
    {
      "name": "not-found",
      "priority": -1,
      "handler": {"module": "respond", "params": {"status": 404, "body": "not found\n"}},
    },
  ],
}"#;

#[test]
fn hello_reads() {
    let c = ok(HELLO);
    assert_eq!(c.schema_version, 1);
    assert_eq!(c.listeners.len(), 1);
    assert_eq!(c.listeners[0].protocols, [Protocol::Http11, Protocol::H2c]);
    assert_eq!(c.routes.len(), 2);
    let r = &c.routes[0];
    assert_eq!(r.matcher.as_ref().unwrap().path, ["/hello"]);
    let Handler::Respond(h) = &r.handler else {
        panic!("not a respond handler")
    };
    assert_eq!((h.status, h.body.as_str()), (200, "hello from fit\n"));
    assert_eq!(c.routes[1].priority, -1);
    let Handler::Respond(h) = &c.routes[1].handler else {
        panic!("not a respond handler")
    };
    assert_eq!(h.status, 404);
}

#[test]
fn defaults() {
    let c = ok(r#"{"listeners": [{"name": "a", "address": "127.0.0.1:0"}],
                   "routes": [{"handler": {"module": "respond"}}]}"#);
    assert_eq!(c.listeners[0].protocols, akit_config::DEFAULT_PROTOCOLS);
    // fit's default without TLS is HTTP/1.1 alone (fit `config.DefaultProtocols`).
    assert_eq!(c.listeners[0].protocols, [Protocol::Http11]);
    let r = &c.routes[0];
    assert!(r.name.is_none() && r.listeners.is_empty() && r.matcher.is_none());
    let Handler::Respond(h) = &r.handler else {
        panic!("not a respond handler")
    };
    assert_eq!((h.status, h.headers.len(), h.body.as_str()), (200, 0, ""));
    assert_eq!(ok("{}").listeners.len(), 0);
}

#[test]
fn not_supported_yet_is_not_wrong() {
    // fit's JSON template, as written out, with a proxy param akit does not read yet. Its empty
    // tls gets the routes' host names (auto_https).
    let e = errs(
        r#"{"schema_version": 1,
            "listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2", "h3"], "tls": {}}],
            "routes": [{"name": "hello", "match": {"host": ["www.example.com"]}, "middleware": [{"module": "wasm"}], "handler": {"module": "proxy", "params": {"upstream": "app", "splice_uploads": true}}},
                       {"access": {}, "handler": {"module": "respond"}}],
            "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"], "deny_internal": true}]}"#,
    );
    assert!(e.only_not_supported_yet(), "{e}");
    let paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "routes[0].middleware[0].module",
            "routes[0].handler.params.splice_uploads",
            "routes[1].access",
            "upstreams[0].deny_internal",
        ]
    );
    assert_eq!(e.as_slice()[1].msg, "not supported yet");
}

#[test]
fn tls_reads() {
    let c = ok(r#"{"listeners": [{"name": "s", "address": ":443", "tls": {
        "certificates": [{"cert_file": "a.pem", "key_file": "a.key"},
                         {"cert_file": "b.pem", "key_file": "b.key"}],
        "client_auth": {"ca_files": ["ca.pem"], "mode": "verify_if_given"},
        "max_version": "1.3"}}]}"#);
    let l = &c.listeners[0];
    // fit's default protocols with tls.
    assert_eq!(l.protocols, [Protocol::Http11, Protocol::H2]);
    let t = l.tls.as_ref().unwrap();
    assert_eq!(
        t.certificates,
        [
            Certificate {
                cert_file: "a.pem".into(),
                key_file: "a.key".into()
            },
            Certificate {
                cert_file: "b.pem".into(),
                key_file: "b.key".into()
            }
        ]
    );
    assert_eq!(
        t.client_auth,
        Some(ClientAuth {
            ca_files: vec!["ca.pem".into()],
            mode: ClientAuthMode::VerifyIfGiven
        })
    );
    assert_eq!(t.max_version, TlsVersion::Tls13);

    // Defaults: client_auth's mode require, max_version 1.3 (bare, as YAML writes it).
    let c = ok(
        r#"{"listeners": [{"name": "s", "address": ":443", "protocols": ["h2"], "tls": {
        "certificates": [{"cert_file": "a.pem", "key_file": "a.key"}],
        "client_auth": {"ca_files": ["ca.pem"]}, "max_version": 1.3}}]}"#,
    );
    let t = c.listeners[0].tls.as_ref().unwrap();
    assert_eq!(c.listeners[0].protocols, [Protocol::H2]);
    assert_eq!(
        t.client_auth.as_ref().unwrap().mode,
        ClientAuthMode::Require
    );
    assert_eq!(t.max_version, TlsVersion::Tls13);
    // 1.2: TLS 1.2 alone over TCP (akio API request A9).
    let c = ok(r#"{"listeners": [{"name": "s", "address": ":443", "tls": {
        "certificates": [{"cert_file": "a.pem", "key_file": "a.key"}], "max_version": "1.2"}}]}"#);
    assert_eq!(
        c.listeners[0].tls.as_ref().unwrap().max_version,
        TlsVersion::Tls12
    );
    // h3 (I10), beside TCP's protocols or alone.
    for (protocols, want) in [
        (
            r#""http/1.1", "h2", "h3""#,
            &[Protocol::Http11, Protocol::H2, Protocol::H3][..],
        ),
        (r#""h3""#, &[Protocol::H3][..]),
    ] {
        let c = ok(&format!(
            r#"{{"listeners": [{{"name": "s", "address": ":443", "protocols": [{protocols}], "tls": {{
            "certificates": [{{"cert_file": "a.pem", "key_file": "a.key"}}]}}}}]}}"#
        ));
        assert_eq!(c.listeners[0].protocols, want);
        assert_eq!(Protocol::H3.as_str(), "h3");
    }
    // Without tls, the default stays HTTP/1.1 alone.
    let c = ok(r#"{"listeners": [{"name": "w", "address": ":80"}]}"#);
    assert_eq!(c.listeners[0].protocols, [Protocol::Http11]);
    assert_eq!(c.listeners[0].tls, None);
}

#[test]
fn tls_problems_as_fit_reports_them() {
    const CERT: &str = r#""certificates": [{"cert_file": "a.pem", "key_file": "a.key"}]"#;
    // fit's config_test cases, with a certificate where fit's have ACME names.
    assert_eq!(
        lines(r#"{"listeners": [{"name": "web", "address": ":80", "protocols": ["h2"]}]}"#),
        ["1:64: listeners[0].protocols[0]: h2 needs tls; h2c is HTTP/2 in the clear"]
    );
    assert_eq!(
        lines(
            r#"{"listeners": [{"name": "web", "address": ":80", "protocols": ["http/1.1", "h3"]}]}"#
        ),
        ["1:76: listeners[0].protocols[1]: h3 needs tls"]
    );
    assert_eq!(
        lines(&format!(
            r#"{{"listeners": [{{"name": "web", "address": ":443", "protocols": ["h3", "h3"], "tls": {{{CERT}}}}}]}}"#
        )),
        [r#"1:71: listeners[0].protocols[1]: "h3" is listed twice"#]
    );
    assert_eq!(
        lines(&format!(
            r#"{{"listeners": [{{"name": "web", "address": ":443", "protocols": ["h2c"], "tls": {{{CERT}}}}}]}}"#
        )),
        [
            "1:65: listeners[0].protocols[0]: h2c is HTTP/2 in the clear, which a listener with tls does not serve; use h2"
        ]
    );
    let with_tls = |tls: &str| {
        lines(&format!(
            r#"{{"listeners": [{{"name": "web", "address": ":443", "tls": {{{CERT}, {tls}}}}}]}}"#
        ))
    };
    assert_eq!(
        with_tls(r#""client_auth": {"ca_files": ["ca.pem"], "mode": "maybe"}"#),
        [
            r#"1:170: listeners[0].tls.client_auth.mode: "maybe" is not one of "require", "verify_if_given""#
        ]
    );
    assert_eq!(
        with_tls(r#""client_auth": {"ca_files": []}"#),
        ["1:150: listeners[0].tls.client_auth.ca_files: at least one file is needed"]
    );
    assert_eq!(
        with_tls(r#""client_auth": {}"#),
        ["1:137: listeners[0].tls.client_auth.ca_files: missing required field"]
    );
    for v in [r#""1.1""#, "1.1", "1", "12", "true", r#""tls1.2""#] {
        let e = with_tls(&format!(r#""max_version": {v}"#));
        assert_eq!(e.len(), 1, "{v}: {e:?}");
        assert!(
            e[0].contains("listeners[0].tls.max_version: "),
            "{v}: {e:?}"
        );
        assert!(!e[0].contains("not supported yet"), "{v}: {e:?}");
    }
    let e = lines(
        r#"{"listeners": [{"name": "web", "address": ":443", "tls": {"certificates": [{"cert_file": "a.pem"}]}}]}"#,
    );
    assert_eq!(
        e,
        ["1:76: listeners[0].tls.certificates[0].key_file: missing required field"]
    );
}

#[test]
fn tls_fields_akit_lacks_are_not_supported_yet() {
    const CERT: &str = r#""certificates": [{"cert_file": "a.pem", "key_file": "a.key"}]"#;
    // max_version 1.2 is taken since akio API request A9 (json_config's tls test).
    let tls = format!(r#"{CERT}, "kernel_tls_offload": true"#);
    let e = errs(&format!(
        r#"{{"listeners": [{{"name": "web", "address": ":443", "tls": {{{tls}}}}}]}}"#
    ));
    assert!(e.only_not_supported_yet(), "{tls}: {e}");
    let paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["listeners[0].tls.kernel_tls_offload"]);
}

#[test]
fn wrong_is_wrong() {
    assert_eq!(
        lines(r#"{"listener": []}"#),
        ["1:2: listener: unknown field"]
    );
    assert_eq!(
        lines(r#"{"Listeners": []}"#),
        ["1:2: Listeners: unknown field (did you mean \"listeners\"?)"]
    );
    assert_eq!(
        lines("{\n  \"listeners\": [{\"address\": \":80\"}]\n}"),
        ["2:17: listeners[0].name: missing required field"]
    );
    assert_eq!(
        lines(r#"{"listeners": [{"name": "a", "address": "80"}]}"#),
        [
            "1:41: listeners[0].address: \"80\" is not a host and port, as in \":8080\" or \"127.0.0.1:8080\""
        ]
    );
    assert_eq!(
        lines(r#"{"listeners": [{"name": 1, "address": ":80"}]}"#),
        ["1:25: listeners[0].name: expected a string, got a number"]
    );
    assert_eq!(
        lines(r#"{"schema_version": 2}"#),
        ["1:20: schema_version: 2 is greater than the maximum 1"]
    );
    assert_eq!(
        lines(r#"{"routes": {}}"#),
        ["1:12: routes: expected an array, got an object"]
    );
    assert_eq!(lines("{\"a\": 1, \"a\": 2}"), ["1:10: duplicate key \"a\""]);
    assert_eq!(lines("[1,]x"), ["1:5: invalid JSON: more after the value"]);
}

#[test]
fn every_problem_is_reported() {
    let l = lines(
        r#"{"listeners": [
              {"name": "a", "address": ":80", "protocols": ["http/1.1", "http/1.1", "spdy"]},
              {"name": "a", "address": ":81"},
              {"name": "-b", "address": ":82"}
            ],
            "routes": [
              {"name": "r", "listeners": ["a", "nope"], "handler": {"module": "respond"}},
              {"name": "r", "handler": {"module": "respond"}},
              {"handler": {"module": ""}},
              {"handler": {"module": "teleport"}, "bogus": 1},
              {"handler": {"module": "respond", "params": {"status": 101}}},
              {"handler": {"module": "respond", "params": {"status": 204, "body": "x"}}},
              {"handler": {"module": "respond", "params": {"headers": {"Bad Name": "v", "X": "a\nb"}}}},
              {"handler": {"module": "respond", "params": {"status": 600, "bodyy": ""}}},
              {"match": {"host": ["*.192.0.2.1", "a..b"], "path": ["x", "/a*b"]}, "handler": {"module": "respond"}},
              {}
            ]}"#,
    );
    let msgs: Vec<&str> = l
        .iter()
        .map(|s| s.split_once(": ").map_or(s.as_str(), |x| x.1))
        .collect();
    let want = [
        "listeners[0].protocols[1]: \"http/1.1\" is listed twice",
        "listeners[0].protocols[2]: \"spdy\" is not one of",
        "listeners[1].name: another listener is already named \"a\"",
        "listeners[2].name: \"-b\" is not a valid name",
        "routes[0].listeners[1]: there is no listener named \"nope\"",
        "routes[1].name: another route is already named \"r\"",
        "routes[2].handler.module: the module name is empty",
        "routes[3].bogus: unknown field",
        "routes[4].handler.params.status: 101 is an informational status",
        "routes[5].handler.params.body: a 204 response cannot have a body",
        "routes[6].handler.params.headers: \"Bad Name\" is not a valid header name",
        "routes[6].handler.params.headers: the value of X holds a line break or NUL",
        "routes[7].handler.params.status: 600 is greater than the maximum 599",
        "routes[7].handler.params.bodyy: unknown field",
        "routes[8].match.host[0]: \"*.192.0.2.1\" puts a wildcard on an IP address",
        "routes[8].match.host[1]: \"a..b\" is not a valid host name",
        "routes[8].match.path[0]: \"x\" does not start with '/'",
        "routes[8].match.path[1]: \"/a*b\" has a '*' other than at its end",
        "routes[9].handler: missing required field",
    ];
    for w in want {
        assert!(
            msgs.iter().any(|m| m.starts_with(w)),
            "missing {w:?} in:\n{}",
            l.join("\n")
        );
    }
    assert!(!errs("{}x").only_not_supported_yet());
}

#[test]
fn file_errors_name_the_file() {
    let dir = std::env::temp_dir().join(format!("akit-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bad.json");
    std::fs::write(&p, "{\"x\": 1}").unwrap();
    let e = akit_config::from_json_file(&p).unwrap_err();
    assert!(
        e.to_string()
            .starts_with(&format!("{}:1:2: x: unknown field", p.display()))
    );
    let missing = akit_config::from_json_file(&dir.join("missing.json")).unwrap_err();
    assert_eq!(missing.as_slice()[0].line, 0);
    std::fs::write(&p, [0xff, 0xfe]).unwrap();
    assert!(
        akit_config::from_json_file(&p)
            .unwrap_err()
            .to_string()
            .contains("not UTF-8")
    );
    std::fs::write(&p, HELLO).unwrap();
    assert_eq!(akit_config::from_json_file(&p).unwrap().routes.len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// fit's full JSON template (every field, mostly commented out), when the fit repository sits
/// beside akit's or `FIT_DIR` names it: it must read with nothing but "not supported yet"
/// problems. The compatibility gate (I07) runs fit's pinned templates in CI; this is the quick
/// local check.
#[test]
fn fit_template_if_present() {
    let dir = std::env::var("FIT_DIR")
        .unwrap_or_else(|_| format!("{}/../../../fit", env!("CARGO_MANIFEST_DIR")));
    let path = std::path::Path::new(&dir).join("templates/json/fit.json");
    let Ok(src) = std::fs::read_to_string(&path) else {
        eprintln!("skipped: no fit template at {}", path.display());
        return;
    };
    // Valid since I14 read its upstreams; a later fit may add fields akit refuses by name.
    if let Err(e) = from_json(&src, Some("fit.json")) {
        assert!(e.only_not_supported_yet(), "{e}");
    }
}

/// fit's `TestValidate` "route matchers" case (`config/config_test.go`), and what the fields read.
#[test]
fn route_matchers() {
    let l = lines(
        r#"{"listeners": [{"name": "web", "address": ":80"}], "routes": [
  {"match": {"method": ["GET", "BAD METHOD", ""], "header": {"X-Ok": [], "Bad Name": ["v"]}, "query": {"": ["1"], "ok": []}}, "handler": {"module": "respond"}},
  {"match": {"remote_ip": ["10.0.0.0/8", "10.0.0.0/33", "a.example", "::1"], "path_regexp": ["^/ok$", "("]}, "handler": {"module": "respond"}},
  {"match": {"expr": "Host(\"a.example\") && Nope(\"x\")"}, "handler": {"module": "respond"}}
]}"#,
    );
    let mut want = vec![
        r#"2:32: routes[0].match.method[1]: "BAD METHOD" is not a valid method"#,
        r#"2:46: routes[0].match.method[2]: "" is not a valid method"#,
        // akit writes a map's key as `["key"]`, where fit writes `.key`.
        r#"2:86: routes[0].match.header["Bad Name"]: "Bad Name" is not a valid header name"#,
        r#"2:108: routes[0].match.query[""]: the query parameter's name is empty"#,
        r#"3:42: routes[1].match.remote_ip[1]: "10.0.0.0/33" is not an IP address or a CIDR prefix"#,
        r#"3:57: routes[1].match.remote_ip[2]: "a.example" is not an IP address or a CIDR prefix"#,
        "4:22: routes[2].match.expr: at byte 22: there is no matcher Nope",
    ];
    // fit's ends in Go's regexp error, akit's in akio-regex's.
    let re = l
        .iter()
        .find(|s| {
            s.starts_with(
                "3:103: routes[1].match.path_regexp[1]: \"(\" is not a valid regular expression: ",
            )
        })
        .unwrap_or_else(|| panic!("no path_regexp error in {l:#?}"));
    want.push(re);
    let mut got: Vec<&str> = l.iter().map(String::as_str).collect();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want);

    let c = ok(
        r#"{"listeners": [{"name": "web", "address": ":80"}], "routes": [{"match": {
        "host": ["a.example"], "path": ["/a/*"], "path_regexp": ["^/a/[0-9]+$"], "method": ["GET", "HEAD"],
        "header": {"X-Env": ["prod", "staging"], "Accept": []}, "query": {"v": ["2"]},
        "remote_ip": ["10.0.0.0/8", "::1"], "expr": "Method(\"GET\") || Header(\"X-A\")"},
        "handler": {"module": "respond"}}]}"#,
    );
    let m = c.routes[0].matcher.as_ref().unwrap();
    assert_eq!(m.path_regexp, ["^/a/[0-9]+$"]);
    assert_eq!(m.method, ["GET", "HEAD"]);
    assert_eq!(
        m.header,
        [
            (
                "X-Env".to_owned(),
                vec!["prod".to_owned(), "staging".to_owned()]
            ),
            ("Accept".to_owned(), vec![])
        ]
    );
    assert_eq!(m.query, [("v".to_owned(), vec!["2".to_owned()])]);
    assert_eq!(m.remote_ip, ["10.0.0.0/8", "::1"]);
    assert_eq!(m.expr, r#"Method("GET") || Header("X-A")"#);
    // An empty expression is no expression, as in fit.
    let c = ok(
        r#"{"listeners": [{"name": "web", "address": ":80"}], "routes": [{"match": {"expr": ""}, "handler": {"module": "respond"}}]}"#,
    );
    assert!(c.routes[0].matcher.as_ref().unwrap().expr.is_empty());
    // Wrong types are reported as such.
    assert_eq!(
        lines(r#"{"listeners": [{"name": "web", "address": ":80"}], "routes": [{"match": {"header": {"X": "v"}, "query": [], "expr": 1}, "handler": {"module": "respond"}}]}"#).len(),
        3
    );
}

/// A module the config model does not know is kept, with its params as written, for the
/// registry of a binary `akit build` made (I70) to find and decode, as fit's `module.Load` does;
/// a name fit has and akit does not yet is still refused as not supported yet.
#[test]
fn modules_the_model_does_not_know_are_kept_for_the_registry() {
    let config = from_json(
        r#"{"listeners": [{"name": "web", "address": ":8080"}],
            "routes": [{"middleware": [{"module": "hello", "params": {"greeting": "hi", "n": [1, 2.5]}}],
                        "handler": {"module": "teleport"}}]}"#,
        None,
    )
    .unwrap();
    let route = &config.routes[0];
    let Handler::Module(m) = &route.middleware[0] else {
        panic!("{:?}", route.middleware[0]);
    };
    assert_eq!(m.name, "hello");
    assert!(m.middleware && route.middleware[0].is_middleware());
    assert_eq!(route.middleware[0].module(), "hello");
    let params = m.params.as_ref().unwrap();
    assert_eq!(
        params.get("greeting"),
        Some(&akit_config::value::Value::String("hi".into()))
    );
    assert_eq!(
        params.get("n"),
        Some(&akit_config::value::Value::Array(vec![
            akit_config::value::Value::Number("1".into()),
            akit_config::value::Value::Number("2.5".into()),
        ]))
    );
    assert_eq!(
        route.handler,
        Handler::Module(akit_config::ModuleRef {
            name: "teleport".into(),
            middleware: false,
            params: None,
        })
    );
    assert!(!route.handler.is_middleware());

    let l = lines(
        r#"{"listeners": [{"name": "web", "address": ":8080"}],
            "routes": [{"handler": {"module": "hls"}}, {"handler": {"module": "respond"}, "middleware": [{"module": "wasm"}]}]}"#,
    );
    assert_eq!(l.len(), 2, "{l:?}");
    assert!(l.iter().all(|e| e.contains("not supported yet")), "{l:?}");
}
