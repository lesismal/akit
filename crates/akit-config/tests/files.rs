//! A config read from several files and directories, merged as fit merges them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use akit_config::{config_files, from_files};

/// A fresh directory under the target's temporary directory.
fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("files")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &Path, name: &str, text: &str) -> PathBuf {
    let p = d.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    p
}

fn errors(r: Result<akit_config::Config, akit_config::Errors>) -> Vec<String> {
    r.unwrap_err()
        .as_slice()
        .iter()
        .map(ToString::to_string)
        .collect()
}

const LISTENER: &str = r#"{"listeners": [{"name": "web", "address": ":8080"}]}"#;
const ROUTE: &str = r#"{"routes": [{"listeners": ["web"], "handler": {"module": "respond"}}]}"#;

#[test]
fn lists_are_joined_across_files() {
    let d = dir("joined");
    let a = write(&d, "a.json", LISTENER);
    let b = write(&d, "b.json", ROUTE);
    let c = from_files(&[&a, &b]).unwrap();
    assert_eq!(c.listeners.len(), 1);
    assert_eq!(c.routes[0].listeners, ["web"]);
    // The route alone names a listener that is in no file (and a config with routes and no
    // listeners is one fit serves over HTTPS by default).
    let errs = errors(from_files(&[&b]));
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert_eq!(
        errs[1],
        format!(
            "{}:1:13: routes[0].listeners[0]: there is no listener named \"web\"",
            b.display()
        )
    );
}

#[test]
fn problems_name_their_file_and_their_path_in_it() {
    let d = dir("dup");
    let a = write(&d, "a.json", LISTENER);
    let b = write(
        &d,
        "b.json",
        "{\n  \"listeners\": [\n    {\"name\": \"other\", \"address\": \":1\"},\n    {\"name\": \"web\", \"address\": \":2\"}]}",
    );
    let errs = errors(from_files(&[&a, &b]));
    assert_eq!(
        errs,
        [format!(
            "{}:4:5: listeners[1].name: another listener is already named \"web\"",
            b.display()
        )]
    );
}

#[test]
fn schema_versions_must_agree() {
    let d = dir("schema");
    let a = write(&d, "a.json", r#"{"schema_version": 1}"#);
    let b = write(&d, "b.json", r#"{"schema_version": 1, "listeners": []}"#);
    from_files(&[&a, &b]).unwrap();
    let c = write(&d, "c.json", r#"{"schema_version": 2}"#);
    let errs = errors(from_files(&[&a, &c]));
    assert_eq!(errs.len(), 1);
    // akit reads schema_version 1 only: 2 is refused in its own file.
    assert!(
        errs[0].starts_with(&format!("{}:1:20: schema_version", c.display())),
        "{errs:?}"
    );
}

#[test]
fn directories_in_lexical_order_without_dotfiles() {
    let d = dir("tree");
    write(&d, "b.json", ROUTE);
    write(&d, "a/listener.json", LISTENER);
    write(&d, ".hidden.json", "not json");
    write(&d, ".git/x.json", "not json");
    write(&d, "notes.txt", "not a config");
    let files = config_files(&[&d]).unwrap();
    let names: Vec<_> = files
        .iter()
        .map(|f| f.strip_prefix(&d).unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [PathBuf::from("a/listener.json"), PathBuf::from("b.json")]
    );
    let c = from_files(&[&d]).unwrap();
    assert_eq!((c.listeners.len(), c.routes.len()), (1, 1));
    // A file named twice, here by the directory and by itself, is read once.
    let again = config_files(&[d.clone(), d.join("./b.json")]).unwrap();
    assert_eq!(again.len(), 2);
}

#[test]
fn other_formats_are_not_supported_yet() {
    let d = dir("formats");
    let conf = write(&d, "fit.conf", "listener web { address :8080 }");
    let yaml = write(&d, "fit.yaml", "listeners: []");
    let errs = errors(from_files(&[&conf, &yaml]));
    assert_eq!(
        errs,
        [format!(
            "{}: not supported yet: .conf files (I23)",
            conf.display()
        )]
    );
    // YAML is read (I24).
    assert!(from_files(&[&yaml]).is_ok());
    // A file named with any other extension is JSON, as in fit.
    let other = write(&d, "fit.cfg", LISTENER);
    assert_eq!(from_files(&[&other]).unwrap().listeners.len(), 1);
}

#[test]
fn missing_and_empty() {
    let d = dir("empty");
    let errs = errors(from_files(&[&d]));
    assert_eq!(
        errs,
        [format!(
            "{}: no config files in the directory (.conf, .yaml, .yml, .json)",
            d.display()
        )]
    );
    let missing = d.join("missing.json");
    let errs = errors(from_files(&[&missing]));
    assert_eq!(errs.len(), 1);
    assert!(
        errs[0].starts_with(&format!("{}: ", missing.display())),
        "{errs:?}"
    );
}
