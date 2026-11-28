//! fit's assistant in the config (I160, fit's `config/assistant.go`): `admin.assistant`, the
//! console's assistant, its model APIs, which model it uses, its bounds and who may do what with
//! it. Read with fit's fields, defaults, bounds and checks; what is left out is kept zero, and the
//! methods give fit's defaults for it. Its keys are not here: the console's Settings page keeps
//! them in the store, or `api_key_env` names a variable that holds one
//! ([decision 044](../../../docs/decisions/044-assistant.md)).
//!
//! The Settings page saves the settings whole into the store's section `assistant`, from which
//! [`load`] reads them again, its errors at their paths under `admin.assistant` as the config's.

use std::net::IpAddr;
use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{Error, Errors, join_field, join_index};
use crate::json::{Kind, Node};
use crate::model::{Console, Network};
use crate::value::write_string;

/// A provider's `connect_timeout` when left out: fit's `DefaultAssistantConnectTimeout`.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A provider's `first_byte_timeout` when left out.
pub const DEFAULT_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(120);
/// A provider's `idle_timeout` when left out.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A provider's `timeout` when left out.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// A provider's `retries` when left out.
pub const DEFAULT_RETRIES: u32 = 2;
/// The most `retries`.
pub const MAX_RETRIES: i64 = 5;
/// `params.max_output_tokens` when left out.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 4096;
/// `limits.max_turns` when left out.
pub const DEFAULT_MAX_TURNS: u64 = 40;
/// `limits.max_tool_calls` when left out.
pub const DEFAULT_MAX_TOOL_CALLS: u64 = 16;
/// `limits.max_tool_result_bytes` when left out: 32 KiB.
pub const DEFAULT_MAX_TOOL_RESULT_BYTES: u64 = 32 << 10;
/// `limits.max_conversations` when left out.
pub const DEFAULT_MAX_CONVERSATIONS: u64 = 50;
/// `limits.max_concurrent_chats` when left out.
pub const DEFAULT_MAX_CONCURRENT_CHATS: u64 = 4;
/// `limits.max_tokens_per_day` when left out.
pub const DEFAULT_MAX_TOKENS_PER_DAY: u64 = 1_000_000;
/// `limits.max_stream_bytes` when left out: 8 MiB.
pub const DEFAULT_MAX_STREAM_BYTES: u64 = 8 << 20;
/// `limits.max_audit_entries` when left out.
pub const DEFAULT_MAX_AUDIT_ENTRIES: u64 = 1000;
/// The most `providers`: fit's `MaxAssistantProviders`.
pub const MAX_PROVIDERS: usize = 32;
/// The most models a provider lists.
pub const MAX_MODELS: usize = 256;
/// The most `headers` a provider sends.
pub const MAX_HEADERS: usize = 16;
/// The most `data.redact` patterns.
pub const MAX_REDACT: usize = 32;
/// `mcp.max_tokens_per_user` when left out.
pub const DEFAULT_MCP_TOKENS_PER_USER: u64 = 10;
/// `mcp.max_token_days` when left out.
pub const DEFAULT_MCP_TOKEN_DAYS: u64 = 90;
/// The most `mcp.max_token_days`.
pub const MAX_MCP_TOKEN_DAYS: i64 = 3650;
/// `mcp.max_sessions` when left out.
pub const DEFAULT_MCP_SESSIONS: u64 = 4;
/// `mcp.session_idle_timeout` when left out.
pub const DEFAULT_MCP_SESSION_IDLE: Duration = Duration::from_secs(3600);
/// `mcp.max_request_bytes` when left out: 1 MiB.
pub const DEFAULT_MCP_REQUEST_BYTES: u64 = 1 << 20;
/// The most `mcp.max_request_bytes`: 8 MiB.
pub const MAX_MCP_REQUEST_BYTES: i64 = 8 << 20;
/// `mcp.max_concurrent_calls` when left out.
pub const DEFAULT_MCP_CONCURRENT_CALLS: u64 = 4;
/// The most `mcp.max_concurrent_calls`.
pub const MAX_MCP_CONCURRENT_CALLS: i64 = 64;
/// `mcp.max_calls_per_minute` when left out.
pub const DEFAULT_MCP_CALLS_PER_MINUTE: u64 = 120;
/// The most `mcp.max_calls_per_minute`.
pub const MAX_MCP_CALLS_PER_MINUTE: i64 = 6000;
/// The most `mcp.allow` entries.
pub const MAX_MCP_ALLOW: usize = 64;

/// The kinds of model API the assistant calls, fit's eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// Anthropic's Messages API.
    Anthropic,
    /// OpenAI's Chat Completions.
    OpenAi,
    /// OpenAI's Responses API.
    OpenAiResponses,
    /// Gemini's `generateContent`.
    Gemini,
    /// The Chat Completions of an Azure OpenAI deployment.
    AzureOpenAi,
    /// Ollama's `/api/chat`.
    Ollama,
    /// Chat Completions at any `base_url` (vLLM, llama.cpp, DeepSeek, a gateway).
    OpenAiCompatible,
    /// This akit's own `llm.proxy`, on its API's address.
    LlmProxy,
}

