//! fit's `provider/provider_test.go` and `edges_test.go`, the merging half: `TestMerge`,
//! `TestMergeIntoEmpty`, `TestParseRefused`, `TestErrors`, `TestPosition`,
//! `TestParseRefusedMore`, `TestErrorsOne` and `TestOffsetPastEnd`. Messages that are JSON
//! syntax errors are akit-config's, where fit's are `encoding/json`'s.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use akit_config::Error;
use akit_config::value::{self, Value};

use super::{KINDS, MAX_PART_BYTES, Part, Shadowed, Source, merge, offset, parse, position};

fn must_parse(name: &str, data: &str) -> Arc<Part> {
    Arc::new(parse(name, data.as_bytes()).unwrap_or_else(|e| panic!("parse {name}: {e:?}")))
}

fn src(s: &str) -> Arc<str> {
    Arc::from(s)
}

/// The names, or "-" for none, of the elements of each kind in a merged config.
fn names(data: &str) -> BTreeMap<String, Vec<String>> {
    let doc = value::parse(data).unwrap_or_else(|e| panic!("not JSON: {e:?}\n{data}"));
    let mut out = BTreeMap::new();
    for k in KINDS {
        if let Some(Value::Array(elems)) = doc.get(k) {
            let names = elems
                .iter()
                .map(|e| match e.get("name") {
                    Some(Value::String(n)) if !n.is_empty() => n.clone(),
                    _ => "-".to_owned(),
                })
                .collect();
            out.insert(k.to_owned(), names);
        }
    }
    out
}

fn want(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect()))
        .collect()
}

fn source(path: &str, s: &str, from: &str) -> Source {
    Source {
        path: path.into(),
        source: src(s),
        from: from.into(),
    }
}

fn shadowed(s: &str, from: &str, name: &str, by: &str) -> Shadowed {
    Shadowed {
        source: src(s),
        from: from.into(),
        name: name.into(),
        by: src(by),
    }
}

#[test]
fn merges() {
    let base = r#"{
  "listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond"}}],
  "upstreams": [{"name": "api", "addresses": ["127.0.0.1:1"]}],
  "limits": {}
}"#;
    let docker = must_parse(
        "docker",
        r#"{
  "upstreams": [{"name": "api", "addresses": ["127.0.0.1:2"]}, {"name": "web", "addresses": ["127.0.0.1:3"]}],
  "routes": [{"name": "d", "handler": {"module": "respond"}}, {"handler": {"module": "respond"}}],
  "services": [{"name": "tcp", "addresses": ["127.0.0.1:4"]}]
}"#,
    );
    let consul = must_parse(
        "consul",
        r#"{
  "upstreams": [{"name": "web", "addresses": ["127.0.0.1:5"]}, {"name": "db", "addresses": ["127.0.0.1:6"]}],
  "routes": [{"name": "d", "handler": {"module": "respond"}}],
  "listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "alt", "address": "127.0.0.1:0"}]
}"#,
    );
    let m = merge(&src(base), &src("file"), &[docker, consul]).unwrap();
    let data = m.data().to_string();
    assert_eq!(
        names(&data),
        want(&[
            ("listeners", &["web", "alt"]),
            ("routes", &["-", "d", "-"]),
            ("upstreams", &["api", "web", "db"]),
            ("services", &["tcp"]),
        ]),
        "{data}"
    );
    assert!(
        data.contains("\"127.0.0.1:3\"")
            && !data.contains("\"127.0.0.1:2\"")
            && !data.contains("\"127.0.0.1:5\""),
        "the wrong upstreams won:\n{data}"
    );
    assert_eq!(
        m.sources(),
        [
            source("/listeners/0", "file", ""),
            source("/listeners/1", "consul", "/listeners/1"),
            source("/routes/0", "file", ""),
            source("/routes/1", "docker", "/routes/0"),
            source("/routes/2", "docker", "/routes/1"),
            source("/upstreams/0", "file", ""),
            source("/upstreams/1", "docker", "/upstreams/1"),
            source("/upstreams/2", "consul", "/upstreams/1"),
            source("/limits", "file", ""),
            source("/services/0", "docker", "/services/0"),
        ]
    );
    assert_eq!(
        m.shadowed(),
        [
            shadowed("consul", "/listeners/0", "web", "file"),
            shadowed("consul", "/routes/0", "d", "docker"),
            shadowed("docker", "/upstreams/0", "api", "file"),
            shadowed("consul", "/upstreams/0", "web", "docker"),
        ]
    );
    // The base config's text is kept as written.
    let kept = &base[..base.find("}],").unwrap()];
    assert!(data.starts_with(kept), "{data}");
}

