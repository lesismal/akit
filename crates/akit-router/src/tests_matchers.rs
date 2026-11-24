//! fit's matcher and expression tests (`router/matchers_test.go`, `router/expr_test.go`,
//! `router/more_test.go`), ported; their allocation tests are in `tests/no_alloc.rs`.

use std::net::IpAddr;

use super::{Expr, MAX_EXPR, Matcher, Parts, Route, Router, parse_prefix};

/// A request as fit's tests build one: `GET example.com/`, changed by the case.
#[derive(Clone)]
struct Req {
    method: String,
    host: String,
    path: Vec<u8>,
    query: String,
    remote: Option<IpAddr>,
    headers: Vec<(String, String)>,
}

impl Default for Req {
    fn default() -> Req {
        Req {
            method: "GET".into(),
            host: "example.com".into(),
            path: b"/".to_vec(),
            query: String::new(),
            remote: None,
            headers: Vec::new(),
        }
    }
}

impl Req {
    fn matches(&self, m: &Matcher) -> bool {
        let h: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        m.matches(&Parts {
            method: &self.method,
            host: &self.host,
            path: &self.path,
            query: &self.query,
            remote_ip: self.remote,
            headers: &h,
        })
    }
}

fn method(m: &str) -> Req {
    Req {
        method: m.into(),
        ..Req::default()
    }
}

fn header(kv: &[&str]) -> Req {
    Req {
        headers: kv
            .chunks(2)
            .map(|c| (c[0].to_owned(), c[1].to_owned()))
            .collect(),
        ..Req::default()
    }
}

fn query(q: &str) -> Req {
    Req {
        query: q.into(),
        ..Req::default()
    }
}

/// Go's `RemoteAddr`, parsed as the listener gives it: `None` for one with no IP address.
fn from(a: &str) -> Req {
    let remote = a
        .parse::<std::net::SocketAddr>()
        .map(|s| s.ip())
        .or_else(|_| a.parse::<IpAddr>())
        .ok()
        .or_else(|| {
            // "[fe80::1%eth0]:1": Rust's parser has no zones; the listener's peer has none.
            let inner = a.strip_prefix('[')?.split(']').next()?;
            inner.split('%').next()?.parse().ok()
        });
    Req {
        remote,
        ..Req::default()
    }
}

fn path(p: &str) -> Req {
    Req {
        path: p.as_bytes().to_vec(),
        ..Req::default()
    }
}

#[track_caller]
fn yes_no(name: &str, m: &Matcher, yes: &[Req], no: &[Req]) {
    for (i, r) in yes.iter().enumerate() {
        assert!(r.matches(m), "{name}: yes[{i}] does not match");
    }
    for (i, r) in no.iter().enumerate() {
        assert!(!r.matches(m), "{name}: no[{i}] matches");
    }
}

#[test]
fn fits_matchers() {
    yes_no(
        "methods",
        &Matcher::methods(&["GET", "POST"]),
        &[method("GET"), method("POST")],
        &[method("PUT"), method("get")],
    );
    yes_no(
        "header present",
        &Matcher::headers(&[("x-env", Vec::<&str>::new())]),
        &[header(&["X-Env", ""]), header(&["X-Env", "prod"])],
        &[header(&[]), header(&["X-Other", "prod"])],
    );
    yes_no(
        "header values",
        &Matcher::headers(&[("X-Env", vec!["prod", "staging"]), ("Accept", vec![])]),
        &[
            header(&["X-Env", "prod", "Accept", "*/*"]),
            header(&["X-Env", "dev", "X-Env", "staging", "Accept", ""]),
        ],
        &[
            header(&["X-Env", "prod"]),
            header(&["X-Env", "Prod", "Accept", "*/*"]),
            header(&["X-Env", "prod,staging", "Accept", "*/*"]),
        ],
    );
    yes_no(
        "query",
        &Matcher::query(&[("v", vec!["2", "a b"]), ("debug", vec![])]),
        &[
            query("v=2&debug"),
            query("debug=0&v=1&v=a+b"),
            query("%64ebug=&v=a%20b"),
        ],
        &[
            query(""),
            query("v=2"),
            query("debug&v=3"),
            query("debug&v=2%"),
            query("debugx&v=2"),
            query("debug&v=%zz"),
        ],
    );
    yes_no(
        "remote IP",
        &Matcher::remote_ip(&["10.0.0.0/8", "192.0.2.7", "2001:db8::/32", "fe80::/10"]).unwrap(),
        &[
            from("10.1.2.3:80"),
            from("192.0.2.7:1"),
            from("[2001:db8::1]:443"),
            from("[::ffff:10.0.0.1]:80"),
            from("10.0.0.1"),
            from("[fe80::1%eth0]:1"),
        ],
        &[
            from("11.0.0.1:80"),
            from("192.0.2.8:1"),
            from("[2001:db9::1]:443"),
            from(""),
            from("pipe"),
        ],
    );
    yes_no(
        "path regexp",
        &Matcher::path_regexp(&[r"^/users/[0-9]+$", r"\.php$"]).unwrap(),
        &[path("/users/42"), path("/a/b.php")],
        &[path("/users/x"), path("/users/42/x"), path("/php")],
    );
}

