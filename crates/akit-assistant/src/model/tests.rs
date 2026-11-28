//! fit's `assistant/model` tests ported (`kinds_test.go`, `codec_test.go`, `edge_test.go`,
//! `call_test.go`), against the stand-ins of [`crate::standin`] on port 0: each kind's
//! conversation with a tool call round-tripping; the requests' shapes; the parse's edges; the
//! timeouts, sizes, retries and cancels; and akit's held reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use akit_config::assistant::ProviderKind;
use akit_config::value::Value;

use super::answer::{Gather, STOPPED, strict_json};
use super::codec::{Codec, parse_models};
use super::frame::Framer;
use super::*;
use crate::standin::{self, Conn, Server, fake_api, find, get};

const KEY: &str = "sk-test-0123456789";

fn engine() -> Arc<akio::Engine> {
    static E: std::sync::OnceLock<Arc<akio::Engine>> = std::sync::OnceLock::new();
    Arc::clone(E.get_or_init(|| {
        let e = akio::Engine::builder().client_threads(1).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    }))
}

fn client(lim: Limits) -> Client {
    Client::new(engine(), lim, None)
}

#[derive(Default)]
struct Outcome {
    events: Vec<Event>,
    res: Option<Answer>,
    err: Option<Error>,
}

/// Makes the call and waits for its end, at most 10 s.
fn stream(c: &Client, p: &Provider, r: &Request, max: u64) -> Outcome {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    let ev = Arc::clone(&events);
    c.stream(
        p,
        r,
        max,
        move |e| ev.lock().unwrap().push(e),
        move |got| {
            let _ = tx.send(got);
        },
    );
    let got = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the call did not end");
    let events = std::mem::take(&mut *events.lock().unwrap());
    match got {
        Ok(a) => Outcome {
            events,
            res: Some(a),
            err: None,
        },
        Err(e) => Outcome {
            events,
            res: None,
            err: Some(e),
        },
    }
}

fn status_tool() -> Tool {
    Tool {
        name: "get_status".into(),
        description: "A listener's status.".into(),
        parameters: r#"{"type":"object","properties":{"listener":{"type":"string"}},"required":["listener"]}"#
            .into(),
    }
}

fn kind(s: &str) -> ProviderKind {
    ProviderKind::parse(s).unwrap()
}

fn user(s: &str) -> Message {
    Message::new(Role::User, s)
}

/// fit's `TestKindsRoundTrip`: each kind calls its recorded API, takes the tool call of the first
/// answer, hands the tool's result back as that API wants it, and gets the second answer's text;
/// the events stream in order, the usage is counted, and the key goes only in the kind's own
/// header.
#[test]
fn kinds_round_trip() {
    let fake = fake_api(KEY);
    let c = client(Limits::default());
    for k in standin::KINDS {
        let mut p = Provider::new(
            &format!("p-{k}"),
            kind(k),
            &standin::base_url(&fake.url(), k),
        );
        p.key = if k == "ollama" {
            Secret::default()
        } else {
            Secret::new(KEY)
        };
        p.api_version = "2024-10-21".into();
        p.headers = vec![("X-Org".into(), "akit".into())];
        let model = match k {
            "anthropic" => "claude-test",
            "gemini" => "gemini-test",
            "ollama" => "qwen-test",
            "azure_openai" => "dep",
            _ => "gpt-test",
        };
        let mut req = Request {
            model: model.into(),
            system: "You are akit's assistant.".into(),
            tools: vec![status_tool()],
            temperature: Some("0.2".into()),
            messages: vec![user("Is the listener web up?")],
            ..Request::default()
        };
        if matches!(k, "anthropic" | "openai_responses") {
            req.thinking = "low".into();
        }
        let o = stream(&c, &p, &req, 0);
        let res = o.res.unwrap_or_else(|| panic!("{k} turn 1: {:?}", o.err));
        assert_eq!(res.tool_calls.len(), 1, "{k}: {res:?}");
        let call = res.tool_calls[0].clone();
        assert_eq!(call.name, "get_status");
        assert!(
            call.arguments == r#"{"listener":"web"}"# || call.arguments == r#"{"listener": "web"}"#,
            "{k}: {}",
            call.arguments
        );
        assert!(
            matches!(o.events.last(), Some(Event::ToolCall(tc)) if tc.name == "get_status"),
            "{k}: {:?}",
            o.events
        );
        assert!(
            res.usage.input_tokens > 0 && res.usage.output_tokens > 0,
            "{k}: {:?}",
            res.usage
        );
        assert_eq!(res.attempts, 1);
        if !matches!(
            k,
            "openai" | "azure_openai" | "openai_compatible" | "llm_proxy"
        ) {
            assert!(!res.thinking.is_empty(), "{k}: turn 1 has no thinking");
        }
        req.messages.push(res.message());
        req.messages.push(Message::tool(
            &call,
            r#"{"listener":"web","up":true}"#,
            false,
        ));
        let o = stream(&c, &p, &req, 0);
        let res = o.res.unwrap_or_else(|| panic!("{k} turn 2: {:?}", o.err));
        assert_eq!(res.text, "The listener web is up.", "{k}");
        assert!(res.tool_calls.is_empty() && !res.stop_reason.is_empty() && !res.model.is_empty());
        let text: String = o
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, res.text);
        assert!(o.events.len() >= 2);
        let reqs = fake.requests();
        let got = reqs.last().unwrap();
        assert_eq!(got.header("X-Org"), Some("akit"));
        for (name, v) in &got.headers {
            if v.contains(KEY) {
                assert!(
                    ["authorization", "x-api-key", "api-key", "x-goog-api-key"]
                        .contains(&name.to_ascii_lowercase().as_str()),
                    "the key is in {name}"
                );
            }
        }
        assert!(!String::from_utf8_lossy(&got.body).contains(KEY));
    }
}