impl ProviderKind {
    /// Every kind, in the schema's order.
    pub const ALL: [ProviderKind; 8] = [
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::OpenAiResponses,
        ProviderKind::Gemini,
        ProviderKind::AzureOpenAi,
        ProviderKind::Ollama,
        ProviderKind::OpenAiCompatible,
        ProviderKind::LlmProxy,
    ];

    /// The kind as the config writes it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::OpenAiResponses => "openai_responses",
            ProviderKind::Gemini => "gemini",
            ProviderKind::AzureOpenAi => "azure_openai",
            ProviderKind::Ollama => "ollama",
            ProviderKind::OpenAiCompatible => "openai_compatible",
            ProviderKind::LlmProxy => "llm_proxy",
        }
    }

    /// The kind written `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<ProviderKind> {
        ProviderKind::ALL.into_iter().find(|k| k.name() == s)
    }

    /// The vendor's base URL, for the kinds that have one; empty for the others.
    #[must_use]
    pub fn default_url(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "https://api.anthropic.com/v1",
            ProviderKind::OpenAi | ProviderKind::OpenAiResponses => "https://api.openai.com/v1",
            ProviderKind::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            ProviderKind::Ollama => "http://127.0.0.1:11434",
            _ => "",
        }
    }
}

/// What a console user may have the assistant do, each level including the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Level {
    /// Nothing: no panel.
    Off,
    /// The read tools only.
    #[default]
    Read,
    /// Also drafts on the open page, never submitted.
    Edit,
    /// Also proposals.
    Propose,
    /// Also applies them.
    Apply,
}

impl Level {
    /// Every level, lowest first.
    pub const ALL: [Level; 5] = [
        Level::Off,
        Level::Read,
        Level::Edit,
        Level::Propose,
        Level::Apply,
    ];

    /// The level as the config writes it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Read => "read",
            Level::Edit => "edit",
            Level::Propose => "propose",
            Level::Apply => "apply",
        }
    }

    /// The level written `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.name() == s)
    }
}

/// `admin.assistant`, fit's `config.Assistant`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assistant {
    /// `providers`: the model APIs the assistant may call, by name.
    pub providers: Vec<Provider>,
    /// `default_model`: the provider and model the chat uses.
    pub default_model: Option<ModelRef>,
    /// `params`: what a model call asks for.
    pub params: Params,
    /// `limits`: the assistant's bounds.
    pub limits: Limits,
    /// `data`: what may be sent to a model.
    pub data: Data,
    /// `users`: each console user's level, by name.
    pub users: Vec<User>,
    /// `default_level`: the level of a console user `users` does not list; `None` is fit's
    /// default, `read`.
    pub default_level: Option<Level>,
    /// `mcp`: the MCP server; off when `None`.
    pub mcp: Option<Mcp>,
    /// The member as JSON, as it was written (comments left out): what the Settings page edits.
    pub raw: String,
}

impl Assistant {
    /// The provider named `name`.
    #[must_use]
    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// The provider and model the chat uses: `default_model`, or the first model of the first
    /// provider that lists one; empty when there is none.
    #[must_use]
    pub fn default_model(&self) -> (&str, &str) {
        if let Some(r) = &self.default_model {
            return (&r.provider, &r.model);
        }
        self.providers
            .iter()
            .find_map(|p| p.models.first().map(|m| (p.name.as_str(), m.name.as_str())))
            .unwrap_or(("", ""))
    }

    /// What the console user `user` may have the assistant do.
    #[must_use]
    pub fn level(&self, user: &str) -> Level {
        self.users
            .iter()
            .find(|u| u.username == user)
            .map(|u| u.level)
            .or(self.default_level)
            .unwrap_or_default()
    }
}

/// A model API the assistant calls, fit's `config.AssistantProvider`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// `name`: letters, digits, '_', '.' and '-'.
    pub name: String,
    /// `kind`: the API it speaks.
    pub kind: ProviderKind,
    /// `base_url`: the API's base URL; the vendor's when empty. Not for `llm_proxy`.
    pub base_url: String,
    /// `api_version`: an `azure_openai` provider's `api-version`.
    pub api_version: String,
    /// `api_key_env`: a variable holding the key, read when a call is made.
    pub api_key_env: String,
    /// `headers`: sent with every call, in the order written.
    pub headers: Vec<(String, String)>,
    /// `models`: the models offered.
    pub models: Vec<Model>,
    /// `connect_timeout`; zero is [`DEFAULT_CONNECT_TIMEOUT`].
    pub connect_timeout: Duration,
    /// `first_byte_timeout`; zero is [`DEFAULT_FIRST_BYTE_TIMEOUT`].
    pub first_byte_timeout: Duration,
    /// `idle_timeout`; zero is [`DEFAULT_IDLE_TIMEOUT`].
    pub idle_timeout: Duration,
    /// `timeout`; zero is [`DEFAULT_TIMEOUT`].
    pub timeout: Duration,
    /// `retries`; `None` is [`DEFAULT_RETRIES`], 0 tries once.
    pub retries: Option<u32>,
}