#[test]
fn fits_matcher_errors() {
    for s in [
        "10.0.0.0/33",
        "10.0.0",
        "",
        "::ffff:10.0.0.0/104",
        "fe80::1%eth0",
        "a.example",
    ] {
        assert!(Matcher::remote_ip(&[s]).is_err(), "RemoteIP({s:?})");
    }
    assert!(Matcher::path_regexp(&["("]).is_err());
    assert_eq!(
        parse_prefix("10.1.2.3/8").unwrap().to_string(),
        "10.0.0.0/8"
    );
    assert_eq!(
        parse_prefix("::ffff:10.0.0.1").unwrap().to_string(),
        "10.0.0.1/32"
    );
}

/// fit's `TestQueryHexEscapes`.
#[test]
fn fits_query_hex_escapes() {
    let m = Matcher::query(&[("v", vec!["a/b"])]);
    for (raw, want) in [("v=a%2fb", true), ("v=a%2Fb", true), ("v=a%2gb", false)] {
        assert_eq!(query(raw).matches(&m), want, "{raw}");
    }
}

/// Not in fit: details of the query, header, prefix and regexp matchers.
#[test]
fn matcher_details() {
    // fit's server takes Host out of the header fields: Header("Host") never matches.
    let m = Matcher::headers(&[("Host", Vec::<&str>::new())]);
    assert!(!header(&["Host", "example.com"]).matches(&m));
    // A query's empty pairs, and Go's `strings.Cut` loop.
    let m = Matcher::query(&[("a", Vec::<&str>::new())]);
    for (q, want) in [
        ("&a", true),
        ("a&", true),
        ("x&&a", true),
        ("&", false),
        ("", false),
    ] {
        assert_eq!(query(q).matches(&m), want, "{q}");
    }
    // Prefixes: the bits are Go's, IPv4 and IPv6 do not mix.
    for bad in [
        "10.0.0.0/08",
        "10.0.0.0/",
        "10.0.0.0/+8",
        "::/129",
        "010.0.0.1",
        "::1/x",
    ] {
        assert!(parse_prefix(bad).is_err(), "{bad}");
    }
    assert_eq!(parse_prefix("0.0.0.0/0").unwrap().to_string(), "0.0.0.0/0");
    assert_eq!(
        parse_prefix("2001:db8::1/32").unwrap().to_string(),
        "2001:db8::/32"
    );
    let p = parse_prefix("0.0.0.0/0").unwrap();
    assert!(p.contains("1.2.3.4".parse().unwrap()));
    assert!(!p.contains("::1".parse().unwrap()));
    assert_eq!(
        Matcher::remote_ip(&["10.0.0.0/33"])
            .unwrap_err()
            .to_string(),
        "\"10.0.0.0/33\" is not an IP address or a CIDR prefix"
    );
    assert_eq!(
        parse_prefix("::ffff:10.0.0.0/104").unwrap_err().to_string(),
        "\"::ffff:10.0.0.0/104\" is an IPv4 prefix written in IPv6: write it in IPv4"
    );
    // Go's \d is ASCII; a path that is not UTF-8 reads as U+FFFD per byte, as in Go.
    let m = Matcher::path_regexp(&[r"^/\d+$"]).unwrap();
    assert!(!path("/٢٠").matches(&m));
    let m = Matcher::path_regexp(&["^/a\u{FFFD}\u{FFFD}b$"]).unwrap();
    let r = Req {
        path: b"/a\xff\xfeb".to_vec(),
        ..Req::default()
    };
    assert!(r.matches(&m));
    let e = Matcher::path_regexp(&["("]).unwrap_err().to_string();
    assert!(
        e.starts_with("\"(\" is not a valid regular expression: "),
        "{e}"
    );
}

