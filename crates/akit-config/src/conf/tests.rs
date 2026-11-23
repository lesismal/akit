//! The `.conf` reader against fit's own tests (`adapter/fitconf`): each config converts into the
//! same tree as its JSON, and each mistake is reported where fit reports it. The templates of the
//! pinned fit, in `.conf` and in JSON, convert into the same trees.

use std::path::{Path, PathBuf};

use super::*;
use crate::json::{self, Kind, position};

fn no_env(_: &str) -> Option<String> {
    None
}

fn conf(src: &str) -> Result<Node, ParseError> {
    parse(src, &no_env)
}

/// Whether two trees hold the same JSON value, whatever the order of objects' members and
/// wherever each was written.
fn same(a: &Node, b: &Node) -> bool {
    match (&a.kind, &b.kind) {
        (Kind::Object(x), Kind::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|m| {
                    y.iter()
                        .find(|n| n.key == m.key)
                        .is_some_and(|n| same(&m.value, &n.value))
                })
        }
        (Kind::Array(x), Kind::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (x, y) => x == y,
    }
}

/// The tree as compact JSON, for messages.
fn show(n: &Node) -> String {
    match &n.kind {
        Kind::Null => "null".into(),
        Kind::Bool(b) => b.to_string(),
        Kind::Number(s) => s.clone(),
        Kind::Str(s) => format!("{s:?}"),
        Kind::Array(a) => format!("[{}]", a.iter().map(show).collect::<Vec<_>>().join(",")),
        Kind::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|m| format!("{:?}:{}", m.key, show(&m.value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn check_same(name: &str, src: &str, want: &str) {
    let got = conf(src).unwrap_or_else(|e| panic!("{name}: {}", e.msg));
    let want = json::parse(want).unwrap_or_else(|e| panic!("{name}: bad JSON: {}", e.msg));
    assert!(
        same(&got, &want),
        "{name}:\n got {}\nwant {}",
        show(&got),
        show(&want)
    );
}

/// The error for `src`, as `line:col: message`.
fn error(src: &str) -> String {
    let e = conf(src).err().unwrap_or_else(|| panic!("{src:?} reads"));
    let (line, col) = position(src, e.off);
    format!("{line}:{col}: {}", e.msg)
}

#[test]
fn converts() {
    let env = |name: &str| (name == "FIT_TEST_PORT").then(|| "9090".to_owned());
    let got = parse(
        "schema_version 1\nlistener web {\n\taddress :{$FIT_TEST_PORT}\n\tprotocols http/1.1\n\tprotocols h2c\n}\nlistener other {\n\taddress {$FIT_TEST_UNSET:127.0.0.1:1}\n}",
        &env,
    )
    .unwrap();
    let want = json::parse(
        r#"{"schema_version": 1, "listeners": [
            {"name": "web", "address": ":9090", "protocols": ["http/1.1", "h2c"]},
            {"name": "other", "address": "127.0.0.1:1"}]}"#,
    )
    .unwrap();
    assert!(same(&got, &want), "{}", show(&got));

    for (name, src, want) in [
        (
            "plural and name in the block",
            "listeners {\n\tname web; address :8080\n}",
            r#"{"listeners": [{"name": "web", "address": ":8080"}]}"#,
        ),
        (
            "objects, booleans, empty blocks",
            "upstream app {\n\taddresses 127.0.0.1:9000\n\tsticky_cookie {\n\t\tsecure\n\t\tmax_age 24h\n\t}\n}\nadmin {}\nauto_https {\n\tdisable false\n}",
            r#"{"upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"], "sticky_cookie": {"secure": true, "max_age": "24h"}}],
                "admin": {}, "auto_https": {"disable": false}}"#,
        ),
        (
            "maps, quoting and comments",
            "route { # a comment\n\tmatch {\n\t\theader {\n\t\t\tX-Env prod staging\n\t\t\t\"X Odd\" \"a b\"\n\t\t}\n\t\tquery { debug }\n\t\texpr `Host(\"example.com\") && Method(\"GET\")`\n\t}\n\thandler respond {\n\t\theaders {\n\t\t\tCache-Control no-store\n\t\t}\n\t\tbody \"a \\\"quoted\\\" line\\n# not a comment\"\n\t\tstatus 201\n\t}\n}",
            r#"{"routes": [{"match": {"header": {"X-Env": ["prod", "staging"], "X Odd": ["a b"]}, "query": {"debug": []},
                "expr": "Host(\"example.com\") && Method(\"GET\")"},
                "handler": {"module": "respond", "params": {"headers": {"Cache-Control": "no-store"}, "body": "a \"quoted\" line\n# not a comment", "status": 201}}}]}"#,
        ),
        (
            "modules",
            "route {\n\tmiddleware compress\n\tmiddleware headers { response { delete Server } }\n\thandler proxy {\n\t\tupstream app\n\t\tupstreams {\n\t\t\tupstream canary\n\t\t\tweight 5\n\t\t}\n\t\tretry { attempts 2; statuses 502 503 }\n\t}\n}",
            r#"{"routes": [{"middleware": [{"module": "compress"}, {"module": "headers", "params": {"response": {"delete": ["Server"]}}}],
                "handler": {"module": "proxy", "params": {"upstream": "app", "upstreams": [{"upstream": "canary", "weight": 5}],
                "retry": {"attempts": 2, "statuses": [502, 503]}}}}]}"#,
        ),
        (
            "a value of the wrong type is kept for the check",
            "limits {\n\tmax_conns \"100\"\n\tmax_body_bytes lots\n}",
            r#"{"limits": {"max_conns": "100", "max_body_bytes": "lots"}}"#,
        ),
        (
            "an unknown field is kept for the check",
            "bogus {\n\ta 1\n\tb x y\n}",
            r#"{"bogus": {"a": 1, "b": ["x", "y"]}}"#,
        ),
        (
            "a site",
            "site example.com www.example.com {\n\tlisteners web\n\tmatch { method GET }\n\tmiddleware compress\n\troute /api/* /v2/* {\n\t\tname api\n\t\tmiddleware headers { response { delete Server } }\n\t\thandler proxy { upstream app }\n\t}\n\troute {\n\t\tpriority 5\n\t\tmatch { method POST }\n\t\thandler respond\n\t}\n}\nroute last {\n\thandler respond\n}",
            r#"{"routes": [
                {"match": {"host": ["example.com", "www.example.com"], "path": ["/api/*", "/v2/*"], "method": ["GET"]},
                 "listeners": ["web"], "name": "api",
                 "middleware": [{"module": "compress"}, {"module": "headers", "params": {"response": {"delete": ["Server"]}}}],
                 "handler": {"module": "proxy", "params": {"upstream": "app"}}},
                {"match": {"host": ["example.com", "www.example.com"], "method": ["POST"]},
                 "listeners": ["web"], "priority": 5, "middleware": [{"module": "compress"}], "handler": {"module": "respond"}},
                {"name": "last", "handler": {"module": "respond"}}]}"#,
        ),
        (
            "a comment at the end of the file",
            "admin {}\n# the end",
            r#"{"admin": {}}"#,
        ),
        (
            "escapes",
            "admin {\n\taddress \"a\\rb\\qc\\{d\"\n}\n",
            r#"{"admin": {"address": "a\rb\\qc{d"}}"#,
        ),
        (
            "fields of no known type",
            &format!(
                "bogus {{\n\tflag\n\tbig {}\n\tnone null\n}}\n",
                "1".repeat(70)
            ),
            &format!(
                r#"{{"bogus": {{"flag": true, "big": "{}", "none": null}}}}"#,
                "1".repeat(70)
            ),
        ),
        ("an empty file", "# nothing\n", "{}"),
    ] {
        check_same(name, src, want);
    }
}

