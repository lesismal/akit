//! fit's `config/roles_test.go`, `config/llm_test.go`, `config/linktoken_test.go` and
//! `tunnel/config_test.go`, ported (I98). The clashes of two addresses are I100's.

use super::*;
use crate::from_json;

/// A well-formed link token, for the checks of a dialer's token.
const TOK: &str = "akitt1.0123456789abcdef.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn refused(doc: &str, want: &str) {
    match from_json(doc, None) {
        Ok(_) => panic!("{doc:.160}: loaded, want {want:?}"),
        Err(e) => {
            let s = e.to_string();
            assert!(s.contains(want), "{doc:.160}: {s}, want {want:?}");
        }
    }
}

fn loads(doc: &str) -> crate::Config {
    match from_json(doc, None) {
        Ok(c) => c,
        Err(e) => panic!("{doc:.160}: refused: {e}"),
    }
}

#[test]
fn link_token_checks() {
    assert!(check_link_token_string(TOK).is_none());
    // fit's prefix is not akit's (decision 037: no alias).
    assert!(check_link_token_string(&TOK.replace("akitt1.", "fitt1.")).is_some());
    assert!(valid_link_token_id("0123456789abcdef"));
    for bad in ["0123456789ABCDEF", "0123", "0123456789abcdeg"] {
        assert!(!valid_link_token_id(bad), "{bad}");
    }
    let lt = LinkToken {
        expires_at: 10,
        ..LinkToken::default()
    };
    assert!(!lt.expired(9) && lt.expired(10));
    assert!(!LinkToken::default().expired(1 << 40));
    let hash = "ab".repeat(32);
    let many: Vec<String> = (0..=MAX_LINK_TOKENS)
        .map(|_| format!(r#"{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}"}}"#))
        .collect();
    refused(
        &format!(
            r#"{{"tunnel": {{"hub": {{"tokens": [{}]}}}}}}"#,
            many.join(",")
        ),
        "at most 1024 tokens",
    );
    let members: Vec<String> = (0..=MAX_LINK_TOKEN_MEMBERS)
        .map(|i| format!("\"m{}\"", "x".repeat(i)))
        .collect();
    refused(
        &format!(
            r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}", "members": [{}]}}]}}}}}}"#,
            members.join(",")
        ),
        "at most 256 members",
    );
    refused(
        &format!(
            r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}", "expires_at": -1}}]}}}}}}"#
        ),
        "less than the minimum",
    );
    loads(&format!(
        r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "lab", "hash": "{hash}", "members": ["node/lab one"]}}]}},
            "node": {{"hubs": [{{"endpoints": [{{"address": "127.0.0.1:1"}}], "token": "{TOK}"}}]}}}},
          "llm": {{"hub": {{"listen": {{"address": "127.0.0.1:9200"}}, "peers": [{{"address": "127.0.0.1:1", "token": "{TOK}", "role": "node"}}]}}}}}}"#
    ));
}

