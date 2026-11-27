//! akit's LLM roles mixed with fit's (decision 038): an agent's call through a fit hub to akit's
//! node from akit's proxy; through akit's hub to fit's node from fit's proxy; and from akit's
//! proxy to fit's node directly. A fake OpenAI-compatible upstream answers. Needs fit's binary:
//! `AKIT_FIT_BIN=/path/to/fit cargo test -p akit-llm --test fit_roles -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::process::{Child, Command, Stdio};

use support::{Roles, Upstream, agent, free_port, key_hash, until};

const KEY: &str = "ah-mixed-key-0123456789";

/// fit running a config, killed when dropped.
struct Fit(Child, std::path::PathBuf);

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn fit(tag: &str, config: &str) -> Fit {
    let bin = std::env::var("AKIT_FIT_BIN").expect("AKIT_FIT_BIN: the path of fit's binary");
    let dir = std::env::temp_dir().join(format!(
        "akit-llm-fit-roles-{tag}-{}-{}",
        std::process::id(),
        akit_roles::linktoken::unix_now()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fit.json");
    std::fs::write(&path, config).unwrap();
    let log = std::fs::File::create(dir.join("fit.log")).unwrap();
    let child = Command::new(bin)
        .args(["run", "-c"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    Fit(child, dir)
}

fn keys() -> String {
    format!(
        r#"[{{"id": "0123456789abcdef", "name": "t", "prefix": "ah-mixed", "hash": "{}", "created_at": 1700000000}}]"#,
        key_hash(KEY)
    )
}

fn models(upstream: u16) -> String {
    format!(
        r#"[{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:{upstream}/v1", "upstream": "up-m1"}},
            {{"name": "m2", "api": "openai", "base_url": "http://127.0.0.1:{upstream}/v1"}}]"#
    )
}

fn calls(api: u16, node: &str) {
    until("the proxy to list the models", || {
        if std::net::TcpStream::connect(("127.0.0.1", api)).is_err() {
            return false; // fit still starting
        }
        let a = agent(api, "GET", "/v1/models", KEY, b"");
        a.status == 200 && a.text().contains("\"m1\"")
    });
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m1"}"#,
    );
    assert_eq!(a.status, 200, "{}", a.text());
    assert!(a.text().contains(r#""model":"up-m1""#), "{}", a.text());
    let node_header = a
        .header("x-akit-node")
        .or_else(|| a.header("x-fit-node"))
        .unwrap_or("");
    assert_eq!(node_header, node);
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m2","stream":true}"#,
    );
    assert_eq!(a.status, 200);
    assert!(a.text().ends_with("data: [DONE]\n\n"), "{}", a.text());
    let big = format!(r#"{{"model":"m2","pad":"{}"}}"#, "x".repeat(3 << 20));
    let a = agent(api, "POST", "/v1/chat/completions", KEY, big.as_bytes());
    assert_eq!(a.status, 200, "{}", a.text());
    assert!(
        a.text().contains(&format!(r#""bytes":{}"#, big.len())),
        "{}",
        a.text()
    );
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"nope"}"#,
    );
    assert_eq!(a.status, 404, "{}", a.text());
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn fit_hub_between_akit_node_and_akit_proxy() {
    let up = Upstream::start();
    // fit's hub's port is named before it starts.
    let hub = free_port();
    let _fit = fit(
        "hub",
        &format!(
            r#"{{"llm": {{"hub": {{"id": "hub-fit00000000000001",
              "listen": {{"address": "127.0.0.1:{hub}", "node_password": "np", "proxy_password": "pp"}}}}}}}}"#
        ),
    );
    let _akit = Roles::start(&format!(
        r#"{{"llm": {{
          "node": {{"id": "node-akit000000000001",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "np", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}}, "models": {}}},
          "proxy": {{"id": "proxy-akit00000000001",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "pp", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}},
            "api": {{"address": "127.0.0.1:0"}}, "api_keys": {}}}}}}}"#,
        models(up.port),
        keys()
    ));
    calls(_akit.api_port(), "node-akit000000000001");
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn akit_hub_between_fit_node_and_fit_proxy() {
    let up = Upstream::start();
    // akit's hub on port 0; fit's proxy's API named before it starts.
    let _akit = Roles::start(
        r#"{"llm": {"hub": {"id": "hub-akit000000000001",
          "listen": {"address": "127.0.0.1:0", "node_password": "np", "proxy_password": "pp"}}}}"#,
    );
    let (hub, api) = (_akit.mesh_port("llm.hub"), free_port());
    let _node = fit(
        "node",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000001",
              "peers": [{{"address": "127.0.0.1:{hub}", "password": "np", "role": "hub"}}],
              "models": {}}}}}}}"#,
            models(up.port)
        ),
    );
    let _proxy = fit(
        "proxy",
        &format!(
            r#"{{"llm": {{"proxy": {{"id": "proxy-fit000000000001",
              "peers": [{{"address": "127.0.0.1:{hub}", "password": "pp", "role": "hub"}}],
              "api": {{"address": "127.0.0.1:{api}"}}, "api_keys": {}}}}}}}"#,
            keys()
        ),
    );
    calls(api, "node-fit0000000000001");
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn akit_proxy_to_fit_node_directly() {
    let up = Upstream::start();
    // fit's node's port is named before it starts.
    let node = free_port();
    let _node = fit(
        "direct",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000002",
              "listen": {{"address": "127.0.0.1:{node}", "proxy_password": "pp"}},
              "models": {}}}}}}}"#,
            models(up.port)
        ),
    );
    let _akit = Roles::start(&format!(
        r#"{{"llm": {{"proxy": {{"id": "proxy-akit00000000002",
          "peers": [{{"address": "127.0.0.1:{node}", "password": "pp", "role": "node"}}],
          "limits": {{"retry_interval": "200ms"}},
          "api": {{"address": "127.0.0.1:0"}}, "api_keys": {}}}}}}}"#,
        keys()
    ));
    calls(_akit.api_port(), "node-fit0000000000002");
}