/// A route whose value is its name, with matchers.
fn mroute(
    name: &'static str,
    priority: i64,
    hosts: &[&str],
    paths: &[&str],
    matchers: Vec<Matcher>,
) -> Route<&'static str> {
    Route {
        hosts: hosts.iter().map(|s| (*s).to_owned()).collect(),
        paths: paths.iter().map(|s| (*s).to_owned()).collect(),
        matchers,
        priority,
        value: name,
    }
}

fn get() -> Matcher {
    Matcher::methods(&["GET"])
}

/// fit's `panicMatcher`: a matcher no test request satisfies, which reads the header fields, so
/// that [`fits_matchers_not_run`] can count its runs.
fn never() -> Matcher {
    Matcher::headers(&[("X-Never-Present", Vec::<&str>::new())])
}

#[track_caller]
fn check_m(r: &Router<&'static str>, cases: &[(&str, &str, &str, &str)]) {
    for &(m, host, p, want) in cases {
        let got = r
            .find_request(&Parts {
                method: m,
                ..Parts::new(host, p.as_bytes())
            })
            .copied();
        let want = (!want.is_empty()).then_some(want);
        assert_eq!(got, want, "{m} {host}{p}");
    }
}

#[test]
fn fits_matcher_order() {
    // A route whose matchers fail gives way to the next.
    let r = Router::new(vec![
        mroute("post", 0, &[], &["/a"], vec![Matcher::methods(&["POST"])]),
        mroute("get", 0, &[], &["/a"], vec![get()]),
    ]);
    check_m(
        &r,
        &[
            ("GET", "x", "/a", "get"),
            ("POST", "x", "/a", "post"),
            ("PUT", "x", "/a", ""),
        ],
    );
    // More matchers beat fewer, at equal priority and specificity.
    let r = Router::new(vec![
        mroute("plain", 0, &[], &["/a"], vec![]),
        mroute("get", 0, &[], &["/a"], vec![get()]),
        mroute(
            "get-v2",
            0,
            &[],
            &["/a"],
            vec![get(), Matcher::headers(&[("X-V", vec!["2"])])],
        ),
    ]);
    check_m(
        &r,
        &[("GET", "x", "/a", "get"), ("POST", "x", "/a", "plain")],
    );
    // Specificity beats matchers.
    let r = Router::new(vec![
        mroute("get-prefix", 0, &[], &["/a/*"], vec![get()]),
        mroute("exact", 0, &[], &["/a/b"], vec![]),
        mroute("host", 0, &["h.com"], &["/a/*"], vec![]),
    ]);
    check_m(
        &r,
        &[
            ("GET", "x", "/a/b", "exact"),
            ("GET", "x", "/a/c", "get-prefix"),
            ("GET", "h.com", "/a/b", "host"),
            ("POST", "x", "/a/c", ""),
        ],
    );
    // Priority beats specificity, matchers included.
    let r = Router::new(vec![
        mroute("exact", 0, &["h.com"], &["/a"], vec![]),
        mroute("get", 1, &[], &[], vec![get()]),
    ]);
    check_m(
        &r,
        &[
            ("GET", "h.com", "/a", "get"),
            ("POST", "h.com", "/a", "exact"),
        ],
    );
    // A failing exact path falls back to a prefix, then to a less specific host.
    let r = Router::new(vec![
        mroute(
            "exact-post",
            0,
            &["h.com"],
            &["/a/b"],
            vec![Matcher::methods(&["POST"])],
        ),
        mroute(
            "prefix-put",
            0,
            &["h.com"],
            &["/a/*"],
            vec![Matcher::methods(&["PUT"])],
        ),
        mroute(
            "wild",
            0,
            &["*.com"],
            &["/a/b"],
            vec![Matcher::methods(&["PUT", "GET"])],
        ),
        mroute("any", 0, &[], &[], vec![]),
    ]);
    check_m(
        &r,
        &[
            ("POST", "h.com", "/a/b", "exact-post"),
            ("PUT", "h.com", "/a/b", "prefix-put"),
            ("GET", "h.com", "/a/b", "wild"),
            ("DELETE", "h.com", "/a/b", "any"),
        ],
    );
    // Ties go to the route written first.
    let r = Router::new(vec![
        mroute("first", 0, &[], &["/a"], vec![get()]),
        mroute("second", 0, &[], &["/a"], vec![get()]),
    ]);
    check_m(&r, &[("GET", "x", "/a", "first")]);
}

