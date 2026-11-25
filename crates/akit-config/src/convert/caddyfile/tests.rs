//! fit's `adapter/caddyfile` tests that look inside the converter or build their inputs, ported;
//! the rest are in fit's golden corpus (`convert::tests`).
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;

use super::directives::{encode_level, size};
use super::matcher::{OutMatch, expr, intersect, same_match, same_set};
use super::parse::{MAX_TOKENS, lex_texts, parse};
use crate::convert::gostd;

fn env(name: &str) -> Option<String> {
    (name == "FIT_TEST_PORT").then(|| "9000".to_owned())
}

fn convert(src: &str) -> crate::convert::Converted {
    super::convert(src, &env).unwrap()
}

fn routes(c: &crate::convert::Converted) -> Vec<crate::value::Value> {
    match crate::value::parse(c.json()).unwrap().get("routes") {
        Some(crate::value::Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

#[test]
fn lex() {
    let toks = lex_texts(
        "# a comment line\na \"quoted # not a comment \\\"x\\\"\" `raw \"q\"` b#c # trailing comment\n\
         d {$FIT_TEST_PORT} {$FIT_TEST_MISSING:dflt} \\\n  continued\n\
         e <<END\n    one\n      two\n    END\nf\n",
        &env,
    )
    .unwrap();
    let texts: Vec<&str> = toks.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(
        texts,
        [
            "a",
            r#"quoted # not a comment "x""#,
            r#"raw "q""#,
            "b#c",
            "d",
            "9000",
            "dflt",
            "continued",
            "e",
            "one\n  two",
            "f"
        ]
    );
    assert!(toks[0].1 == 2 && toks[0].2 && !toks[1].2 && !toks[7].2 && toks[7].1 == 4);
    assert!(toks[10].1 == 9 && toks[10].2);
    for (src, msg) in [
        ("a \"open", "does not end"),
        ("a `open", "does not end"),
        ("a <<X\nb\n", "heredoc"),
        ("a {\n b\n", "does not end"),
        ("a {\n}\n}\n", "unexpected"),
        ("a {\n}\nb\n", "no braces"),
        ("{\n}\n{\n}\n", "global options come first"),
    ] {
        let err = parse(src, &env).err().unwrap_or_default();
        assert!(err.contains(msg), "{src:?}: {err}, want {msg}");
    }
}

#[test]
fn heredoc_at_the_end() {
    let toks = lex_texts("a <<EOF\n\thi\n\tEOF", &env).unwrap();
    assert_eq!(toks.len(), 2);
    assert_eq!(toks[1].0, "hi");
}

#[test]
fn parse_snippets_and_sites() {
    let (blocks, ws) = parse(
        "{\n\temail a@b.test\n}\n(snip) {\n\theader X-Snip {args[0]}\n\trespond {args[:]}\n}\n\
         a.test,\n  b.test c.test {\n\timport snip one two\n\timport missing.conf\n\troute /x {\n\t\timport snip three\n\t}\n}\n",
        &env,
    )
    .unwrap();
    assert_eq!(blocks.len(), 2);
    assert!(blocks[0].keys.is_empty());
    assert_eq!(blocks[1].keys, ["a.test", "b.test", "c.test"]);
    let d = &blocks[1].dirs;
    assert_eq!(d.len(), 3);
    assert!(d[0].name == "header" && d[0].args[1] == "one");
    assert!(d[1].name == "respond" && d[1].args == ["one two"]);
    assert!(d[2].name == "route" && d[2].block.len() == 2 && d[2].block[0].args[1] == "three");
    assert_eq!(ws.len(), 1);
    assert!(ws[0].contains("line 11: import missing.conf: skipped"));
    // One site may leave out its braces.
    let (blocks, _) = parse("# comment\nlocalhost:8080\nrespond \"hi\"\n", &env).unwrap();
    assert!(blocks.len() == 1 && blocks[0].keys[0] == "localhost:8080");
    assert_eq!(blocks[0].dirs[0].args[0], "hi");
}

fn m() -> OutMatch {
    OutMatch::default()
}

fn hdr(k: &str, v: &[&str]) -> Option<BTreeMap<String, Vec<String>>> {
    Some(BTreeMap::from([(
        k.to_owned(),
        v.iter().map(|s| (*s).to_owned()).collect(),
    )]))
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn intersections_as_fit() {
    let cases: Vec<(Option<OutMatch>, OutMatch, Option<&str>)> = vec![
        (
            Some(OutMatch {
                path: strs(&["/*"]),
                ..m()
            }),
            OutMatch {
                path: strs(&["/api/*"]),
                ..m()
            },
            Some(r#"{"path":["/api/*"]}"#),
        ),
        (
            Some(OutMatch {
                path: strs(&["/api/*"]),
                ..m()
            }),
            OutMatch {
                path: strs(&["/web/*"]),
                ..m()
            },
            None,
        ),
        (
            Some(OutMatch {
                path: strs(&["/a/x"]),
                ..m()
            }),
            OutMatch {
                path: strs(&["/a/*"]),
                ..m()
            },
            Some(r#"{"path":["/a/x"]}"#),
        ),
        (
            Some(OutMatch {
                method: strs(&["GET"]),
                ..m()
            }),
            OutMatch {
                method: strs(&["POST"]),
                ..m()
            },
            None,
        ),
        (
            Some(OutMatch {
                host: strs(&["*.a.test"]),
                ..m()
            }),
            OutMatch {
                host: strs(&["b.a.test"]),
                ..m()
            },
            Some(r#"{"host":["*.a.test"],"expr":"Host(\"b.a.test\")"}"#),
        ),
        (
            Some(OutMatch {
                path_regexp: strs(&["x"]),
                expr: r#"Method("GET")"#.into(),
                ..m()
            }),
            OutMatch {
                path_regexp: strs(&["y"]),
                ..m()
            },
            Some(
                r#"{"path_regexp":["x"],"expr":"Method(\"GET\") \u0026\u0026 PathRegexp(\"y\")"}"#,
            ),
        ),
        (
            Some(OutMatch {
                header: hdr("x-a", &[]),
                ..m()
            }),
            OutMatch {
                header: hdr("X-A", &["1"]),
                ..m()
            },
            Some(r#"{"header":{"x-a":["1"]}}"#),
        ),
        // TestMatchHelpers.
        (None, m(), Some("{}")),
        (
            Some(m()),
            OutMatch {
                host: strs(&["x.test"]),
                ..m()
            },
            Some(r#"{"host":["x.test"]}"#),
        ),
        (
            Some(OutMatch {
                host: strs(&["a.test", "b.test"]),
                ..m()
            }),
            OutMatch {
                host: strs(&["B.test"]),
                ..m()
            },
            Some(r#"{"host":["b.test"]}"#),
        ),
        (
            Some(OutMatch {
                host: strs(&["a.test"]),
                ..m()
            }),
            OutMatch {
                host: strs(&["c.test"]),
                ..m()
            },
            None,
        ),
        (
            Some(OutMatch {
                path: strs(&["/a*"]),
                ..m()
            }),
            OutMatch {
                path: strs(&["/a/b*"]),
                ..m()
            },
            Some(r#"{"path":["/a/b*"]}"#),
        ),
        (
            Some(OutMatch {
                path_regexp: strs(&["x"]),
                ..m()
            }),
            OutMatch {
                path_regexp: strs(&["x"]),
                ..m()
            },
            Some(r#"{"path_regexp":["x"]}"#),
        ),
        (
            Some(OutMatch {
                method: strs(&["GET", "POST"]),
                ..m()
            }),
            OutMatch {
                method: strs(&["POST"]),
                ..m()
            },
            Some(r#"{"method":["POST"]}"#),
        ),
        (
            Some(OutMatch {
                header: hdr("X", &["1"]),
                ..m()
            }),
            OutMatch {
                header: hdr("x", &["2"]),
                ..m()
            },
            None,
        ),
        (
            Some(OutMatch {
                header: hdr("X", &["1", "2"]),
                ..m()
            }),
            OutMatch {
                header: hdr("X", &["2"]),
                ..m()
            },
            Some(r#"{"header":{"X":["2"]}}"#),
        ),
        (
            Some(OutMatch {
                header: hdr("X", &["1"]),
                ..m()
            }),
            OutMatch {
                header: hdr("X", &[]),
                ..m()
            },
            Some(r#"{"header":{"X":["1"]}}"#),
        ),
        (
            Some(OutMatch {
                query: hdr("q", &["1"]),
                ..m()
            }),
            OutMatch {
                query: hdr("q", &["2"]),
                ..m()
            },
            None,
        ),
        (
            Some(m()),
            OutMatch {
                remote_ip: strs(&["1.0.0.0/8"]),
                ..m()
            },
            Some(r#"{"remote_ip":["1.0.0.0/8"]}"#),
        ),
        (
            Some(OutMatch {
                remote_ip: strs(&["1.0.0.0/8"]),
                ..m()
            }),
            OutMatch {
                remote_ip: strs(&["2.0.0.0/8"]),
                ..m()
            },
            Some(r#"{"remote_ip":["1.0.0.0/8"],"expr":"RemoteIP(\"2.0.0.0/8\")"}"#),
        ),
        (
            Some(OutMatch {
                expr: r#"Method("GET") || Method("PUT")"#.into(),
                ..m()
            }),
            OutMatch {
                expr: "X".into(),
                ..m()
            },
            Some(r#"{"expr":"(Method(\"GET\") || Method(\"PUT\")) \u0026\u0026 X"}"#),
        ),
        // TestIntersectSameHosts.
        (
            Some(OutMatch {
                host: strs(&["a.test", "b.test"]),
                ..m()
            }),
            OutMatch {
                host: strs(&["b.test", "a.test"]),
                ..m()
            },
            Some(r#"{"host":["a.test","b.test"]}"#),
        ),
    ];
    for (a, b, want) in cases {
        let got = intersect(a.as_ref(), Some(&b)).map(|r| r.json().marshal().unwrap());
        assert_eq!(got.as_deref(), want, "{a:?} ∩ {b:?}");
    }
    let all = OutMatch {
        host: strs(&["a"]),
        path: strs(&["/x", "/y*"]),
        path_regexp: strs(&["r"]),
        method: strs(&["GET"]),
        header: hdr("H", &["1"]),
        query: hdr("q", &[]),
        remote_ip: strs(&["1.2.3.4"]),
        expr: "E".into(),
    };
    for (mm, want) in [
        (m(), ""),
        (
            OutMatch {
                path: strs(&["/x"]),
                ..m()
            },
            r#"Path("/x")"#,
        ),
        (
            OutMatch {
                path: strs(&["/y*"]),
                ..m()
            },
            r#"PathPrefix("/y")"#,
        ),
        (
            all,
            r#"Host("a") && (Path("/x") || PathPrefix("/y")) && PathRegexp("r") && Method("GET") && Header("H", "1") && Query("q") && RemoteIP("1.2.3.4") && E"#,
        ),
    ] {
        assert_eq!(expr(&mm), want);
    }
    assert!(!same_set(&strs(&["a", "b"]), &strs(&["a", "c"])));
    assert!(!same_match(
        &m(),
        &OutMatch {
            path: strs(&["/"]),
            ..m()
        }
    ));
    assert_eq!(size("99999999999T"), None);
    assert_eq!(size("ten"), None);
}

#[test]
fn route_names_as_fit() {
    let mut names = HashSet::new();
    let long = "a".repeat(70);
    for (site, hint, want) in [
        ("-", "", "route"),
        ("-", "", "route-2"),
        (long.as_str(), "", &long[..60]),
        ("a.test", "/x y/*", "a.test-x_y"),
    ] {
        assert_eq!(gostd::route_name(&mut names, site, hint, "/*"), want);
    }
}

#[test]
fn encode_levels_as_fit() {
    for (encoding, level, want) in [
        ("gzip", "1", Some(("gzip_level", 1))),
        ("gzip", "9", Some(("gzip_level", 9))),
        ("gzip", "10", None),
        ("gzip", "0", None),
        ("br", "11", Some(("brotli_level", 11))),
        ("br", "fast", None),
        ("zstd", "3", Some(("zstd_level", 3))),
        ("zstd", "22", Some(("zstd_level", 22))),
        ("zstd", "fastest", Some(("zstd_level", 1))),
        ("zstd", "best", Some(("zstd_level", 10))),
        ("zstd", "23", None),
    ] {
        assert_eq!(encode_level(encoding, level), want, "{encoding} {level}");
    }
}

#[test]
fn a_size_out_of_range_is_skipped() {
    let big = format!("1{}", "0".repeat(400));
    assert_eq!(size(&big), None);
    let c = convert(&format!(
        ":8080 {{\n\trequest_body {{\n\t\tmax_size {big}MB\n\t}}\n\trespond 200\n}}\n"
    ));
    assert!(
        c.warnings()
            .iter()
            .any(|w| w.contains("request_body: skipped"))
    );
    let rs = routes(&c);
    assert_eq!(rs.len(), 1);
    assert!(rs[0].get("middleware").is_none());
}

#[test]
fn errors_on_what_is_too_big_or_broken() {
    let deep = format!(
        "(deep) {{\n{}{}}}\n{}import deep\n{}",
        "a {\n".repeat(60),
        "}\n".repeat(60),
        "b {\n".repeat(5),
        "}\n".repeat(5)
    );
    for (src, want) in [
        ("a ".repeat(MAX_TOKENS + 1), "more than 200000 tokens"),
        (
            "(s) {\n}\n(s) {\n}\n:80 {\n}\n".into(),
            "snippet s is defined twice",
        ),
        (
            format!("{}{}", "a {\n".repeat(65), "}\n".repeat(65)),
            "nested too deep",
        ),
        (deep, "blocks nested too deep"),
        (":80 {\n import\n}\n".into(), "import names no snippet"),
        (
            format!(
                "(s) {{\n header X 1\n}}\n:80 {{\n{}}}\n",
                " import s\n".repeat(1001)
            ),
            "more than 1000 imports",
        ),
        (
            format!(
                "(s) {{\n{}\n}}\n:80 {{\n{}}}\n",
                "x ".repeat(1000),
                " import s\n".repeat(201)
            ),
            "once snippets are imported",
        ),
        (
            "{\n {\n }\n}\n".into(),
            "a block with no directive before it",
        ),
        (
            "a {\n {\n }\n}\n".into(),
            "a block with no directive before it",
        ),
        (
            ":80 {\n route {\n  {\n  }\n }\n}\n".into(),
            "a block with no directive before it",
        ),
        (
            "(s) {\n route {args[0]}\n}\n:80 {\n import s \"{\"\n}\n".into(),
            "a block that does not end",
        ),
        (
            ":80 {\n php_fastcgi :9000\n}\n".into(),
            "none of the Caddyfile's sites converted",
        ),
        (
            "ftp://a.test {\n respond 200\n}\n".into(),
            "no site to convert",
        ),
    ] {
        let err = super::convert(&src, &env).err().unwrap_or_default();
        assert!(err.contains(want), "{err}, want {want}");
    }
}

#[test]
fn too_many_routes() {
    let mut b = String::from(":80 {\n");
    for i in 0..=super::routes::MAX_SITE_ROUTES {
        writeln!(b, " respond /p{i} 200").unwrap();
    }
    b.push_str("}\n:81 {\n respond 200\n}\n");
    let c = convert(&b);
    assert_eq!(routes(&c).len(), 1);
    assert!(
        c.warnings()
            .join("\n")
            .contains("it makes more than 2000 routes")
    );

    let mut b = String::from(":80 {\n");
    for i in 0..super::routes::MAX_SITE_ROUTES - 1 {
        writeln!(b, " respond /p{i} 200").unwrap();
    }
    b.push_str(" header /zz* X 1\n}\n");
    let c = convert(&b);
    assert_eq!(routes(&c).len(), super::routes::MAX_SITE_ROUTES);
    assert!(
        c.warnings()
            .join("\n")
            .contains("header: not applied: the site makes too many routes")
    );
}

#[test]
fn positions() {
    let c = convert(":80 {\n log\n respond 200\n}\n");
    for (path, want) in [
        ("listeners[0]", Some(1)),
        ("routes[0]", Some(3)),
        ("routes[0].handler", Some(3)),
        ("access_log", Some(2)),
        ("upstreams[0]", None),
    ] {
        assert_eq!(c.line(path), want, "{path}");
    }
}
