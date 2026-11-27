//! A node's management across akit and fit (decision 038, I106b and I108): akit's hub manages
//! fit's node through `/hub/node/call` and `/hub/node/stream`. Its jobs run a stand-in `ollama`
//! put first on the node's PATH, never a real tool. Needs fit's binary:
//! `AKIT_FIT_BIN=/path/to/fit cargo test -p akit-llm --test fit_manage -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use support::{Roles, call, free_port, read_stream, until};

/// fit running a config, killed when dropped.
struct Fit(Child, PathBuf);

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

/// A directory of its own for `tag`, with a stand-in `ollama` that says what it was asked.
fn dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "akit-llm-fit-manage-{tag}-{}-{}",
        std::process::id(),
        akit_roles::linktoken::unix_now()
    ));
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let tool = dir.join("bin/ollama");
    std::fs::write(&tool, "#!/bin/sh\necho \"listed $1\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn fit(tag: &str, config: &str) -> Fit {
    let bin = std::env::var("AKIT_FIT_BIN").expect("AKIT_FIT_BIN: the path of fit's binary");
    let dir = dir(tag);
    let path = dir.join("fit.json");
    std::fs::write(&path, config).unwrap();
    let log = std::fs::File::create(dir.join("fit.log")).unwrap();
    let child = Command::new(bin)
        .args(["run", "-c"])
        .arg(&path)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", dir.join("bin").display()),
        )
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    Fit(child, dir)
}

fn json(s: &str) -> akit_config::value::Value {
    akit_llm::json::parse(s.as_bytes()).unwrap()
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn akit_hub_manages_fit_node() {
    let hub_port = free_port();
    let roles = Roles::start(&format!(
        r#"{{"llm": {{"hub": {{"id": "hub-akit000000000011",
          "listen": {{"address": "127.0.0.1:{hub_port}", "node_password": "np"}}}}}}}}"#
    ));
    let node = fit(
        "node",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000011", "name": "box",
              "peers": [{{"address": "127.0.0.1:{hub_port}", "password": "np", "role": "hub"}}],
              "settings": {{"allow_hub_manage": true, "gpu_sudo": false}},
              "models": [{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:1/v1", "api_key": "sk-0123456789abcdef"}}]}}}}}}"#
        ),
    );
    let hub = roles.local("llm.hub");
    let id = "node-fit0000000000011";
    let nc = |method: &str, req: &str| {
        call(
            &hub,
            "/hub/node/call",
            &format!(r#"{{"id":"{id}","method":"{method}","req":{req}}}"#),
        )
    };
    until("fit's node to link", || {
        call(&hub, "/hub/nodes", "")
            .unwrap()
            .contains(r#""online":true"#)
    });
    let list = nc("/node/model/list", "{}").unwrap();
    assert!(list.contains(r#""api_key":"sk-0****cdef""#), "{list}");

    // A job of fit's node, followed through akit's hub.
    let job = nc("/node/action/run", r#"{"id":"ollama_list"}"#).unwrap();
    let jid = akit_llm::json::get_int(&json(&job), "id");
    let st = hub
        .open_stream(
            "/hub/node/stream",
            &format!(r#"{{"id":"{id}","method":"/node/job/follow","req":{{"id":{jid}}}}}"#),
        )
        .unwrap();
    let (frames, end) = read_stream(&st);
    let out: String = frames
        .iter()
        .map(|f| akit_llm::json::get_str(&json(std::str::from_utf8(f).unwrap()), "output"))
        .collect();
    assert!(
        out.contains("$ ollama list") && out.contains("listed list"),
        "{out}"
    );
    let end = String::from_utf8(end.unwrap()).unwrap();
    assert_eq!(
        akit_llm::json::get_str(&json(&end), "status"),
        "done",
        "{end}"
    );
    let r = nc("/node/action/run", r#"{"id":"rm_rf"}"#);
    assert!(r.unwrap_err().to_string().contains("unknown action"));

    // A model saved through the hub lands in fit's running config.
    nc(
        "/node/model/save",
        r#"{"name":"m2","api":"openai","base_url":"http://127.0.0.1:2/v1"}"#,
    )
    .unwrap();
    let list = nc("/node/model/list", "{}").unwrap();
    assert!(list.contains(r#""name":"m2""#), "{list}");
    drop(node);
}