/// fit's "routes after one without matchers are never tried" and "a lower priority route's
/// matchers do not run once a higher one matched", with a request that counts the matchers
/// that read it (fit's `panicMatcher`).
#[test]
fn fits_matchers_not_run() {
    use std::cell::Cell;

    struct Counting<'a>(Parts<'a>, &'a Cell<usize>);
    impl super::Request for Counting<'_> {
        fn method(&self) -> &str {
            self.0.method
        }
        fn host(&self) -> &str {
            self.0.host
        }
        fn path(&self) -> &[u8] {
            self.0.path
        }
        fn query(&self) -> &str {
            self.0.query
        }
        fn remote_ip(&self) -> Option<IpAddr> {
            self.0.remote_ip
        }
        fn any_header(&self, name: &str, f: impl FnMut(&str) -> bool) -> bool {
            self.1.set(self.1.get() + 1);
            self.0.any_header(name, f)
        }
    }
    let runs = Cell::new(0);
    let r = Router::new(vec![
        mroute("plain", 1, &[], &["/a"], vec![]),
        mroute("never", 0, &[], &["/a"], vec![never()]),
    ]);
    let req = Counting(Parts::new("x", b"/a"), &runs);
    assert_eq!(r.find_request(&req).copied(), Some("plain"));
    let r = Router::new(vec![
        mroute("high", 1, &["h.com"], &["/a/*"], vec![]),
        mroute("low", 0, &["h.com"], &["/*"], vec![never()]),
        mroute("low-any", 0, &[], &["/a/b"], vec![never()]),
    ]);
    let req = Counting(Parts::new("h.com", b"/a/b"), &runs);
    assert_eq!(r.find_request(&req).copied(), Some("high"));
    assert_eq!(runs.get(), 0, "a matcher ran");
    // The count is real.
    let r = Router::new(vec![mroute("never", 0, &[], &[], vec![never()])]);
    assert_eq!(
        r.find_request(&Counting(Parts::new("x", b"/"), &runs)),
        None
    );
    assert_eq!(runs.get(), 1);
}

/// fit's `TestSameBucket`: routes for one host and path are tried best first, in whatever order
/// they were given.
#[test]
fn fits_same_bucket() {
    let r = Router::new(vec![
        mroute("post", 0, &[], &["/a"], vec![Matcher::methods(&["POST"])]),
        mroute("any", 0, &[], &["/a"], vec![]),
    ]);
    check_m(
        &r,
        &[
            ("GET", "example.com", "/a", "any"),
            ("POST", "example.com", "/a", "post"),
        ],
    );
}

/// fit's `TestPathTwice`, with a matcher.
#[test]
fn fits_path_twice_with_matchers() {
    let r = Router::new(vec![mroute("a", 0, &[], &["/a", "/a"], vec![get()])]);
    check_m(
        &r,
        &[
            ("GET", "example.com", "/a", "a"),
            ("GET", "example.com", "/b", ""),
        ],
    );
}

