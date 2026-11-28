//! The assistant's client of model APIs (I161, fit's `assistant/model`): for each kind of API
//! (Anthropic's Messages, OpenAI's Chat Completions and Responses, Gemini's
//! `generateContent`, Azure OpenAI, Ollama's chat, any server that speaks Chat Completions, and
//! this akit's own `llm.proxy`) it builds the request from a system prompt, messages and tools,
//! sends it on akio-http's asynchronous client, and parses the streamed answer, SSE or Ollama's
//! NDJSON, as it arrives, into text, thinking and tool calls, with the tokens used.
//!
//! Nothing here blocks (rule 1): a call is started, its events and its end are handed to
//! callbacks on akio's I/O threads, and it is bounded throughout: the connect, the answer's head,
//! the silence between pieces and the whole call by timeouts, a line, an event and the whole
//! answer by sizes, the tool calls by number. Its reads can be held while what it hands on
//! waits to be taken ([`Call::hold`]), so that a slow reader slows the model rather than growing
//! a buffer (rule 8). A call is tried again only at once, only while none of its answer has been
//! taken, and only when its connection failed or closed before an answer: never after a wait
//! ([decision 044](../../../../docs/decisions/044-assistant.md), 6). A key is never written
//! anywhere but the request's own header: errors have it replaced, and [`Secret`] prints masked.

mod answer;
mod client;
mod codec;
mod frame;

pub use client::{Call, Client, Limits, MAX_TRANSPORTS};

use std::fmt;
use std::time::Duration;

use akit_config::assistant::ProviderKind;
use akit_config::value::Value;

pub use crate::settings::Secret;

/// The `anthropic-version` the calls send: fit's `AnthropicVersion`.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// A model API as a call needs it: where it is, how it is reached and with which key, and its
/// bounds.
#[derive(Debug, Clone)]
pub struct Provider {
    /// Names it in errors.
    pub name: String,
    /// The API it speaks; `llm_proxy` is called as `openai_compatible` is.
    pub kind: ProviderKind,
    /// The API's base, without a trailing slash, as `https://api.openai.com/v1`.
    pub base_url: String,
    /// `azure_openai`'s `api-version`.
    pub api_version: String,
    /// The API key; none when empty. Sent only over https or to a loopback host.
    pub key: Secret,
    /// Sent with every call.
    pub headers: Vec<(String, String)>,
    /// The SHA-256 of the only certificate an https base may present, in place of a CA's
    /// verification: `llm.proxy`'s own, on loopback.
    pub pin: Option<[u8; 32]>,
    /// A connect, TLS included.
    pub connect_timeout: Duration,
    /// The wait for the answer's head, and for the first piece of its body.
    pub first_byte_timeout: Duration,
    /// The silence between the pieces of the answer.
    pub idle_timeout: Duration,
    /// The whole call.
    pub timeout: Duration,
    /// How often a call is tried again, at once, when its connection failed or closed before
    /// any answer.
    pub retries: u32,
}

impl Provider {
    /// A provider of the settings' `p`, with `key`, at `base_url` (`p`'s own but for
    /// `llm_proxy`, whose address is llm.proxy's).
    #[must_use]
    pub fn of(p: &akit_config::assistant::Provider, key: Secret, base_url: &str) -> Provider {
        Provider {
            name: p.name.clone(),
            kind: p.kind,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_version: p.api_version.clone(),
            key,
            headers: p.headers.clone(),
            pin: None,
            connect_timeout: p.connect(),
            first_byte_timeout: p.first_byte(),
            idle_timeout: p.idle(),
            timeout: p.total(),
            retries: p.retry_count(),
        }
    }

    /// A provider of `kind` at `base_url`, with the defaults' bounds and no retries.
    #[must_use]
    pub fn new(name: &str, kind: ProviderKind, base_url: &str) -> Provider {
        let p = akit_config::assistant::Provider::new(name, kind);
        let mut q = Provider::of(&p, Secret::default(), base_url);
        q.retries = 0;
        q
    }
}

/// The roles of messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The user's words.
    User,
    /// The model's answer, with the tools it called.
    Assistant,
    /// A tool's result.
    Tool,
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Whose it is.
    pub role: Role,
    /// The text: the user's, the answer's, or the tool's result.
    pub content: String,
    /// An assistant message's thinking as Anthropic gave it, which goes back with the tool calls
    /// it led to, with its signature.
    pub thinking: String,
    /// See [`thinking`](Message::thinking).
    pub thinking_signature: String,
    /// The tools an assistant message called.
    pub tool_calls: Vec<ToolCall>,
    /// Which call a tool message answers.
    pub tool_call_id: String,
    /// The tool's name.
    pub tool_name: String,
    /// Whether the tool failed.
    pub is_error: bool,
}

