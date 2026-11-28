//! Each kind of API's call and the parse of its streamed answer, fit's codecs: the request's URL
//! and body, and each event of the answer read into a [`Gather`]. A codec keeps the state of one
//! answer's parse: each attempt has its own.

use std::collections::HashMap;

use akit_config::assistant::ProviderKind;
use akit_config::value::Value;

use super::answer::Gather;
use super::{ErrorKind, Message, ModelInfo, Provider, Request, Role, Tool, ToolCall, api_message};
use super::{json_value, thinking_budget};
use crate::json::{Obj, quoted};

/// A codec: which API, and what its parse of one answer has seen.
pub(crate) enum Codec {
    /// Chat Completions: openai, azure_openai, openai_compatible, llm_proxy; whether a choice gave
    /// its finish_reason.
    Chat(ProviderKind, bool),
    /// Anthropic's Messages: the kinds of the content blocks begun, by index.
    Anthropic(HashMap<i64, String>),
    /// OpenAI's Responses.
    Responses,
    /// Gemini's generateContent.
    Gemini,
    /// Ollama's /api/chat.
    Ollama,
}

impl Codec {
    pub(crate) fn of(kind: ProviderKind) -> Codec {
        match kind {
            ProviderKind::Anthropic => Codec::Anthropic(HashMap::new()),
            ProviderKind::Gemini => Codec::Gemini,
            ProviderKind::Ollama => Codec::Ollama,
            ProviderKind::OpenAiResponses => Codec::Responses,
            ProviderKind::LlmProxy => Codec::Chat(ProviderKind::OpenAiCompatible, false),
            k => Codec::Chat(k, false),
        }
    }

    /// Whether its answer is NDJSON, not SSE.
    pub(crate) fn ndjson(&self) -> bool {
        matches!(self, Codec::Ollama)
    }

    /// The call's URL and body.
    pub(crate) fn call(&self, p: &Provider, r: &Request) -> (String, Vec<u8>) {
        let (url, body) = match self {
            Codec::Chat(kind, _) => chat_call(*kind, p, r),
            Codec::Anthropic(_) => anthropic_call(p, r),
            Codec::Responses => responses_call(p, r),
            Codec::Gemini => gemini_call(p, r),
            Codec::Ollama => ollama_call(p, r),
        };
        (url, body.into_bytes())
    }

    /// Takes one event of the answer: an SSE event's name and data, or an NDJSON line with no
    /// name.
    pub(crate) fn parse(
        &mut self,
        a: &mut Gather,
        name: &str,
        data: &[u8],
    ) -> Result<(), ErrorKind> {
        match self {
            Codec::Chat(_, finished) => chat_parse(finished, a, data),
            Codec::Anthropic(blocks) => anthropic_parse(blocks, a, data),
            Codec::Responses => responses_parse(a, name, data),
            Codec::Gemini => gemini_parse(a, data),
            Codec::Ollama => ollama_parse(a, data),
        }
    }

    /// Takes the end of the answer's body.
    pub(crate) fn finish(&self, a: &mut Gather) -> Result<(), ErrorKind> {
        match self {
            Codec::Chat(_, finished) => {
                if !a.done && !finished {
                    return Err(ErrorKind::Incomplete);
                }
                a.end_all()
            }
            _ if !a.done => Err(ErrorKind::Incomplete),
            _ => Ok(()),
        }
    }
}

// ---- JSON helpers ----

fn arr(items: impl IntoIterator<Item = String>) -> String {
    let mut b = String::from("[");
    for (i, it) in items.into_iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str(&it);
    }
    b.push(']');
    b
}

fn get<'v>(v: &'v Value, k: &str) -> Option<&'v Value> {
    v.get(k).filter(|x| **x != Value::Null)
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    match v.get(k) {
        Some(Value::String(x)) => x,
        _ => "",
    }
}