#[test]
fn shorthands() {
    for (name, src, want) in [
        (
            "def and use",
            "def pool { max_conns 8192; connect_timeout 5s }\nupstream a { addresses 10.0.0.1:80; use pool }\nupstream b { addresses 10.0.0.2:80; use pool }\n",
            r#"{"upstreams": [
                {"name": "a", "addresses": ["10.0.0.1:80"], "max_conns": 8192, "connect_timeout": "5s"},
                {"name": "b", "addresses": ["10.0.0.2:80"], "max_conns": 8192, "connect_timeout": "5s"}]}"#,
        ),
        (
            "defs using defs, at the top and in blocks",
            "def hdr { middleware headers { response { delete Server } } }\ndef routes {\n\troute a { match { path /a }; use hdr; handler respond }\n}\nuse routes\n",
            r#"{"routes": [{"name": "a", "match": {"path": ["/a"]},
                "middleware": [{"module": "headers", "params": {"response": {"delete": ["Server"]}}}],
                "handler": {"module": "respond"}}]}"#,
        ),
        (
            "tls",
            "listener a { address :443; tls cert.pem key.pem }",
            r#"{"listeners": [{"name": "a", "address": ":443", "tls": {"certificates": [{"cert_file": "cert.pem", "key_file": "key.pem"}]}}]}"#,
        ),
        (
            "tls with a block",
            "listener a { address :443; tls a.pem a.key { certificates { cert_file b.pem; key_file b.key } } }",
            r#"{"listeners": [{"name": "a", "address": ":443", "tls": {"certificates": [
                {"cert_file": "a.pem", "key_file": "a.key"}, {"cert_file": "b.pem", "key_file": "b.key"}]}}]}"#,
        ),
        (
            "listener",
            "listener a :8080\nlistener b :8443 h2 h3 { tls c.pem k.pem }\n",
            r#"{"listeners": [{"name": "a", "address": ":8080"},
                {"name": "b", "address": ":8443", "protocols": ["h2", "h3"], "tls": {"certificates": [{"cert_file": "c.pem", "key_file": "k.pem"}]}}]}"#,
        ),
        (
            "upstream",
            "upstream a 10.0.0.1:80 10.0.0.2:80 { max_conns 8 }\n",
            r#"{"upstreams": [{"name": "a", "addresses": ["10.0.0.1:80", "10.0.0.2:80"], "max_conns": 8}]}"#,
        ),
        (
            "site *",
            "site * {\n\tlisteners web\n\troute /a /b { handler respond }\n\troute { handler respond { status 404 } }\n}\n",
            r#"{"routes": [
                {"match": {"path": ["/a", "/b"]}, "listeners": ["web"], "handler": {"module": "respond"}},
                {"listeners": ["web"], "handler": {"module": "respond", "params": {"status": 404}}}]}"#,
        ),
        (
            "bytes",
            "limits { max_header_bytes 16KiB; max_pending_bytes 1gib }\nroute a {\n\tmiddleware cache { max_bytes 1GB; max_entry_bytes 2MiB }\n\thandler respond\n}\n",
            r#"{"limits": {"max_header_bytes": 16384, "max_pending_bytes": 1073741824},
                "routes": [{"name": "a", "middleware": [{"module": "cache", "params": {"max_bytes": 1000000000, "max_entry_bytes": 2097152}}],
                "handler": {"module": "respond"}}]}"#,
        ),
        (
            "bytes in no unit, or quoted, are left for the check",
            "limits { max_header_bytes 16XB; max_body_bytes \"16KiB\"; max_conns 1KiB }",
            r#"{"limits": {"max_header_bytes": "16XB", "max_body_bytes": "16KiB", "max_conns": "1KiB"}}"#,
        ),
    ] {
        check_same(name, src, want);
    }
}