fn list_models(c: &Client, p: &Provider) -> Result<Vec<ModelInfo>, Error> {
    let (tx, rx) = mpsc::channel();
    c.models(p, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the model list did not come")
}

/// fit's `TestKindsModels`: each kind's model list is read from its own endpoint, and a bad key
/// fails it without the key in the error.
#[test]
fn kinds_models() {
    let fake = fake_api(KEY);
    let c = client(Limits::default());
    for k in standin::KINDS {
        let mut p = Provider::new("p", kind(k), &standin::base_url(&fake.url(), k));
        p.key = Secret::new(KEY);
        p.api_version = "v1".into();
        let models = list_models(&c, &p).unwrap_or_else(|e| panic!("{k}: {e}"));
        assert!(!models.is_empty(), "{k}");
        if k == "gemini" {
            assert_eq!(
                models,
                vec![ModelInfo {
                    id: "gemini-test".into(),
                    context: 1_048_576
                }]
            );
        }
        if k == "ollama" {
            continue;
        }
        let mut bad = p.clone();
        bad.key = Secret::new("sk-wrong-0123456789");
        let e = list_models(&c, &bad).expect_err(k);
        assert!(!e.to_string().contains("sk-wrong-0123456789"), "{k}: {e}");
    }
}

/// fit's `TestWrongKeyRedacted`: an API that says the key back in its error has it replaced, the
/// error names the provider and the status, and a 401 is not tried again.
#[test]
fn wrong_key_redacted() {
    let fake = fake_api(KEY);
    let c = client(Limits::default());
    let key = "sk-wrong-abcdefgh";
    let mut p = Provider::new(
        "openai",
        ProviderKind::OpenAi,
        &standin::base_url(&fake.url(), "openai"),
    );
    p.key = Secret::new(key);
    p.retries = 3;
    let o = stream(
        &c,
        &p,
        &Request {
            model: "gpt-test".into(),
            system: "s".into(),
            tools: vec![status_tool()],
            messages: vec![user("hi")],
            ..Request::default()
        },
        0,
    );
    let e = o.err.unwrap();
    assert!(matches!(e.kind, ErrorKind::Api { status: 401, .. }), "{e}");
    assert_eq!(e.provider, "openai");
    assert!(
        !e.to_string().contains(key) && e.to_string().contains("***"),
        "{e}"
    );
    assert_eq!(fake.requests().len(), 1, "a 401 was tried again");
    assert!(
        !format!("{p:?}").contains(key),
        "the provider prints its key"
    );
    assert_eq!(format!("{:?}", Secret::default()), "\"\"");
}

/// Every kind of message: a user's, an answer with text and two calls (one whose arguments were
/// not JSON), their results (one an object, one an error), and the user again.
fn conversation() -> Request {
    let call = |id: &str, name: &str, args: &str, invalid: bool, sig: &str| ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args.into(),
        invalid,
        signature: sig.into(),
    };
    let mut asst = Message::new(Role::Assistant, "let me look");
    asst.thinking = "hmm".into();
    asst.thinking_signature = "sig".into();
    asst.tool_calls = vec![
        call("c1", "get_status", r#"{"listener":"web"}"#, false, "ts"),
        call("c2", "bare", r#""not json {""#, true, ""),
    ];
    let mut asst2 = Message::new(Role::Assistant, "");
    asst2.tool_calls = vec![call("c3", "bare", "", false, "")];
    let tool = |id: &str, name: &str, content: &str, err: bool| Message {
        tool_call_id: id.into(),
        tool_name: name.into(),
        is_error: err,
        ..Message::new(Role::Tool, content)
    };
    Request {
        model: "m".into(),
        system: "sys".into(),
        temperature: Some("0.7".into()),
        thinking: "high".into(),
        max_tokens: 100,
        tools: vec![
            status_tool(),
            Tool {
                name: "bare".into(),
                description: "no params".into(),
                parameters: String::new(),
            },
        ],
        messages: vec![
            user("q"),
            asst,
            tool("c1", "get_status", r#"{"up":true}"#, false),
            tool("c2", "bare", "bad arguments", true),
            asst2,
            tool("c3", "bare", "plain text", false),
            user(""),
            Message::new(Role::Assistant, ""),
            user("thanks"),
        ],
    }
}

fn body(k: ProviderKind, p: &Provider, r: &Request) -> (String, Value) {
    let (u, b) = Codec::of(k).call(p, r);
    let v = akit_config::value::parse(std::str::from_utf8(&b).unwrap()).unwrap();
    (u, v)
}

fn g<'v>(v: &'v Value, path: &str) -> Option<&'v Value> {
    let parts: Vec<&str> = path.split('.').collect();
    get(v, &parts)
}

fn gs<'v>(v: &'v Value, path: &str) -> &'v str {
    match g(v, path) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

fn gn(v: &Value, path: &str) -> String {
    match g(v, path) {
        Some(Value::Number(n)) => n.clone(),
        other => format!("{other:?}"),
    }
}

fn len(v: &Value, path: &str) -> usize {
    match g(v, path) {
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    }
}

fn provider(base: &str) -> Provider {
    let mut p = Provider::new("p", ProviderKind::OpenAi, base);
    p.api_version = "2024".into();
    p
}

#[test]
fn chat_request() {
    let p = provider("https://x/v1");
    let (u, m) = body(ProviderKind::OpenAi, &p, &conversation());
    assert_eq!(u, "https://x/v1/chat/completions");
    assert_eq!(gs(&m, "messages.0.content"), "sys");
    assert_eq!(gn(&m, "max_completion_tokens"), "100");
    assert_eq!(gs(&m, "reasoning_effort"), "high");
    assert_eq!(gn(&m, "temperature"), "0.7");
    assert_eq!(gs(&m, "model"), "m");
    assert_eq!(
        gs(&m, "messages.2.tool_calls.1.function.arguments"),
        "not json {"
    );
    assert_eq!(gs(&m, "messages.4.role"), "tool");
    assert_eq!(g(&m, "messages.5.content"), Some(&Value::Null));
    assert_eq!(gs(&m, "messages.5.tool_calls.0.function.arguments"), "{}");
    let (u, m) = body(
        ProviderKind::AzureOpenAi,
        &p,
        &Request {
            model: "dep a".into(),
            messages: vec![user("q")],
            ..Request::default()
        },
    );
    assert_eq!(
        u,
        "https://x/v1/openai/deployments/dep%20a/chat/completions?api-version=2024"
    );
    assert!(
        g(&m, "model").is_none() && g(&m, "tools").is_none() && g(&m, "reasoning_effort").is_none()
    );
    let (_, m) = body(ProviderKind::OpenAiCompatible, &p, &conversation());
    assert_eq!(gn(&m, "max_tokens"), "100");
    assert!(g(&m, "reasoning_effort").is_none() && g(&m, "max_completion_tokens").is_none());
}