#[test]
fn merges_into_empty() {
    let part = must_parse(
        "p",
        r#"{"routes": [{"handler": {"module": "respond"}}], "listeners": [{"name": "l", "address": "127.0.0.1:0"}]}"#,
    );
    for base in [
        "{}",
        " { } ",
        r#"{"routes": []}"#,
        r#"{"routes": [ ], "listeners": []}"#,
        r#"{"limits": {}}"#,
    ] {
        let m = merge(&src(base), &src("file"), &[Arc::clone(&part)]).unwrap();
        assert_eq!(
            names(m.data()),
            want(&[("listeners", &["l"]), ("routes", &["-"])]),
            "{base}: {}",
            m.data()
        );
        if let Err(e) = akit_config::from_json(m.data(), None) {
            panic!(
                "{base}: the merged config does not load: {e:?}\n{}",
                m.data()
            );
        }
    }
    // Nothing to add leaves the base config itself.
    let base = r#"{"routes": [{"handler": {"module": "respond"}}]}"#;
    let base = src(base);
    let m = merge(&base, &src("file"), &[must_parse("p", "")]).unwrap();
    assert!(Arc::ptr_eq(m.data(), &base));
    assert_eq!(m.sources(), [source("/routes/0", "file", "")]);
}

#[test]
fn parts_refused() {
    for (data, want) in [
        ("[]", "1:1: must be a JSON object"),
        (r#"{"limits": {}}"#, "limits: a provider may provide only"),
        (r#"{"routes": {}}"#, "routes: must be an array"),
        (r#"{"routes": [1]}"#, "routes[0]: must be an object"),
        (
            r#"{"routes": [{"name": 1}]}"#,
            "routes[0].name: must be a string",
        ),
        (
            r#"{"routes": [], "routes": []}"#,
            "duplicate key \"routes\"",
        ),
        (r#"{"routes": []} {}"#, "more after the value"),
        (r#"{"routes": [}"#, "invalid JSON"),
        (
            "{\n  \"routes\": [\n    1]}",
            "3:5: routes[0]: must be an object",
        ),
    ] {
        let got = parse("p", data.as_bytes()).map(|_| ()).unwrap_err();
        let got = got.0[0].to_string();
        assert!(got.contains(want), "parse({data}): {got}, want {want:?}");
    }
    let got = parse("p", b"{\"routes\": [\xff]}").map(|_| ()).unwrap_err();
    assert!(got.0[0].msg.contains("not UTF-8"), "{got:?}");
    let big = format!("{{\"routes\": [{}]}}", " ".repeat(MAX_PART_BYTES));
    let got = parse("p", big.as_bytes()).map(|_| ()).unwrap_err();
    assert!(got.0[0].msg.contains("larger than"), "{got:?}");
    // At the limit, a part is read.
    let at = format!("{{\"routes\": [{}]}}", " ".repeat(MAX_PART_BYTES - 14));
    assert_eq!(at.len(), MAX_PART_BYTES);
    assert_eq!(parse("p", at.as_bytes()).unwrap().bytes(), MAX_PART_BYTES);
    // A part with comments and trailing commas, as a file may have them.
    let p = parse("p", b"{\"routes\": [{\"name\": \"r\"}, /* more */],}").unwrap();
    assert!(value::parse(p.data()).is_ok() && !p.data().contains("more"));
    // A base config whose routes are null.
    let got = merge(&src(r#"{"routes": null}"#), &src("file"), &[]).unwrap_err();
    assert!(
        got.0[0].to_string().contains("routes: must be an array"),
        "{got:?}"
    );
}

/// fit's `TestParseRefusedMore`: what the errors say is akit-config's; where they are, ours.
#[test]
fn parts_refused_more() {
    for data in [
        "{1}",
        r#"{"routes": }"#,
        r#"{"routes": []"#,
        r#"{"routes": []]"#,
    ] {
        let got = parse("p", data.as_bytes()).map(|_| ()).unwrap_err();
        assert!(got.0[0].to_string().starts_with("1:"), "{data}: {got:?}");
    }
    // A base config's member not of KINDS that is not JSON.
    let got = merge(&src(r#"{"limits": [}"#), &src("file"), &[]).unwrap_err();
    assert!(got.0[0].to_string().starts_with("1:"), "{got:?}");
}

fn find<'a>(errs: &'a [Error], prefix: &str) -> Option<&'a Error> {
    errs.iter().find(|e| e.path.starts_with(prefix))
}

/// fit's `TestErrors`: errors in a merged config are reported where they were written.
#[test]
fn errors_where_written() {
    let base = r#"{
  "routes": [{"handler": {"module": "respond"}}],
  "upstreams": [
    {"name": "u", "addresses": ["127.0.0.1:1"], "bogus": 1}
  ]
}"#;
    let part = r#"{"routes": [
  {"name": "ok", "handler": {"module": "respond"}},
  {"name": "bad", "hanlder": {"module": "respond"}}
]}"#;
    let m = merge(&src(base), &src("file"), &[must_parse("docker", part)]).unwrap();
    let errs = akit_config::from_json(m.data(), None)
        .map(|_| ())
        .expect_err("the merged config loaded");
    let es = m.errors(errs).0;
    let in_base = find(&es, "upstreams[0].bogus").unwrap_or_else(|| panic!("{es:?}"));
    assert_eq!(
        (in_base.line, in_base.col),
        (4, 49),
        "the error in the base config: {in_base:?}"
    );
    assert!(!in_base.msg.contains("provided by"), "{in_base:?}");
    let in_part = find(&es, "routes[1].hanlder").unwrap_or_else(|| panic!("{es:?}"));
    assert_eq!(
        (in_part.line, in_part.col),
        (3, 19),
        "the error in the part: {in_part:?}"
    );
    assert!(in_part.msg.ends_with("(provided by docker)"), "{in_part:?}");
}

/// fit's `TestErrorsOne`: an error not in an element, in the base config, keeps its place.
#[test]
fn errors_one() {
    let base = "{\n  \"limits\": {\"bogus\": 1}\n}";
    let m = merge(
        &src(base),
        &src("file"),
        &[must_parse(
            "p",
            r#"{"routes": [{"handler": {"module": "respond"}}]}"#,
        )],
    )
    .unwrap();
    let e = Error {
        file: None,
        path: "limits.bogus".into(),
        line: 2,
        col: 14,
        msg: "unknown".into(),
    };
    let es = m.errors(akit_config::Errors(vec![e.clone()])).0;
    assert_eq!(es, [e]);
}

/// An error past what a part provided keeps its path, without a position, and one in what
/// merging wrote has none either.
#[test]
fn errors_elsewhere() {
    let m = merge(
        &src("{}"),
        &src("file"),
        &[must_parse("p", r#"{"routes": [{"handler": {}}]}"#)],
    )
    .unwrap();
    let e = |path: &str, line, col| Error {
        file: None,
        path: path.into(),
        line,
        col,
        msg: "m".into(),
    };
    let es = m
        .errors(akit_config::Errors(vec![
            e("routes[7]", 0, 0),
            e("", 1, 2),
            e("routes[0].handler", 1, 24),
        ]))
        .0;
    assert_eq!((es[0].path.as_str(), es[0].line), ("routes[7]", 0));
    assert_eq!((es[1].line, es[1].msg.as_str()), (0, "m"));
    assert_eq!(
        (es[2].line, es[2].col, es[2].msg.as_str()),
        (1, 25, "m (provided by p)"),
        "{es:?}"
    );
}

#[test]
fn positions() {
    let data = "ab\néc\nd";
    for off in 0..=data.len() {
        if off == 4 {
            continue; // inside é, which starts at 3
        }
        let (line, col) = position(data, off);
        assert_eq!(offset(data, line, col), off, "offset {off}: {line}:{col}");
    }
    assert_eq!(position(data, 5), (2, 2), "after é");
    assert_eq!(offset("a\nb", 5, 1), 3, "a line past the end");
}