#[test]
fn errors() {
    for (src, want) in [
        (
            "listener web {\n\taddress :1\n",
            "1:14: this { is not closed",
        ),
        ("}\n", "1:1: a } closes no block"),
        ("admin {\n\taddress \"x\n}\n", "2:10: a quote is not closed"),
        (
            "admin {\n\taddress `x\n}\n",
            "2:10: a backquote is not closed",
        ),
        ("admin {} x\n", "1:10: a block's } ends its directive"),
        ("{\n}\n", "1:1: a block needs a directive before its {"),
        (
            "admin {\n\taddress a b\n}\n",
            "2:12: address takes one value",
        ),
        ("admin {\n\taddress\n}\n", "2:2: address takes a value"),
        (
            "admin {\n\taddress { x }\n}\n",
            "2:2: address takes a value, not a block",
        ),
        ("admin x\n", "1:7: admin takes a block, not arguments"),
        ("admin {}\nadmin {}\n", "2:1: admin is given twice"),
        (
            "route a b {}\n",
            "1:1: route takes one argument, its name, and a block",
        ),
        (
            "listener a {\n\tname b\n}\n",
            "1:10: the name is given twice",
        ),
        (
            "route {\n\thandler\n}\n",
            "2:2: handler takes the name of a module",
        ),
        (
            "route {\n\tmatch {\n\t\tpath x { }\n\t}\n}\n",
            "3:3: path takes a list of values, not a block",
        ),
        (
            "route {\n\tmatch {\n\t\theader {\n\t\t\tA 1\n\t\t\tA 2\n\t\t}\n\t}\n}\n",
            "5:4: \"A\" is a key twice",
        ),
        ("admin \"a\"b\n", "1:10: a quoted word must end"),
        ("admin {$}\n", "1:7: {$} names no environment variable"),
        ("site {\n}\n", "1:1: a site needs its host names"),
        (
            "site a.example\n",
            "1:1: a site needs a block with its routes",
        ),
        ("site a.example {\n}\n", "1:1: a site needs a route"),
        (
            "site a.example {\n\thandler respond\n}\n",
            "2:2: a site holds listeners, priority, match, middleware and its routes, not handler",
        ),
        (
            "site a.example {\n\tmatch { host b }\n\troute { handler respond }\n}\n",
            "2:10: a site's routes take their hosts from the site",
        ),
        (
            "site a.example {\n\troute {\n\t\tmatch { host b }\n\t\thandler respond\n\t}\n}\n",
            "3:11: a site's route takes its hosts from the site",
        ),
        (
            "site a.example {\n\troute /a {\n\t\tmatch { path /b }\n\t\thandler respond\n\t}\n}\n",
            "3:11: the route's paths are given twice",
        ),
        (
            "site a.example {\n\troute /a\n}\n",
            "2:2: a route needs a block with its handler",
        ),
        (
            "site a.example {\n\tlisteners a\n\tlisteners b\n\troute { handler respond }\n}\n",
            "3:2: a site's listeners is given twice",
        ),
        // fit's TestMoreErrors.
        ("bogus 1\nbogus 2\n", "2:1: bogus is given twice"),
        (
            "bogus x {}\n",
            "1:7: bogus takes either arguments or a block",
        ),
        ("bogus {\n\ta 1\n\ta 2\n}\n", "3:2: a is given twice"),
        (
            "bogus {\n\ta x {}\n}\n",
            "2:4: a takes either arguments or a block",
        ),
        (
            "route {\n\tmiddleware\n\thandler respond\n}\n",
            "2:2: middleware takes the name of a module",
        ),
        (
            "route {\n\tmatch { header x }\n\thandler respond\n}\n",
            "2:17: header takes a block of keys and their values, not arguments",
        ),
        (
            "route {\n\tmatch {\n\t\theader {\n\t\t\tX {}\n\t\t}\n\t}\n\thandler respond\n}\n",
            "4:4: \"X\" takes a list of values, not a block",
        ),
        (
            "route {\n\thandler respond {\n\t\theaders {\n\t\t\tX a b\n\t\t}\n\t}\n}\n",
            "4:8: X takes one value; quote one that holds spaces",
        ),
        (
            "route {\n\thandler respond {\n\t\tbogus x {}\n\t}\n}\n",
            "3:9: bogus takes either arguments or a block",
        ),
        (
            "site a.example {\n\troute {\n\t\tmatch { method GET }\n\t\tmatch { method POST }\n\t\thandler respond\n\t}\n}\n",
            "4:3: a route's match is given twice",
        ),
        ("admin \"{$X\"\n", "1:7: {$ is not closed with }"),
        (
            "admin {\n\taddress \"{$}\"\n}\n",
            "2:10: {$} names no environment variable",
        ),
        (
            "admin {\n\taddress {$X\n}\n",
            "2:10: {$ is not closed with }",
        ),
        (
            "admin {\n\taddress `a`b\n}\n",
            "2:13: a quoted word must end",
        ),
        (
            "a {\n{\n}\n}\n",
            "2:1: a block needs a directive before its {",
        ),
        // fit's TestShorthandErrors.
        (
            "upstream a { use pool }\ndef pool { max_conns 1 }\n",
            "1:18: use pool: there is no def pool defined before it",
        ),
        ("def a { }\ndef a { }\n", "2:5: def a is defined twice"),
        (
            "route r { def a { } }\n",
            "1:11: def is given at the top of a file only",
        ),
        ("def a\n", "1:1: def takes its name and a block"),
        ("def a/b { }\n", "1:5: \"a/b\" cannot name a def"),
        ("def a { use a }\nuse a\n", "1:13: def a uses itself"),
        (
            "def a { use b }\ndef b { use a }\nuse a\n",
            "2:13: def a uses itself",
        ),
        ("use\n", "1:1: use takes the defs to read"),
        ("def a { }\nuse a { }\n", "2:1: use takes the defs to read"),
        (
            "listener a { address :1; tls c.pem }\n",
            "1:30: tls takes a certificate's file and its key's",
        ),
        (
            "listener a :1 { address :2 }\n",
            "1:12: address is given twice, as an argument and in the block",
        ),
        (
            "listener a :1 h2 { protocols h3 }\n",
            "1:15: protocols is given twice",
        ),
        (
            "route a /x { handler respond }\n",
            "1:1: route takes one argument, its name, and a block",
        ),
        (
            "site * a.example { route { handler respond } }\n",
            "1:6: * is a site of any host, given alone",
        ),
        // What akit does not read yet.
        ("include more.conf\n", "1:1: not supported yet: include"),
        ("import sites\n", "1:1: not supported yet: import"),
        (
            "route {\n\timport x\n}\n",
            "2:2: import is given at the top of a config file only",
        ),
        ("include\n", "1:1: include takes the files to include"),
    ] {
        let got = error(src);
        assert!(got.contains(want), "{src:?}: {got}, want {want:?}");
    }
}

