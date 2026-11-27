//! The pages' methods of akit's LLM roles (I109d) with fit's roles (decision 038): a link token
//! akit's hub issues from its pages takes fit's node; akit's proxy dials fit's hub from its pages,
//! and lists fit's hub's nodes and models, and counts a call through them. A fake
//! OpenAI-compatible upstream answers. Needs fit's binary:
//! `AKIT_FIT_BIN=/path/to/fit cargo test -p akit-llm --test fit_pages -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::process::{Child, Command, Stdio};

use akit_llm::json;
use support::{Roles, Upstream, agent, call, free_port, key_hash, until};

const KEY: &str = "ah-pages-key-0123456789";

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
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "akit-llm-fit-pages-{tag}-{}-{}",
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

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn akit_pages_with_fit_roles() {
    let up = Upstream::start();
    // akit's listeners on port 0; fit's hub's port named before it starts.
    let roles = Roles::start(&format!(
        r#"{{"llm": {{
          "hub": {{"id": "hub-akit000000000061", "name": "ahub",
            "listen": {{"address": "127.0.0.1:0"}}}},
          "proxy": {{"id": "proxy-akit00000000061", "name": "aproxy",
            "limits": {{"retry_interval": "200ms"}},
            "api": {{"address": "127.0.0.1:0"}},
            "api_keys": [{{"id": "0123456789abcdef", "name": "t", "prefix": "ah-pages", "hash": "{}"}}]}}}}}}"#,
        key_hash(KEY)
    ));
    let hub = roles.local("llm.hub");
    let proxy = roles.local("llm.proxy");
    let (ahub, fhub, api) = (roles.mesh_port("llm.hub"), free_port(), roles.api_port());

    // A token akit's hub issues from its pages takes fit's node.
    let is = call(&hub, "/mesh/tokens/create", r#"{"name": "fit node"}"#).unwrap();
    // fit names its tokens `fitt1.`, akit `akitt1.` (decision 038): the same token otherwise.
    let token = json::get_str(&json::parse(is.as_bytes()).unwrap(), "token")
        .replacen("akitt1.", "fitt1.", 1);
    let _fit = fit(
        "roles",
        &format!(
            r#"{{"llm": {{
              "hub": {{"id": "hub-fit00000000000061", "name": "fhub",
                "listen": {{"address": "127.0.0.1:{fhub}", "node_password": "np", "proxy_password": "pp"}}}},
              "node": {{"id": "node-fit0000000000061", "name": "fnode",
                "peers": [{{"address": "127.0.0.1:{fhub}", "password": "np", "role": "hub"}},
                          {{"address": "127.0.0.1:{ahub}", "token": "{token}", "role": "hub"}}],
                "models": [{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:{}/v1"}}]}}}}}}"#,
            up.port
        ),
    );
    until("fit's node linked to akit's hub with the token", || {
        call(&hub, "/mesh/peers", "")
            .unwrap()
            .contains("node-fit0000000000061")
    });
    let list = call(&hub, "/mesh/tokens/list", "").unwrap();
    assert!(list.contains("node-fit0000000000061"), "{list}");

    // akit's proxy dials fit's hub from its pages, and lists its nodes and models.
    let out = call(
        &proxy,
        "/mesh/dial",
        &format!(r#"{{"addr": "127.0.0.1:{fhub}", "password": "pp", "role": "hub"}}"#),
    )
    .unwrap();
    assert!(out.contains(&format!("127.0.0.1:{fhub}")), "{out}");
    until("fit's hub's node list on akit's proxy", || {
        let v = json::parse(call(&proxy, "/proxy/node/list", "").unwrap().as_bytes()).unwrap();
        json::get_arr(&v, "hubs").iter().any(|h| {
            json::get_bool(h, "online")
                && json::get_arr(h, "nodes")
                    .iter()
                    .any(|n| json::get_str(n, "id") == "node-fit0000000000061")
        })
    });
    until("the model through fit's hub", || {
        let m = call(&proxy, "/proxy/models", "").unwrap();
        m.contains("hub-fit00000000000061") && m.contains(r#""ready":true"#)
    });
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m1"}"#,
    );
    assert_eq!(a.status, 200, "{}", a.text());
    until("the call counted", || {
        call(&proxy, "/proxy/stats", "")
            .unwrap()
            .contains(r#""model":"m1","calls":1,"errors":0"#)
    });

    // Revoked from the pages: fit's node's link closes.
    let id = json::get_str(&json::parse(is.as_bytes()).unwrap(), "id");
    call(&hub, "/mesh/tokens/revoke", &format!(r#"{{"id": "{id}"}}"#)).unwrap();
    until("fit's node's link closed", || {
        !call(&hub, "/mesh/peers", "")
            .unwrap()
            .contains("node-fit0000000000061")
    });
    drop(roles);
}
