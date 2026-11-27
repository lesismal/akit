//! The methods the LLM roles' pages call (I109d), ported from fit's tests of them
//! (`mesh/listen_test.go`'s `TestRegisterAdmin`, `token_admin_test.go`, `edge_test.go`'s
//! `TestAdminEdges`, `role/settings_test.go`, `proxy/proxy_test.go`'s `TestKeysMethods`,
//! `edge_test.go`'s `TestProxyRoleEdges`): called as the roles' own pages call them, their edits
//! written into the members by a control thread as `akit run`'s.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use akit_llm::json;
use akit_llm::model::{CODE_BAD_REQUEST, CODE_INTERNAL, CODE_NOT_FOUND};
use support::{Roles, Upstream, agent, call, code, free_port, key_hash, until};

fn console(port: u16) -> String {
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    format!(
        r#"{{"address": "127.0.0.1:{port}", "users": [{{"username": "admin", "password_hash": "{hash}"}}]}}"#
    )
}

/// The mesh's methods of a node and a hub: dial, undial, kick, listen, limits, tokens, and the
/// identity's `http_url`.
#[test]
#[allow(clippy::too_many_lines)]
fn mesh_methods() {
    let (hub, con) = (free_port(), free_port());
    let roles = Roles::start(&format!(
        r#"{{"llm": {{
          "hub": {{"id": "hub-0000000000000041", "name": "hub",
                   "listen": {{"address": "127.0.0.1:{hub}", "node_password": "npw"}}}},
          "node": {{"id": "node-0000000000000042", "name": "box", "console": {},
                    "limits": {{"retry_interval": "200ms"}}}}}}}}"#,
        console(con)
    ));
    let node = roles.local("llm.node");
    let hubl = roles.local("llm.hub");

    // The identity says where the console is.
    let peers = call(&node, "/mesh/peers", "").unwrap();
    assert!(
        peers.contains(&format!(r#""http_url":"http://127.0.0.1:{con}/""#)),
        "{peers}"
    );

    // Limits: the defaults filled in; written without them; a refused edit changes nothing.
    let lim = call(&hubl, "/mesh/limits/get", "").unwrap();
    assert_eq!(
        lim,
        r#"{"max_conns":1024,"max_inflight":1024,"ping_interval":"15s","slow_timeout":"30s","retry_interval":"5s","stream_idle":"2m0s"}"#
    );
    let lim = call(
        &hubl,
        "/mesh/limits/set",
        r#"{"max_inflight": 8, "slow_timeout": "1m", "retry_interval": "5s"}"#,
    )
    .unwrap();
    assert!(lim.contains(r#""max_inflight":8"#), "{lim}");
    let written = r#""limits":{"max_inflight":8,"slow_timeout":"1m0s"}"#;
    assert!(
        roles.member("llm.hub").contains(written),
        "{}",
        roles.member("llm.hub")
    );
    *roles.fail.lock().unwrap() = Some("refused".to_owned());
    let r = call(&hubl, "/mesh/limits/set", r#"{"max_conns": 3}"#);
    assert_eq!(code(&r), CODE_BAD_REQUEST, "{r:?}");
    *roles.fail.lock().unwrap() = None;
    assert!(roles.member("llm.hub").contains(written));

    // Bad dials change nothing and save nothing.
    let tok_bad = r#"{"addr": "127.0.0.1:9", "token": "nope", "role": "hub"}"#;
    for req in [
        r#"{"addr": "127.0.0.1:9", "password": "pw", "role": "node"}"#,
        r#"{"addr": "127.0.0.1:9", "password": "pw", "role": "edge"}"#,
        r#"{"addr": "127.0.0.1", "password": "pw", "role": "hub"}"#,
        r#"{"addr": ":9", "password": "pw", "role": "hub"}"#,
        r#"{"addr": "127.0.0.1:9", "role": "hub"}"#,
        r#"{"addr": "127.0.0.1:99999", "password": "pw", "role": "hub"}"#,
        r#"{"addr": "10.0.0.9:9", "password": "pw", "role": "hub"}"#,
        tok_bad,
    ] {
        let r = call(&node, "/mesh/dial", req);
        assert_eq!(code(&r), CODE_BAD_REQUEST, "{req}: {r:?}");
    }
    let r = call(&node, "/mesh/undial", r#"{"addr": "127.0.0.1:9"}"#);
    assert_eq!(code(&r), CODE_NOT_FOUND, "{r:?}");
    assert!(!roles.member("llm.node").contains("peers"));

    // A dial: written, answered without its password, linked.
    let target = format!("127.0.0.1:{hub}");
    let out = call(
        &node,
        "/mesh/dial",
        &format!(r#"{{"addr": "{target}", "password": "npw", "role": "hub"}}"#),
    )
    .unwrap();
    assert!(out.contains(&target) && !out.contains("npw"), "{out}");
    assert!(roles.member("llm.node").contains(&format!(
        r#""peers":[{{"address":"{target}","password":"npw","role":"hub"}}]"#
    )));
    until("the node linked to the hub", || {
        call(&hubl, "/mesh/peers", "")
            .unwrap()
            .contains("node-0000000000000042")
    });
    // The hub kicks it; it links again.
    let out = call(&hubl, "/mesh/kick", r#"{"id": "node-0000000000000042"}"#).unwrap();
    assert!(!out.contains(r#""closed":0"#), "{out}");
    let out = call(&hubl, "/mesh/kick", r#"{"id": "nobody"}"#).unwrap();
    assert_eq!(out, r#"{"closed":0}"#);
    until("the node linked again", || {
        call(&hubl, "/mesh/peers", "")
            .unwrap()
            .contains("node-0000000000000042")
    });

    // Listen: an empty password keeps the current one, each role's apart; cleared at once.
    let (l1, l2) = (free_port(), free_port());
    call(
        &node,
        "/mesh/listen",
        &format!(r#"{{"addr": "127.0.0.1:{l1}", "password": "lpw"}}"#),
    )
    .unwrap();
    call(
        &node,
        "/mesh/listen",
        &format!(r#"{{"addr": "127.0.0.1:{l2}", "hub_password": "hubpw"}}"#),
    )
    .unwrap();
    let out = call(
        &node,
        "/mesh/listen",
        &format!(r#"{{"addr": "127.0.0.1:{l2}"}}"#),
    )
    .unwrap();
    assert!(
        out.contains(&format!(
            r#""listen":{{"addr":"127.0.0.1:{l2}","has_password":true}}"#
        )),
        "{out}"
    );
    assert!(
        roles.member("llm.node").contains(&format!(
            r#""listen":{{"address":"127.0.0.1:{l2}","password":"lpw","hub_password":"hubpw"}}"#
        )),
        "{}",
        roles.member("llm.node")
    );
    let out = call(
        &node,
        "/mesh/listen",
        &format!(r#"{{"addr": "127.0.0.1:{l2}", "clear_passwords": true}}"#),
    )
    .unwrap();
    assert!(out.contains(r#""has_password":false"#), "{out}");
    for req in [
        r#"{"addr": "nope", "password": "pw"}"#,
        r#"{"addr": "192.0.2.1:0", "password": "pw"}"#,
    ] {
        let r = call(&node, "/mesh/listen", req);
        assert_eq!(code(&r), CODE_BAD_REQUEST, "{req}: {r:?}");
    }
    // A listener that cannot bind: the pages are told why.
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let r = call(
        &node,
        "/mesh/listen",
        &format!(r#"{{"addr": "{}"}}"#, held.local_addr().unwrap()),
    );
    assert_eq!(code(&r), CODE_INTERNAL, "{r:?}");
    drop(held);
    call(&node, "/mesh/listen", r#"{"addr": ""}"#).unwrap();
    assert!(!roles.member("llm.node").contains("\"listen\""));

    // Undial.
    call(&node, "/mesh/undial", &format!(r#"{{"addr": "{target}"}}"#)).unwrap();
    assert!(!roles.member("llm.node").contains("peers"));
    until("the node unlinked", || {
        !call(&hubl, "/mesh/peers", "")
            .unwrap()
            .contains("node-0000000000000042")
    });

    // The hub's tokens: issued, the node links with one, its expiry changed, revoked.
    let r = call(&hubl, "/mesh/tokens/create", r#"{"name": " "}"#);
    assert_eq!(code(&r), CODE_BAD_REQUEST, "{r:?}");
    let is = call(
        &hubl,
        "/mesh/tokens/create",
        r#"{"name": "box", "ttl": "1h"}"#,
    )
    .unwrap();
    let v = json::parse(is.as_bytes()).unwrap();
    let (token, id) = (json::get_str(&v, "token"), json::get_str(&v, "id"));
    assert!(
        !token.is_empty() && json::get_int(&v, "expires_at") > 0,
        "{is}"
    );
    let list = call(&hubl, "/mesh/tokens/list", "").unwrap();
    assert!(
        list.starts_with(r#"{"tokens":["#) && list.contains(&id),
        "{list}"
    );
    call(
        &node,
        "/mesh/dial",
        &format!(r#"{{"addr": "{target}", "token": "{token}", "password": "x", "role": "hub"}}"#),
    )
    .unwrap();
    assert!(
        roles.member("llm.node").contains(&format!(
            r#""peers":[{{"address":"{target}","token":"{token}","role":"hub"}}]"#
        )),
        "{}",
        roles.member("llm.node")
    );
    until("the node linked with the token", || {
        call(&hubl, "/mesh/peers", "")
            .unwrap()
            .contains("node-0000000000000042")
    });
    let out = call(
        &hubl,
        "/mesh/tokens/expiry",
        &format!(r#"{{"id": "{id}", "ttl": "2h"}}"#),
    )
    .unwrap();
    assert!(out.contains(&id), "{out}");
    let r = call(
        &hubl,
        "/mesh/tokens/expiry",
        r#"{"id": "nope", "ttl": "2h"}"#,
    );
    assert_eq!(code(&r), CODE_NOT_FOUND, "{r:?}");
    assert_eq!(
        call(
            &hubl,
            "/mesh/tokens/revoke",
            &format!(r#"{{"id": "{id}"}}"#)
        )
        .unwrap(),
        r#"{"ok":true}"#
    );
    let r = call(
        &hubl,
        "/mesh/tokens/revoke",
        &format!(r#"{{"id": "{id}"}}"#),
    );
    assert_eq!(code(&r), CODE_NOT_FOUND, "{r:?}");
    until("the revoked token's link closed", || {
        !call(&hubl, "/mesh/peers", "")
            .unwrap()
            .contains("node-0000000000000042")
    });
    drop(roles);
}

const KEY: &str = "ah-test-key-0123456789";

/// The proxy's methods: its models, nodes, stats, keys, profile and API's settings.
#[test]
#[allow(clippy::too_many_lines)]
fn proxy_methods() {
    let up = Upstream::start();
    let (hub, api) = (free_port(), free_port());
    let roles = Roles::start(&format!(
        r#"{{"llm": {{
          "hub": {{"id": "hub-0000000000000051", "name": "hub",
            "listen": {{"address": "127.0.0.1:{hub}", "node_password": "np", "proxy_password": "pp"}},
            "limits": {{"retry_interval": "200ms"}}}},
          "node": {{"id": "node-0000000000000051", "name": "gpu",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "np", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}},
            "models": [{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:{}/v1"}}]}},
          "proxy": {{"id": "proxy-0000000000000051", "name": "agents",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "pp", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}},
            "api": {{"address": "127.0.0.1:{api}"}},
            "api_keys": [{{"id": "0123456789abcdef", "name": "test", "prefix": "ah-test", "hash": "{}"}}]}}
        }}}}"#,
        up.port,
        key_hash(KEY)
    ));
    let p = roles.local("llm.proxy");
    until("the proxy to list the model", || {
        call(&p, "/proxy/models", "")
            .unwrap()
            .contains(r#""ready":true"#)
    });
    let models = call(&p, "/proxy/models", "").unwrap();
    assert!(
        models.starts_with(r#"[{"name":"m1","api":"openai","ready":true,"routes":[{"kind":"hub","id":"hub-0000000000000051","name":"hub","ready":true"#),
        "{models}"
    );
    until("the hub's node list", || {
        call(&p, "/proxy/node/list", "")
            .unwrap()
            .contains("node-0000000000000051")
    });
    let nodes = call(&p, "/proxy/node/list", "").unwrap();
    let v = json::parse(nodes.as_bytes()).unwrap();
    let hubs = json::get_arr(&v, "hubs");
    assert_eq!(hubs.len(), 1, "{nodes}");
    assert!(json::get_bool(&hubs[0], "online"));
    assert_eq!(json::get_int(&hubs[0], "total"), 1, "{nodes}");
    assert!(json::get_arr(&v, "direct").is_empty());

    // A call counted, and the key's use.
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m1","messages":[]}"#,
    );
    assert_eq!(a.status, 200, "{}", a.text());
    until("the call counted", || {
        call(&p, "/proxy/stats", "")
            .unwrap()
            .contains(r#""model":"m1","calls":1,"errors":0"#)
    });
    let stats = call(&p, "/proxy/stats", "").unwrap();
    assert!(stats.contains(r#""inflight":0"#), "{stats}");
    let keys = call(&p, "/proxy/keys/list", "").unwrap();
    assert!(
        keys.starts_with(r#"[{"id":"0123456789abcdef","name":"test","prefix":"ah-test","created_at":0,"last_used_at":"#)
            && keys.ends_with(r#""calls":1}]"#)
            && !keys.contains("hash"),
        "{keys}"
    );

    // Keys made and deleted: a new one works at once; a deleted one no more.
    let r = call(&p, "/proxy/keys/create", r#"{"name": " "}"#);
    assert_eq!(code(&r), CODE_BAD_REQUEST);
    let made = call(&p, "/proxy/keys/create", r#"{"name": "agent"}"#).unwrap();
    let v = json::parse(made.as_bytes()).unwrap();
    let (key, id) = (json::get_str(&v, "key"), json::get_str(&v, "id"));
    assert!(
        key.starts_with("ah-") && key.starts_with(&json::get_str(&v, "prefix")),
        "{made}"
    );
    assert_eq!(agent(api, "GET", "/v1/models", &key, b"").status, 200);
    let r = call(&p, "/proxy/keys/delete", r#"{"id": "nope"}"#);
    assert_eq!(code(&r), CODE_NOT_FOUND);
    call(&p, "/proxy/keys/delete", &format!(r#"{{"id": "{id}"}}"#)).unwrap();
    assert_eq!(agent(api, "GET", "/v1/models", &key, b"").status, 401);
    call(&p, "/proxy/keys/delete", r#"{"id": "0123456789abcdef"}"#).unwrap();
    assert!(!roles.member("llm.proxy").contains("api_keys"));

    // The profile.
    let r = call(&p, "/proxy/profile/set", r#"{"name": "  "}"#);
    assert_eq!(code(&r), CODE_BAD_REQUEST);
    call(&p, "/proxy/profile/set", r#"{"name": "renamed"}"#).unwrap();
    assert!(roles.member("llm.proxy").contains(r#""name":"renamed""#));

    // The API's settings: read, changed, refused, taken out.
    let got = call(&p, "/proxy/api/get", "").unwrap();
    assert_eq!(
        got,
        format!(
            r#"{{"address":"127.0.0.1:{api}","cert_file":"","key_file":"","max_body_bytes":33554432,"max_conns":256,"max_inflight":256,"first_byte_timeout":"5m0s","timeout":"30m0s","url":"http://127.0.0.1:{api}"}}"#
        )
    );
    let api2 = free_port();
    let got = call(
        &p,
        "/proxy/api/set",
        &format!(r#"{{"address": "127.0.0.1:{api2}", "max_inflight": 8, "timeout": "30m"}}"#),
    )
    .unwrap();
    assert!(
        got.contains(r#""max_inflight":8"#) && got.contains(&format!("http://127.0.0.1:{api2}")),
        "{got}"
    );
    assert!(roles.member("llm.proxy").contains(&format!(
        r#""api":{{"address":"127.0.0.1:{api2}","max_inflight":8}}"#
    )));
    for req in [
        r#"{"address": "10.1.1.1:9"}"#,
        r#"{"address": "127.0.0.1:9", "cert_file": "/nonexistent"}"#,
        r#"{"address": "127.0.0.1:9", "cert_file": "/nonexistent", "key_file": "/nonexistent"}"#,
    ] {
        let r = call(&p, "/proxy/api/set", req);
        assert_eq!(code(&r), CODE_BAD_REQUEST, "{req}: {r:?}");
    }
    let got = call(&p, "/proxy/api/set", r#"{"address": ""}"#).unwrap();
    assert!(
        got.starts_with(r#"{"address":"","#) && !got.contains("url"),
        "{got}"
    );
    assert!(!roles.member("llm.proxy").contains("\"api\""));
    drop(roles);
}

/// A console whose port is chosen when it starts: the console server tells the role where it
/// listens (`Module::console_url`), and the identity's `http_url` says so; none once it stops.
#[test]
fn http_url_of_a_chosen_port() {
    let roles = Roles::start(&format!(
        r#"{{"llm": {{"node": {{"id": "node-0000000000000043", "name": "box", "console": {}}}}}}}"#,
        console(0)
    ));
    let node = roles.local("llm.node");
    let peers = call(&node, "/mesh/peers", "").unwrap();
    let url = peers
        .split(r#""http_url":""#)
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or_default()
        .to_owned();
    assert!(
        url.starts_with("http://127.0.0.1:") && !url.ends_with(":0/"),
        "{peers}"
    );
    let port: u16 = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "{url}"
    );
    // The console removed from the member: the URL goes with it.
    let doc = r#"{"llm": {"node": {"id": "node-0000000000000043", "name": "box"}}}"#;
    let errs = roles
        .roles
        .lock()
        .unwrap()
        .apply(&akit_config::from_json(doc, None).unwrap());
    assert!(errs.is_empty(), "{errs:?}");
    let peers = call(&node, "/mesh/peers", "").unwrap();
    assert!(!peers.contains(&url), "{peers}");
}