#[test]
fn fits_parse_expr() {
    for (input, want) in [
        (r#"Host("a.com")"#, r#"Host("a.com")"#),
        ("Host( `a.com` ,\"b.com\" )", r#"Host("a.com", "b.com")"#),
        (
            r#"Host("a") && Path("/x") || Method("GET")"#,
            r#"Host("a") && Path("/x") || Method("GET")"#,
        ),
        (
            r#"Host("a") || Path("/x") && Method("GET")"#,
            r#"Host("a") || Path("/x") && Method("GET")"#,
        ),
        (
            r#"(Host("a") || Path("/x")) && Method("GET")"#,
            r#"(Host("a") || Path("/x")) && Method("GET")"#,
        ),
        (
            r#"Host("a") && (Path("/x") && Method("GET"))"#,
            r#"Host("a") && (Path("/x") && Method("GET"))"#,
        ),
        (
            r#"(Host("a") && Path("/x")) && Method("GET")"#,
            r#"Host("a") && Path("/x") && Method("GET")"#,
        ),
        (
            r#"Host("a") && (Path("/x") || Method("GET"))"#,
            r#"Host("a") && (Path("/x") || Method("GET"))"#,
        ),
        (
            r#"!Host("a") && !!Path("/")"#,
            r#"!Host("a") && !!Path("/")"#,
        ),
        (r#"((Host("a")))"#, r#"Host("a")"#),
        (
            r#"!(Host("a") || Host("b"))"#,
            r#"!(Host("a") || Host("b"))"#,
        ),
        ("a()", ""),
        (
            r#"Header("X-A") && Header("X-B", "1", "2") && Query("q") && RemoteIP("10.0.0.0/8") && PathRegexp("^/a$") && PathPrefix("/api/")"#,
            r#"Header("X-A") && Header("X-B", "1", "2") && Query("q") && RemoteIP("10.0.0.0/8") && PathRegexp("^/a$") && PathPrefix("/api/")"#,
        ),
        (r#"Path("/a\"bé")"#, r#"Path("/a\"bé")"#),
    ] {
        let e = Expr::parse(input);
        if want.is_empty() {
            assert!(e.is_err(), "{input}: {e:?}");
            continue;
        }
        let e = e.unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(e.to_string(), want, "{input}");
    }
}

#[test]
fn fits_parse_expr_errors() {
    let bangs = "!".repeat(200) + r#"Host("a")"#;
    let parens = "(".repeat(200) + r#"Host("a")"#;
    for (input, want) in [
        ("", "at byte 1: expected a matcher, ! or (, found the end"),
        (
            r#"Host("a") &&"#,
            "at byte 13: expected a matcher, ! or (, found the end",
        ),
        (r#"Host("a") & Path("/")"#, "at byte 11: expected &&"),
        (
            r#"Host("a") Path("/")"#,
            "at byte 11: expected && or ||, found Path",
        ),
        (
            r#"(Host("a")"#,
            "at byte 11: expected ) or an operator, found the end",
        ),
        (
            r#"Host("a"))"#,
            r#"at byte 10: expected && or ||, found ")""#,
        ),
        (r#"Nope("a")"#, "at byte 1: there is no matcher Nope"),
        ("Host", "at byte 5: expected ( after Host, found the end"),
        ("Host()", "at byte 1: Host needs at least one argument"),
        (
            r#"Host("a",)"#,
            r#"at byte 10: expected a string, found ")""#,
        ),
        (
            r#"Host("a" "b")"#,
            r#"at byte 10: expected , or ), found "b""#,
        ),
        ("Host(a)", "at byte 6: expected a string, found a"),
        (r#"Host("a)"#, "at byte 6: the string is not closed"),
        ("Host(`a\n`)", "at byte 6: the string is not closed"),
        (r#"Host("\q")"#, r#"at byte 6: "\q" is not a valid string"#),
        (r#"Host("a") # x"#, "at byte 11: unexpected '#'"),
        (
            r#"Host("*")"#,
            r#"at byte 1: Host: "*" is not a host or a "*." wildcard"#,
        ),
        (
            r#"Host("a.*.com")"#,
            r#"at byte 1: Host: "a.*.com" is not a host or a "*." wildcard"#,
        ),
        (
            r#"Host("")"#,
            r#"at byte 1: Host: "" is not a host or a "*." wildcard"#,
        ),
        (
            r#"Path("x")"#,
            r#"at byte 1: Path: "x" does not start with '/'"#,
        ),
        (
            r#"Method("GET") && PathPrefix("")"#,
            r#"at byte 18: PathPrefix: "" does not start with '/'"#,
        ),
        (
            r#"RemoteIP("10.0.0.0/33")"#,
            r#"at byte 1: RemoteIP: "10.0.0.0/33" is not an IP address or a CIDR prefix"#,
        ),
        (&bangs, "at byte 100: the expression nests deeper than 100"),
        (&parens, "at byte 100: the expression nests deeper than 100"),
        (
            r#"Header("")"#,
            "at byte 1: Header: the header's name is empty",
        ),
        (
            r#"Query("", "1")"#,
            "at byte 1: Query: the query parameter's name is empty",
        ),
    ] {
        let got = Expr::parse(input).map(|e| e.to_string());
        assert_eq!(got.unwrap_err().to_string(), want, "{input}");
    }
    // fit's message ends in Go's regexp error; akit's in akio-regex's.
    let e = Expr::parse(r#"PathRegexp("(")"#).unwrap_err().to_string();
    assert!(
        e.starts_with(r#"at byte 1: PathRegexp: "(" is not a valid regular expression: "#),
        "{e}"
    );
    let long = format!(r#"Host("{}")"#, "a".repeat(MAX_EXPR));
    assert!(Expr::parse(&long).is_err());
    // A long chain is not deep: it parses, and prints back without parentheses.
    let long = r#"Method("GET") && "#.repeat(1000) + r#"Method("GET")"#;
    assert_eq!(Expr::parse(&long).unwrap().to_string(), long);
}

/// The expression of fit's `TestExprMatch` and `TestExprAllocs`.
pub(crate) const EXPR: &str = r#"(Host("example.com", "*.example.org") && (PathPrefix("/api/") || Path("/health"))) || (Method("POST") && !Header("X-Skip") && Query("v", "2")) || RemoteIP("10.0.0.0/8")"#;

#[test]
fn fits_expr_match() {
    let e = Matcher::expr(EXPR).unwrap();
    for (m, host, target, remote, skip, want) in [
        ("GET", "example.com", "/api/x", "192.0.2.1:1", "", true),
        (
            "GET",
            "EXAMPLE.com.:8080",
            "/health",
            "192.0.2.1:1",
            "",
            true,
        ),
        ("GET", "www.example.org", "/api/", "192.0.2.1:1", "", true),
        ("GET", "example.org", "/api/", "192.0.2.1:1", "", false),
        ("GET", "a.b.example.org", "/api/", "192.0.2.1:1", "", false),
        ("GET", "example.com", "/apix", "192.0.2.1:1", "", false),
        ("POST", "other", "/?v=2", "192.0.2.1:1", "", true),
        ("POST", "other", "/?v=2", "192.0.2.1:1", "1", false),
        ("POST", "other", "/?v=3", "192.0.2.1:1", "", false),
        ("GET", "other", "/", "10.9.9.9:1", "", true),
    ] {
        let (p, q) = target.split_once('?').unwrap_or((target, ""));
        let mut r = from(remote);
        r.method = m.into();
        r.host = host.into();
        r.path = p.as_bytes().to_vec();
        r.query = q.into();
        if !skip.is_empty() {
            r.headers.push(("X-Skip".into(), skip.into()));
        }
        assert_eq!(
            r.matches(&e),
            want,
            "{m} {host}{target} from {remote}, skip {skip:?}"
        );
    }
}

/// fit's `FuzzParseExpr` seeds, and more: what parses prints back as an expression that parses
/// to the same.
#[test]
fn expr_round_trips() {
    for s in [
        r#"Host("a.com") && PathPrefix("/api")"#,
        r#"!(Method("GET", "HEAD") || Header("X-A", "1")) && Query("q")"#,
        "Path(`/a`) || RemoteIP(\"10.0.0.0/8\", \"::1\")",
        r#"PathRegexp("^/u/[0-9]+$")"#,
        r#"((Host("*.a.b")))"#,
        r#"Host("a" && "#,
        r#""\x00""#,
        r#"Path("/\x00\xff\u00e9\U0001F600\a\b\f\n\r\t\v\\\"\101")"#,
        "Path(`/a\r\nb`)",
        r#"Path("/\u200b\u00ad\u0085")"#,
        r#"A() || Host("a")"#,
        r#"Host("a") || (Host("b") || Host("c")) && !(Host("d") && Host("e"))"#,
    ] {
        let Ok(e) = Expr::parse(s) else { continue };
        let again = Expr::parse(&e.to_string())
            .unwrap_or_else(|err| panic!("{s:?} parsed to {e}, which does not parse: {err}"));
        assert_eq!(again.to_string(), e.to_string(), "{s:?}");
    }
    assert_eq!(
        Expr::parse(r#"Path("/\x00\xff\u00e9\U0001F600\101\u0085")"#)
            .unwrap()
            .to_string(),
        r#"Path("/\x00\xffé😀A\u0085")"#
    );
    // Raw strings drop carriage returns, as Go's do.
    assert_eq!(
        Expr::parse("Path(`/a\rb`)").unwrap().to_string(),
        r#"Path("/ab")"#
    );
    assert_eq!(
        Expr::parse("Host(\"a\") \u{e9}").unwrap_err().to_string(),
        "at byte 11: unexpected 'Ã'"
    );
}
