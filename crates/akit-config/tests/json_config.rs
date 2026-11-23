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
    // fit's JSON template, as written out, with a proxy param akit does not read yet.
    let e = errs(
        r#"{"schema_version": 1,
            "listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2", "h3"], "tls": {}}],
            "routes": [{"name": "hello", "middleware": [], "handler": {"module": "proxy", "params": {"upstream": "app", "timeout": "60s"}}},
                       {"match": {"method": ["GET"]}, "handler": {"module": "respond"}}],
            "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"], "load_balancing": "p2c"}]}"#,
    );
    assert!(e.only_not_supported_yet(), "{e}");
    let paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "listeners[0].protocols[2]",
            "listeners[0].tls",
            "routes[0].handler.params.timeout",
            "routes[0].middleware",
            "routes[1].match.method",
            "upstreams[0].load_balancing",
        ]
    );
    assert_eq!(
        e.as_slice()[0].msg,
        "not supported yet: the \"h3\" protocol"
    );
    assert_eq!(e.as_slice()[2].msg, "not supported yet");
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
    for (tls, path) in [
        (r#""acme": ["a.example"]"#, "listeners[0].tls.acme"),
        (r#""on_demand": true"#, "listeners[0].tls.on_demand"),
        (
            &format!(r#"{CERT}, "kernel_tls_offload": true"#) as &str,
            "listeners[0].tls.kernel_tls_offload",
        ),
        (
            &format!(r#"{CERT}, "max_version": "1.2""#),
            "listeners[0].tls.max_version",
        ),
        // No certificates: fit obtains them with ACME for the routes' names.
        ("", "listeners[0].tls"),
    ] {
        let e = errs(&format!(
            r#"{{"listeners": [{{"name": "web", "address": ":443", "tls": {{{tls}}}}}]}}"#
        ));
        assert!(e.only_not_supported_yet(), "{tls}: {e}");
        let paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, [path], "{tls}");
    }
}

#[test]
fn no_listeners_is_https_by_default_in_fit() {
    let e = errs(r#"{"routes": [{"handler": {"module": "respond"}}]}"#);
    assert!(e.only_not_supported_yet(), "{e}");
    assert!(e.as_slice()[0].msg.contains("auto_https"));
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
              {"handler": {"module": "teleport"}},
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
        "routes[3].handler.module: there is no handler module \"teleport\"",
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