fn u(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

fn b(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

fn list<'v>(v: &'v Value, k: &str) -> &'v [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn event(data: &[u8]) -> Result<Value, ErrorKind> {
    let text = std::str::from_utf8(data).map_err(|_| bad_event("not UTF-8"))?;
    match akit_config::value::parse(text) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err(bad_event("not an object")),
        Err(e) => Err(bad_event(&e.to_string())),
    }
}

fn bad_event(why: &str) -> ErrorKind {
    ErrorKind::Other(format!(
        "the model API sent an event that is not the JSON expected: {why}"
    ))
}

/// An error the API sent in the middle of its answer.
fn stream_error(data: &[u8]) -> ErrorKind {
    ErrorKind::Api {
        status: 200,
        message: api_message(data),
        retry_after: String::new(),
    }
}

/// A call's arguments as a JSON object: those of a call marked invalid as `{}`, since the APIs
/// that take an object refuse a string.
fn tool_input(tc: &ToolCall) -> &str {
    if tc.invalid || tc.arguments.is_empty() {
        "{}"
    } else {
        &tc.arguments
    }
}

/// A call's arguments as the text the model wrote: a call marked invalid has them as a JSON
/// string.
fn tool_args(tc: &ToolCall) -> String {
    if tc.invalid {
        if let Some(Value::String(s)) = json_value(&tc.arguments) {
            return s;
        }
    }
    if tc.arguments.is_empty() {
        "{}".to_owned()
    } else {
        tc.arguments.clone()
    }
}

fn schema_or_empty(t: &Tool) -> &str {
    if t.parameters.is_empty() {
        r#"{"type":"object","properties":{}}"#
    } else {
        &t.parameters
    }
}

/// `s` escaped as a URL path's segment, as Go's `url.PathEscape`.
pub(crate) fn path_escape(s: &str) -> String {
    escape(s, |c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b':'
                    | b'='
                    | b'@'
                    | b'!'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
            )
    })
}

/// `s` escaped as a query's value, as Go's `url.QueryEscape` (a space as `+`).
pub(crate) fn query_escape(s: &str) -> String {
    escape(s, |c| {
        c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~')
    })
    .replace("%20", "+")
}