#[test]
fn anthropic_request() {
    let (u, m) = body(
        ProviderKind::Anthropic,
        &provider("https://a/v1"),
        &conversation(),
    );
    assert_eq!(u, "https://a/v1/messages");
    assert_eq!(gs(&m, "system"), "sys");
    assert!(g(&m, "temperature").is_none());
    assert_eq!(gn(&m, "thinking.budget_tokens"), "24576");
    assert_eq!(gn(&m, "max_tokens"), "24676");
    // user, assistant, user (two results), assistant, user (a result, then the user's words).
    assert_eq!(len(&m, "messages"), 5);
    assert_eq!(gs(&m, "messages.1.content.0.type"), "thinking");
    assert_eq!(len(&m, "messages.2.content"), 2);
    assert_eq!(
        g(&m, "messages.2.content.1.is_error"),
        Some(&Value::Bool(true))
    );
    assert_eq!(gs(&m, "messages.4.content.1.text"), "thanks");
    assert_eq!(
        crate::json::compact(g(&m, "messages.1.content.3.input").unwrap()),
        "{}"
    );
    assert_eq!(gs(&m, "tools.1.input_schema.type"), "object");
    let (_, m) = body(
        ProviderKind::Anthropic,
        &provider(""),
        &Request {
            temperature: Some("0.1".into()),
            ..Request::default()
        },
    );
    assert_eq!(gn(&m, "temperature"), "0.1");
    assert_eq!(gn(&m, "max_tokens"), "4096");
    assert!(g(&m, "system").is_none() && len(&m, "messages") == 0);
}