#[test]
fn llm_checks() {
    let long = |n: usize| "a".repeat(n);
    let model = |m: &str| format!(r#"{{"llm": {{"node": {{"models": [{m}]}}}}}}"#);
    let peers: Vec<String> = (0..=MAX_LLM_PEERS)
        .map(|i| {
            format!(
                r#"{{"address": "127.0.0.1:{}", "password": "p", "role": "hub"}}"#,
                1000 + i
            )
        })
        .collect();
    let keys: Vec<String> = (0..=MAX_LLM_KEYS)
        .map(|_| r#"{"id": "x", "name": "k", "hash": "y"}"#.to_owned())
        .collect();
    let models: Vec<String> = (0..=MAX_LLM_MODELS)
        .map(|i| format!(r#"{{"name": "m{i}", "api": "openai", "base_url": "http://h"}}"#))
        .collect();
    let h64 = long(64);
    for (doc, want) in [
        (r#"{"llm": {"hub": {"id": "a b"}}}"#.to_owned(), "the ID is up to 128 characters"),
        (r#"{"llm": {"hub": {"listen": {"address": "127.0.0.1:9200", "password": "p", "hub_password": "h"}}}}"#.to_owned(), "a hub takes no links from hubs"),
        (r#"{"llm": {"node": {"listen": {"address": "127.0.0.1:9200", "node_password": "n"}}}}"#.to_owned(), "a node takes no links from nodes"),
        (r#"{"llm": {"proxy": {"listen": {"address": "127.0.0.1:9200", "proxy_password": "x"}}}}"#.to_owned(), "a proxy takes no links from proxys"),
        (format!(r#"{{"llm": {{"node": {{"peers": [{}]}}}}}}"#, peers.join(",")), "at most 64 peers"),
        (r#"{"llm": {"hub": {"peers": [{"address": "127.0.0.1:1", "password": "", "role": "node"}]}}}"#.to_owned(), "a peer needs the token"),
        (r#"{"llm": {"hub": {"peers": [{"address": "127.0.0.1:1", "token": "nope", "role": "node"}]}}}"#.to_owned(), "not a link token"),
        (format!(r#"{{"llm": {{"hub": {{"peers": [{{"address": "127.0.0.1:1", "token": "{TOK}", "password": "p", "role": "node"}}]}}}}}}"#), "not both"),
        (r#"{"llm": {"hub": {"tokens": [{"id": "x", "name": "", "hash": "y"}]}}}"#.to_owned(), "the ID is 16 lowercase hex digits"),
        (r#"{"llm": {"node": {"tokens": [{"id": "0123456789abcdef", "name": "", "hash": "y"}]}}}"#.to_owned(), "the name is 1 to 64"),
        (r#"{"llm": {"proxy": {"tokens": [{"id": "0123456789abcdef", "name": "a", "hash": "y"}]}}}"#.to_owned(), "the hash is 64 hex digits"),
        (r#"{"llm": {"hub": {"limits": {"ping_interval": "10ms"}}}}"#.to_owned(), "ping_interval"),
        (r#"{"llm": {"hub": {"limits": {"slow_timeout": "-1s"}}}}"#.to_owned(), "slow_timeout"),
        (r#"{"llm": {"hub": {"limits": {"stream_idle": "10ms"}}}}"#.to_owned(), "stream_idle"),
        (model(r#"{"name": "a", "api": "openai", "base_url": "http://h", "upstream": "a b"}"#), "upstream model ID"),
        (model(r#"{"name": "a", "api": "openai", "base_url": "http://h", "api_key": "a b"}"#), "API key"),
        (model(&format!(r#"{{"name": "a", "api": "openai", "base_url": "http://h", "description": "{}"}}"#, long(1025))), "description is longer"),
        (format!(r#"{{"llm": {{"node": {{"description": "{}"}}}}}}"#, long(1025)), "description is longer"),
        (format!(r#"{{"llm": {{"node": {{"models": [{}]}}}}}}"#, models.join(",")), "at most 256 models"),
        (r#"{"llm": {"node": {"settings": {"job_timeout": "1s"}}}}"#.to_owned(), "job_timeout is 10s to 168h"),
        (r#"{"llm": {"proxy": {"api": {"address": "nope"}}}}"#.to_owned(), "nope"),
        (r#"{"llm": {"proxy": {"api": {"address": "127.0.0.1:9300", "cert_file": "c"}}}}"#.to_owned(), "cert_file and key_file go together"),
        (format!(r#"{{"llm": {{"proxy": {{"api_keys": [{}]}}}}}}"#, keys.join(",")), "at most 256 keys"),
        (format!(r#"{{"llm": {{"proxy": {{"api_keys": [{{"id": "0123456789abcdef", "name": "k", "hash": "{h64}"}}, {{"id": "0123456789abcdef", "name": "l", "hash": "{h64}"}}]}}}}}}"#), "listed twice"),
        (format!(r#"{{"llm": {{"proxy": {{"api_keys": [{{"id": "0123456789abcdef", "name": " ", "hash": "{h64}"}}]}}}}}}"#), "the name needs 1 to 64"),
        (format!(r#"{{"llm": {{"proxy": {{"api_keys": [{{"id": "0123456789abcdef", "name": "k", "prefix": "{}", "hash": "{h64}"}}]}}}}}}"#, long(17)), "the prefix is up to 16"),
    ] {
        refused(&doc, want);
    }
}

#[test]
fn llm_model_checks_beyond_decoding() {
    let m = |api: &str, version: &str| LlmModel {
        name: "a".into(),
        api: api.into(),
        base_url: "http://h".into(),
        api_version: version.into(),
        ..LlmModel::default()
    };
    let mut over = m("openai", "");
    over.max_inflight = 100_001;
    assert_eq!(check_llm_model(&over).map(|(f, _)| f), Some("max_inflight"));
    assert_eq!(check_llm_model(&m("palm", "")).map(|(f, _)| f), Some("api"));
    for api in LLM_APIS {
        let v = if *api == "azure_openai" {
            "2024-10-21"
        } else {
            ""
        };
        assert!(check_llm_model(&m(api, v)).is_none(), "{api}");
    }
}

#[test]
fn llm_defaults() {
    for l in [None, Some(&LlmLimits::default())] {
        assert_eq!(LlmLimits::conns(l), DEFAULT_LLM_MAX_CONNS);
        assert_eq!(LlmLimits::inflight(l), DEFAULT_LLM_MAX_INFLIGHT);
        assert_eq!(LlmLimits::ping(l), DEFAULT_LLM_PING_INTERVAL);
        assert_eq!(LlmLimits::slow(l), DEFAULT_LLM_SLOW_TIMEOUT);
        assert_eq!(LlmLimits::retry(l), DEFAULT_LLM_RETRY_INTERVAL);
        assert_eq!(LlmLimits::idle(l), DEFAULT_LLM_STREAM_IDLE);
    }
    let l = LlmLimits {
        max_conns: 3,
        max_inflight: 4,
        ping_interval: Duration::from_secs(1),
        ..LlmLimits::default()
    };
    assert_eq!(
        (
            LlmLimits::conns(Some(&l)),
            LlmLimits::inflight(Some(&l)),
            LlmLimits::ping(Some(&l))
        ),
        (3, 4, Duration::from_secs(1))
    );
    let a = LlmProxyApi::default();
    assert_eq!(
        (a.conns(), a.inflight()),
        (DEFAULT_LLM_PROXY_MAX_CONNS, DEFAULT_LLM_PROXY_MAX_INFLIGHT)
    );
    for (role, want) in [
        ("hub", "node proxy"),
        ("node", "hub proxy"),
        ("proxy", "hub node"),
        ("x", ""),
    ] {
        assert_eq!(llm_peer_roles(role).join(" "), want);
    }
}

#[test]
fn llm_members() {
    let hash = "$2a$10$abcdefghijklmnopqrstuuK3o5D2wC6Qn7oW7v3yTzR3z0e6Gm9Ka";
    let good = format!(
        r#"{{"llm": {{
        "hub": {{"listen": {{"address": ":9200", "password": "p", "self_signed": true}}, "max_nodes": 10}},
        "node": {{"id": "node-1", "name": "gpu", "peers": [{{"address": "10.0.0.1:9200", "password": "p", "role": "hub", "pin": "{}AB"}},
                {{"address": "hub.example.net:9200", "password": "p", "role": "proxy", "ca_file": "/etc/akit/ca.pem", "server_name": "hub.example.net"}}],
            "models": [{{"name": "m", "api": "openai", "base_url": "http://127.0.0.1:11434/v1", "max_inflight": 4}},
                {{"name": "m", "api": "anthropic", "base_url": "https://api.example.com/v1"}},
                {{"name": "m", "api": "gemini", "base_url": "https://generativelanguage.googleapis.com/v1beta"}},
                {{"name": "m", "api": "ollama", "base_url": "http://127.0.0.1:11434"}},
                {{"name": "m", "api": "cohere", "base_url": "https://api.cohere.com/v2"}},
                {{"name": "z", "api": "azure_openai", "base_url": "https://r.openai.azure.com", "api_version": "2024-10-21"}}],
            "settings": {{"allow_hub_manage": true, "gpu_sudo": false, "refresh_interval": "10s", "job_timeout": "2h"}},
            "proxy": {{"timeout": "10m", "max_inflight": 8}}}},
        "proxy": {{"listen": {{"address": ":9201", "hub_password": "h", "node_password": "n", "cert_file": "/etc/akit/p.pem", "key_file": "/etc/akit/p.key"}},
            "peers": [{{"address": "127.0.0.1:9200", "password": "p", "role": "hub"}}, {{"address": "localhost:9202", "password": "p", "role": "node"}}], "api": {{"address": "127.0.0.1:9300"}}, "console": {{"address": "127.0.0.1:9301", "users": [{{"username": "a", "password_hash": "{hash}"}}]}},
            "api_keys": [{{"id": "0123456789abcdef", "name": "k", "prefix": "ah-xxxxxxx", "hash": "{}"}}]}}}}}}"#,
        "AB:".repeat(31),
        "a".repeat(64)
    );
    let c = loads(&good);
    let llm = c.llm.unwrap();
    let node = llm.node.unwrap();
    assert!(!LlmNodeSettings::sudo(node.settings.as_ref()));
    assert_eq!(
        LlmNodeSettings::job(node.settings.as_ref()),
        Duration::from_secs(7200)
    );
    assert_eq!(LlmNodeProxy::inflight(node.proxy.as_ref()), 8);
    assert_eq!(
        LlmNodeProxy::idle(node.proxy.as_ref()),
        DEFAULT_LLM_IDLE_TIMEOUT
    );
    assert_eq!(llm.hub.unwrap().nodes(), 10);
    let proxy = llm.proxy.unwrap();
    assert_eq!(proxy.api.unwrap().body(), DEFAULT_LLM_MAX_BODY_BYTES);
    let listen = proxy.role.listen.unwrap();
    assert_eq!(
        (
            listen.hub_password.as_str(),
            listen.node_password.as_str(),
            listen.password.as_str()
        ),
        ("h", "n", "")
    );
    let pin = "a".repeat(64);
    for (doc, want) in [
        (r#"{"llm": {"hub": {"peers": [{"address": "127.0.0.1:1", "password": "p", "role": "hub"}]}}}"#.to_owned(), "links with node and proxy"),
        (r#"{"llm": {"hub": {"peers": [{"address": ":1", "password": "p", "role": "node"}]}}}"#.to_owned(), "not a host and port"),
        (r#"{"llm": {"hub": {"peers": [{"address": "127.0.0.1:1", "password": "p", "role": "node"}, {"address": "127.0.0.1:1", "password": "q", "role": "node"}]}}}"#.to_owned(), "listed twice"),
        (r#"{"llm": {"node": {"models": [{"name": "a b", "api": "openai", "base_url": "http://h"}]}}}"#.to_owned(), "without spaces"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "palm", "base_url": "http://h"}]}}}"#.to_owned(), "palm"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "azure_openai", "base_url": "https://r.openai.azure.com"}]}}}"#.to_owned(), "needs its api_version"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "openai", "base_url": "http://h", "api_version": "2024-10-21"}]}}}"#.to_owned(), "only for azure_openai"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "azure_openai", "base_url": "http://h", "api_version": "2024 10"}]}}}"#.to_owned(), "api_version is up to 64"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "openai", "base_url": "http://h"}, {"name": "a", "api": "azure_openai", "base_url": "http://i", "api_version": "v"}]}}}"#.to_owned(), "listed twice for agents' openai API"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "openai", "base_url": "ftp://h"}]}}}"#.to_owned(), "base_url"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "openai", "base_url": "http://h/"}]}}}"#.to_owned(), "trailing slash"),
        (r#"{"llm": {"node": {"models": [{"name": "a", "api": "openai", "base_url": "http://h"}, {"name": "a", "api": "openai", "base_url": "http://i"}]}}}"#.to_owned(), "listed twice"),
        (r#"{"llm": {"node": {"settings": {"refresh_interval": "1s"}}}}"#.to_owned(), "refresh_interval is 5s to 1h"),
        (r#"{"llm": {"node": {"listen": {"address": "nope", "password": "p"}}}}"#.to_owned(), "not a host and port"),
        (r#"{"llm": {"proxy": {"api": {"address": "0.0.0.0:9300"}}}}"#.to_owned(), "not a loopback address"),
        (r#"{"llm": {"proxy": {"api_keys": [{"id": "x", "name": "k", "hash": "y"}]}}}"#.to_owned(), "16 hex digits"),
        (r#"{"llm": {"proxy": {"api_keys": [{"id": "0123456789abcdef", "name": "k", "hash": "y"}]}}}"#.to_owned(), "64 hex digits"),
        (r#"{"llm": {"node": {"name": " x"}}}"#.to_owned(), "spaces around it"),
        // Off loopback, links go over TLS, the listener's certificate checked.
        (r#"{"llm": {"hub": {"listen": {"address": "0.0.0.0:9200", "password": "p"}}}}"#.to_owned(), "set cert_file and key_file, or self_signed"),
        (r#"{"llm": {"hub": {"listen": {"address": ":9200", "password": "p"}}}}"#.to_owned(), "is not a loopback address"),
        (r#"{"llm": {"node": {"peers": [{"address": "10.0.0.1:9200", "password": "p", "role": "hub"}]}}}"#.to_owned(), "set pin to the fingerprint"),
        (r#"{"llm": {"node": {"peers": [{"address": "10.0.0.1:9200", "password": "p", "role": "hub", "pin": "abcd"}]}}}"#.to_owned(), "bad certificate pin"),
        (format!(r#"{{"llm": {{"node": {{"peers": [{{"address": "10.0.0.1:9200", "password": "p", "role": "hub", "pin": "{pin}", "ca_file": "c"}}]}}}}}}"#), "pin or ca_file, one of the two"),
        (r#"{"llm": {"node": {"peers": [{"address": "127.0.0.1:9200", "password": "p", "role": "hub", "server_name": "h"}]}}}"#.to_owned(), "server_name goes with ca_file"),
        (r#"{"llm": {"hub": {"listen": {"address": ":9200", "password": "p", "cert_file": "c"}}}}"#.to_owned(), "cert_file and key_file go together"),
        (r#"{"llm": {"hub": {"listen": {"address": ":9200", "password": "p", "cert_file": "c", "key_file": "k", "self_signed": true}}}}"#.to_owned(), "one of the two"),
        (r#"{"llm": {"hub": {"listen": {"address": ":9200", "password": "p", "self_signed": true, "cert": "x"}}}}"#.to_owned(), "cert and key go together"),
        (r#"{"llm": {"hub": {"listen": {"address": ":9200", "password": "p", "self_signed": true, "cert": "x", "key": "y"}}}}"#.to_owned(), "do not load"),
    ] {
        refused(&doc, want);
    }
}

#[test]
fn tunnel_checks() {
    let up = |u: &str| format!(r#"{{"tunnel": {{"node": {{"hubs": [{u}]}}}}}}"#);
    let hash = "ab".repeat(32);
    for (doc, want) in [
        (r#"{"tunnel": {"hub": {"name": " s"}}}"#.to_owned(), "is not a name"),
        (r#"{"tunnel": {"hub": {"listeners": [{"protocol": "tcp", "address": "nope"}]}}}"#.to_owned(), "nope"),
        (r#"{"tunnel": {"hub": {"listeners": [{"protocol": "ws", "address": "127.0.0.1:7000", "path": "x"}]}}}"#.to_owned(), "does not start with '/'"),
        (r#"{"tunnel": {"hub": {"password": "p", "console_socket": {"address": "nope"}}}}"#.to_owned(), "nope"),
        (r#"{"tunnel": {"hub": {"console_socket": {"address": ":7001"}}}}"#.to_owned(), "a password is needed"),
        (r#"{"tunnel": {"node": {"name": "a/b"}}}"#.to_owned(), "is not a name"),
        (r#"{"tunnel": {"node": {"services": [{"name": "", "address": "h:22"}]}}}"#.to_owned(), "the name is empty"),
        (r#"{"tunnel": {"node": {"services": [{"name": "a/b", "address": "h:22"}]}}}"#.to_owned(), "is not a name"),
        (r#"{"tunnel": {"proxy": {"name": " c"}}}"#.to_owned(), "is not a name"),
        (up(r#"{"name": " s", "endpoints": [{"address": "127.0.0.1:1"}], "password": "p"}"#), "is not a name"),
        (up(r#"{"name": "s", "endpoints": [], "password": "p"}"#), "list at least one endpoint"),
        (up(r#"{"endpoints": [{"address": "127.0.0.1:1"}, {"address": "127.0.0.1:1"}], "password": "p"}"#), "listed twice"),
        (up(r#"{"endpoints": [{"address": "127.0.0.1:1"}], "password": ""}"#), "set the token the hub issued"),
        (up(r#"{"endpoints": [{"address": "127.0.0.1:1"}], "token": "akitt1.x"}"#), "not a link token"),
        (up(&format!(r#"{{"endpoints": [{{"address": "127.0.0.1:1"}}], "token": "{TOK}", "password": "p"}}"#)), "not both"),
        (format!(r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}", "note": "{}"}}]}}}}}}"#, "n".repeat(1025)), "the note is up to 1024"),
        (format!(r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}"}}, {{"id": "0123456789abcdef", "name": "b", "hash": "{hash}"}}]}}}}}}"#), "another token has the ID"),
        (format!(r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}", "members": ["node/a"]}}, {{"id": "0123456789abcde0", "name": "b", "hash": "{hash}", "members": ["node/a", ""]}}]}}}}}}"#), "bound to token 0123456789abcdef"),
        (format!(r#"{{"tunnel": {{"hub": {{"tokens": [{{"id": "0123456789abcdef", "name": "a", "hash": "{hash}", "members": ["\u0001"]}}]}}}}}}"#), "a member is 1 to 256 bytes"),
        (up(r#"{"endpoints": [{"address": "127.0.0.1:0"}], "password": "p"}"#), "not a host and port to dial"),
        (up(r#"{"endpoints": [{"address": "ws://:1/x"}], "password": "p"}"#), "not a ws:// URL"),
        (up(r#"{"endpoints": [{"address": "quic://h"}], "password": "p"}"#), "not a quic:// URL"),
        (up(r#"{"endpoints": [{"address": "quic://h:1/x"}], "password": "p"}"#), "has a path"),
        (r#"{"tunnel": {"proxy": {"forwards": [{"listen": "nope", "node": "n", "service": "s"}]}}}"#.to_owned(), "nope"),
        (r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:0", "node": "", "service": "s"}]}}}"#.to_owned(), "the node is empty"),
        (r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:0", "node": "n", "service": ""}]}}}"#.to_owned(), "the service is empty"),
        (r#"{"tunnel": {"proxy": {"web_proxies": [{"listen": "127.0.0.1:0", "url": "ws://h:1/", "password": ""}]}}}"#.to_owned(), "the password is empty"),
        (r#"{"tunnel": {"hub": {"console": {"address": "nope", "users": []}}}}"#.to_owned(), "nope"),
        (r#"{"tunnel": {"hub": {"console": {"users": [{"username": "a", "password_hash": "$2a$10$x"}]}}}}"#.to_owned(), "tunnel.hub.console.address: missing required field"),
    ] {
        refused(&doc, want);
    }
    // Two forwards on port 0 each pick a free port: not the same listener.
    loads(
        r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:0", "node": "n", "service": "s"},
        {"listen": "127.0.0.1:0", "node": "n", "service": "s"}]}}}"#,
    );
}

#[test]
fn tunnel_names() {
    let want = host_name();
    assert!(!want.is_empty());
    assert_eq!(TunnelHub::default().hub_name(), want);
    assert_eq!(TunnelNode::default().node_name(), want);
    assert_eq!(TunnelProxy::default().proxy_name(), want);
    assert_eq!(TunnelUplink::default().hub_name(), "");
    let l = TunnelListener {
        protocol: TunnelProtocol::Ws,
        path: "/x".into(),
        ..TunnelListener::default()
    };
    assert_eq!(l.ws_path(), "/x");
}

/// fit's `tunnel/config_test.go`: the members load as strictly as the rest of the config, and are
/// checked beyond decoding.
#[test]
fn tunnel_config() {
    let pin = "ab".repeat(32);
    let cd = "cd".repeat(32);
    let up = format!(
        r#"{{"endpoints": [{{"address": "wss://hub.example/tunnel"}}, {{"address": "tls://hub.example:7000"}}, {{"address": "quic://hub.example:7443"}}], "password": "pw", "pin": "{pin}"}}"#
    );
    let bare = |address: &str, extra: &str| {
        format!(
            r#"{{"tunnel": {{"node": {{"hubs": [{{"endpoints": [{{"address": "{address}"}}], "password": "pw"{extra}}}]}}}}}}"#
        )
    };
    let listener = |protocol: &str, address: &str, extra: &str| {
        format!(
            r#"{{"tunnel": {{"hub": {{"password": "pw"{extra}, "listeners": [{{"protocol": "{protocol}", "address": "{address}"}}]}}}}}}"#
        )
    };
    let with_pin = format!(r#", "pin": "{pin}""#);
    let colons = format!(r#", "pin": "{}""#, "AB:".repeat(32).trim_end_matches(':'));
    let cases: Vec<(&str, String, &str)> = vec![
        ("a hub", r#"{"tunnel": {"hub": {"password": "pw", "listeners": [{"protocol": "tls", "address": ":7000"}, {"protocol": "wss", "address": ":7001", "path": "/t"}, {"protocol": "quic", "address": ":7000"}]}}}"#.into(), ""),
        ("a hub in the clear with body_password", r#"{"tunnel": {"hub": {"password": "pw", "body_password": "b", "listeners": [{"protocol": "tcp", "address": ":7000"}, {"protocol": "ws", "address": ":7001"}]}}}"#.into(), ""),
        ("a listener's own certificate", r#"{"tunnel": {"hub": {"listeners": [{"protocol": "wss", "address": ":7001", "cert_file": "c.pem", "key_file": "k.pem"}]}}}"#.into(), ""),
        ("tcp on every address", listener("tcp", ":7000", ""), "a tcp listener on \":7000\", which is not a loopback address"),
        ("ws on a public address", listener("ws", "0.0.0.0:7001", ""), "use tls or wss"),
        ("tcp on IPv6's every address", listener("tcp", "[::]:7000", ""), "or set the hub's body_password"),
        ("tcp on the loopback", listener("tcp", "127.0.0.1:7000", ""), ""),
        ("ws on the loopback", listener("ws", "127.8.0.1:7000", ""), ""),
        ("tcp on IPv6's loopback", listener("tcp", "[::1]:7000", ""), ""),
        ("tcp on localhost", listener("tcp", "localhost:7000", ""), ""),
        ("tcp with body_password", listener("tcp", ":7000", r#", "body_password": "b""#), ""),
        ("a tcp endpoint", bare("hub.example:7000", ""), "carries the links in the clear and unauthenticated: use tls:// or wss:// with pin"),
        ("a tcp:// endpoint", bare("tcp://10.0.0.1:7000", ""), "use tls:// or wss://"),
        ("a ws endpoint", bare("ws://hub.example/tunnel", ""), "or set body_password"),
        ("a disabled bare endpoint", r#"{"tunnel": {"node": {"hubs": [{"endpoints": [{"address": "h:1", "disabled": true}, {"address": "tls://h:2"}], "password": "pw"}]}}}"#.into(), "\"h:1\", whose host is not a loopback one"),
        ("a tcp endpoint on the loopback", bare("127.0.0.1:7000", ""), ""),
        ("a ws endpoint on localhost", bare("ws://localhost:7001/tunnel", ""), ""),
        ("a tcp endpoint on IPv6's loopback", bare("tcp://[::1]:7000", ""), ""),
        ("a tcp endpoint with body_password", bare("hub.example:7000", r#", "body_password": "b""#), ""),
        ("a tls endpoint", bare("tls://hub.example:7000", ""), ""),
        ("a tls endpoint with a path", bare("tls://hub.example:7000/x", ""), "has a path"),
        ("a tls endpoint without a port", bare("tls://hub.example", ""), "not a tls:// URL"),
        ("a pin in the address", bare("wss://hub.example/t?pin=ab", ""), "has a query"),
        ("a pin in a tls fragment", bare(&format!("tls://hub.example:7000#?pin={cd}&"), &with_pin), "has a query, a fragment or a user"),
        ("insecure in a quic fragment", bare("quic://hub.example:7000#?insecure=1&", &with_pin), "a fragment"),
        ("a CA file in a wss fragment", bare("wss://hub.example/tunnel#?ca=/tmp/evil.pem&", &with_pin), "a fragment"),
        ("an empty query in a fragment", bare("tls://hub.example:443#?x=", &with_pin), "a fragment"),
        ("a bare fragment", bare("tls://hub.example:443#", &with_pin), "a fragment"),
        ("an empty query", bare("tls://hub.example:443?", &with_pin), "a query"),
        ("a user", bare("tls://u:p@hub.example:443", &with_pin), "a user"),
        ("a fragment on tcp", bare("127.0.0.1:7000#x", ""), "a fragment"),
        ("a tcp port that is not one", bare("127.0.0.1:x", ""), "not a host and port"),
        ("a tcp endpoint with a path", bare("tcp://127.0.0.1:7000/x", ""), "not a host and port"),
        ("quic_insecure off the loopback", bare("quic://hub.example:7443", r#", "quic_insecure": true"#), "quic_insecure takes any certificate"),
        ("quic_insecure on the loopback", bare("quic://127.0.0.1:7443", r#", "quic_insecure": true"#), ""),
        ("quic_insecure on localhost", bare("quic://localhost:7443", r#", "quic_insecure": true"#), ""),
        ("quic_insecure with body_password", bare("quic://hub.example:7443", r#", "quic_insecure": true, "body_password": "b""#), ""),
        ("quic_insecure under a pin", bare("quic://hub.example:7443", &format!(r#", "quic_insecure": true, "pin": "{pin}""#)), ""),
        ("quic_insecure beside tls", bare("tls://hub.example:7443", r#", "quic_insecure": true"#), ""),
        ("quic_insecure with a bad quic endpoint", bare("quic://hub.example", r#", "quic_insecure": true"#), "not a quic:// URL"),
        ("quic_insecure past a loopback endpoint", r#"{"tunnel": {"node": {"hubs": [{"endpoints": [{"address": "quic://127.0.0.1:1"}, {"address": "quic://[::1]:2"}, {"address": "quic://hub.example:3", "disabled": true}], "password": "pw", "quic_insecure": true}]}}}"#.into(), "\"quic://hub.example:3\", whose host is not a loopback one"),
        ("a bad pin", bare("tls://hub.example:7000", r#", "pin": "abcd""#), "not a certificate pin"),
        ("a pin with colons", bare("tls://hub.example:7000", &colons), ""),
        ("pin and ca_file", bare("tls://hub.example:7000", &format!(r#", "pin": "{pin}", "ca_file": "ca.pem""#)), "set one"),
        ("quic_pin, pin's old name", bare("quic://hub.example:7443", &format!(r#", "quic_pin": "{pin}""#)), ""),
        ("quic_pin and pin", bare("quic://hub.example:7443", &format!(r#", "quic_pin": "{pin}", "pin": "{pin}""#)), "old name"),
        ("a certificate on tcp", r#"{"tunnel": {"hub": {"listeners": [{"protocol": "tcp", "address": "127.0.0.1:7000", "cert_file": "c", "key_file": "k"}]}}}"#.into(), "only a tls, wss or quic listener"),
        ("half a listener's certificate", r#"{"tunnel": {"hub": {"listeners": [{"protocol": "tls", "address": ":7000", "cert_file": "c"}]}}}"#.into(), "cert_file and key_file go together"),
        ("a node", format!(r#"{{"tunnel": {{"node": {{"hubs": [{up}], "services": [{{"name": "ssh", "address": "10.0.0.5:22"}}]}}}}}}"#), ""),
        ("a proxy", format!(r#"{{"tunnel": {{"proxy": {{"hubs": [{up}], "forwards": [{{"listen": "127.0.0.1:2222", "node": "lab", "service": "ssh", "hub": "wss://hub.example/tunnel"}}]}}}}}}"#), ""),
        ("camelCase", r#"{"tunnel": {"hub": {"linkWsAddr": ":7001"}}}"#.into(), "unknown field"),
        ("an unknown protocol", r#"{"tunnel": {"hub": {"listeners": [{"protocol": "udp", "address": ":7000"}]}}}"#.into(), "listeners[0].protocol"),
        ("a path on tcp", r#"{"tunnel": {"hub": {"listeners": [{"protocol": "tcp", "address": "127.0.0.1:7000", "path": "/x"}]}}}"#.into(), "only a ws or wss listener"),
        ("a socket without a console", r#"{"tunnel": {"hub": {"password": "pw", "console_socket": {"address": ":7002"}}}}"#.into(), "lacks"),
        ("half a certificate", r#"{"tunnel": {"hub": {"quic": {"cert": "x"}}}}"#.into(), "cert and key go together"),
        ("a bad certificate", r#"{"tunnel": {"hub": {"quic": {"cert": "x", "key": "y"}}}}"#.into(), "do not load"),
        ("two services of a name", r#"{"tunnel": {"node": {"services": [{"name": "a", "address": "h:1"}, {"name": "a", "address": "h:2"}]}}}"#.into(), "already named"),
        ("a service without a port", r#"{"tunnel": {"node": {"services": [{"name": "a", "address": "nowhere"}]}}}"#.into(), "services[0].address"),
        ("a hub without a token or password", r#"{"tunnel": {"node": {"hubs": [{"endpoints": [{"address": "127.0.0.1:1"}]}]}}}"#.into(), "set the token the hub issued"),
        ("a bad endpoint", r#"{"tunnel": {"node": {"hubs": [{"endpoints": [{"address": "http://h:1"}], "password": "x"}]}}}"#.into(), "the scheme http"),
        ("every endpoint off", r#"{"tunnel": {"node": {"hubs": [{"endpoints": [{"address": "127.0.0.1:1", "disabled": true}], "password": "x"}]}}}"#.into(), "every endpoint is disabled"),
        ("two hubs of a name", format!(r#"{{"tunnel": {{"node": {{"hubs": [{up}, {up}]}}}}}}"#), "already named"),
        ("a pool too large", r#"{"tunnel": {"node": {"hubs": [{"endpoints": [{"address": "127.0.0.1:1"}], "password": "x", "pool_size": 101}]}}}"#.into(), "pool_size"),
        ("a forward to no hub", r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:1", "node": "n", "service": "s", "hub": "nope"}]}}}"#.into(), "no hub named"),
        ("a web proxy to http", r#"{"tunnel": {"proxy": {"web_proxies": [{"listen": "127.0.0.1:1", "url": "http://h/", "password": "p"}]}}}"#.into(), "not a ws:// URL"),
        ("a bad duration", r#"{"tunnel": {"proxy": {"open_timeout": "soon"}}}"#.into(), "not a duration"),
        ("a console off the loopback without TLS", r#"{"tunnel": {"proxy": {"console": {"address": "0.0.0.0:7280", "users": [{"username": "a", "password_hash": "$2a$10$x"}]}}}}"#.into(), "not a loopback address"),
    ];
    for (name, doc, want) in cases {
        match (from_json(&doc, None), want) {
            (Ok(_), "") => {}
            (Err(e), "") => panic!("{name}: refused: {e}"),
            (Ok(_), w) => panic!("{name}: loaded, want {w:?}"),
            (Err(e), w) => assert!(e.to_string().contains(w), "{name}: {e}, want {w:?}"),
        }
    }
}

#[test]
fn quic_pin_is_read_with_a_warning() {
    let pin = "ab".repeat(32);
    let c = loads(&format!(
        r#"{{"tunnel": {{"node": {{"hubs": [{{"endpoints": [{{"address": "quic://hub.example:7443"}}], "token": "{TOK}", "quic_pin": "{pin}"}}]}}}}}}"#
    ));
    assert_eq!(c.tunnel.unwrap().node.unwrap().hubs[0].pin, pin);
    assert!(
        c.warnings
            .0
            .iter()
            .any(|w| w.msg.contains("renamed \"pin\""))
    );
}

#[test]
fn endpoints_are_taken_apart() {
    let e = parse_endpoint("wss://hub.example/tunnel").unwrap();
    assert_eq!(
        (e.protocol, e.host_port(), e.path.as_str()),
        (TunnelProtocol::Wss, "hub.example:443".to_owned(), "/tunnel")
    );
    let e = parse_endpoint("tcp://[::1]:7000").unwrap();
    assert_eq!(
        (e.protocol, e.host_port()),
        (TunnelProtocol::Tcp, "[::1]:7000".to_owned())
    );
    let e = parse_endpoint("hub:7000").unwrap();
    assert_eq!(e.protocol, TunnelProtocol::Tcp);
    assert!(parse_endpoint("tls://hub.example:7000#?pin=x").is_none());
    assert_eq!(parse_pin(&"AB:".repeat(32)[..95]), Some([0xab; 32]));
    assert!(parse_pin("ab").is_none());
}
