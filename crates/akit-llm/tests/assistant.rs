//! The console's assistant's `llm_proxy` provider (I161) against akit's own LLM roles: the
//! assistant's model client calls this akit's `llm.proxy`, which relays through a hub to a node,
//! which forwards to a stand-in of an OpenAI-compatible API (never a real one); the model list
//! comes from the mesh, and a tool call round-trips through all three.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::sync::{Arc, mpsc};
use std::time::Duration;

use akit_assistant::model::{self, Client, Event, Message, Provider, Request, Role, Secret, Tool};
use akit_config::assistant::ProviderKind;
use support::{Ports, Roles, agent, key_hash, until};

const KEY: &str = "ah-test-key-0123456789";
const UPSTREAM_KEY: &str = "sk-upstream-0123456789";

fn config(p: &Ports, upstream: &str) -> String {
    let hub = p.hub;
    format!(
        r#"{{"llm": {{
          "hub": {{"id": "hub-0000000000000001", "name": "hub",
            "listen": {{"address": "127.0.0.1:0", "node_password": "np", "proxy_password": "pp"}},
            "limits": {{"retry_interval": "200ms"}}}},
          "node": {{"id": "node-0000000000000001", "name": "gpu",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "np", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}},
            "models": [
              {{"name": "m1", "api": "openai", "base_url": "{upstream}/v1", "upstream": "gpt-test", "api_key": "{UPSTREAM_KEY}"}}
            ]}},
          "proxy": {{"id": "proxy-0000000000000001", "name": "agents",
            "peers": [{{"address": "127.0.0.1:{hub}", "password": "pp", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}},
            "api": {{"address": "127.0.0.1:0"}},
            "api_keys": [{{"id": "0123456789abcdef", "name": "test", "prefix": "ah-test", "hash": "{}"}}]}}
        }}}}"#,
        key_hash(KEY)
    )
}

#[test]
fn llm_proxy_provider() {
    let up = akit_assistant::standin::fake_api(UPSTREAM_KEY);
    let (roles, p) = Roles::start_with(|p| config(p, &up.url()));
    let api = p.api;
    until("the proxy to list the node's models", || {
        let a = agent(api, "GET", "/v1/models", KEY, b"");
        a.status == 200 && a.text().contains("\"m1\"")
    });
    // Where the assistant finds the proxy's API: its port as bound.
    let bound = roles.local("llm.proxy").api_addr().unwrap();
    assert_eq!(bound.port(), api);
    let c = Client::new(Arc::clone(&roles.engine), model::Limits::default(), None);
    let mut prov = Provider::new(
        "mesh",
        ProviderKind::LlmProxy,
        &format!("http://127.0.0.1:{}/v1", bound.port()),
    );
    prov.key = Secret::new(KEY);
    let (tx, rx) = mpsc::channel();
    c.models(&prov, move |r| {
        let _ = tx.send(r);
    });
    let models = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert!(models.iter().any(|m| m.id == "m1"), "{models:?}");

    let mut req = Request {
        model: "m1".into(),
        system: "You are akit's assistant.".into(),
        tools: vec![Tool {
            name: "get_status".into(),
            description: "A listener's status.".into(),
            parameters: r#"{"type":"object","properties":{"listener":{"type":"string"}}}"#.into(),
        }],
        messages: vec![Message::new(Role::User, "Is the listener web up?")],
        ..Request::default()
    };
    let call = |prov: &Provider, req: &Request| {
        let (tx, rx) = mpsc::channel();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        c.stream(
            prov,
            req,
            0,
            move |e| ev.lock().unwrap().push(e),
            move |r| {
                let _ = tx.send(r);
            },
        );
        let r = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        (r, std::mem::take(&mut *events.lock().unwrap()))
    };
    let (r, events) = call(&prov, &req);
    let a = r.unwrap_or_else(|e| panic!("turn 1: {e}"));
    assert_eq!(a.tool_calls.len(), 1, "{a:?}");
    assert!(matches!(events.last(), Some(Event::ToolCall(_))));
    let tc = a.tool_calls[0].clone();
    req.messages.push(a.message());
    req.messages
        .push(Message::tool(&tc, r#"{"listener":"web","up":true}"#, false));
    let (r, _) = call(&prov, &req);
    let a = r.unwrap_or_else(|e| panic!("turn 2: {e}"));
    assert_eq!(a.text, "The listener web is up.");
    // The upstream got the node's key, never the proxy's.
    for q in up.requests() {
        assert_eq!(
            q.header("Authorization"),
            Some(format!("Bearer {UPSTREAM_KEY}").as_str())
        );
        assert!(!String::from_utf8_lossy(&q.body).contains(KEY));
    }
    // A wrong key is refused by the proxy, and not said back.
    prov.key = Secret::new("ah-wrong-0123456789");
    let (r, _) = call(&prov, &req);
    let e = r.unwrap_err();
    assert!(e.to_string().contains("HTTP 401"), "{e}");
    assert!(!e.to_string().contains("ah-wrong-0123456789"), "{e}");
}