impl Provider {
    /// A provider of `kind` named `name`, everything else left out.
    #[must_use]
    pub fn new(name: &str, kind: ProviderKind) -> Provider {
        Provider {
            name: name.to_owned(),
            kind,
            base_url: String::new(),
            api_version: String::new(),
            api_key_env: String::new(),
            headers: Vec::new(),
            models: Vec::new(),
            connect_timeout: Duration::ZERO,
            first_byte_timeout: Duration::ZERO,
            idle_timeout: Duration::ZERO,
            timeout: Duration::ZERO,
            retries: None,
        }
    }

    /// The base URL without a trailing slash, the vendor's when left out; empty for
    /// `llm_proxy`, whose address is `llm.proxy`'s.
    #[must_use]
    pub fn url(&self) -> &str {
        if self.base_url.is_empty() {
            self.kind.default_url()
        } else {
            self.base_url.trim_end_matches('/')
        }
    }

    /// `connect_timeout`, its default when left out.
    #[must_use]
    pub fn connect(&self) -> Duration {
        or(self.connect_timeout, DEFAULT_CONNECT_TIMEOUT)
    }

    /// `first_byte_timeout`, its default when left out.
    #[must_use]
    pub fn first_byte(&self) -> Duration {
        or(self.first_byte_timeout, DEFAULT_FIRST_BYTE_TIMEOUT)
    }

    /// `idle_timeout`, its default when left out.
    #[must_use]
    pub fn idle(&self) -> Duration {
        or(self.idle_timeout, DEFAULT_IDLE_TIMEOUT)
    }

    /// `timeout`, its default when left out.
    #[must_use]
    pub fn total(&self) -> Duration {
        or(self.timeout, DEFAULT_TIMEOUT)
    }

    /// `retries`, its default when left out.
    #[must_use]
    pub fn retry_count(&self) -> u32 {
        self.retries.unwrap_or(DEFAULT_RETRIES)
    }
}

/// A model a provider offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// `name`: the model's ID.
    pub name: String,
    /// `context`: its window in tokens; 0 when not known.
    pub context: u64,
    /// `tools`: whether it calls tools; `None` is yes.
    pub tools: Option<bool>,
}

impl Model {
    /// Whether the model calls tools: so unless it says not.
    #[must_use]
    pub fn calls_tools(&self) -> bool {
        self.tools.unwrap_or(true)
    }
}

/// `default_model`: a provider's model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    /// `provider`.
    pub provider: String,
    /// `model`.
    pub model: String,
}

/// `params`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params {
    /// `max_output_tokens`; 0 is [`DEFAULT_MAX_OUTPUT_TOKENS`].
    pub max_output_tokens: u64,
    /// `temperature`, 0 to 2, as written; the model's own when left out.
    pub temperature: Option<String>,
    /// `thinking`: `off`, `low`, `medium` or `high`; empty is `off`.
    pub thinking: String,
}

impl Params {
    /// `max_output_tokens`, its default when left out.
    #[must_use]
    pub fn max_output(&self) -> u64 {
        nonzero(self.max_output_tokens, DEFAULT_MAX_OUTPUT_TOKENS)
    }

    /// The thinking asked for, `off` when left out.
    #[must_use]
    pub fn thinking_level(&self) -> &str {
        if self.thinking.is_empty() {
            "off"
        } else {
            &self.thinking
        }
    }
}

/// `limits`: each 0 when left out, its default then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    /// `max_turns`.
    pub max_turns: u64,
    /// `max_tool_calls`.
    pub max_tool_calls: u64,
    /// `max_tool_result_bytes`.
    pub max_tool_result_bytes: u64,
    /// `max_conversations`.
    pub max_conversations: u64,
    /// `max_concurrent_chats`.
    pub max_concurrent_chats: u64,
    /// `max_tokens_per_day`.
    pub max_tokens_per_day: u64,
    /// `max_stream_bytes`.
    pub max_stream_bytes: u64,
    /// `max_audit_entries`.
    pub max_audit_entries: u64,
}

impl Limits {
    /// `max_turns`, its default when left out.
    #[must_use]
    pub fn turns(&self) -> u64 {
        nonzero(self.max_turns, DEFAULT_MAX_TURNS)
    }
    /// `max_tool_calls`, its default when left out.
    #[must_use]
    pub fn tool_calls(&self) -> u64 {
        nonzero(self.max_tool_calls, DEFAULT_MAX_TOOL_CALLS)
    }
    /// `max_tool_result_bytes`, its default when left out.
    #[must_use]
    pub fn tool_result_bytes(&self) -> u64 {
        nonzero(self.max_tool_result_bytes, DEFAULT_MAX_TOOL_RESULT_BYTES)
    }
    /// `max_conversations`, its default when left out.
    #[must_use]
    pub fn conversations(&self) -> u64 {
        nonzero(self.max_conversations, DEFAULT_MAX_CONVERSATIONS)
    }
    /// `max_concurrent_chats`, its default when left out.
    #[must_use]
    pub fn concurrent_chats(&self) -> u64 {
        nonzero(self.max_concurrent_chats, DEFAULT_MAX_CONCURRENT_CHATS)
    }
    /// `max_tokens_per_day`, its default when left out.
    #[must_use]
    pub fn tokens_per_day(&self) -> u64 {
        nonzero(self.max_tokens_per_day, DEFAULT_MAX_TOKENS_PER_DAY)
    }
    /// `max_stream_bytes`, its default when left out.
    #[must_use]
    pub fn stream_bytes(&self) -> u64 {
        nonzero(self.max_stream_bytes, DEFAULT_MAX_STREAM_BYTES)
    }
    /// `max_audit_entries`, its default when left out.
    #[must_use]
    pub fn audit_entries(&self) -> u64 {
        nonzero(self.max_audit_entries, DEFAULT_MAX_AUDIT_ENTRIES)
    }
}

