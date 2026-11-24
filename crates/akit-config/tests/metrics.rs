//! `metrics` (I37): read as fit reads it, `{}` with no settings; refused without `admin`, which
//! serves it, with fit's message; given by one file only.
#![allow(clippy::panic, clippy::unwrap_used, clippy::indexing_slicing)]

use akit_config::{Format, Source, from_json, from_sources};

const BASE: &str = r#""listeners": [{"name": "web", "address": ":8080"}],
  "routes": [{"handler": {"module": "respond"}}]"#;

fn errors(doc: &str) -> Vec<String> {
    from_json(doc, None)
        .unwrap_err()
        .as_slice()
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn needs_admin() {
    assert_eq!(
        errors(&format!("{{{BASE},\n\"metrics\": {{}}}}")),
        ["3:12: metrics: the Admin API serves metrics: set admin too"]
    );
    // With admin, admin alone is refused, until akit has an Admin API (I26).
    let errs = errors(&format!("{{{BASE}, \"admin\": {{}}, \"metrics\": {{}}}}"));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("admin: not supported yet"), "{errs:?}");
    // Left out, nothing is counted.
    assert_eq!(
        from_json(&format!("{{{BASE}}}"), None).unwrap().metrics,
        None
    );
}

#[test]
fn has_no_settings() {
    let errs = errors(&format!("{{{BASE}, \"metrics\": {{\"x\": 1}}}}"));
    assert!(errs.iter().any(|e| e.contains("metrics.x")), "{errs:?}");
    let errs = errors(&format!("{{{BASE}, \"metrics\": true}}"));
    assert!(errs.iter().any(|e| e.contains("metrics")), "{errs:?}");
}

#[test]
fn one_file_only() {
    let a = format!("{{{BASE}, \"metrics\": {{}}, \"admin\": {{}}}}");
    let srcs = [
        Source {
            name: Some("a.json"),
            format: Format::Json,
            src: &a,
        },
        Source {
            name: Some("b.json"),
            format: Format::Json,
            src: r#"{"metrics": {}}"#,
        },
    ];
    let errs = from_sources(&srcs).unwrap_err();
    let errs: Vec<String> = errs.as_slice().iter().map(ToString::to_string).collect();
    assert!(
        errs.iter()
            .any(|e| e.starts_with("b.json:1:13: metrics: also given in a.json")),
        "{errs:?}"
    );
}
