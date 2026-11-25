#![allow(clippy::unwrap_used, clippy::panic, clippy::arithmetic_side_effects)]

use super::{Document, Target, conf_from_json, document, has_comments, yaml_from_json};
use crate::{Format, Source};

/// fit's writers' verdicts on a corpus of JSON documents, written by fit itself
/// (`tools/adapt-golden/run.sh`): a line is the document, fit's YAML and fit's `.conf`, each
/// hex-encoded, an output as `OK <hex>` or `ERR <hex of the message>`.
const WRITERS: &str = include_str!("../../testdata/adapt-writers.txt");

fn unhex(s: &str) -> String {
    let b: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    String::from_utf8(b).unwrap()
}

/// Whether `got` is fit's verdict `want`: the same output byte for byte, or an error, with the
/// same message but for one from Go's JSON decoder, whose words are its own.
fn same(got: &Result<String, String>, want: &str) -> bool {
    match (got, want.split_once(' ')) {
        (Ok(out), Some(("OK", hex))) => *out == unhex(hex),
        (Err(msg), Some(("ERR", fit))) => {
            let fit = unhex(fit);
            if fit.starts_with("JSON: ") {
                msg.starts_with("JSON: ")
            } else {
                *msg == fit
            }
        }
        _ => false,
    }
}

#[test]
fn writers_write_what_fits_write() {
    let mut n = 0;
    let mut diffs = Vec::new();
    for line in WRITERS.lines() {
        let mut cols = line.split('\t');
        let (doc, yaml, conf) = (
            unhex(cols.next().unwrap()),
            cols.next().unwrap(),
            cols.next().unwrap(),
        );
        n += 1;
        let y = yaml_from_json(&doc);
        if !same(&y, yaml) {
            diffs.push(format!(
                "YAML of {doc}\n  akit: {y:?}\n  fit:  {:?}\n",
                show(yaml)
            ));
        }
        let c = conf_from_json(&doc);
        if !same(&c, conf) {
            diffs.push(format!(
                ".conf of {doc}\n  akit: {c:?}\n  fit:  {:?}\n",
                show(conf)
            ));
        }
    }
    assert!(n > 3500, "only {n} documents");
    assert!(
        diffs.is_empty(),
        "{} of {n} documents written otherwise than fit writes them:\n{}",
        diffs.len(),
        diffs.iter().take(20).cloned().collect::<String>()
    );
}

fn show(want: &str) -> String {
    match want.split_once(' ') {
        Some(("OK" | "ERR", hex)) => unhex(hex),
        _ => want.to_owned(),
    }
}

fn src<'a>(name: &'a str, format: Format, src: &'a str) -> Source<'a> {
    Source {
        name: Some(name),
        format,
        src,
    }
}

fn doc(srcs: &[Source<'_>]) -> Document {
    document(srcs).unwrap()
}

#[test]
fn a_json_file_keeps_its_strings_numbers_and_trailing_space() {
    let d = doc(&[src(
        "a.json",
        Format::Json,
        "// c\n{\"a\": \"x<y\\u00e9\\/\", \"b\": [ ], \"c\": {}, \"n\": 1.50,} // end\n/* x\n */\n",
    )]);
    assert_eq!(
        d.write(Target::Json).unwrap(),
        "{\n  \"a\": \"x<y\\u00e9\\/\",\n  \"b\": [],\n  \"c\": {},\n  \"n\": 1.50\n}       \n    \n   \n\n"
    );
}

#[test]
fn yaml_and_conf_strings_are_written_as_go_marshals_them() {
    let d = doc(&[src("a.yaml", Format::Yaml, "a: \"x<y & z>\"\nb: 200.0\n")]);
    assert_eq!(
        d.write(Target::Json).unwrap(),
        "{\n  \"a\": \"x\\u003cy \\u0026 z\\u003e\",\n  \"b\": 200\n}\n"
    );
}

#[test]
fn several_files_are_merged_as_fit_merges_them() {
    let d = doc(&[
        src(
            "a.json",
            Format::Json,
            r#"{"listeners": [{"name": "a"}], "schema_version": 1}"#,
        ),
        src(
            "b.yaml",
            Format::Yaml,
            "schema_version: 1\nlisteners:\n  - name: b\nroutes: null\nadmin: {}\n",
        ),
    ]);
    assert_eq!(
        d.write(Target::Json).unwrap(),
        "{\n  \"listeners\": [\n    {\n      \"name\": \"a\"\n    },\n    {\n      \"name\": \"b\"\n    }\n  ],\n  \"schema_version\": 1,\n  \"routes\": [],\n  \"admin\": {}\n}\n"
    );
    for (b, want) in [
        (
            "admin: {}\n",
            "a.json:1:1: admin: also given in a.json: only the lists",
        ),
        (
            "schema_version: 2\n",
            "a.json:1:1: schema_version: differs from the one in a.json",
        ),
    ] {
        let err = document(&[
            src(
                "a.json",
                Format::Json,
                r#"{"admin": {}, "schema_version": 1}"#,
            ),
            src("b.yaml", Format::Yaml, b),
        ])
        .unwrap_err();
        let got = err.to_string().replace("b.yaml", "a.json");
        assert!(got.starts_with(want), "{got}");
    }
}

#[test]
fn round_trips_through_every_format() {
    let src0 = r#"{"listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1", "h2c"]}],
        "routes": [{"name": "r", "match": {"path": ["/a/*"]},
          "handler": {"module": "respond", "params": {"status": 200, "body": "hi <you>\n", "headers": {"X-A": "1"}}}}],
        "limits": {"max_body_bytes": 1048576}}"#;
    let json = doc(&[src("a.json", Format::Json, src0)])
        .write(Target::Json)
        .unwrap();
    for (to, format) in [(Target::Yaml, Format::Yaml), (Target::Conf, Format::Conf)] {
        let out = doc(&[src("a.json", Format::Json, src0)]).write(to).unwrap();
        let back = doc(&[src("b", format, &out)]).write(Target::Json).unwrap();
        assert_eq!(
            crate::value::parse(&back).unwrap(),
            crate::value::parse(&json).unwrap(),
            "{out}"
        );
    }
}

#[test]
fn comments_are_found() {
    for (src, f, want) in [
        ("{\"a\": \"//x /*\"}", Format::Json, false),
        ("{\"a\": 1} // x", Format::Json, true),
        ("{/* x */}", Format::Json, true),
        ("a: \"#x\" # y\n", Format::Yaml, true),
        ("a: b#c\n", Format::Yaml, false),
        ("# x\na: 1\n", Format::Yaml, true),
        ("admin { address a#b; x \"#\" `#` }\n", Format::Conf, false),
        ("admin { address a #b }\n", Format::Conf, true),
        ("admin {\n;# x\n}\n", Format::Conf, true),
        ("admin {;# x\n}\n", Format::Conf, false),
    ] {
        assert_eq!(has_comments(src, f), want, "{src:?}");
    }
}
