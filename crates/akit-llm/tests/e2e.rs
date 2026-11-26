//! fit's `llm/e2e` tests, ported (I105-I107): an agent calls a proxy, which relays through a hub
//! to a node, which forwards to a fake OpenAI-compatible upstream; all three roles of akit on one
//! engine, as `akit run` runs them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use support::{Roles, Upstream, agent, free_port, key_hash, until};

const KEY: &str = "ah-test-key-0123456789";

fn config(hub: u16, api: u16, upstream: u16, direct: Option<u16>) -> String {
    let node_listen = if let Some(n) = direct {
        format!(r#", "listen": {{"address": "127.0.0.1:{n}", "password": "np"}}"#)
    } else {
        String::new()
    };
    let direct_peer = if let Some(n) = direct {
        format!(r#", {{"address": "127.0.0.1:{n}", "password": "np", "role": "node"}}"#)
    } else {
        String::new()
    };
    format!(
        r#"{{"llm": {{
          "hub": {{"id": "hub-0000000000000001", "name": "hub",
            "listen": {{"address": "127.0.0.1:{hub}", "node_password": "np", "proxy_password": "pp"}},
            "limits": {{"retry_interval": "200ms"}}}},
          "node": {{"id": "node-0000000000000001", "name": "gpu"{node_listen},
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "np", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}},
            "models": [
              {{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:{upstream}/v1", "upstream": "up-m1", "api_key": "upstream-key"}},
              {{"name": "m2", "api": "openai", "base_url": "http://127.0.0.1:{upstream}/v1"}},
              {{"name": "off", "api": "openai", "base_url": "http://127.0.0.1:{upstream}/v1", "disabled": true}}
            ]}},
          "proxy": {{"id": "proxy-0000000000000001", "name": "agents",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "pp", "role": "hub"}}{direct_peer}],
            "limits": {{"retry_interval": "200ms"}},
            "api": {{"address": "127.0.0.1:{api}"}},
            "api_keys": [{{"id": "0123456789abcdef", "name": "test", "prefix": "ah-test", "hash": "{}"}}]}}
        }}}}"#,
        key_hash(KEY)
    )
}

fn models_listed(api: u16) -> bool {
    let a = agent(api, "GET", "/v1/models", KEY, b"");
    a.status == 200 && a.text().contains("\"m1\"")
}

#[test]
fn agent_proxy_hub_node_upstream() {
    let up = Upstream::start();
    let (hub, api) = (free_port(), free_port());
    let _roles = Roles::start(&config(hub, api, up.port, None));
    until("the proxy to list the node's models", || models_listed(api));
    let list = agent(api, "GET", "/v1/models", KEY, b"").text();
    assert!(
        list.contains(r#""object":"list""#) && list.contains("\"m2\""),
        "{list}"
    );
    assert!(
        !list.contains("\"off\""),
        "a disabled model is not listed: {list}"
    );

    // A call, the model renamed upstream, the upstream's key sent, the node named.
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m1","messages":[]}"#,
    );
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(a.header("x-akit-node"), Some("node-0000000000000001"));
    assert!(a.text().contains(r#""model":"up-m1""#), "{}", a.text());
    let seen = up.last_post();
    assert_eq!(seen.path, "/v1/chat/completions");
    assert_eq!(seen.header("authorization"), Some("Bearer upstream-key"));
    assert!(String::from_utf8_lossy(&seen.body).contains(r#""model":"up-m1""#));

    // Streamed events come as they are made.
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m2","stream":true}"#,
    );
    assert_eq!(a.status, 200);
    assert_eq!(a.header("content-type"), Some("text/event-stream"));
    assert!(a.text().ends_with("data: [DONE]\n\n"), "{}", a.text());
    assert!(a.chunks >= 2, "{} chunks", a.chunks);

    // A body past 1 MiB streams through the hub to the node, and upstream.
    let big = format!(r#"{{"model":"m2","pad":"{}"}}"#, "x".repeat(3 << 20));
    let a = agent(api, "POST", "/v1/chat/completions", KEY, big.as_bytes());
    assert_eq!(a.status, 200, "{}", a.text());
    assert!(
        a.text().contains(&format!(r#""bytes":{}"#, big.len())),
        "{}",
        a.text()
    );

    // Refusals, in OpenAI's shape.
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        "ah-wrong",
        br#"{"model":"m1"}"#,
    );
    assert_eq!(a.status, 401);
    assert!(a.text().contains("invalid_api_key"));
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"nope"}"#,
    );
    assert_eq!(a.status, 404, "{}", a.text());
    assert!(a.text().contains("model_not_found"));
    let a = agent(api, "POST", "/v1/messages", KEY, br#"{"model":"m1"}"#);
    assert_eq!(a.status, 404, "{}", a.text());
    assert!(
        a.text().contains("served with the openai API"),
        "{}",
        a.text()
    );
    let a = agent(api, "POST", "/v1/chat/completions", KEY, b"{not json");
    assert_eq!(a.status, 400);
    let a = agent(api, "POST", "/v1/nope", KEY, b"{}");
    assert_eq!(a.status, 404);
    assert!(a.text().contains("unknown_url"));
}

#[test]
fn a_dead_upstream_and_a_direct_node() {
    // The node is linked to the proxy directly too: the direct route comes first.
    let up = Upstream::start();
    let (hub, api, node) = (free_port(), free_port(), free_port());
    let _roles = Roles::start(&config(hub, api, up.port, Some(node)));
    until("the proxy to list the node's models", || models_listed(api));
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m2"}"#,
    );
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(a.header("x-akit-node"), Some("node-0000000000000001"));

    // An upstream nobody answers on: unavailable, in the agent's shape.
    let dead = free_port();
    let doc = config(hub, api, dead, Some(node));
    drop(_roles);
    let _roles = Roles::start(&doc);
    until("the proxy to list the node's models", || models_listed(api));
    let a = agent(
        api,
        "POST",
        "/v1/chat/completions",
        KEY,
        br#"{"model":"m2"}"#,
    );
    // The checks found the upstream dead: no ready node, 503 in OpenAI's shape.
    assert_eq!(a.status, 503, "{}", a.text());
    assert!(a.text().contains("server_error"), "{}", a.text());
}

#[test]
fn an_expired_token_closes_its_links() {
    // The hub issues a token expiring in 2 s; the node links with it, and is cut when it
    // expires (the hub's sweep, armed on every apply).
    let sec = akit_roles::linktoken::Secret::new().unwrap();
    let hub = free_port();
    let expires = akit_roles::linktoken::unix_now() + 2;
    let doc = format!(
        r#"{{"llm": {{
          "hub": {{"id": "hub-0000000000000009",
            "listen": {{"address": "127.0.0.1:{hub}"}},
            "tokens": [{{"id": "{}", "name": "n", "hash": "{}", "expires_at": {expires}}}]}},
          "node": {{"id": "node-0000000000000009",
            "peers": [{{"address": "127.0.0.1:{hub}", "token": "{}", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}}}}}}}}"#,
        sec.id(),
        sec.hash(),
        sec.token()
    );
    let roles = Roles::start(&doc);
    let reg = roles.roles.registries();
    let links = || {
        akit_roles::host::token_views(&reg, "llm.hub")
            .and_then(|v| v.first().map(|t| t.links.len()))
            .unwrap_or(0)
    };
    until("the token's link", || links() == 1);
    until("the link closed at the expiry", || links() == 0);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(links(), 0, "an expired token links no more");
}