/// `data`: what may be sent to a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Data {
    /// `send_logs`.
    pub send_logs: bool,
    /// `send_access_log`.
    pub send_access_log: bool,
    /// `send_subjects`.
    pub send_subjects: bool,
    /// `send_records`.
    pub send_records: bool,
    /// `redact`: more patterns redacted, Go's syntax.
    pub redact: Vec<String>,
}

/// A console user's level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// `username`: a console user's name.
    pub username: String,
    /// `level`.
    pub level: Level,
}

/// `mcp`: the MCP server, each bound 0 when left out, its default then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mcp {
    /// `allow`: addresses and prefixes MCP clients may call from; loopback when empty.
    pub allow: Vec<String>,
    /// `max_tokens_per_user`.
    pub max_tokens_per_user: u64,
    /// `max_token_days`.
    pub max_token_days: u64,
    /// `max_sessions`.
    pub max_sessions: u64,
    /// `session_idle_timeout`.
    pub session_idle_timeout: Duration,
    /// `max_request_bytes`.
    pub max_request_bytes: u64,
    /// `max_concurrent_calls`.
    pub max_concurrent_calls: u64,
    /// `max_calls_per_minute`.
    pub max_calls_per_minute: u64,
}

impl Mcp {
    /// `max_tokens_per_user`, its default when left out.
    #[must_use]
    pub fn tokens_per_user(&self) -> u64 {
        nonzero(self.max_tokens_per_user, DEFAULT_MCP_TOKENS_PER_USER)
    }
    /// `max_token_days`, its default when left out.
    #[must_use]
    pub fn token_days(&self) -> u64 {
        nonzero(self.max_token_days, DEFAULT_MCP_TOKEN_DAYS)
    }
    /// `max_sessions`, its default when left out.
    #[must_use]
    pub fn sessions(&self) -> u64 {
        nonzero(self.max_sessions, DEFAULT_MCP_SESSIONS)
    }
    /// `session_idle_timeout`, its default when left out.
    #[must_use]
    pub fn session_idle(&self) -> Duration {
        or(self.session_idle_timeout, DEFAULT_MCP_SESSION_IDLE)
    }
    /// `max_request_bytes`, its default when left out.
    #[must_use]
    pub fn request_bytes(&self) -> u64 {
        nonzero(self.max_request_bytes, DEFAULT_MCP_REQUEST_BYTES)
    }
    /// `max_concurrent_calls`, its default when left out.
    #[must_use]
    pub fn concurrent_calls(&self) -> u64 {
        nonzero(self.max_concurrent_calls, DEFAULT_MCP_CONCURRENT_CALLS)
    }
    /// `max_calls_per_minute`, its default when left out.
    #[must_use]
    pub fn calls_per_minute(&self) -> u64 {
        nonzero(self.max_calls_per_minute, DEFAULT_MCP_CALLS_PER_MINUTE)
    }

    /// `allow` as networks, an address its own; loopback when `allow` is empty. What does not
    /// parse, which the check refuses, is left out. fit's `AllowedPrefixes`.
    #[must_use]
    pub fn allowed(&self) -> Vec<Network> {
        if self.allow.is_empty() {
            return loopback();
        }
        self.allow
            .iter()
            .filter_map(|s| parse_addr_or_prefix(s))
            .collect()
    }
}

/// The networks MCP clients may call from without `mcp.allow`: loopback.
#[must_use]
pub fn loopback() -> Vec<Network> {
    vec![
        Network {
            addr: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 0)),
            bits: 8,
        },
        Network {
            addr: IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            bits: 128,
        },
    ]
}

/// An address (`127.0.0.1`) as the network of itself alone, or a prefix (`10.0.0.0/8`); an
/// IPv4 address mapped into IPv6 is unmapped, a prefix's length with it. fit's
/// `ParseAddrOrPrefix`.
#[must_use]
pub fn parse_addr_or_prefix(s: &str) -> Option<Network> {
    if let Some((addr, _)) = s.split_once('/') {
        if let Ok(IpAddr::V6(v6)) = addr.parse::<IpAddr>() {
            if let Some(v4) = v6.to_ipv4_mapped() {
                let n = Network::parse(s)?;
                let b = n.bits.saturating_sub(96);
                return Network::parse(&format!("{v4}/{b}"));
            }
        }
        return Network::parse(s);
    }
    // Network::parse takes an address alone, mapped ones unmapped; a zone is refused by the
    // address's parse.
    Network::parse(s)
}

fn or(d: Duration, default: Duration) -> Duration {
    if d.is_zero() { default } else { d }
}

fn nonzero(n: u64, default: u64) -> u64 {
    if n == 0 { default } else { n }
}