#[test]
fn gemini_request() {
    let (u, m) = body(
        ProviderKind::Gemini,
        &provider("https://g/v1beta"),
        &conversation(),
    );
    assert_eq!(u, "https://g/v1beta/models/m:streamGenerateContent?alt=sse");
    assert_eq!(
        gn(&m, "generationConfig.thinkingConfig.thinkingBudget"),
        "24576"
    );
    assert_eq!(gn(&m, "generationConfig.temperature"), "0.7");
    assert_eq!(gs(&m, "systemInstruction.parts.0.text"), "sys");
    assert_eq!(len(&m, "contents"), 5);
    assert_eq!(gs(&m, "contents.1.role"), "model");
    assert_eq!(gs(&m, "contents.1.parts.1.thoughtSignature"), "ts");
    assert_eq!(
        g(&m, "contents.2.parts.0.functionResponse.response.up"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        gs(&m, "contents.2.parts.1.functionResponse.response.error"),
        "bad arguments"
    );
    assert_eq!(
        gs(&m, "contents.4.parts.0.functionResponse.response.result"),
        "plain text"
    );
    let (_, m) = body(ProviderKind::Gemini, &provider(""), &Request::default());
    assert!(g(&m, "generationConfig.thinkingConfig").is_none());
    assert!(g(&m, "systemInstruction").is_none() && g(&m, "tools").is_none());
    assert_eq!(len(&m, "contents"), 0);
}

#[test]
fn ollama_and_responses_requests() {
    let (u, m) = body(ProviderKind::Ollama, &provider("http://o"), &conversation());
    assert_eq!(u, "http://o/api/chat");
    assert_eq!(g(&m, "think"), Some(&Value::Bool(true)));
    assert_eq!(gn(&m, "options.num_predict"), "100");
    assert_eq!(gn(&m, "options.temperature"), "0.7");
    assert_eq!(
        gs(&m, "messages.2.tool_calls.0.function.arguments.listener"),
        "web"
    );
    assert_eq!(gs(&m, "messages.4.tool_name"), "bare");
    let (_, m) = body(ProviderKind::Ollama, &provider(""), &Request::default());
    assert!(g(&m, "think").is_none() && g(&m, "tools").is_none());
    let (u, m) = body(
        ProviderKind::OpenAiResponses,
        &provider("https://x/v1"),
        &conversation(),
    );
    assert_eq!(u, "https://x/v1/responses");
    assert_eq!(gs(&m, "instructions"), "sys");
    assert_eq!(gs(&m, "reasoning.effort"), "high");
    assert_eq!(gn(&m, "temperature"), "0.7");
    assert_eq!(gs(&m, "input.1.content"), "let me look");
    assert_eq!(gs(&m, "input.3.arguments"), "not json {");
    assert_eq!(gs(&m, "input.4.type"), "function_call_output");
    assert_eq!(gs(&m, "tools.1.parameters.type"), "object");
    let (_, m) = body(
        ProviderKind::OpenAiResponses,
        &provider(""),
        &Request::default(),
    );
    assert!(
        g(&m, "instructions").is_none() && g(&m, "reasoning").is_none() && g(&m, "tools").is_none()
    );
}

fn compat(url: &str) -> Provider {
    Provider::new("p", ProviderKind::OpenAiCompatible, url)
}

fn hello() -> Request {
    Request {
        model: "m".into(),
        messages: vec![user("hi")],
        ..Request::default()
    }
}

#[test]
fn request_check() {
    let c = client(Limits::default());
    for r in [
        Request {
            tools: vec![Tool {
                name: "t".into(),
                description: String::new(),
                parameters: "{".into(),
            }],
            ..Request::default()
        },
        Request {
            messages: vec![Message {
                tool_calls: vec![ToolCall {
                    id: "c".into(),
                    name: String::new(),
                    arguments: "{".into(),
                    invalid: false,
                    signature: String::new(),
                }],
                ..Message::new(Role::Assistant, "")
            }],
            ..Request::default()
        },
    ] {
        let o = stream(&c, &compat("http://127.0.0.1:1"), &r, 0);
        assert!(o.err.unwrap().to_string().contains("not JSON"));
    }
}

/// Runs `events` of `kind` through its codec: the answer and the first error.
fn parse(k: ProviderKind, events: &[&str]) -> (Answer, Result<(), ErrorKind>) {
    let mut cd = Codec::of(k);
    let mut a = Gather::new(Answer::default(), Box::new(|_| true), 4);
    for e in events {
        let (name, data) = e.split_once('|').unwrap_or(("", e));
        if let Err(err) = cd.parse(&mut a, name, data.as_bytes()) {
            return (a.res, Err(err));
        }
    }
    let r = cd.finish(&mut a);
    (a.res, r)
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn parse_edges() {
    use ProviderKind::{Anthropic, Gemini, Ollama, OpenAiCompatible, OpenAiResponses};
    // A tool call with no arguments, one with arguments not JSON, and a stream without [DONE]
    // but with its finish_reason.
    let (a, r) = parse(
        OpenAiCompatible,
        &[
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"a","function":{"name":"x"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"y","arguments":"oops"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
        ],
    );
    assert!(r.is_ok());
    assert_eq!(a.tool_calls.len(), 2);
    assert_eq!(a.tool_calls[0].arguments, "{}");
    assert!(a.tool_calls[1].invalid && a.tool_calls[1].arguments == r#""oops""#);
    assert_eq!(a.usage.output_tokens, 2);
    // A URL in a string is JSON as Go takes it; a comment, or a trailing comma, is not.
    for (args, invalid) in [
        (
            r#"{"urls": ["https://example.com/a"], "c": "/*x*/"}"#,
            false,
        ),
        ("{\"a\": 1 // one\n}", true),
        (r#"{"a": 1 /* one */}"#, true),
        (r#"{"a": [1,]}"#, true),
    ] {
        assert_eq!(strict_json(args), !invalid, "{args}");
    }
    let (a, r) = parse(
        OpenAiCompatible,
        &[
            r#"{"choices":[{"delta":{"content":"x"}}],"error":null}"#,
            "[DONE]",
        ],
    );
    assert!(r.is_ok() && a.text == "x");
    let too_many = |k, e: &[&str]| assert_eq!(parse(k, e).1, Err(ErrorKind::TooLarge), "{e:?}");
    too_many(
        OpenAiCompatible,
        &[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0},{"index":1},{"index":2},{"index":3},{"index":4}]}}]}"#,
        ],
    );
    too_many(
        Ollama,
        &[
            r#"{"message":{"tool_calls":[{"function":{"name":"a"}},{"function":{"name":"a"}},{"function":{"name":"a"}},{"function":{"name":"a"}},{"function":{"name":"a"}}]}}"#,
        ],
    );
    let (_, r) = parse(Anthropic, &[r#"{"type":"content_block_start","index":0}"#]);
    assert!(format!("{r:?}").contains("without its block"));
    let (_, r) = parse(Anthropic, &[r#"{"type":"content_block_delta","index":0}"#]);
    assert!(format!("{r:?}").contains("without its delta"));
    let (a, r) = parse(
        Anthropic,
        &[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"a"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"n"}}"#,
            r#"{"type":"message_stop"}"#,
        ],
    );
    assert!(r.is_ok() && a.text == "a" && a.tool_calls.len() == 1);
    assert_eq!(a.tool_calls[0].arguments, "{}");
    let starts: Vec<String> = (0..5)
        .map(|i| {
            format!(
                r#"{{"type":"content_block_start","index":{i},"content_block":{{"type":"tool_use","id":"t{i}","name":"n"}}}}"#
            )
        })
        .collect();
    too_many(Anthropic, &strs(&starts));
    let deltas: Vec<String> = (5..10)
        .map(|i| {
            format!(
                r#"{{"type":"content_block_delta","index":{i},"delta":{{"type":"input_json_delta","partial_json":"{{}}"}}}}"#
            )
        })
        .collect();
    too_many(Anthropic, &strs(&deltas));
    let (a, r) = parse(
        Gemini,
        &[
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"id":"g1","name":"n","args":{"a":1}}}]},"finishReason":"STOP"}]}"#,
        ],
    );
    assert!(r.is_ok() && a.tool_calls[0].id == "g1" && a.tool_calls[0].arguments == r#"{"a":1}"#);
    let (a, r) = parse(
        OpenAiResponses,
        &[
            r#"response.output_text.delta|{"delta":"hi"}"#,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"i1","call_id":"c1","name":"n"}}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"i1","call_id":"c1","name":"n"}}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"i2","delta":"{\"a\":"}"#,
            r#"{"type":"response.function_call_arguments.delta","item_id":"i2","delta":"2}"}"#,
            r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":1,"output_tokens":2}}}"#,
        ],
    );
    assert!(r.is_ok(), "{r:?}");
    assert!(a.text == "hi" && a.tool_calls.len() == 2);
    assert_eq!(a.tool_calls[1].arguments, r#"{"a":2}"#);
    assert_eq!(a.stop_reason, "max_output_tokens");
    let (_, r) = parse(
        OpenAiResponses,
        &[r#"{"type":"response.failed","response":{"status":"failed"}}"#],
    );
    assert!(matches!(r, Err(ErrorKind::Api { .. })));
    let added: Vec<String> = (0..5)
        .map(|i| {
            format!(
                r#"{{"type":"response.output_item.added","item":{{"type":"function_call","id":"{i}"}}}}"#
            )
        })
        .collect();
    too_many(OpenAiResponses, &strs(&added));
    let mut args: Vec<String> = (0..5)
        .map(|i| format!(r#"{{"type":"response.function_call_arguments.delta","item_id":"{i}"}}"#))
        .collect();
    too_many(OpenAiResponses, &strs(&args));
    args[4] =
        r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"9"}}"#.into();
    too_many(OpenAiResponses, &strs(&args));
}

/// fit's `TestStoppedParse`: once the call has ended, a parse stops at its next piece.
#[test]
fn stopped_parse() {
    for (k, ev) in [
        (
            ProviderKind::OpenAiCompatible,
            r#"{"choices":[{"delta":{"reasoning":"r","content":"x","tool_calls":[{"id":"a","function":{"name":"x"}}]},"finish_reason":"stop"}]}"#,
        ),
        (
            ProviderKind::Ollama,
            r#"{"message":{"thinking":"r","content":"x","tool_calls":[{"function":{"name":"a"}}]}}"#,
        ),
        (
            ProviderKind::Gemini,
            r#"{"candidates":[{"content":{"parts":[{"text":"r","thought":true},{"text":"x"},{"functionCall":{"name":"n"}}]}}]}"#,
        ),
    ] {
        for skip in 0..3 {
            let n = Arc::new(AtomicUsize::new(0));
            let m = Arc::clone(&n);
            let mut cd = Codec::of(k);
            let mut a = Gather::new(
                Answer::default(),
                Box::new(move |_| m.fetch_add(1, Ordering::SeqCst) < skip),
                4,
            );
            let r = cd.parse(&mut a, "", ev.as_bytes());
            assert_eq!(
                r,
                Err(ErrorKind::Other(STOPPED.into())),
                "{k:?} after {skip}"
            );
        }
    }
    for ev in [
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"a"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"a"}}"#,
    ] {
        let mut a = Gather::new(Answer::default(), Box::new(|_| false), 4);
        assert!(
            Codec::of(ProviderKind::Anthropic)
                .parse(&mut a, "", ev.as_bytes())
                .is_err()
        );
    }
}

