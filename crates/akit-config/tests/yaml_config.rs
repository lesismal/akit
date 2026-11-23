//! Configs written in YAML (I24, decision 005): fit's YAML templates read into the same model,
//! with the same problems, as their JSON twins; YAML and JSON files merge; problems point at the
//! YAML.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use akit_config::{Config, Errors, from_files, from_json, from_yaml};

fn templates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/fixtures/fit/templates")
}

/// The outcome without the files' names and positions, which differ between the formats.
fn outcome(r: Result<Config, Errors>) -> Result<Config, Vec<(String, String)>> {
    r.map_err(|e| {
        e.as_slice()
            .iter()
            .map(|e| (e.path.clone(), e.msg.clone()))
            .collect()
    })
}

#[test]
fn templates_read_as_their_json_twins() {
    let t = templates();
    for (yaml, json) in [
        ("yaml/fit.yaml", "json/fit.json"),
        ("yaml/conf.d", "json/conf.d"),
    ] {
        let y = outcome(from_files(&[t.join(yaml)]));
        let j = outcome(from_files(&[t.join(json)]));
        // The same config when both read (the single-file templates since I14), the same
        // problems when both are refused.
        assert_eq!(y, j, "{yaml} and {json}");
    }
}

#[test]
fn the_same_config_in_both_formats() {
    let y = from_yaml(
        "# comment\nlisteners:\n  - name: web\n    address: ':8080'\n    protocols: [http/1.1]\nroutes:\n  - name: hello\n    listeners: [web]\n    handler:\n      module: respond\n      params:\n        status: 201\n        body: |\n          hello\n",
        Some("t.yaml"),
    )
    .unwrap();
    let j = from_json(
        r#"{"listeners": [{"name": "web", "address": ":8080", "protocols": ["http/1.1"]}],
            "routes": [{"name": "hello", "listeners": ["web"],
                        "handler": {"module": "respond", "params": {"status": 201, "body": "hello\n"}}}]}"#,
        None,
    )
    .unwrap();
    assert_eq!(y, j);
}

#[test]
fn problems_point_at_the_yaml() {
    let src = "listeners:\n  - name: web\n    address: \":8080\"\n    protocols: [http/1.1, h9]\n";
    let e = from_yaml(src, Some("t.yaml")).unwrap_err();
    let e = &e.as_slice()[0];
    assert_eq!(e.file.as_deref(), Some("t.yaml"));
    assert_eq!(e.path, "listeners[0].protocols[1]");
    assert_eq!((e.line, e.col), (4, 27), "{e}");
    let e = from_yaml("a: [1\n", Some("t.yaml")).unwrap_err();
    assert!(e.to_string().contains("did not find expected"), "{e}");
    let e = from_yaml("a: 1\n---\nb: 2\n", None).unwrap_err();
    assert!(e.to_string().contains("more than one document"), "{e}");
}

#[test]
fn yaml_and_json_files_merge() {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("yaml-merge");
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("a.yml"),
        "listeners:\n  - {name: web, address: \":8080\"}\n",
    )
    .unwrap();
    fs::write(
        d.join("b.json"),
        r#"{"routes": [{"listeners": ["web"], "handler": {"module": "respond"}}]}"#,
    )
    .unwrap();
    let c = from_files(&[&d]).unwrap();
    assert_eq!(c.listeners.len(), 1);
    assert_eq!(c.routes.len(), 1);
    // An empty YAML file is an empty config, as fit reads it.
    fs::write(d.join("c.yaml"), "# nothing yet\n").unwrap();
    assert_eq!(from_files(&[&d]).unwrap(), c);
}