/// Whether `name`, lower case, is a header a provider's `headers` may not set: the keys' and the
/// request's framing (fit's `assistantHeadersRefused`).
fn refused_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
            | "host"
            | "content-length"
            | "content-type"
            | "transfer-encoding"
            | "connection"
            | "anthropic-version"
    )
}

const ASSISTANT_FIELDS: &[&str] = &[
    "providers",
    "default_model",
    "params",
    "limits",
    "data",
    "users",
    "default_level",
    "mcp",
];
const ASSISTANT_PROVIDER_FIELDS: &[&str] = &[
    "name",
    "kind",
    "base_url",
    "api_version",
    "api_key_env",
    "headers",
    "models",
    "connect_timeout",
    "first_byte_timeout",
    "idle_timeout",
    "timeout",
    "retries",
];
const ASSISTANT_MODEL_FIELDS: &[&str] = &["name", "context", "tools"];
const ASSISTANT_MODEL_REF_FIELDS: &[&str] = &["provider", "model"];
const ASSISTANT_PARAMS_FIELDS: &[&str] = &["max_output_tokens", "temperature", "thinking"];
const ASSISTANT_LIMITS_FIELDS: &[&str] = &[
    "max_turns",
    "max_tool_calls",
    "max_tool_result_bytes",
    "max_conversations",
    "max_concurrent_chats",
    "max_tokens_per_day",
    "max_stream_bytes",
    "max_audit_entries",
];
const ASSISTANT_DATA_FIELDS: &[&str] = &[
    "send_logs",
    "send_access_log",
    "send_subjects",
    "send_records",
    "redact",
];
const ASSISTANT_USER_FIELDS: &[&str] = &["username", "level"];
const ASSISTANT_MCP_FIELDS: &[&str] = &[
    "allow",
    "max_tokens_per_user",
    "max_token_days",
    "max_sessions",
    "session_idle_timeout",
    "max_request_bytes",
    "max_concurrent_calls",
    "max_calls_per_minute",
];

/// Reads settings of the assistant kept apart from the config, as the store keeps those the
/// Settings page saved: `src`, JSON, read and checked as `admin.assistant` is, with the paths of
/// its errors under `admin.assistant`. `console` is the config's `admin.console`, whose users the
/// settings' users must be. fit's `LoadAssistant`.
///
/// # Errors
///
/// What is wrong, each at its path.
pub fn load(src: &str, console: Option<&Console>) -> Result<Assistant, Errors> {
    let node = match crate::json::parse(src) {
        Ok(n) => n,
        Err(e) => {
            let (line, col) = crate::json::position(src, e.off);
            return Err(Errors(vec![Error {
                file: None,
                path: String::new(),
                line,
                col,
                msg: e.msg,
            }]));
        }
    };
    let mut d = Decoder::new(src, None);
    let a = assistant(&mut d, &node, "admin.assistant", console);
    match a {
        Some(a) if d.errors.is_empty() => Ok(a),
        _ => Err(Errors(d.errors)),
    }
}

/// `admin.assistant`, as fit's `config.Assistant` and its `checkAssistant`, beside `console`,
/// `admin.console`.
pub(crate) fn assistant(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
    console: Option<&Console>,
) -> Option<Assistant> {
    let mut o = d.object(n, path)?;
    let mut a = Assistant {
        raw: to_json(n),
        ..Assistant::default()
    };
    if console.is_none() {
        d.error(
            n.off,
            path,
            "the assistant lives in the console's workbench: set admin.console too",
        );
    }
    if let Some(v) = o.take("providers") {
        let p = o.at("providers");
        if let Kind::Array(items) = &v.kind {
            if items.len() > MAX_PROVIDERS {
                d.error(
                    v.off,
                    &p,
                    format!("{} providers, more than {MAX_PROVIDERS}", items.len()),
                );
            }
        }
        let mut names: Vec<String> = Vec::new();
        a.providers = d.array(v, &p, |d, item, ip| {
            let pr = provider(d, item, ip)?;
            if valid_name(&pr.name) && names.contains(&pr.name) {
                d.error(
                    item.off,
                    &join_field(ip, "name"),
                    format!("another provider is already named {:?}", pr.name),
                );
            }
            names.push(pr.name.clone());
            Some(pr)
        });
    }
    if let Some(v) = o.take("default_model") {
        let p = o.at("default_model");
        if let Some(mut r) = d.object(v, &p) {
            let provider = r
                .require(d, "provider")
                .and_then(|x| d.string(x, &join_field(&p, "provider")).map(|s| (x.off, s)));
            let model = r
                .require(d, "model")
                .and_then(|x| d.string(x, &join_field(&p, "model")));
            r.finish(d, ASSISTANT_MODEL_REF_FIELDS, &[]);
            if let (Some((off, provider)), Some(model)) = (provider, model) {
                if a.provider(&provider).is_none() {
                    d.error(
                        off,
                        &join_field(&p, "provider"),
                        format!("no provider is named {provider:?}"),
                    );
                }
                a.default_model = Some(ModelRef { provider, model });
            }
        }
    }
    if let Some(v) = o.take("params") {
        a.params = params(d, v, &o.at("params")).unwrap_or_default();
    }
    if let Some(v) = o.take("limits") {
        a.limits = limits(d, v, &o.at("limits")).unwrap_or_default();
    }
    if let Some(v) = o.take("data") {
        a.data = data(d, v, &o.at("data")).unwrap_or_default();
    }
    if let Some(v) = o.take("users") {
        let p = o.at("users");
        let mut seen: Vec<String> = Vec::new();
        a.users = d.array(v, &p, |d, item, ip| {
            let mut u = d.object(item, ip)?;
            let name = u
                .require(d, "username")
                .and_then(|x| d.string(x, &join_field(ip, "username")).map(|s| (x.off, s)));
            let level = u
                .require(d, "level")
                .and_then(|x| level(d, x, &join_field(ip, "level")));
            u.finish(d, ASSISTANT_USER_FIELDS, &[]);
            let (off, username) = name?;
            let at = join_field(ip, "username");
            if seen.contains(&username) {
                d.error(off, &at, format!("{username:?} is listed twice"));
            } else if console.is_some_and(|c| !c.users.iter().any(|cu| cu.username == username)) {
                d.error(off, &at, format!("{username:?} is not a console user"));
            }
            seen.push(username.clone());
            Some(User {
                username,
                level: level?,
            })
        });
    }
    if let Some(v) = o.take("default_level") {
        a.default_level = level(d, v, &o.at("default_level"));
    }
    if let Some(v) = o.take("mcp") {
        a.mcp = mcp(d, v, &o.at("mcp"));
    }
    o.finish(d, ASSISTANT_FIELDS, &[]);
    Some(a)
}