#[test]
fn bounds() {
    let deep = "a {\n".repeat(MAX_DEPTH + 1);
    assert!(error(&deep).contains("blocks nested more than 64 deep"));
    let words = "\n".repeat(MAX_TOKENS);
    for tail in ["\n", ";", "\"x\""] {
        let src = format!("{words}{tail}");
        assert!(
            conf(&src)
                .unwrap_err()
                .msg
                .contains("more than 1048576 words")
        );
    }
    // Uses nested too deep.
    let mut defs = String::new();
    for i in (1..=18).rev() {
        defs.push_str(&format!("def d{i} {{ use d{} }}\n", i + 1));
    }
    defs.push_str("def d19 { }\nuse d1\n");
    assert!(error(&defs).contains("nested more than 16 deep"));
    // Defs used until the words are too many.
    let big = format!(
        "def big {{\n{}}}\n{}",
        "x 1 2 3 4 5 6 7\n".repeat(MAX_TOKENS / 64),
        "use big\n".repeat(10)
    );
    assert!(error(&big).contains("with the defs it uses"));
}

/// Problems the decoder finds are reported at their place in the `.conf` file.
#[test]
fn positions() {
    let src = "listener web {\n\taddress nowhere\n}\nsite a.example {\n\troute /x {\n\t\thandler respond { status 999 }\n\t}\n}\nroute {\n\tmatch { bogus 1 }\n\thandler respond\n}\n";
    let errs = crate::from_conf(src, Some("fit.conf"))
        .unwrap_err()
        .to_string();
    assert!(
        errs.contains("fit.conf:10:10: routes[1].match.bogus: unknown field"),
        "{errs}"
    );
    assert!(
        errs.contains("fit.conf:2:10: listeners[0].address:"),
        "{errs}"
    );
    assert!(
        errs.contains(
            "fit.conf:6:28: routes[0].handler.params.status: 999 is greater than the maximum 599"
        ),
        "{errs}"
    );
    let e = crate::from_conf("route {\n\tpriority high\n\thandler respond\n}\n", None)
        .unwrap_err()
        .0[0]
        .to_string();
    assert_eq!(
        e,
        "2:11: routes[0].priority: expected an integer, got a string"
    );
    let e = crate::from_conf("upstream app 127.0.0.1:1\n", None)
        .unwrap_err()
        .to_string();
    assert_eq!(e, "1:1: upstreams: not supported yet");
    let e = crate::from_conf("listener web :1 { tls c.pem k.pem }\n", None)
        .unwrap_err()
        .to_string();
    assert_eq!(e, "1:19: listeners[0].tls: not supported yet");
    let e = crate::from_conf("route {\n\thandler static { root /srv }\n}\n", None)
        .unwrap_err()
        .0[0]
        .to_string();
    assert_eq!(
        e,
        "2:10: routes[0].handler.module: not supported yet: the \"static\" handler"
    );
}

