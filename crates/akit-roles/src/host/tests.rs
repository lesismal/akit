//! fit's `app/app_test.go`: a config starts, moves and stops a role; a role's failure is a
//! warning (I100).

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

static APPLIED: AtomicUsize = AtomicUsize::new(0);
static STOPPED: AtomicUsize = AtomicUsize::new(0);

/// A role that counts what is asked of it, and fails on a hub named "bad".
struct Stub;

impl Role for Stub {
    fn apply(&mut self, c: &Config) -> Result<(), String> {
        let name = c
            .tunnel
            .as_ref()
            .and_then(|t| t.hub.as_ref())
            .map(|h| h.name.clone())
            .unwrap_or_default();
        if name == "bad" {
            return Err("cannot".into());
        }
        APPLIED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self, _grace: Duration) {
        STOPPED.fetch_add(1, Ordering::SeqCst);
    }

    fn tokens(&self) -> Option<Registry> {
        Some(Registry::default())
    }
}

const STUB: Info = Info {
    id: "tunnel.hub",
    set: |c| c.tunnel.as_ref().is_some_and(|t| t.hub.is_some()),
    new: |_| Ok(Box::new(Stub)),
};

fn engine() -> Arc<akio::Engine> {
    let e = akio::Engine::builder().client_threads(1).build().unwrap();
    e.start().unwrap();
    Arc::new(e)
}

fn config(doc: &str) -> Config {
    akit_config::from_json(doc, None).unwrap()
}

#[test]
fn a_config_starts_moves_and_stops_a_role() {
    let e = engine();
    let mut m = Manager::new(&[STUB], Arc::clone(&e), None);
    assert!(m.apply(&config("{}")).is_empty());
    assert!(m.running().is_empty());
    assert!(
        m.apply(&config(r#"{"tunnel": {"hub": {"name": "a"}}}"#))
            .is_empty()
    );
    assert_eq!(m.running(), ["tunnel.hub"]);
    assert_eq!(APPLIED.load(Ordering::SeqCst), 1);
    assert!(token_views(&m.registries(), "tunnel.hub").is_some());
    // Moved: applied again, not started anew.
    assert!(
        m.apply(&config(r#"{"tunnel": {"hub": {"name": "b"}}}"#))
            .is_empty()
    );
    assert_eq!(APPLIED.load(Ordering::SeqCst), 2);
    // A failure is a warning, and the role runs as it did.
    let errs = m.apply(&config(r#"{"tunnel": {"hub": {"name": "bad"}}}"#));
    assert_eq!(errs, ["tunnel.hub: cannot; it runs as it did"]);
    assert_eq!(m.running(), ["tunnel.hub"]);
    // Left out: stopped.
    assert!(m.apply(&config("{}")).is_empty());
    assert!(m.running().is_empty());
    assert_eq!(STOPPED.load(Ordering::SeqCst), 1);
    assert!(token_views(&m.registries(), "tunnel.hub").is_none());
    // A role that fails to start is not running.
    let errs = m.apply(&config(r#"{"tunnel": {"hub": {"name": "bad"}}}"#));
    assert_eq!(errs, ["tunnel.hub: cannot"]);
    assert!(m.running().is_empty());
    m.stop(Duration::ZERO);
    e.shutdown();
    e.wait();
}

#[test]
fn members_no_role_runs_are_named() {
    let c = config(r#"{"tunnel": {"hub": {}, "node": {}}, "llm": {"proxy": {}}}"#);
    assert_eq!(unknown(&c, &[STUB]), ["llm.proxy", "tunnel.node"]);
    assert!(unknown(&config("{}"), &[]).is_empty());
}

#[test]
fn edits_change_the_member() {
    let mut root = akit_config::value::parse(r#"{"listeners": []}"#).unwrap();
    let edit = Edit {
        id: "tunnel.hub",
        change: Box::new(|m| {
            *m = Value::Object(vec![("name".into(), Value::String("x".into()))]);
            Ok(())
        }),
        done: Box::new(|_| {}),
    };
    assert!(edit.apply_to(&mut root).is_ok());
    assert_eq!(
        root.to_json(),
        "{\n  \"listeners\": [],\n  \"tunnel\": {\n    \"hub\": {\n      \"name\": \"x\"\n    }\n  }\n}\n"
    );
    // A host without an editor says so.
    let e = engine();
    let m = Manager::new(&[], Arc::clone(&e), None);
    let said = Arc::new(Mutex::new(None));
    let s2 = Arc::clone(&said);
    m.host("tunnel.hub").edit(
        Box::new(|_| Ok(())),
        Box::new(move |r| *s2.lock().unwrap() = Some(r)),
    );
    assert!(matches!(said.lock().unwrap().as_ref(), Some(Err(_))));
    e.shutdown();
    e.wait();
}

#[test]
fn files_are_kept_beside_the_config() {
    let dir = std::env::temp_dir();
    assert_eq!(beside_prefix(""), None);
    assert_eq!(beside_prefix("-"), None);
    assert_eq!(
        beside_prefix(&dir.to_string_lossy()),
        Some(dir.join("akit").to_string_lossy().into_owned())
    );
    let f = dir.join("conf").join("akit.json");
    assert_eq!(
        beside_prefix(&f.to_string_lossy()),
        Some(dir.join("conf").join("akit").to_string_lossy().into_owned())
    );
}