/// A level, one of fit's.
fn level(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Level> {
    let s = d.string(n, path)?;
    let l = Level::parse(&s);
    if l.is_none() {
        d.error(
            n.off,
            path,
            format!(
                "{s:?} is not one of {}",
                quote_list(Level::ALL.iter().map(|l| l.name()))
            ),
        );
    }
    l
}

fn quote_list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names
        .map(|n| format!("{n:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn valid_name(s: &str) -> bool {
    crate::model::valid_name(s)
}

/// A provider, as fit's `config.AssistantProvider` and `checkAssistantProvider` (but for the
/// names' clashes, which the list checks).
fn provider(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Provider> {
    let mut o = d.object(n, path)?;
    let name = o.require(d, "name").and_then(|v| {
        let p = o.at("name");
        let s = d.string(v, &p)?;
        if !valid_name(&s) {
            d.error(
                v.off,
                &p,
                format!(
                    "{s:?} is not a valid name: use letters, digits, '_', '.' and '-', starting \
                     with a letter or digit"
                ),
            );
        }
        Some(s)
    });
    let kind = o.require(d, "kind").and_then(|v| {
        let p = o.at("kind");
        let s = d.string(v, &p)?;
        let k = ProviderKind::parse(&s);
        if k.is_none() {
            d.error(
                v.off,
                &p,
                format!(
                    "{s:?} is not one of {}",
                    quote_list(ProviderKind::ALL.iter().map(|k| k.name()))
                ),
            );
        }
        k
    });
    let mut pr = Provider::new("", ProviderKind::OpenAi);
    let mut url_off = n.off;
    for (key, slot) in [
        ("base_url", &mut pr.base_url),
        ("api_version", &mut pr.api_version),
        ("api_key_env", &mut pr.api_key_env),
    ] {
        if let Some(v) = o.take(key) {
            if key == "base_url" {
                url_off = v.off;
            }
            *slot = d.string(v, &o.at(key)).unwrap_or_default();
        }
    }
    if let Some(v) = o.take("headers") {
        let p = o.at("headers");
        let hs = d.string_map(v, &p);
        if hs.len() > MAX_HEADERS {
            d.error(
                v.off,
                &p,
                format!("{} headers, more than {MAX_HEADERS}", hs.len()),
            );
        }
        for (off, k, val) in hs {
            let at = join_field(&p, &k);
            if !crate::model::is_token(&k) {
                d.error(off, &at, format!("{k:?} is not a header's name"));
            } else if refused_header(&k.to_ascii_lowercase()) {
                d.error(
                    off,
                    &at,
                    format!(
                        "{k} is the call's own: the key is kept in the store or named by \
                         api_key_env"
                    ),
                );
            } else if val.chars().any(|c| (c < ' ' && c != '\t') || c == '\u{7f}') {
                d.error(off, &at, "the value holds a control character");
            }
            pr.headers.push((k, val));
        }
    }
    if let Some(v) = o.take("models") {
        let p = o.at("models");
        if let Kind::Array(items) = &v.kind {
            if items.len() > MAX_MODELS {
                d.error(
                    v.off,
                    &p,
                    format!("{} models, more than {MAX_MODELS}", items.len()),
                );
            }
        }
        let mut seen: Vec<String> = Vec::new();
        pr.models = d.array(v, &p, |d, item, ip| {
            let m = model(d, item, ip)?;
            let at = join_field(ip, "name");
            if m.name.is_empty()
                || m.name.len() > 256
                || m.name.chars().any(|c| c < ' ' || c == '\u{7f}')
            {
                d.error(item.off, &at, "a model's ID is 1 to 256 printable bytes");
            } else if seen.contains(&m.name) {
                d.error(item.off, &at, format!("{:?} is listed twice", m.name));
            }
            seen.push(m.name.clone());
            Some(m)
        });
    }
    for (key, slot) in [
        ("connect_timeout", &mut pr.connect_timeout),
        ("first_byte_timeout", &mut pr.first_byte_timeout),
        ("idle_timeout", &mut pr.idle_timeout),
        ("timeout", &mut pr.timeout),
    ] {
        if let Some(v) = o.take(key) {
            *slot = d.duration(v, &o.at(key)).unwrap_or_default();
        }
    }
    if let Some(v) = o.take("retries") {
        pr.retries = d
            .int(v, &o.at("retries"), 0, MAX_RETRIES)
            .and_then(|r| u32::try_from(r).ok());
    }
    o.finish(d, ASSISTANT_PROVIDER_FIELDS, &[]);
    let kind = kind?;
    pr.name = name?;
    pr.kind = kind;
    let base = join_field(path, "base_url");
    if kind == ProviderKind::LlmProxy && !pr.base_url.is_empty() {
        d.error(
            url_off,
            &base,
            "an llm_proxy provider is reached on llm.proxy's api address: leave base_url out",
        );
    } else if pr.base_url.is_empty()
        && matches!(
            kind,
            ProviderKind::OpenAiCompatible | ProviderKind::AzureOpenAi
        )
    {
        d.error(
            n.off,
            &base,
            format!("a {} provider needs base_url", kind.name()),
        );
    } else if !pr.base_url.is_empty() && !http_url(&pr.base_url) {
        d.error(
            url_off,
            &base,
            format!(
                "{:?} is not an http or https URL without a query",
                pr.base_url
            ),
        );
    }
    if pr.api_version.is_empty() == (kind == ProviderKind::AzureOpenAi) {
        d.error(
            n.off,
            &join_field(path, "api_version"),
            "api_version is for azure_openai, which needs it",
        );
    }
    if !pr.api_key_env.is_empty() && !env_name(&pr.api_key_env) {
        d.error(
            n.off,
            &join_field(path, "api_key_env"),
            format!("{:?} is not an environment variable's name", pr.api_key_env),
        );
    }
    Some(pr)
}

fn model(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Model> {
    let mut o = d.object(n, path)?;
    let name = o
        .require(d, "name")
        .and_then(|v| d.string(v, &join_field(path, "name")));
    let mut m = Model {
        name: String::new(),
        context: 0,
        tools: None,
    };
    if let Some(v) = o.take("context") {
        m.context = d
            .int(v, &o.at("context"), 0, i64::MAX)
            .and_then(|c| u64::try_from(c).ok())
            .unwrap_or(0);
    }
    if let Some(v) = o.take("tools") {
        match &v.kind {
            Kind::Bool(b) => m.tools = Some(*b),
            other => d.error(
                v.off,
                &o.at("tools"),
                format!("expected a boolean, got {}", other.name()),
            ),
        }
    }
    o.finish(d, ASSISTANT_MODEL_FIELDS, &[]);
    m.name = name?;
    Some(m)
}

/// A count at least 0, kept as `u64`.
fn count(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, max: i64, slot: &mut u64) {
    if let Some(v) = o.take(key) {
        if let Some(n) = d
            .int(v, &o.at(key), 0, max)
            .and_then(|n| u64::try_from(n).ok())
        {
            *slot = n;
        }
    }
}

fn flag(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, slot: &mut bool) {
    if let Some(v) = o.take(key) {
        match &v.kind {
            Kind::Bool(b) => *slot = *b,
            other => d.error(
                v.off,
                &o.at(key),
                format!("expected a boolean, got {}", other.name()),
            ),
        }
    }
}

fn params(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Params> {
    let mut o = d.object(n, path)?;
    let mut p = Params::default();
    count(
        d,
        &mut o,
        "max_output_tokens",
        i64::MAX,
        &mut p.max_output_tokens,
    );
    if let Some(v) = o.take("temperature") {
        let at = o.at("temperature");
        if let (Some(t), Kind::Number(raw)) = (d.number(v, &at), &v.kind) {
            if t < 0.0 {
                d.error(v.off, &at, format!("{raw} is less than the minimum 0"));
            } else if t > 2.0 {
                d.error(v.off, &at, format!("{raw} is greater than the maximum 2"));
            } else {
                p.temperature = Some(raw.clone());
            }
        }
    }
    if let Some(v) = o.take("thinking") {
        let at = o.at("thinking");
        if let Some(s) = d.string(v, &at) {
            if !["off", "low", "medium", "high"].contains(&s.as_str()) {
                d.error(
                    v.off,
                    &at,
                    format!("{s:?} is not one of \"off\", \"low\", \"medium\", \"high\""),
                );
            } else {
                p.thinking = s;
            }
        }
    }
    o.finish(d, ASSISTANT_PARAMS_FIELDS, &[]);
    Some(p)
}

fn limits(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Limits> {
    let mut o = d.object(n, path)?;
    let mut l = Limits::default();
    for (key, slot) in [
        ("max_turns", &mut l.max_turns),
        ("max_tool_calls", &mut l.max_tool_calls),
        ("max_tool_result_bytes", &mut l.max_tool_result_bytes),
        ("max_conversations", &mut l.max_conversations),
        ("max_concurrent_chats", &mut l.max_concurrent_chats),
        ("max_tokens_per_day", &mut l.max_tokens_per_day),
        ("max_stream_bytes", &mut l.max_stream_bytes),
        ("max_audit_entries", &mut l.max_audit_entries),
    ] {
        count(d, &mut o, key, i64::MAX, slot);
    }
    o.finish(d, ASSISTANT_LIMITS_FIELDS, &[]);
    Some(l)
}

fn data(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Data> {
    let mut o = d.object(n, path)?;
    let mut x = Data::default();
    flag(d, &mut o, "send_logs", &mut x.send_logs);
    flag(d, &mut o, "send_access_log", &mut x.send_access_log);
    flag(d, &mut o, "send_subjects", &mut x.send_subjects);
    flag(d, &mut o, "send_records", &mut x.send_records);
    if let Some(v) = o.take("redact") {
        let p = o.at("redact");
        let res = d.strings(v, &p);
        if res.len() > MAX_REDACT {
            d.error(
                v.off,
                &p,
                format!("{} patterns, more than {MAX_REDACT}", res.len()),
            );
        }
        for (i, (off, re)) in res.into_iter().enumerate() {
            if re.is_empty() || re.len() > 256 || akit_router::regexp(re.as_bytes()).is_err() {
                d.error(
                    off,
                    &join_index(&p, i),
                    format!("{re:?} is not a regular expression of 1 to 256 bytes"),
                );
            }
            x.redact.push(re);
        }
    }
    o.finish(d, ASSISTANT_DATA_FIELDS, &[]);
    Some(x)
}

fn mcp(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Mcp> {
    let mut o = d.object(n, path)?;
    let mut m = Mcp::default();
    if let Some(v) = o.take("allow") {
        let p = o.at("allow");
        let given = d.strings(v, &p);
        if given.len() > MAX_MCP_ALLOW {
            d.error(
                v.off,
                &p,
                format!("{} addresses, more than {MAX_MCP_ALLOW}", given.len()),
            );
        }
        for (i, (off, s)) in given.into_iter().enumerate() {
            if parse_addr_or_prefix(&s).is_none() {
                d.error(
                    off,
                    &join_index(&p, i),
                    format!("{s:?} is not an address, as 127.0.0.1, nor a prefix, as 10.0.0.0/8"),
                );
            }
            m.allow.push(s);
        }
    }
    count(
        d,
        &mut o,
        "max_tokens_per_user",
        i64::MAX,
        &mut m.max_tokens_per_user,
    );
    count(
        d,
        &mut o,
        "max_token_days",
        MAX_MCP_TOKEN_DAYS,
        &mut m.max_token_days,
    );
    count(d, &mut o, "max_sessions", i64::MAX, &mut m.max_sessions);
    if let Some(v) = o.take("session_idle_timeout") {
        m.session_idle_timeout = d
            .duration(v, &o.at("session_idle_timeout"))
            .unwrap_or_default();
    }
    count(
        d,
        &mut o,
        "max_request_bytes",
        MAX_MCP_REQUEST_BYTES,
        &mut m.max_request_bytes,
    );
    count(
        d,
        &mut o,
        "max_concurrent_calls",
        MAX_MCP_CONCURRENT_CALLS,
        &mut m.max_concurrent_calls,
    );
    count(
        d,
        &mut o,
        "max_calls_per_minute",
        MAX_MCP_CALLS_PER_MINUTE,
        &mut m.max_calls_per_minute,
    );
    o.finish(d, ASSISTANT_MCP_FIELDS, &[]);
    Some(m)
}

/// Whether `s` is an environment variable's name: fit's `envNamePattern`.
fn env_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Whether `s` is an http or https URL with a host and without a user, a query or a fragment,
/// as fit's check of `base_url` reads it with Go's `url.Parse`.
#[must_use]
pub fn http_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    if s.chars().any(|c| c.is_control() || c == ' ') {
        return false;
    }
    if rest.contains('#') {
        return false;
    }
    if let Some((_, q)) = rest.split_once('?') {
        if !q.is_empty() {
            return false;
        }
    }
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    !authority.is_empty() && !authority.contains('@') && !authority.starts_with(':')
}

/// `n` written back as compact JSON, its numbers and strings as written: fit's `Value`'s
/// `MarshalJSON`.
fn to_json(n: &Node) -> String {
    let mut b = String::new();
    write_node(&mut b, n);
    b
}

fn write_node(b: &mut String, n: &Node) {
    match &n.kind {
        Kind::Null => b.push_str("null"),
        Kind::Bool(x) => b.push_str(if *x { "true" } else { "false" }),
        Kind::Number(raw) => b.push_str(raw),
        Kind::Str(s) => write_string(b, s),
        Kind::Array(items) => {
            b.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_node(b, x);
            }
            b.push(']');
        }
        Kind::Object(ms) => {
            b.push('{');
            for (i, m) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_string(b, &m.key);
                b.push(':');
                write_node(b, &m.value);
            }
            b.push('}');
        }
    }
}

#[cfg(test)]
mod tests;