impl Message {
    /// A message of `role` with `content`.
    #[must_use]
    pub fn new(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_owned(),
            thinking: String::new(),
            thinking_signature: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            tool_name: String::new(),
            is_error: false,
        }
    }

    /// A tool's result for the call `call`.
    #[must_use]
    pub fn tool(call: &ToolCall, content: &str, is_error: bool) -> Message {
        Message {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            is_error,
            ..Message::new(Role::Tool, content)
        }
    }
}

/// A call of a tool by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The API's ID for the call, or one made here when it gives none.
    pub id: String,
    /// The tool.
    pub name: String,
    /// A JSON object; `{}` for none. When the model wrote something that is not one, it is
    /// held as a JSON string and [`invalid`](ToolCall::invalid) is set, for the caller to hand
    /// back as an error.
    pub arguments: String,
    /// See [`arguments`](ToolCall::arguments).
    pub invalid: bool,
    /// Gemini's thought signature of the call, which goes back with it.
    pub signature: String,
}

/// A tool the model may call: its name, what it does, and a JSON Schema of its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// Its name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// The JSON Schema of its arguments, an object; none when empty.
    pub parameters: String,
}

/// The levels of thinking, as `params.thinking` writes them.
pub const THINKING_LEVELS: [&str; 4] = ["off", "low", "medium", "high"];

/// The budget of tokens the APIs that take one (Anthropic, Gemini) get for `level`.
#[must_use]
pub fn thinking_budget(level: &str) -> u64 {
    match level {
        "low" => 2048,
        "medium" => 8192,
        "high" => 24576,
        _ => 0,
    }
}

/// One call: the model, the system prompt, the conversation and the tools, and what the answer
/// may be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// The model.
    pub model: String,
    /// The system prompt; none when empty.
    pub system: String,
    /// The conversation.
    pub messages: Vec<Message>,
    /// The tools.
    pub tools: Vec<Tool>,
    /// The most tokens of the answer; 4096 when 0.
    pub max_tokens: u64,
    /// The sampling temperature as written; the model's own when `None`.
    pub temperature: Option<String>,
    /// One of [`THINKING_LEVELS`]; off when empty.
    pub thinking: String,
}

impl Request {
    pub(crate) fn max_tokens(&self) -> u64 {
        if self.max_tokens == 0 {
            4096
        } else {
            self.max_tokens
        }
    }

    pub(crate) fn thinks(&self) -> bool {
        !self.thinking.is_empty() && self.thinking != "off"
    }

    /// What the request may not hold: a tool's parameters, or a call's arguments, that are not
    /// JSON.
    pub(crate) fn check(&self) -> Result<(), String> {
        for t in &self.tools {
            if !t.parameters.is_empty() && json_value(&t.parameters).is_none() {
                return Err(format!(
                    "the parameters of the tool {:?} are not JSON",
                    t.name
                ));
            }
        }
        for m in &self.messages {
            for c in &m.tool_calls {
                if !c.arguments.is_empty() && json_value(&c.arguments).is_none() {
                    return Err(format!("the arguments of the call {:?} are not JSON", c.id));
                }
            }
        }
        Ok(())
    }
}

/// What an event of the answer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A piece of the answer's text.
    Text(String),
    /// A piece of its thinking.
    Thinking(String),
    /// A tool call, once its arguments have all arrived.
    ToolCall(ToolCall),
}

/// What a call took, as the API counts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Tokens in.
    pub input_tokens: u64,
    /// Tokens out.
    pub output_tokens: u64,
    /// The part of `output_tokens` spent thinking, where the API says.
    pub thinking_tokens: u64,
    /// The part of `input_tokens` read from the API's cache.
    pub cached_tokens: u64,
}

impl Usage {
    /// As fit's JSON writes it.
    #[must_use]
    pub fn json(&self) -> String {
        let mut o = crate::json::Obj::new()
            .num("input_tokens", self.input_tokens)
            .num("output_tokens", self.output_tokens);
        if self.thinking_tokens > 0 {
            o = o.num("thinking_tokens", self.thinking_tokens);
        }
        if self.cached_tokens > 0 {
            o = o.num("cached_tokens", self.cached_tokens);
        }
        o.end()
    }
}

/// A whole answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    /// The model the API says answered.
    pub model: String,
    /// Its text.
    pub text: String,
    /// Its thinking, and the signature Anthropic gave it.
    pub thinking: String,
    /// See [`thinking`](Answer::thinking).
    pub thinking_signature: String,
    /// The tools it called.
    pub tool_calls: Vec<ToolCall>,
    /// What it took.
    pub usage: Usage,
    /// Why it ended, in the API's words: stop, end_turn, tool_calls, length and the like.
    pub stop_reason: String,
    /// How many times the call was sent.
    pub attempts: u32,
}

impl Answer {
    /// The answer as the assistant message that goes back in the conversation.
    #[must_use]
    pub fn message(&self) -> Message {
        Message {
            thinking: self.thinking.clone(),
            thinking_signature: self.thinking_signature.clone(),
            tool_calls: self.tool_calls.clone(),
            ..Message::new(Role::Assistant, &self.text)
        }
    }
}