/// The same configs in `.conf` and in JSON read into the same model.
#[test]
fn reads_the_same_model() {
    let c = crate::from_conf(
        "listener web :8080 http/1.1 h2c\nsite a.example {\n\troute /x/* {\n\t\tname x\n\t\tpriority -1\n\t\thandler respond { status 201; headers { X-A b }; body \"hi\\n\" }\n\t}\n}\n",
        None,
    )
    .unwrap();
    let j = crate::from_json(
        r#"{"listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2c"]}],
            "routes": [{"name": "x", "priority": -1, "match": {"host": ["a.example"], "path": ["/x/*"]},
            "handler": {"module": "respond", "params": {"status": 201, "headers": {"X-A": "b"}, "body": "hi\n"}}}]}"#,
        None,
    )
    .unwrap();
    assert_eq!(c, j);
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/fixtures")
}

/// Every `.conf` template of the pinned fit converts into the same tree as its JSON twin.
#[test]
fn templates_match_their_json_twins() {
    let root = fixtures().join("fit/templates");
    let mut pairs = vec![(root.join("conf/fit.conf"), root.join("json/fit.json"))];
    let mut stack = vec![root.join("conf/conf.d")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root.join("conf")).unwrap();
                pairs.push((
                    p.clone(),
                    root.join("json").join(rel).with_extension("json"),
                ));
            }
        }
    }
    assert!(pairs.len() >= 9, "{pairs:?}");
    for (c, j) in pairs {
        let csrc = std::fs::read_to_string(&c).unwrap();
        let jsrc = std::fs::read_to_string(&j).unwrap();
        let got = conf(&csrc).unwrap_or_else(|e| {
            let (l, col) = position(&csrc, e.off);
            panic!("{}:{l}:{col}: {}", c.display(), e.msg)
        });
        let want = json::parse(&jsrc).unwrap();
        assert!(
            same(&got, &want),
            "{}:\n got {}\nwant {}",
            c.display(),
            show(&got),
            show(&want)
        );
    }
}

/// Every example of fit-examples converts: what akit-config then makes of it is the gate's.
#[test]
fn examples_convert() {
    let dir = fixtures().join("fit-examples");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path().join("fit.conf");
        if !p.is_file() {
            continue;
        }
        let src = std::fs::read_to_string(&p).unwrap();
        if let Err(e) = conf(&src) {
            let (l, c) = position(&src, e.off);
            panic!("{}:{l}:{c}: {}", p.display(), e.msg);
        }
        n += 1;
    }
    assert!(n >= 50, "only {n} examples");
}