#[test]
fn framer() {
    let mut got: Vec<String> = Vec::new();
    let mut f = Framer::new(false, 100);
    {
        let mut h = |name: &str, data: &[u8]| {
            got.push(format!("{name}={}", String::from_utf8_lossy(data)));
            Ok(())
        };
        for p in [
            "event: a\r\ndata: 1\r\n",
            "data:2\n\n: comment\nid: 7\n\n",
            "data: x",
            "y\nretry: 1\n\ndata: last",
        ] {
            f.write(p.as_bytes(), &mut h).unwrap();
        }
        f.end(&mut h).unwrap();
    }
    assert_eq!(got.join(","), "a=1\n2,=xy,=last");
    let mut got: Vec<String> = Vec::new();
    let mut f = Framer::new(true, 100);
    {
        let mut h = |_: &str, data: &[u8]| {
            got.push(String::from_utf8_lossy(data).into_owned());
            Ok(())
        };
        f.write(b"{}\n\n  \n{\"a\"", &mut h).unwrap();
        f.write(b":1}", &mut h).unwrap();
        f.end(&mut h).unwrap();
    }
    assert_eq!(got.join(","), r#"{},{"a":1}"#);
    let mut f = Framer::new(true, 100);
    let mut boom = |_: &str, _: &[u8]| Err(ErrorKind::Other("boom".into()));
    assert!(f.write(b"{}\n", &mut boom).is_err());
    f.write(b"{}", &mut boom).unwrap();
    assert!(f.end(&mut boom).is_err());
    // A line and an event's data are bounded.
    let mut ok = |_: &str, _: &[u8]| Ok(());
    let mut f = Framer::new(false, 10);
    assert_eq!(
        f.write(b"data: 0123456789", &mut ok),
        Err(ErrorKind::TooLarge)
    );
    let mut f = Framer::new(false, 12);
    assert_eq!(
        f.write(b"data: 012345\ndata: 678901\n", &mut ok),
        Err(ErrorKind::TooLarge)
    );
}

#[test]
fn api_messages() {
    for (b, want) in [
        (r#"{"error":{"message":"a"}}"#.to_owned(), "a".to_owned()),
        (r#"{"error":"b"}"#.to_owned(), "b".to_owned()),
        (r#"{"message":"c"}"#.to_owned(), "c".to_owned()),
        (r#"{"detail":"d"}"#.to_owned(), "d".to_owned()),
        ("plain".to_owned(), "plain".to_owned()),
        (String::new(), "no message".to_owned()),
        ("z".repeat(600), format!("{}...", "z".repeat(512))),
    ] {
        assert_eq!(api_message(b.as_bytes()), want);
    }
    let e = Error::new(
        "p",
        ErrorKind::Api {
            status: 500,
            message: "m".into(),
            retry_after: String::new(),
        },
    );
    assert_eq!(e.to_string(), "p: HTTP 500: m");
    assert_eq!(redact("abc", &Secret::new("ab")), "abc");
    for (l, n) in [("low", 2048), ("medium", 8192), ("high", 24576), ("off", 0)] {
        assert_eq!(thinking_budget(l), n);
    }
}

#[test]
fn models_parsed() {
    assert!(parse_models(ProviderKind::OpenAi, b"{").is_err());
    let m = parse_models(
        ProviderKind::Ollama,
        br#"{"models":[{"model":"a"},{"name":""}]}"#,
    )
    .unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].id, "a");
    let m = parse_models(ProviderKind::OpenAi, br#"{"data":[{"id":""},{"id":"x"}]}"#).unwrap();
    assert_eq!(m.len(), 1);
}

// ---- The edges of a call, against stand-ins (fit's edge_test.go and call_test.go) ----

fn sse(c: &mut Conn, events: &[&str]) {
    c.head(200, &[("Content-Type", "text/event-stream")]);
    for e in events {
        c.chunk(format!("data: {e}\n\n").as_bytes());
    }
}

const CHUNK_HI: &str = r#"{"choices":[{"delta":{"content":"hi"}}]}"#;

/// fit's `TestFirstByteTimeout`: an API that sends its head and nothing more is given up at the
/// first-byte timeout.
#[test]
fn first_byte_timeout() {
    let s = Server::start(|_, c| {
        c.head(200, &[("Content-Type", "text/event-stream")]);
        c.wait_gone(Duration::from_secs(5));
    });
    let mut p = compat(&s.url());
    p.first_byte_timeout = Duration::from_millis(200);
    let start = Instant::now();
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    assert_eq!(o.err.map(|e| e.kind), Some(ErrorKind::FirstByte));
    assert!(start.elapsed() < Duration::from_secs(4));
}

/// fit's `TestFirstByteTimeoutBeforeHeader`.
#[test]
fn first_byte_timeout_before_head() {
    let s = Server::start(|_, c| {
        c.wait_gone(Duration::from_secs(5));
    });
    let mut p = compat(&s.url());
    p.first_byte_timeout = Duration::from_millis(200);
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    assert_eq!(o.err.map(|e| e.kind), Some(ErrorKind::FirstByte));
}

/// fit's `TestIdleTimeout`: an answer that goes silent is given up at the idle timeout, after
/// what came before it was handed on.
#[test]
fn idle_timeout() {
    let s = Server::start(|_, c| {
        sse(c, &[CHUNK_HI]);
        c.wait_gone(Duration::from_secs(5));
    });
    let mut p = compat(&s.url());
    p.idle_timeout = Duration::from_millis(200);
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    assert_eq!(o.err.map(|e| e.kind), Some(ErrorKind::Idle));
    assert_eq!(o.events, vec![Event::Text("hi".into())]);
}

/// fit's `TestTotalTimeout`: an answer that trickles on is given up at the call's timeout.
#[test]
fn total_timeout() {
    let s = Server::start(|_, c| {
        c.head(200, &[("Content-Type", "text/event-stream")]);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && c.chunk(format!("data: {CHUNK_HI}\n\n").as_bytes())
        {
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let mut p = compat(&s.url());
    p.timeout = Duration::from_millis(300);
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    assert_eq!(o.err.map(|e| e.kind), Some(ErrorKind::Timeout));
    assert!(!o.events.is_empty());
}

/// fit's `TestCutStream`: an answer whose connection is cut midway fails, and is not tried again.
#[test]
fn cut_stream() {
    let calls = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&calls);
    let s = Server::start(move |_, c| {
        n.fetch_add(1, Ordering::SeqCst);
        c.head(200, &[("Content-Type", "text/event-stream")]);
        c.chunk(format!("data: {CHUNK_HI}\n\ndata: {{\"cho").as_bytes());
        std::thread::sleep(Duration::from_millis(50));
        c.cut();
    });
    let mut p = compat(&s.url());
    p.retries = 3;
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    assert!(o.err.is_some());
    assert_eq!(o.events.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// fit's `TestIncomplete`: an answer that ends without saying it is done fails, for every kind.
#[test]
fn incomplete() {
    for (k, b) in [
        (
            ProviderKind::OpenAiCompatible,
            format!("data: {CHUNK_HI}\n\n"),
        ),
        (
            ProviderKind::Anthropic,
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"m\"}}\n\n"
                .into(),
        ),
        (
            ProviderKind::Gemini,
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n".into(),
        ),
        (
            ProviderKind::OpenAiResponses,
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n".into(),
        ),
        (
            ProviderKind::Ollama,
            "{\"message\":{\"content\":\"hi\"},\"done\":false}\n".into(),
        ),
    ] {
        let s = Server::start(move |_, c| c.whole(200, "text/event-stream", b.as_bytes()));
        let mut p = compat(&s.url());
        p.kind = k;
        let o = stream(&client(Limits::default()), &p, &hello(), 0);
        assert_eq!(
            o.err.map(|e| e.kind),
            Some(ErrorKind::Incomplete),
            "{k:?}"
        );
    }
}

type Body = Box<dyn Fn(&mut Conn) + Send + Sync>;

/// fit's `TestTooLarge`: an answer past the call's bound, a line past the line's, an event's data
/// past it, or too many tool calls, fails, and the server sees the connection go.
#[test]
fn too_large() {
    let big = "x".repeat(4096);
    let small = Limits {
        max_event_bytes: 1000,
        ..Limits::default()
    };
    let cases: Vec<(&str, Limits, u64, Body)> = vec![
        ("answer", Limits::default(), 10_000, {
            let big = big.clone();
            Box::new(move |c: &mut Conn| {
                for _ in 0..100 {
                    let e = format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{big}\"}}}}]}}\n\n"
                    );
                    if !c.chunk(e.as_bytes()) {
                        return;
                    }
                }
            })
        }),
        ("line", small, 0, {
            let big = big.clone();
            Box::new(move |c: &mut Conn| {
                c.chunk(format!("data: {big}").as_bytes());
            })
        }),
        ("line in pieces", small, 0, {
            let big = big.clone();
            Box::new(move |c: &mut Conn| {
                c.chunk(format!("data: {}", &big[..600]).as_bytes());
                std::thread::sleep(Duration::from_millis(20));
                c.chunk(format!("{}\n", &big[..600]).as_bytes());
            })
        }),
        ("event", small, 0, {
            let big = big.clone();
            Box::new(move |c: &mut Conn| {
                let mut b = String::new();
                for _ in 0..10 {
                    b.push_str(&format!("data: {}\n", &big[..600]));
                }
                c.chunk(b.as_bytes());
            })
        }),
        (
            "tool calls",
            Limits {
                max_tool_calls: 2,
                ..Limits::default()
            },
            0,
            Box::new(|c: &mut Conn| {
                let mut b = String::new();
                for i in 0..3 {
                    b.push_str(&format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":{i},\"id\":\"c{i}\",\"function\":{{\"name\":\"t\",\"arguments\":\"{{}}\"}}}}]}}}}]}}\n\n"
                    ));
                }
                c.chunk(b.as_bytes());
            }),
        ),
    ];
    for (name, lim, max, body) in cases {
        let gone = Arc::new(AtomicBool::new(false));
        let g2 = Arc::clone(&gone);
        let s = Server::start(move |_, c| {
            c.head(200, &[("Content-Type", "text/event-stream")]);
            body(c);
            g2.store(c.wait_gone(Duration::from_secs(5)), Ordering::SeqCst);
        });
        let o = stream(&client(lim), &compat(&s.url()), &hello(), max);
        assert_eq!(o.err.map(|e| e.kind), Some(ErrorKind::TooLarge), "{name}");
        let deadline = Instant::now() + Duration::from_secs(6);
        while !gone.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            gone.load(Ordering::SeqCst),
            "{name}: the server did not see the connection go"
        );
    }
}

/// fit's `TestRetries`, as akit's rule has it (decision row 100): a connection that closed before
/// any answer is tried again at once, as often as `retries` allows; an answer of 503 or 429 fails
/// the call at once, saying the `Retry-After` the API asked; a 400 is not tried again.
#[test]
fn retries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&calls);
    let s = Server::start(move |_, c| match n.fetch_add(1, Ordering::SeqCst) {
        0 | 1 => c.cut(), // no answer at all
        _ => sse(
            c,
            &[
                CHUNK_HI,
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ],
        ),
    });
    let mut p = compat(&s.url());
    p.retries = 3;
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    let res = o.res.unwrap_or_else(|| panic!("{:?}", o.err));
    assert_eq!((res.attempts, res.text.as_str()), (3, "hi"));
    // Retries spent.
    calls.store(0, Ordering::SeqCst);
    p.retries = 1;
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    assert!(o.err.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // A 503 or a 429 is not waited for: it fails at once with what the API asked.
    for (status, header, want) in [
        (503u16, ("Retry-After", "7"), "7 s"),
        (429, ("Retry-After-Ms", "10"), "10 ms"),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let n = Arc::clone(&calls);
        let s = Server::start(move |_, c| {
            n.fetch_add(1, Ordering::SeqCst);
            let body = br#"{"error":{"message":"overloaded"}}"#;
            c.raw(
                format!(
                    "HTTP/1.1 {status} X\r\n{}: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    header.0,
                    header.1,
                    body.len()
                )
                .as_bytes(),
            );
            c.raw(body);
        });
        let mut p = compat(&s.url());
        p.retries = 3;
        let e = stream(&client(Limits::default()), &p, &hello(), 0)
            .err
            .unwrap();
        assert_eq!(
            e.kind,
            ErrorKind::Api {
                status,
                message: "overloaded".into(),
                retry_after: want.into()
            }
        );
        assert!(e.to_string().contains(want), "{e}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    let bad = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&bad);
    let s = Server::start(move |_, c| {
        n.fetch_add(1, Ordering::SeqCst);
        c.whole(400, "application/json", br#"{"error":"bad model"}"#);
    });
    let mut p = compat(&s.url());
    p.retries = 3;
    let e = stream(&client(Limits::default()), &p, &hello(), 0)
        .err
        .unwrap();
    assert!(
        matches!(e.kind, ErrorKind::Api { status: 400, ref message, .. } if message == "bad model")
    );
    assert_eq!(bad.load(Ordering::SeqCst), 1);
}

/// fit's `TestRetryConnectRefused`: a connect refused is tried again, and the call fails once the
/// retries are spent, naming its provider.
#[test]
fn retry_connect_refused() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let mut p = compat(&format!("http://{addr}"));
    p.retries = 2;
    let o = stream(&client(Limits::default()), &p, &hello(), 0);
    let e = o.err.unwrap();
    assert!(e.to_string().starts_with("p: "), "{e}");
}

/// fit's `TestCancel`: a call cancelled midway, as when the browser goes away, ends with the
/// cancel's reason, hands on nothing after it, and its connection is closed; so is one cancelled
/// before its answer.
#[test]
fn cancel() {
    let gone = Arc::new(AtomicBool::new(false));
    let g2 = Arc::clone(&gone);
    let s = Server::start(move |_, c| {
        sse(c, &[CHUNK_HI]);
        g2.store(c.wait_gone(Duration::from_secs(5)), Ordering::SeqCst);
    });
    let c = client(Limits::default());
    let (started_tx, started_rx) = mpsc::channel();
    let started_tx = Mutex::new(started_tx);
    let (tx, rx) = mpsc::channel();
    let ended = Arc::new(AtomicBool::new(false));
    let e2 = Arc::clone(&ended);
    let k = c.stream(
        &compat(&s.url()),
        &hello(),
        0,
        move |_| {
            assert!(!e2.load(Ordering::SeqCst), "an event after the end");
            let _ = started_tx.lock().unwrap().send(());
        },
        move |got| {
            let _ = tx.send(got);
        },
    );
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    k.cancel("the browser went away");
    k.cancel("");
    let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    ended.store(true, Ordering::SeqCst);
    assert_eq!(
        got.unwrap_err().kind,
        ErrorKind::Cancelled("the browser went away".into())
    );
    let deadline = Instant::now() + Duration::from_secs(6);
    while !gone.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        gone.load(Ordering::SeqCst),
        "the server did not see the call go"
    );
    // Before the answer.
    let s2 = Server::start(|_, c| {
        c.wait_gone(Duration::from_secs(5));
    });
    let (tx, rx) = mpsc::channel();
    let k = c.stream(
        &compat(&s2.url()),
        &hello(),
        0,
        |_| {},
        move |got| {
            let _ = tx.send(got);
        },
    );
    // Once the server has the request.
    let deadline = Instant::now() + Duration::from_secs(5);
    while s2.requests().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    k.cancel("");
    let e = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert!(e.to_string().contains("cancelled"), "{e}");
}

/// fit's `TestCancelFromEvent`: a caller that cancels while it is handed a piece hears no more
/// pieces, and the call ends with its reason.
#[test]
fn cancel_from_event() {
    let s = Server::start(|_, c| {
        c.head(200, &[("Content-Type", "text/event-stream")]);
        c.chunk(format!("data: {CHUNK_HI}\n\ndata: {CHUNK_HI}\n\ndata: {CHUNK_HI}\n\n").as_bytes());
        c.wait_gone(Duration::from_secs(5));
    });
    let c = client(Limits::default());
    let slot: Arc<Mutex<Option<Call>>> = Arc::new(Mutex::new(None));
    let n = Arc::new(AtomicUsize::new(0));
    let (s2, n2) = (Arc::clone(&slot), Arc::clone(&n));
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let ready_rx = Mutex::new(ready_rx);
    let (tx, rx) = mpsc::channel();
    let k = c.stream(
        &compat(&s.url()),
        &hello(),
        0,
        move |_| {
            let _ = ready_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            n2.fetch_add(1, Ordering::SeqCst);
            if let Some(k) = s2.lock().unwrap().as_ref() {
                k.cancel("stop");
            }
        },
        move |got| {
            let _ = tx.send(got);
        },
    );
    *slot.lock().unwrap() = Some(k);
    let _ = ready_tx.send(());
    let e = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled("stop".into()));
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

/// fit's `TestCancelWhileReadingError`.
#[test]
fn cancel_while_reading_error() {
    let s = Server::start(|_, c| {
        c.head(503, &[]);
        c.chunk(br#"{"error":"#);
        std::thread::sleep(Duration::from_millis(100));
        c.chunk(br#"{"message":"slow"}}"#);
        c.wait_gone(Duration::from_secs(5));
    });
    let (tx, rx) = mpsc::channel();
    let k = client(Limits::default()).stream(
        &compat(&s.url()),
        &hello(),
        0,
        |_| {},
        move |g| {
            let _ = tx.send(g);
        },
    );
    std::thread::sleep(Duration::from_millis(50));
    k.cancel("stop");
    let e = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled("stop".into()));
}

/// fit's `TestRefusedAtOnce`: a key that would go in the clear, a bad URL, and a closed client
/// fail before anything is sent.
#[test]
fn refused_at_once() {
    let c = client(Limits::default());
    for base in ["http://192.0.2.1/v1", "ftp://x", "http://[::1"] {
        let mut p = Provider::new("p", ProviderKind::OpenAi, base);
        if base.starts_with("http://192") {
            p.key = Secret::new("sk-123456");
        }
        let (tx, rx) = mpsc::channel();
        c.stream(
            &p,
            &hello(),
            0,
            |_| {},
            move |g| {
                let _ = tx.send(g);
            },
        );
        let e = rx.try_recv().expect("no error at once").unwrap_err();
        if base.starts_with("http://192") {
            assert_eq!(e.kind, ErrorKind::KeyInClear);
        }
    }
    c.close();
    let (tx, rx) = mpsc::channel();
    c.stream(
        &compat("http://127.0.0.1:1"),
        &hello(),
        0,
        |_| {},
        move |g| {
            let _ = tx.send(g);
        },
    );
    assert_eq!(rx.try_recv().unwrap().unwrap_err().kind, ErrorKind::Closed);
    assert!(client::loopback("localhost") && client::loopback("::1"));
    assert!(!client::loopback("example.com"));
}

/// fit's `TestTransportsBounded`: providers with many connect timeouts share at most 16 akio
/// clients.
#[test]
fn transports_bounded() {
    let s = Server::start(|_, c| {
        sse(
            c,
            &[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#],
        );
    });
    let c = client(Limits::default());
    for i in 0..MAX_TRANSPORTS + 3 {
        let mut p = compat(&s.url());
        p.connect_timeout = Duration::from_secs(u64::try_from(i).unwrap() + 1);
        let o = stream(&c, &p, &hello(), 0);
        assert!(o.err.is_none(), "{:?}", o.err);
    }
    assert_eq!(c.transports(), MAX_TRANSPORTS);
}

/// fit's `TestErrorBodyBounded`: of a failed answer only `max_error_bytes` are read.
#[test]
fn error_body_bounded() {
    let s = Server::start(|_, c| {
        c.head(500, &[]);
        for _ in 0..100 {
            if !c.chunk(&[b'e'; 1000]) {
                return;
            }
        }
        c.wait_gone(Duration::from_secs(5));
    });
    let e = stream(
        &client(Limits {
            max_error_bytes: 2000,
            ..Limits::default()
        }),
        &compat(&s.url()),
        &hello(),
        0,
    )
    .err
    .unwrap();
    assert!(
        matches!(&e.kind, ErrorKind::Api { status: 500, message, .. } if message.len() <= 600),
        "{e}"
    );
}

/// fit's `TestStreamErrors`: an error the API sends midway, or an event that is not JSON, fails
/// the call with what it said.
#[test]
fn stream_errors() {
    for (k, b) in [
        (
            ProviderKind::OpenAiCompatible,
            "data: {\"error\":{\"message\":\"boom\"}}\n\n",
        ),
        (
            ProviderKind::Anthropic,
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"boom\"}}\n\n",
        ),
        (
            ProviderKind::Gemini,
            "data: {\"error\":{\"code\":500,\"message\":\"boom\"}}\n\n",
        ),
        (
            ProviderKind::OpenAiResponses,
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"boom\"}}}\n\n",
        ),
        (ProviderKind::Ollama, "{\"error\":\"boom\"}\n"),
        (
            ProviderKind::AzureOpenAi,
            "data: {\"error\":{\"message\":\"boom\"}}\n\n",
        ),
        (
            ProviderKind::OpenAiResponses,
            "event: error\ndata: {\"type\":\"error\",\"message\":\"boom\"}\n\n",
        ),
    ] {
        let s = Server::start(move |_, c| c.whole(200, "text/event-stream", b.as_bytes()));
        let mut p = compat(&s.url());
        p.kind = k;
        p.api_version = "v".into();
        let e = stream(&client(Limits::default()), &p, &hello(), 0)
            .err
            .unwrap();
        assert!(
            matches!(&e.kind, ErrorKind::Api { message, .. } if message == "boom"),
            "{k:?}: {e}"
        );
    }
    for k in [
        ProviderKind::OpenAiCompatible,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
        ProviderKind::OpenAiResponses,
        ProviderKind::Ollama,
    ] {
        let b = if k == ProviderKind::Ollama {
            "{nope\n"
        } else {
            "data: {nope\n\n"
        };
        let s = Server::start(move |_, c| c.whole(200, "text/event-stream", b.as_bytes()));
        let mut p = compat(&s.url());
        p.kind = k;
        let e = stream(&client(Limits::default()), &p, &hello(), 0)
            .err
            .unwrap();
        assert!(
            e.to_string().contains("not the JSON expected"),
            "{k:?}: {e}"
        );
    }
}

/// fit's `TestModelsBadList` and `TestBadLastEvent`.
#[test]
fn bad_lists_and_last_events() {
    let s = Server::start(|_, c| c.whole(200, "application/json", b"{"));
    let e = list_models(
        &client(Limits::default()),
        &Provider::new("p", ProviderKind::OpenAi, &s.url()),
    )
    .unwrap_err();
    assert!(e.to_string().starts_with("p: the model list"), "{e}");
    let s = Server::start(|_, c| c.whole(200, "text/event-stream", b"data: {nope"));
    let e = stream(&client(Limits::default()), &compat(&s.url()), &hello(), 0)
        .err
        .unwrap();
    assert!(e.to_string().contains("not the JSON"), "{e}");
}

/// akit's own (rule 8): a call's reads held while what it handed on waits, the API is held back
/// once the sockets' buffers fill, the idle timeout does not end a held call, and the answer
/// comes whole once the reads resume.
#[test]
fn held_reads() {
    let sent = Arc::new(AtomicUsize::new(0));
    let s2 = Arc::clone(&sent);
    let s = Server::start(move |_, c| {
        c.head(200, &[("Content-Type", "text/event-stream")]);
        // 32 MiB in 64 KiB events: far past what the sockets' buffers hold, macOS's too.
        let piece = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            "y".repeat(64 << 10)
        );
        for _ in 0..512 {
            if !c.chunk(piece.as_bytes()) {
                return;
            }
            s2.fetch_add(1, Ordering::SeqCst);
        }
        c.chunk(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        );
    });
    let c = client(Limits::default());
    let mut p = compat(&s.url());
    p.idle_timeout = Duration::from_millis(300);
    let (tx, rx) = mpsc::channel();
    let k = c.stream(
        &p,
        &hello(),
        64 << 20,
        |_| {},
        move |g| {
            let _ = tx.send(g);
        },
    );
    k.hold(true);
    std::thread::sleep(Duration::from_millis(800));
    // Held past the idle timeout: not ended, and the server is held back.
    assert!(rx.try_recv().is_err(), "a held call ended");
    let while_held = sent.load(Ordering::SeqCst);
    assert!(
        while_held < 480,
        "the server sent {while_held} of 512 pieces while held"
    );
    k.hold(false);
    let a = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!(a.text.len(), 512 * (64 << 10));
}

/// The stand-in's own helpers, as fit's `fakeapi.Get` and `Find`.
#[test]
fn standin_helpers() {
    let v = akit_config::value::parse(r#"{"a": [{"k": "x", "v": 1}, {"k": "y"}]}"#).unwrap();
    assert!(find(&v, "k", "y", &["a"]).is_some());
    assert_eq!(get(&v, &["a", "-1", "k"]), Some(&Value::String("y".into())));
}
