//! fit's router table tests (`router/router_test.go`, `router/more_test.go`), the host, path and
//! priority ones; the matchers' are I39's.

use super::{Route, Router, host_only};

/// A route whose value is its name.
fn route(name: &'static str, priority: i64, hosts: &[&str], paths: &[&str]) -> Route<&'static str> {
    Route {
        hosts: hosts.iter().map(|s| (*s).to_owned()).collect(),
        paths: paths.iter().map(|s| (*s).to_owned()).collect(),
        matchers: Vec::new(),
        priority,
        value: name,
    }
}

/// Matches each of `cases` (host, path, want) against `r`: want `""` means no route.
#[track_caller]
fn check(r: &Router<&'static str>, cases: &[(&str, &str, &str)]) {
    for &(host, path, want) in cases {
        let got = r.find(host, path).copied();
        let want = (!want.is_empty()).then_some(want);
        assert_eq!(got, want, "find({host:?}, {path:?})");
    }
}

#[test]
fn fits_host_only() {
    for (input, want) in [
        ("example.com", "example.com"),
        ("example.com:8080", "example.com"),
        ("127.0.0.1:80", "127.0.0.1"),
        ("[::1]:8080", "::1"),
        ("[::1]", "::1"),
        ("::1", "::1"),
        ("", ""),
        ("[::1", "[::1"),
    ] {
        assert_eq!(host_only(input.as_bytes()), want.as_bytes(), "{input}");
    }
}

#[test]
fn fits_host() {
    let r = Router::new(vec![
        route("exact", 0, &["example.com"], &[]),
        route("wild", 0, &["*.example.com"], &[]),
        route("upper", 0, &["Mixed.Example.ORG"], &[]),
        route("v6", 0, &["[::1]"], &[]),
        route("v4", 0, &["127.0.0.1"], &[]),
    ]);
    check(
        &r,
        &[
            ("example.com", "/", "exact"),
            ("EXAMPLE.com:8080", "/", "exact"),
            ("example.com.", "/", "exact"),
            ("www.example.com", "/", "wild"),
            ("WWW.Example.COM", "/", "wild"),
            ("a.b.example.com", "/", ""),
            (".example.com", "/", ""),
            ("wwwexample.com", "/", ""),
            ("example.org", "/", ""),
            ("mixed.example.org", "/", "upper"),
            ("[::1]:8080", "/", "v6"),
            ("::1", "/", "v6"),
            ("127.0.0.1:80", "/", "v4"),
            ("", "/", ""),
        ],
    );
}

/// `**.` hosts: one label or more, below an exact host and a `*.` wildcard, the longest suffix
/// first, and only where their paths match.
#[test]
fn fits_deep_wildcard() {
    let r = Router::new(vec![
        route("deep", 0, &["**.example.com"], &[]),
        route("deeper", 0, &["**.b.example.com"], &["/b/*"]),
        route("wild", 0, &["*.example.com"], &["/w"]),
        route("exact", 0, &["a.b.example.com"], &["/e"]),
        route("any", 0, &[], &[]),
    ]);
    check(
        &r,
        &[
            ("x.example.com", "/", "deep"),
            ("x.y.z.example.com", "/", "deep"),
            ("X.Y.Example.COM", "/", "deep"),
            ("example.com", "/", "any"),
            ("x.example.org", "/", "any"),
            ("a.b.example.com", "/b/1", "deeper"),
            ("A.B.Example.com", "/b/1", "deeper"),
            ("a.b.example.com", "/", "deep"), // deeper's path does not match
            ("x.example.com", "/w", "wild"),
            ("x.y.example.com", "/w", "deep"), // * is one label
            ("a.b.example.com", "/e", "exact"),
        ],
    );
    // Without `**.` hosts, nothing is looked for: a host of many labels costs no more.
    let r = Router::new(vec![route("wild", 0, &["*.example.com"], &[])]);
    assert!(r.deep.is_empty());
}

#[test]
fn fits_path() {
    let r = Router::new(vec![
        route("hello", 0, &[], &["/hello", "/hi"]),
        route("api", 0, &[], &["/api/*"]),
    ]);
    check(
        &r,
        &[
            ("", "/hello", "hello"),
            ("", "/hi", "hello"),
            ("", "/hello/", ""),
            ("", "/HELLO", ""),
            ("", "/api/", "api"),
            ("", "/api/v1/users", "api"),
            ("", "/api", ""),
            ("", "/", ""),
        ],
    );
    check(
        &Router::new(vec![route("root", 0, &[], &["/*"])]),
        &[("", "/", "root"), ("", "/anything", "root")],
    );
}

#[test]
fn fits_no_routes() {
    check(&Router::new(vec![]), &[("example.com", "/", "")]);
}

#[test]
fn fits_order() {
    type Case = (&'static str, &'static str, &'static str);
    type Table = (&'static str, Vec<Route<&'static str>>, Vec<Case>);
    let tables: Vec<Table> = vec![
        (
            "priority beats specificity",
            vec![
                route("specific", 0, &["example.com"], &["/a"]),
                route("broad", 1, &[], &[]),
            ],
            vec![("example.com", "/a", "broad")],
        ),
        (
            "a lower priority still serves what the higher does not",
            vec![
                route("high", 5, &["example.com"], &["/a"]),
                route("low", -5, &[], &[]),
            ],
            vec![
                ("example.com", "/a", "high"),
                ("example.com", "/b", "low"),
                ("other.com", "/a", "low"),
            ],
        ),
        (
            "exact host beats wildcard beats any",
            vec![
                route("any", 0, &[], &["/a"]),
                route("wild", 0, &["*.example.com"], &[]),
                route("exact", 0, &["www.example.com"], &[]),
            ],
            vec![
                ("www.example.com", "/a", "exact"),
                ("api.example.com", "/a", "wild"),
                ("other.com", "/a", "any"),
            ],
        ),
        (
            "host beats path",
            vec![
                route("path", 0, &[], &["/api/v1"]),
                route("host", 0, &["example.com"], &[]),
            ],
            vec![
                ("example.com", "/api/v1", "host"),
                ("other.com", "/api/v1", "path"),
            ],
        ),
        (
            "exact path beats longest prefix beats shorter prefix beats none",
            vec![
                route("none", 0, &[], &[]),
                route("root", 0, &[], &["/*"]),
                route("api", 0, &[], &["/api/*"]),
                route("v1", 0, &[], &["/api/v1/*"]),
                route("users", 0, &[], &["/api/v1/users"]),
            ],
            vec![
                ("", "/api/v1/users", "users"),
                ("", "/api/v1/users/1", "v1"),
                ("", "/api/v2", "api"),
                ("", "/x", "root"),
            ],
        ),
        (
            "ties go to the route written first",
            vec![
                route("first", 0, &["example.com"], &["/a", "/b/*"]),
                route("second", 0, &["example.com"], &["/a", "/b/*"]),
                route("third", 0, &["*.example.com"], &[]),
                route("fourth", 0, &["*.EXAMPLE.com"], &[]),
            ],
            vec![
                ("example.com", "/a", "first"),
                ("example.com", "/b/c", "first"),
                ("www.example.com", "/", "third"),
            ],
        ),
        (
            "a later route with a higher priority replaces an earlier one",
            vec![
                route("first", 0, &[], &["/a", "/b/*"]),
                route("second", 1, &[], &["/a", "/b/*"]),
            ],
            vec![("", "/a", "second"), ("", "/b/c", "second")],
        ),
        (
            "a shorter prefix with a higher priority beats a longer one",
            vec![
                route("long", 0, &[], &["/a/b/*"]),
                route("short", 1, &[], &["/a/*"]),
                route("exact", 0, &[], &["/a/b/c"]),
            ],
            vec![("", "/a/b/c", "short"), ("", "/a/b/d", "short")],
        ),
        (
            "each pattern of a route is matched on its own",
            vec![
                route(
                    "mixed",
                    0,
                    &["example.com", "*.example.org"],
                    &["/x", "/y/*"],
                ),
                route("fallback", -1, &[], &[]),
            ],
            vec![
                ("example.com", "/x", "mixed"),
                ("www.example.org", "/y/z", "mixed"),
                ("example.org", "/x", "fallback"),
                ("example.com", "/z", "fallback"),
            ],
        ),
    ];
    for (name, routes, cases) in tables {
        let r = Router::new(routes);
        for (host, path, want) in cases {
            assert_eq!(
                r.find(host, path),
                Some(&want),
                "{name}: find({host:?}, {path:?})"
            );
        }
    }
}

/// fit's `TestPathTwice`: a route that lists a path twice is matched as once.
#[test]
fn fits_path_twice() {
    let r = Router::new(vec![route("a", 0, &[], &["/a", "/a"])]);
    check(&r, &[("example.com", "/a", "a"), ("example.com", "/b", "")]);
}

/// v0's tests (I05), kept.
#[test]
fn fits_hello_example() {
    let r = Router::new(vec![
        route("hello", 0, &[], &["/hello"]),
        route("not-found", -1, &[], &[]),
    ]);
    check(
        &r,
        &[
            ("127.0.0.1:8080", "/hello", "hello"),
            ("x", "/hello/", "not-found"),
            ("x", "/", "not-found"),
        ],
    );
}

#[test]
fn v0_hosts_and_paths() {
    let r = Router::new(vec![
        route("deep", 0, &["**.example.com"], &[]),
        route("wild", 0, &["*.example.com"], &[]),
        route("exact", 0, &["www.example.com"], &[]),
        route("any", 0, &[], &[]),
    ]);
    check(
        &r,
        &[
            ("WWW.Example.COM.", "/", "exact"),
            ("www.example.com:8080", "/", "exact"),
            ("api.example.com", "/", "wild"),
            ("a.b.example.com", "/", "deep"),
            ("example.com", "/", "any"),
            ("xexample.com", "/", "any"),
            ("[::1]:8080", "/", "any"),
        ],
    );
    let r = Router::new(vec![route("v6", 0, &["::1"], &[])]);
    check(
        &r,
        &[
            ("[::1]:8080", "/", "v6"),
            ("[::1]", "/", "v6"),
            ("::1", "/", "v6"),
        ],
    );
}

/// What the lookups add to fit's: a host longer than a DNS name matches only routes with no
/// hosts; prefixes of many lengths, of which only those no longer than the path are tried; a path
/// that is not UTF-8.
#[test]
fn edges() {
    let long = format!("{}.example.com", "A".repeat(300));
    let r = Router::new(vec![
        route("deep", 0, &["**.example.com"], &[]),
        route("any", -1, &[], &[]),
    ]);
    check(
        &r,
        &[(&long, "/", "any"), (&long.to_lowercase(), "/", "deep")],
    );

    let r = Router::new(vec![
        route("a", 0, &[], &["/a*"]),
        route("abc", 0, &[], &["/abc*"]),
        route("long", 0, &[], &["/abcdefgh/*"]),
        route("low", -1, &[], &["/*"]),
    ]);
    check(
        &r,
        &[
            ("", "/ab", "a"),
            ("", "/abcd", "abc"),
            ("", "/abcdefgh/", "long"),
            ("", "/b", "low"),
            ("", "", ""),
        ],
    );
    let r = Router::new(vec![route("latin1", 0, &[], &["/caf*"])]);
    assert_eq!(r.find("h", b"/caf\xe9"), Some(&"latin1"));
}

/// More `**.` suffixes than are looked at: the four longest.
#[test]
fn deep_bound() {
    let r = Router::new(vec![
        route("e", 0, &["**.e"], &[]),
        route("de", 0, &["**.d.e"], &["/de"]),
        route("cde", 0, &["**.c.d.e"], &["/cde"]),
        route("bcde", 0, &["**.b.c.d.e"], &["/bcde"]),
        route("abcde", 0, &["**.a.b.c.d.e"], &["/abcde"]),
    ]);
    check(
        &r,
        &[
            ("x.a.b.c.d.e", "/abcde", "abcde"),
            ("x.a.b.c.d.e", "/de", "de"),
            ("x.a.b.c.d.e", "/", ""),
            ("x.b.c.d.e", "/", "e"),
        ],
    );
}