fn escape(s: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.bytes() {
        if keep(c) {
            out.push(char::from(c));
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

fn temperature(o: Obj, r: &Request) -> Obj {
    match &r.temperature {
        Some(t) => o.raw("temperature", t),
        None => o,
    }
}

fn function_decl(t: &Tool, schema: bool) -> String {
    let mut o = Obj::new()
        .str("name", &t.name)
        .str_omit("description", &t.description);
    if schema {
        o = o.raw("parameters", schema_or_empty(t));
    } else if !t.parameters.is_empty() {
        o = o.raw("parameters", &t.parameters);
    }
    o.end()
}

// ---- Chat Completions: openai, azure_openai, openai_compatible, llm_proxy ----

fn chat_call(kind: ProviderKind, p: &Provider, r: &Request) -> (String, String) {
    let url = if kind == ProviderKind::AzureOpenAi {
        format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            p.base_url,
            path_escape(&r.model),
            query_escape(&p.api_version)
        )
    } else {
        format!("{}/chat/completions", p.base_url)
    };
    let mut msgs = Vec::with_capacity(r.messages.len().saturating_add(1));
    if !r.system.is_empty() {
        msgs.push(
            Obj::new()
                .str("role", "system")
                .str("content", &r.system)
                .end(),
        );
    }
    for m in &r.messages {
        msgs.push(match m.role {
            Role::Tool => Obj::new()
                .str("role", "tool")
                .str("content", &m.content)
                .str_omit("tool_call_id", &m.tool_call_id)
                .end(),
            Role::Assistant => {
                let mut o = Obj::new().str("role", "assistant");
                o = if m.content.is_empty() && !m.tool_calls.is_empty() {
                    o.raw("content", "null")
                } else {
                    o.str("content", &m.content)
                };
                if !m.tool_calls.is_empty() {
                    o = o.raw(
                        "tool_calls",
                        &arr(m.tool_calls.iter().map(|tc| {
                            Obj::new()
                                .str_omit("id", &tc.id)
                                .str("type", "function")
                                .raw(
                                    "function",
                                    &Obj::new()
                                        .str_omit("name", &tc.name)
                                        .str_omit("arguments", &tool_args(tc))
                                        .end(),
                                )
                                .end()
                        })),
                    );
                }
                o.end()
            }
            Role::User => Obj::new()
                .str("role", "user")
                .str("content", &m.content)
                .end(),
        });
    }
    let mut o = Obj::new()
        .raw("messages", &arr(msgs))
        .bool("stream", true)
        .raw("stream_options", r#"{"include_usage":true}"#);
    if kind != ProviderKind::AzureOpenAi {
        o = o.str("model", &r.model);
    }
    if matches!(kind, ProviderKind::OpenAi | ProviderKind::AzureOpenAi) {
        o = o.num("max_completion_tokens", r.max_tokens());
        if r.thinks() {
            o = o.str("reasoning_effort", &r.thinking);
        }
    } else {
        o = o.num("max_tokens", r.max_tokens());
    }
    o = temperature(o, r);
    if !r.tools.is_empty() {
        o = o.raw(
            "tools",
            &arr(r.tools.iter().map(|t| {
                Obj::new()
                    .str("type", "function")
                    .raw("function", &function_decl(t, false))
                    .end()
            })),
        );
    }
    (url, o.end())
}

fn chat_parse(finished: &mut bool, a: &mut Gather, data: &[u8]) -> Result<(), ErrorKind> {
    if data == b"[DONE]" {
        a.done = true;
        return a.end_all();
    }
    let v = event(data)?;
    if get(&v, "error").is_some() {
        return Err(stream_error(data));
    }
    let model = s(&v, "model");
    if !model.is_empty() {
        model.clone_into(&mut a.res.model);
    }
    for choice in list(&v, "choices") {
        let empty = Value::Object(Vec::new());
        let d = get(choice, "delta").unwrap_or(&empty);
        a.thinking(&format!(
            "{}{}",
            s(d, "reasoning_content"),
            s(d, "reasoning")
        ))?;
        a.text(s(d, "content"))?;
        for (i, tc) in list(d, "tool_calls").iter().enumerate() {
            let key = match tc.get("index") {
                Some(Value::Number(n)) => n.clone(),
                _ => i.to_string(),
            };
            let f = get(tc, "function").unwrap_or(&empty);
            let pc = a.begin(&key, s(tc, "id"), s(f, "name"))?;
            pc.args.push_str(s(f, "arguments"));
        }
        let reason = s(choice, "finish_reason");
        if !reason.is_empty() {
            reason.clone_into(&mut a.res.stop_reason);
            *finished = true;
            a.end_all()?;
        }
    }
    if let Some(us) = get(&v, "usage") {
        a.res.usage.input_tokens = u(us, "prompt_tokens");
        a.res.usage.output_tokens = u(us, "completion_tokens");
        if let Some(d) = get(us, "prompt_tokens_details") {
            a.res.usage.cached_tokens = u(d, "cached_tokens");
        }
        if let Some(d) = get(us, "completion_tokens_details") {
            a.res.usage.thinking_tokens = u(d, "reasoning_tokens");
        }
    }
    Ok(())
}

// ---- Anthropic's Messages ----

fn anthropic_call(p: &Provider, r: &Request) -> (String, String) {
    // The roles must alternate: one role's messages in a row merge, tool results go as the
    // user's.
    let mut msgs: Vec<(&'static str, Vec<String>)> = Vec::new();
    let mut add = |role: &'static str, blocks: Vec<String>| {
        if blocks.is_empty() {
            return;
        }
        match msgs.last_mut() {
            Some((r, bs)) if *r == role => bs.extend(blocks),
            _ => msgs.push((role, blocks)),
        }
    };
    for m in &r.messages {
        match m.role {
            Role::Tool => {
                let mut o = Obj::new()
                    .str("type", "tool_result")
                    .str("tool_use_id", &m.tool_call_id)
                    .str("content", &m.content);
                if m.is_error {
                    o = o.bool("is_error", true);
                }
                add("user", vec![o.end()]);
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                if !m.thinking.is_empty() && !m.thinking_signature.is_empty() {
                    blocks.push(
                        Obj::new()
                            .str("type", "thinking")
                            .str("thinking", &m.thinking)
                            .str("signature", &m.thinking_signature)
                            .end(),
                    );
                }
                if !m.content.is_empty() {
                    blocks.push(Obj::new().str("type", "text").str("text", &m.content).end());
                }
                for tc in &m.tool_calls {
                    blocks.push(
                        Obj::new()
                            .str("type", "tool_use")
                            .str("id", &tc.id)
                            .str("name", &tc.name)
                            .raw("input", tool_input(tc))
                            .end(),
                    );
                }
                add("assistant", blocks);
            }
            Role::User => {
                if !m.content.is_empty() {
                    add(
                        "user",
                        vec![Obj::new().str("type", "text").str("text", &m.content).end()],
                    );
                }
            }
        }
    }
    let msgs = arr(msgs.into_iter().map(|(role, blocks)| {
        Obj::new()
            .str("role", role)
            .raw("content", &arr(blocks))
            .end()
    }));
    let mut o = Obj::new().str("model", &r.model);
    if r.thinks() {
        // The budget is spent within max_tokens, which must exceed it; the API takes no
        // temperature with thinking.
        let bud = thinking_budget(&r.thinking);
        o = o.num("max_tokens", r.max_tokens().saturating_add(bud));
    } else {
        o = o.num("max_tokens", r.max_tokens());
    }
    o = o.raw("messages", &msgs).bool("stream", true);
    if !r.system.is_empty() {
        o = o.str("system", &r.system);
    }
    if r.thinks() {
        o = o.raw(
            "thinking",
            &Obj::new()
                .str("type", "enabled")
                .num("budget_tokens", thinking_budget(&r.thinking))
                .end(),
        );
    } else {
        o = temperature(o, r);
    }
    if !r.tools.is_empty() {
        o = o.raw(
            "tools",
            &arr(r.tools.iter().map(|t| {
                Obj::new()
                    .str("name", &t.name)
                    .str_omit("description", &t.description)
                    .raw("input_schema", schema_or_empty(t))
                    .end()
            })),
        );
    }
    (format!("{}/messages", p.base_url), o.end())
}

fn anthropic_parse(
    blocks: &mut HashMap<i64, String>,
    a: &mut Gather,
    data: &[u8],
) -> Result<(), ErrorKind> {
    let e = event(data)?;
    let index = match e.get("index") {
        Some(Value::Number(n)) => n.parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    let key = index.to_string();
    match s(&e, "type") {
        "message_start" => {
            if let Some(m) = get(&e, "message") {
                s(m, "model").clone_into(&mut a.res.model);
                if let Some(us) = get(m, "usage") {
                    let cached = u(us, "cache_read_input_tokens");
                    a.res.usage.input_tokens = u(us, "input_tokens")
                        .saturating_add(cached)
                        .saturating_add(u(us, "cache_creation_input_tokens"));
                    a.res.usage.cached_tokens = cached;
                    a.res.usage.output_tokens = u(us, "output_tokens");
                }
            }
        }
        "content_block_start" => {
            let Some(b) = get(&e, "content_block") else {
                return Err(bad_event("content_block_start without its block"));
            };
            let kind = s(b, "type");
            blocks.insert(index, kind.to_owned());
            match kind {
                "tool_use" => {
                    a.begin(&key, s(b, "id"), s(b, "name"))?;
                }
                "text" => return a.text(s(b, "text")),
                _ => {}
            }
        }
        "content_block_delta" => {
            let Some(d) = get(&e, "delta") else {
                return Err(bad_event("content_block_delta without its delta"));
            };
            match s(d, "type") {
                "text_delta" => return a.text(s(d, "text")),
                "thinking_delta" => return a.thinking(s(d, "thinking")),
                "signature_delta" => a.res.thinking_signature.push_str(s(d, "signature")),
                "input_json_delta" => {
                    let pc = a.begin(&key, "", "")?;
                    pc.args.push_str(s(d, "partial_json"));
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            if blocks.get(&index).map(String::as_str) == Some("tool_use") {
                return a.end(&key);
            }
        }
        "message_delta" => {
            if let Some(d) = get(&e, "delta") {
                let r = s(d, "stop_reason");
                if !r.is_empty() {
                    r.clone_into(&mut a.res.stop_reason);
                }
            }
            if let Some(us) = get(&e, "usage") {
                a.res.usage.output_tokens = u(us, "output_tokens");
            }
        }
        "message_stop" => {
            a.done = true;
            return a.end_all();
        }
        "error" => return Err(stream_error(data)),
        _ => {}
    }
    Ok(())
}

// ---- OpenAI's Responses ----

fn responses_call(p: &Provider, r: &Request) -> (String, String) {
    let mut input = Vec::with_capacity(r.messages.len());
    for m in &r.messages {
        match m.role {
            Role::Tool => input.push(
                Obj::new()
                    .str("type", "function_call_output")
                    .str("call_id", &m.tool_call_id)
                    .str("output", &m.content)
                    .end(),
            ),
            Role::Assistant => {
                if !m.content.is_empty() {
                    input.push(
                        Obj::new()
                            .str("role", "assistant")
                            .str("content", &m.content)
                            .end(),
                    );
                }
                for tc in &m.tool_calls {
                    input.push(
                        Obj::new()
                            .str("type", "function_call")
                            .str("call_id", &tc.id)
                            .str("name", &tc.name)
                            .str("arguments", &tool_args(tc))
                            .end(),
                    );
                }
            }
            Role::User => input.push(
                Obj::new()
                    .str("role", "user")
                    .str("content", &m.content)
                    .end(),
            ),
        }
    }
    // store false: the conversation is akit's to keep, not the API's.
    let mut o = Obj::new()
        .str("model", &r.model)
        .raw("input", &arr(input))
        .bool("stream", true)
        .bool("store", false)
        .num("max_output_tokens", r.max_tokens());
    if !r.system.is_empty() {
        o = o.str("instructions", &r.system);
    }
    o = temperature(o, r);
    if r.thinks() {
        o = o.raw(
            "reasoning",
            &Obj::new()
                .str("effort", &r.thinking)
                .str("summary", "auto")
                .end(),
        );
    }
    if !r.tools.is_empty() {
        o = o.raw(
            "tools",
            &arr(r.tools.iter().map(|t| {
                Obj::new()
                    .str("type", "function")
                    .str("name", &t.name)
                    .str("description", &t.description)
                    .raw("parameters", schema_or_empty(t))
                    .end()
            })),
        );
    }
    (format!("{}/responses", p.base_url), o.end())
}

fn responses_parse(a: &mut Gather, name: &str, data: &[u8]) -> Result<(), ErrorKind> {
    let e = event(data)?;
    let mut typ = s(&e, "type");
    if typ.is_empty() {
        typ = name;
    }
    match typ {
        "response.created" | "response.in_progress" => {
            if let Some(r) = get(&e, "response") {
                let m = s(r, "model");
                if !m.is_empty() {
                    m.clone_into(&mut a.res.model);
                }
            }
        }
        "response.output_text.delta" | "response.refusal.delta" => {
            return a.text(s(&e, "delta"));
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            return a.thinking(s(&e, "delta"));
        }
        "response.output_item.added" => {
            if let Some(it) = get(&e, "item").filter(|it| s(it, "type") == "function_call") {
                let pc = a.begin(s(it, "id"), s(it, "call_id"), s(it, "name"))?;
                pc.args.push_str(s(it, "arguments"));
            }
        }
        "response.function_call_arguments.delta" => {
            let pc = a.begin(s(&e, "item_id"), "", "")?;
            pc.args.push_str(s(&e, "delta"));
        }
        "response.output_item.done" => {
            if let Some(it) = get(&e, "item").filter(|it| s(it, "type") == "function_call") {
                let pc = a.begin(s(it, "id"), s(it, "call_id"), s(it, "name"))?;
                let args = s(it, "arguments");
                if !args.is_empty() {
                    args.clone_into(&mut pc.args);
                }
                return a.end(s(it, "id"));
            }
        }
        "response.completed" | "response.incomplete" => {
            a.done = true;
            if let Some(r) = get(&e, "response") {
                s(r, "status").clone_into(&mut a.res.stop_reason);
                if let Some(d) = get(r, "incomplete_details") {
                    let why = s(d, "reason");
                    if !why.is_empty() {
                        why.clone_into(&mut a.res.stop_reason);
                    }
                }
                let m = s(r, "model");
                if !m.is_empty() {
                    m.clone_into(&mut a.res.model);
                }
                if let Some(us) = get(r, "usage") {
                    a.res.usage.input_tokens = u(us, "input_tokens");
                    a.res.usage.output_tokens = u(us, "output_tokens");
                    if let Some(d) = get(us, "input_tokens_details") {
                        a.res.usage.cached_tokens = u(d, "cached_tokens");
                    }
                    if let Some(d) = get(us, "output_tokens_details") {
                        a.res.usage.thinking_tokens = u(d, "reasoning_tokens");
                    }
                }
            }
            return a.end_all();
        }
        "response.failed" => {
            if let Some(err) = get(&e, "response").and_then(|r| get(r, "error")) {
                let mut b = String::from("{\"error\":");
                crate::json::write(&mut b, err);
                b.push('}');
                return Err(stream_error(b.as_bytes()));
            }
            return Err(stream_error(data));
        }
        "error" => return Err(stream_error(data)),
        _ => {}
    }
    Ok(())
}

// ---- Gemini's generateContent ----

fn gemini_call(p: &Provider, r: &Request) -> (String, String) {
    let mut contents: Vec<(&'static str, Vec<String>)> = Vec::new();
    let mut add = |role: &'static str, parts: Vec<String>| {
        if parts.is_empty() {
            return;
        }
        match contents.last_mut() {
            Some((r, ps)) if *r == role => ps.extend(parts),
            _ => contents.push((role, parts)),
        }
    };
    for m in &r.messages {
        match m.role {
            Role::Tool => add(
                "user",
                vec![
                    Obj::new()
                        .raw(
                            "functionResponse",
                            &Obj::new()
                                .str("name", &m.tool_name)
                                .raw("response", &tool_response(m))
                                .end(),
                        )
                        .end(),
                ],
            ),
            Role::Assistant => {
                let mut parts = Vec::new();
                if !m.content.is_empty() {
                    parts.push(Obj::new().str("text", &m.content).end());
                }
                for tc in &m.tool_calls {
                    let mut o = Obj::new().raw(
                        "functionCall",
                        &Obj::new()
                            .str("name", &tc.name)
                            .raw("args", tool_input(tc))
                            .end(),
                    );
                    if !tc.signature.is_empty() {
                        o = o.str("thoughtSignature", &tc.signature);
                    }
                    parts.push(o.end());
                }
                add("model", parts);
            }
            Role::User => {
                if !m.content.is_empty() {
                    add("user", vec![Obj::new().str("text", &m.content).end()]);
                }
            }
        }
    }
    let contents = arr(contents
        .into_iter()
        .map(|(role, parts)| Obj::new().str("role", role).raw("parts", &arr(parts)).end()));
    let mut generation = Obj::new().num("maxOutputTokens", r.max_tokens());
    generation = temperature(generation, r);
    if r.thinks() {
        generation = generation.raw(
            "thinkingConfig",
            &Obj::new()
                .num("thinkingBudget", thinking_budget(&r.thinking))
                .bool("includeThoughts", true)
                .end(),
        );
    }
    let mut o = Obj::new()
        .raw("contents", &contents)
        .raw("generationConfig", &generation.end());
    if !r.system.is_empty() {
        o = o.raw(
            "systemInstruction",
            &format!(r#"{{"parts":[{{"text":{}}}]}}"#, quoted(&r.system)),
        );
    }
    if !r.tools.is_empty() {
        let decls = arr(r.tools.iter().map(|t| {
            Obj::new()
                .str("name", &t.name)
                .str("description", &t.description)
                .raw("parametersJsonSchema", schema_or_empty(t))
                .end()
        }));
        o = o.raw("tools", &format!(r#"[{{"functionDeclarations":{decls}}}]"#));
    }
    (
        format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            p.base_url,
            path_escape(&r.model)
        ),
        o.end(),
    )
}

/// A tool's result as Gemini takes it, an object: the result itself when it is one, else
/// `{"result": ...}`, or `{"error": ...}` for a tool that failed.
fn tool_response(m: &Message) -> String {
    if m.is_error {
        return Obj::new().str("error", &m.content).end();
    }
    if let Some(v @ Value::Object(_)) = json_value(&m.content) {
        return crate::json::compact(&v);
    }
    Obj::new().str("result", &m.content).end()
}

fn gemini_parse(a: &mut Gather, data: &[u8]) -> Result<(), ErrorKind> {
    let v = event(data)?;
    if v.get("error").is_some() {
        return Err(stream_error(data));
    }
    let m = s(&v, "modelVersion");
    if !m.is_empty() {
        m.clone_into(&mut a.res.model);
    }
    let empty = Value::Object(Vec::new());
    for cand in list(&v, "candidates") {
        let content = get(cand, "content").unwrap_or(&empty);
        for part in list(content, "parts") {
            if let Some(fc) = get(part, "functionCall") {
                let args = fc.get("args").map(crate::json::compact).unwrap_or_default();
                a.call(ToolCall {
                    id: s(fc, "id").to_owned(),
                    name: s(fc, "name").to_owned(),
                    arguments: args,
                    invalid: false,
                    signature: s(part, "thoughtSignature").to_owned(),
                })?;
            } else if b(part, "thought") {
                a.thinking(s(part, "text"))?;
            } else {
                a.text(s(part, "text"))?;
            }
        }
        let r = s(cand, "finishReason");
        if !r.is_empty() {
            r.clone_into(&mut a.res.stop_reason);
            a.done = true;
        }
    }
    if let Some(us) = get(&v, "usageMetadata") {
        let thoughts = u(us, "thoughtsTokenCount");
        a.res.usage = super::Usage {
            input_tokens: u(us, "promptTokenCount"),
            output_tokens: u(us, "candidatesTokenCount").saturating_add(thoughts),
            thinking_tokens: thoughts,
            cached_tokens: u(us, "cachedContentTokenCount"),
        };
    }
    Ok(())
}

// ---- Ollama's /api/chat ----

fn ollama_call(p: &Provider, r: &Request) -> (String, String) {
    let mut msgs = Vec::with_capacity(r.messages.len().saturating_add(1));
    if !r.system.is_empty() {
        msgs.push(
            Obj::new()
                .str("role", "system")
                .str("content", &r.system)
                .end(),
        );
    }
    for m in &r.messages {
        msgs.push(match m.role {
            Role::Tool => Obj::new()
                .str("role", "tool")
                .str("content", &m.content)
                .str_omit("tool_name", &m.tool_name)
                .end(),
            Role::Assistant => {
                let mut o = Obj::new()
                    .str("role", "assistant")
                    .str("content", &m.content);
                if !m.tool_calls.is_empty() {
                    o = o.raw(
                        "tool_calls",
                        &arr(m.tool_calls.iter().map(|tc| {
                            format!(
                                r#"{{"function":{}}}"#,
                                Obj::new()
                                    .str("name", &tc.name)
                                    .raw("arguments", tool_input(tc))
                                    .end()
                            )
                        })),
                    );
                }
                o.end()
            }
            Role::User => Obj::new()
                .str("role", "user")
                .str("content", &m.content)
                .end(),
        });
    }
    let opts = temperature(Obj::new().num("num_predict", r.max_tokens()), r).end();
    let mut o = Obj::new()
        .str("model", &r.model)
        .raw("messages", &arr(msgs))
        .bool("stream", true)
        .raw("options", &opts);
    if r.thinks() {
        o = o.bool("think", true);
    }
    if !r.tools.is_empty() {
        o = o.raw(
            "tools",
            &arr(r.tools.iter().map(|t| {
                Obj::new()
                    .str("type", "function")
                    .raw("function", &function_decl(t, true))
                    .end()
            })),
        );
    }
    (format!("{}/api/chat", p.base_url), o.end())
}

fn ollama_parse(a: &mut Gather, data: &[u8]) -> Result<(), ErrorKind> {
    let l = event(data)?;
    if !s(&l, "error").is_empty() {
        return Err(stream_error(data));
    }
    let m = s(&l, "model");
    if !m.is_empty() {
        m.clone_into(&mut a.res.model);
    }
    let empty = Value::Object(Vec::new());
    let msg = get(&l, "message").unwrap_or(&empty);
    a.thinking(s(msg, "thinking"))?;
    a.text(s(msg, "content"))?;
    for tc in list(msg, "tool_calls") {
        let f = get(tc, "function").unwrap_or(&empty);
        let args = f
            .get("arguments")
            .map(crate::json::compact)
            .unwrap_or_default();
        a.call(ToolCall {
            id: String::new(),
            name: s(f, "name").to_owned(),
            arguments: args,
            invalid: false,
            signature: String::new(),
        })?;
    }
    if b(&l, "done") {
        a.done = true;
        s(&l, "done_reason").clone_into(&mut a.res.stop_reason);
        a.res.usage.input_tokens = u(&l, "prompt_eval_count");
        a.res.usage.output_tokens = u(&l, "eval_count");
    }
    Ok(())
}

// ---- Model lists ----

/// The URL of `p`'s model list: GET `<base>/models` (openai, openai_responses,
/// openai_compatible, llm_proxy, anthropic, gemini), `<base>/openai/models?api-version=`
/// (azure_openai) or `<base>/api/tags` (ollama).
pub(crate) fn models_url(p: &Provider) -> String {
    match p.kind {
        ProviderKind::AzureOpenAi => format!(
            "{}/openai/models?api-version={}",
            p.base_url,
            query_escape(&p.api_version)
        ),
        ProviderKind::Ollama => format!("{}/api/tags", p.base_url),
        ProviderKind::Anthropic => format!("{}/models?limit=1000", p.base_url),
        ProviderKind::Gemini => format!("{}/models?pageSize=1000", p.base_url),
        _ => format!("{}/models", p.base_url),
    }
}

/// Reads a model list of `kind`.
pub(crate) fn parse_models(kind: ProviderKind, body: &[u8]) -> Result<Vec<ModelInfo>, String> {
    let bad = || "the model list is not the JSON expected".to_owned();
    let text = std::str::from_utf8(body).map_err(|_| bad())?;
    let v = akit_config::value::parse(text).map_err(|_| bad())?;
    if !matches!(v, Value::Object(_)) {
        return Err(bad());
    }
    let mut out = Vec::new();
    for d in list(&v, "data") {
        let id = s(d, "id");
        if !id.is_empty() {
            out.push(ModelInfo {
                id: id.to_owned(),
                context: 0,
            });
        }
    }
    for m in list(&v, "models") {
        let mut id = s(m, "name");
        if id.is_empty() {
            id = s(m, "model");
        }
        if kind == ProviderKind::Gemini {
            let methods = list(m, "supportedGenerationMethods");
            if !methods.is_empty()
                && !methods
                    .iter()
                    .any(|x| matches!(x, Value::String(s) if s == "generateContent"))
            {
                continue;
            }
            id = id.strip_prefix("models/").unwrap_or(id);
        }
        if !id.is_empty() {
            out.push(ModelInfo {
                id: id.to_owned(),
                context: u(m, "inputTokenLimit"),
            });
        }
    }
    Ok(out)
}