/// A model of a provider's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Its ID.
    pub id: String,
    /// Its context window in tokens, where the list says; 0 otherwise.
    pub context: u64,
}

impl ModelInfo {
    /// As fit's JSON writes it.
    #[must_use]
    pub fn json(&self) -> String {
        let o = crate::json::Obj::new().str("id", &self.id);
        if self.context > 0 {
            o.num("context", self.context).end()
        } else {
            o.end()
        }
    }
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The key would cross the network in the clear.
    KeyInClear,
    /// The API did not begin its answer in time.
    FirstByte,
    /// Its answer went silent.
    Idle,
    /// Its answer took longer than the call may.
    Timeout,
    /// Its answer is larger than the assistant takes: the whole of it, a line, an event, or the
    /// number of tool calls.
    TooLarge,
    /// Its answer ended before it said it was done.
    Incomplete,
    /// The client is closed.
    Closed,
    /// The caller cancelled the call, saying why.
    Cancelled(String),
    /// The API answered that the call failed: its status, its message (the key replaced), and
    /// how long it asked to be left alone (`Retry-After`), when it said.
    Api {
        /// The status; 200 for an error sent in the middle of an answer.
        status: u16,
        /// The API's message.
        message: String,
        /// Its `Retry-After`, as written.
        retry_after: String,
    },
    /// Anything else: the URL, the request, the connection, an event that does not parse.
    Other(String),
}

/// A call's failure, naming its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The provider's name.
    pub provider: String,
    /// What went wrong.
    pub kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(provider: &str, kind: ErrorKind) -> Error {
        Error {
            provider: provider.to_owned(),
            kind,
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::KeyInClear => f.write_str(
                "the key would cross the network in the clear: use an https base_url, or a \
                 loopback host",
            ),
            ErrorKind::FirstByte => f.write_str("the model API did not begin its answer in time"),
            ErrorKind::Idle => f.write_str("the model API's answer went silent"),
            ErrorKind::Timeout => {
                f.write_str("the model API's answer took longer than the call may")
            }
            ErrorKind::TooLarge => {
                f.write_str("the model API's answer is larger than the assistant takes")
            }
            ErrorKind::Incomplete => {
                f.write_str("the model API's answer ended before it was complete")
            }
            ErrorKind::Closed => f.write_str("the model client is closed"),
            ErrorKind::Cancelled(why) => f.write_str(why),
            ErrorKind::Api {
                status,
                message,
                retry_after,
            } => {
                write!(f, "HTTP {status}: {message}")?;
                if !retry_after.is_empty() {
                    write!(
                        f,
                        " (the API asks to be called again after {retry_after}; send the \
                         message again then)"
                    )?;
                }
                Ok(())
            }
            ErrorKind::Other(msg) => f.write_str(msg),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.provider, self.kind)
    }
}

impl std::error::Error for Error {}

/// Whether an answer of `status` is one the APIs send when they may take the call later: a
/// timeout, a rate limit or an overloaded server. akit fails it at once, saying so
/// (decision row 100); fit waited and tried again.
#[must_use]
pub fn retryable(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// `s` with `key` replaced by `***`.
#[must_use]
pub fn redact(s: &str, key: &Secret) -> String {
    let k = key.expose();
    if k.len() < 4 {
        return s.to_owned();
    }
    s.replace(k, "***")
}

/// `text` as a JSON value, when it is one.
pub(crate) fn json_value(text: &str) -> Option<Value> {
    akit_config::value::parse(text).ok()
}

/// The message of an API's error body: `{"error": {"message": ...}}`, `{"error": "..."}`,
/// `{"message": ...}` or `{"detail": ...}`, else the body itself, cut to 512 bytes. fit's
/// `apiMessage`.
#[must_use]
pub fn api_message(body: &[u8]) -> String {
    if let Some(v) = std::str::from_utf8(body).ok().and_then(json_value) {
        if let Value::Object(_) = &v {
            match v.get("error") {
                Some(Value::Object(_)) => {
                    if let Some(Value::String(m)) = v.get("error").and_then(|e| e.get("message")) {
                        if !m.is_empty() {
                            return m.clone();
                        }
                    }
                }
                Some(Value::String(s)) if !s.is_empty() => return s.clone(),
                _ => {}
            }
            for k in ["message", "detail"] {
                if let Some(Value::String(s)) = v.get(k) {
                    if !s.is_empty() {
                        return s.clone();
                    }
                }
            }
        }
    }
    let s = String::from_utf8_lossy(body);
    let s = s.trim();
    if s.is_empty() {
        return "no message".to_owned();
    }
    if s.len() > 512 {
        let mut end = 512;
        while !s.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        return format!("{}...", s.get(..end).unwrap_or(""));
    }
    s.to_owned()
}

#[cfg(test)]
mod tests;
