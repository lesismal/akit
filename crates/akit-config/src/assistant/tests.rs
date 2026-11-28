//! fit's `config/assistant_test.go`, ported.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::{Config, from_json};

const CONSOLE: &str = r#""console": {"users": [{"username": "ada", "password_hash": "$2a$10$abcdefghijklmnopqrstuuG2rmOVKzOv2Hd2t1Gc9i1dK1Szy/7Ri"}]}"#;

fn load_config(assistant: &str) -> Result<Config, Errors> {
    from_json(
        &format!(r#"{{"admin": {{{CONSOLE}, "assistant": {assistant}}}}}"#),
        None,
    )
}

fn assistant_of(c: &Config) -> &Assistant {
    c.admin.as_ref().unwrap().assistant.as_ref().unwrap()
}

#[test]
fn loads() {
    let c = load_config(
        r#"{
  "providers": [
    {"name": "claude", "kind": "anthropic", "models": [{"name": "claude-x", "context": 200000}]},
    {"name": "local", "kind": "openai_compatible", "base_url": "http://127.0.0.1:8000/v1/", "headers": {"X-Org": "akit"},
     "connect_timeout": "1s", "first_byte_timeout": "2s", "idle_timeout": "3s", "timeout": "4s", "retries": 0,
     "models": [{"name": "m", "tools": false}]},
    {"name": "az", "kind": "azure_openai", "base_url": "https://x.openai.azure.com", "api_version": "2024-10-21", "api_key_env": "AZ_KEY"},
    {"name": "mesh", "kind": "llm_proxy"}
  ],
  "default_model": {"provider": "local", "model": "m"},
  "params": {"max_output_tokens": 100, "temperature": 0.5, "thinking": "high"},
  "limits": {"max_turns": 1, "max_tool_calls": 2, "max_tool_result_bytes": 3, "max_conversations": 4, "max_concurrent_chats": 5,
             "max_tokens_per_day": 6, "max_stream_bytes": 7, "max_audit_entries": 8},
  "data": {"send_logs": true, "redact": ["secret-[0-9]+"]},
  "users": [{"username": "ada", "level": "apply"}],
  "default_level": "off"
}"#,
    )
    .unwrap();
    let a = assistant_of(&c);
    let (claude, local) = (a.provider("claude").unwrap(), a.provider("local").unwrap());
    assert!(a.provider("none").is_none());
    assert_eq!(claude.url(), "https://api.anthropic.com/v1");
    assert_eq!(local.url(), "http://127.0.0.1:8000/v1");
    assert_eq!(a.provider("mesh").unwrap().url(), "");
    assert_eq!(claude.connect(), DEFAULT_CONNECT_TIMEOUT);
    assert_eq!(claude.first_byte(), DEFAULT_FIRST_BYTE_TIMEOUT);
    assert_eq!(claude.idle(), DEFAULT_IDLE_TIMEOUT);
    assert_eq!(claude.total(), DEFAULT_TIMEOUT);
    assert_eq!(claude.retry_count(), DEFAULT_RETRIES);
    assert_eq!(local.connect(), Duration::from_secs(1));
    assert_eq!(local.first_byte(), Duration::from_secs(2));
    assert_eq!(local.idle(), Duration::from_secs(3));
    assert_eq!(local.total(), Duration::from_secs(4));
    assert_eq!(local.retry_count(), 0);
    assert_eq!(local.headers, vec![("X-Org".to_owned(), "akit".to_owned())]);
    assert!(claude.models[0].calls_tools() && !local.models[0].calls_tools());
    assert_eq!(claude.models[0].context, 200_000);
    assert_eq!(a.default_model(), ("local", "m"));
    assert_eq!(a.level("ada"), Level::Apply);
    assert_eq!(a.level("bob"), Level::Off);
    assert_eq!(a.params.max_output(), 100);
    assert_eq!(a.params.thinking_level(), "high");
    assert_eq!(a.params.temperature.as_deref(), Some("0.5"));
    let l = &a.limits;
    assert_eq!(
        [
            l.turns(),
            l.tool_calls(),
            l.tool_result_bytes(),
            l.conversations(),
            l.concurrent_chats(),
            l.tokens_per_day(),
            l.stream_bytes(),
            l.audit_entries()
        ],
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert!(a.data.send_logs && !a.data.send_access_log);
    // The member as written, comments and spaces left out: what the Settings page edits.
    assert!(
        a.raw
            .starts_with(r#"{"providers":[{"name":"claude","kind":"anthropic""#)
    );
    assert!(a.raw.contains(r#""temperature":0.5"#));
    assert!(load(&a.raw, c.admin.as_ref().unwrap().console.as_ref()).unwrap() == *a);
}

#[test]
fn defaults() {
    let mut o = Provider::new("o", ProviderKind::Ollama);
    o.models.clear();
    let mut p = Provider::new("p", ProviderKind::OpenAi);
    p.models.push(Model {
        name: "gpt".into(),
        context: 0,
        tools: None,
    });
    let a = Assistant {
        providers: vec![o, p],
        ..Assistant::default()
    };
    assert_eq!(a.default_model(), ("p", "gpt"));
    assert_eq!(Assistant::default().default_model(), ("", ""));
    assert_eq!(a.level("x"), Level::Read);
    assert_eq!(a.provider("o").unwrap().url(), "http://127.0.0.1:11434");
    let (p, l) = (Params::default(), Limits::default());
    assert_eq!(p.max_output(), DEFAULT_MAX_OUTPUT_TOKENS);
    assert_eq!(p.thinking_level(), "off");
    assert_eq!(l.turns(), DEFAULT_MAX_TURNS);
    assert_eq!(l.tool_calls(), DEFAULT_MAX_TOOL_CALLS);
    assert_eq!(l.tool_result_bytes(), DEFAULT_MAX_TOOL_RESULT_BYTES);
    assert_eq!(l.conversations(), DEFAULT_MAX_CONVERSATIONS);
    assert_eq!(l.concurrent_chats(), DEFAULT_MAX_CONCURRENT_CHATS);
    assert_eq!(l.tokens_per_day(), DEFAULT_MAX_TOKENS_PER_DAY);
    assert_eq!(l.stream_bytes(), DEFAULT_MAX_STREAM_BYTES);
    assert_eq!(l.audit_entries(), DEFAULT_MAX_AUDIT_ENTRIES);
}

#[test]
fn errors() {
    let with = |a: &str| format!(r#"{{"admin": {{{CONSOLE}, "assistant": {a}}}}}"#);
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no console",
            r#"{"admin": {"assistant": {}}}"#.to_owned(),
            vec!["admin.assistant: the assistant lives in the console's workbench"],
        ),
        (
            "bad names",
            with(
                r#"{"providers": [{"name": "-x", "kind": "openai"}, {"name": "a", "kind": "openai"}, {"name": "a", "kind": "openai"}]}"#,
            ),
            vec![
                "providers[0].name: \"-x\" is not a valid name",
                "providers[2].name: another provider is already named \"a\"",
            ],
        ),
        (
            "kinds",
            with(r#"{"providers": [{"name": "a", "kind": "x"}]}"#),
            vec!["providers[0].kind: \"x\" is not one of"],
        ),
        (
            "base urls",
            with(
                r#"{"providers": [
            {"name": "a", "kind": "llm_proxy", "base_url": "http://x"},
            {"name": "b", "kind": "openai_compatible"},
            {"name": "c", "kind": "azure_openai", "api_version": "v"},
            {"name": "d", "kind": "openai", "base_url": "ftp://x"},
            {"name": "e", "kind": "openai", "base_url": "https://x/?k=1"},
            {"name": "f", "kind": "openai", "api_version": "v"},
            {"name": "g", "kind": "openai", "api_key_env": "1X"}]}"#,
            ),
            vec![
                "providers[0].base_url: an llm_proxy provider is reached",
                "providers[1].base_url: a openai_compatible provider needs base_url",
                "providers[2].base_url: a azure_openai provider needs base_url",
                "providers[3].base_url: \"ftp://x\" is not an http or https URL",
                "providers[4].base_url:",
                "providers[5].api_version: api_version is for azure_openai",
                "providers[6].api_key_env: \"1X\" is not",
            ],
        ),
        (
            "azure without api_version",
            with(
                r#"{"providers": [{"name": "a", "kind": "azure_openai", "base_url": "https://x"}]}"#,
            ),
            vec!["providers[0].api_version: api_version is for azure_openai, which needs it"],
        ),
        (
            "headers",
            with(
                r#"{"providers": [{"name": "a", "kind": "openai", "headers": {"Authorization": "x", "bad name": "x", "X-Ok": "a\nb"}}]}"#,
            ),
            vec![
                "headers.Authorization: Authorization is the call's own",
                "\"bad name\" is not a header's name",
                "headers.X-Ok: the value holds a control character",
            ],
        ),
        (
            "models",
            with(
                r#"{"providers": [{"name": "a", "kind": "openai", "models": [{"name": "m"}, {"name": "m"}, {"name": ""}]}]}"#,
            ),
            vec![
                "models[1].name: \"m\" is listed twice",
                "models[2].name: a model's ID is 1 to 256 printable bytes",
            ],
        ),
        (
            "default model",
            with(r#"{"default_model": {"provider": "x", "model": "m"}}"#),
            vec!["default_model.provider: no provider is named \"x\""],
        ),
        (
            "redact",
            with(r#"{"data": {"redact": ["(", ""]}}"#),
            vec![
                "data.redact[0]: \"(\" is not a regular expression",
                "data.redact[1]:",
            ],
        ),
        (
            "users",
            with(
                r#"{"users": [{"username": "bob", "level": "read"}, {"username": "ada", "level": "x"}]}"#,
            ),
            vec!["users[1].level: \"x\" is not one of"],
        ),
        (
            "not a console user",
            with(r#"{"users": [{"username": "bob", "level": "read"}]}"#),
            vec!["users[0].username: \"bob\" is not a console user"],
        ),
        (
            "users twice",
            with(
                r#"{"users": [{"username": "ada", "level": "read"}, {"username": "ada", "level": "edit"}]}"#,
            ),
            vec!["users[1].username: \"ada\" is listed twice"],
        ),
        (
            "bounds",
            with(
                r#"{"providers": [{"name": "a", "kind": "openai", "retries": 6}], "params": {"temperature": 3}}"#,
            ),
            vec!["providers[0].retries", "params.temperature"],
        ),
        (
            "unknown",
            with(r#"{"provider": []}"#),
            vec!["admin.assistant.provider: unknown field"],
        ),
    ];
    for (name, doc, want) in cases {
        let err = from_json(&doc, None).expect_err(name).to_string();
        for w in want {
            assert!(err.contains(w), "{name}: {err:?} lacks {w:?}");
        }
    }
}

#[test]
fn bounds_of_lists() {
    let mut b = String::from(r#"{"providers": ["#);
    for i in 0..=MAX_PROVIDERS {
        if i > 0 {
            b.push(',');
        }
        b.push_str(&format!(
            r#"{{"name": "p{}", "kind": "ollama"}}"#,
            "x".repeat(i)
        ));
    }
    b.push_str(&format!(
        r#"], "data": {{"redact": [{}]}}}}"#,
        vec![r#""a""#; MAX_REDACT + 1].join(",")
    ));
    let err = load(&b, None).unwrap_err().to_string();
    assert!(err.contains("providers, more than"), "{err}");
    assert!(err.contains("patterns, more than"), "{err}");
    let headers: Vec<String> = (0..=MAX_HEADERS)
        .map(|i| format!(r#""X-H{}": "v""#, "h".repeat(i)))
        .collect();
    let models = vec![r#"{"name": "m"}"#; MAX_MODELS + 1].join(",");
    let err = load(
        &format!(
            r#"{{"providers": [{{"name": "a", "kind": "openai", "headers": {{{}}}, "models": [{models}]}}]}}"#,
            headers.join(",")
        ),
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("headers, more than"), "{err}");
    assert!(err.contains("models, more than"), "{err}");
}

#[test]
fn load_kept_settings() {
    let console = Console {
        users: vec![crate::ConsoleUser {
            username: "ada".into(),
            password_hash: String::new(),
        }],
        ..Console::default()
    };
    let a = load(
        r#"{"providers": [{"name": "o", "kind": "ollama"}], "users": [{"username": "ada", "level": "edit"}]}"#,
        Some(&console),
    )
    .unwrap();
    assert_eq!(a.level("ada"), Level::Edit);
    assert!(load("{", Some(&console)).is_err());
    let err = load(r#"{"x": 1}"#, Some(&console)).unwrap_err().to_string();
    assert!(err.contains("admin.assistant.x: unknown field"), "{err}");
    assert!(
        load(
            r#"{"users": [{"username": "bob", "level": "edit"}]}"#,
            Some(&console)
        )
        .is_err()
    );
    assert!(load(r#"{"users": [{"username": "bob", "level": "edit"}]}"#, None).is_err());
}

#[test]
fn mcp_settings() {
    let err = load_config(
        r#"{"mcp": {"allow": ["10.0.0.0/8", "nowhere", "fe80::1%eth0", "10.0.0.0/99"]}}"#,
    )
    .unwrap_err()
    .to_string();
    for w in [
        "mcp.allow[1]: \"nowhere\" is not an address",
        "mcp.allow[2]:",
        "mcp.allow[3]:",
    ] {
        assert!(err.contains(w), "{err} lacks {w}");
    }
    let err = load_config(r#"{"mcp": {"max_token_days": 4000}}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("max_token_days"), "{err}");
    let many = vec![r#""::1""#; MAX_MCP_ALLOW + 1].join(",");
    let err = load_config(&format!(r#"{{"mcp": {{"allow": [{many}]}}}}"#))
        .unwrap_err()
        .to_string();
    assert!(err.contains("more than"), "{err}");
    let c = load_config(r#"{"mcp": {"allow": ["10.1.2.3", "::ffff:192.168.0.0/112"]}}"#).unwrap();
    let m = assistant_of(&c).mcp.clone().unwrap();
    let p: Vec<String> = m.allowed().iter().map(ToString::to_string).collect();
    assert_eq!(p, ["10.1.2.3/32", "192.168.0.0/16"]);
    let none = Mcp::default();
    assert_eq!(none.allowed().len(), 2);
    assert_eq!(none.tokens_per_user(), 10);
    assert_eq!(none.token_days(), 90);
    assert_eq!(none.sessions(), 4);
    assert_eq!(none.session_idle(), Duration::from_secs(3600));
    assert_eq!(none.request_bytes(), 1 << 20);
    assert_eq!(none.concurrent_calls(), 4);
    assert_eq!(none.calls_per_minute(), 120);
    let set = Mcp {
        allow: vec!["x".into()],
        max_tokens_per_user: 1,
        max_token_days: 2,
        max_sessions: 3,
        session_idle_timeout: Duration::from_secs(60),
        max_request_bytes: 5,
        max_concurrent_calls: 6,
        max_calls_per_minute: 7,
    };
    assert_eq!(
        (
            set.tokens_per_user(),
            set.token_days(),
            set.sessions(),
            set.request_bytes(),
            set.concurrent_calls(),
            set.calls_per_minute()
        ),
        (1, 2, 3, 5, 6, 7)
    );
    assert_eq!(set.session_idle(), Duration::from_secs(60));
    assert!(set.allowed().is_empty());
}

#[test]
fn urls() {
    for ok in [
        "https://api.openai.com/v1",
        "http://127.0.0.1:8000/v1/",
        "HTTPS://x",
        "https://x/?",
    ] {
        assert!(http_url(ok), "{ok}");
    }
    for bad in [
        "ftp://x",
        "https://",
        "https://u@x/",
        "https://x/?k=1",
        "https://x/#f",
        "x",
        "https://x y/",
    ] {
        assert!(!http_url(bad), "{bad}");
    }
}
