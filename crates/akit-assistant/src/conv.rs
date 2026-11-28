//! The users' conversations (I162, fit's `assistant/conversations.go`): kept in the store, a
//! section a user (`assistant.chats.<hash>`, fit's document), bounded in number, in size each
//! and in size together; read the first time a user asks, written back by the assistant's pool
//! after each chat. What akit does not use of a conversation yet (a proposal, a skill's inputs)
//! is kept as it was read, so that a store moves between akit and fit.

use std::collections::{BTreeMap, BTreeSet};

use akit_config::value::Value;

use crate::json::{self, Obj};
use crate::model::{Message, Role, ToolCall};

/// The most bytes of a conversation as it is kept: past it, its earliest turns go.
pub const MAX_CONVERSATION_BYTES: usize = 512 << 10;
/// The most bytes of a user's conversations together: past it, the oldest go.
pub const MAX_USER_BYTES: usize = 8 << 20;
/// The most bytes of a user's message.
pub const MAX_MESSAGE_BYTES: usize = 32 << 10;
/// The prefix of the store's sections that keep the users' conversations.
pub const CHATS_SECTION: &str = "assistant.chats.";

/// The section of `user`'s conversations: a hash of the name, as fit's `sectionOf`.
#[must_use]
pub fn section_of(user: &str) -> String {
    let h = akio_crypto::hash::Sha256::digest(user.as_bytes());
    format!(
        "{CHATS_SECTION}{}",
        akit_roles::linktoken::hex(h.get(..12).unwrap_or(&h))
    )
}

/// A message as it is kept: the model's message, when it was made, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    /// The message.
    pub message: Message,
    /// When it was made, Unix milliseconds.
    pub at: i64,
    /// An answer the person stopped, kept as far as it came.
    pub stopped: bool,
    /// A tool's result cut to its bound.
    pub cut: bool,
    /// An answer that failed, and why; it is not sent again.
    pub error: String,
}

impl Msg {
    /// `m`, made now.
    #[must_use]
    pub fn of(m: Message) -> Msg {
        Msg {
            message: m,
            at: now_ms(),
            stopped: false,
            cut: false,
            error: String::new(),
        }
    }

    /// As fit writes it: the model's message's members, then its own.
    #[must_use]
    pub fn json(&self) -> String {
        let m = &self.message;
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        let mut o = Obj::new()
            .str("role", role)
            .str_omit("content", &m.content)
            .str_omit("thinking", &m.thinking)
            .str_omit("thinking_signature", &m.thinking_signature);
        if !m.tool_calls.is_empty() {
            let calls: Vec<String> = m.tool_calls.iter().map(call_json).collect();
            o = o.raw("tool_calls", &format!("[{}]", calls.join(",")));
        }
        o = o
            .str_omit("tool_call_id", &m.tool_call_id)
            .str_omit("tool_name", &m.tool_name);
        if m.is_error {
            o = o.bool("is_error", true);
        }
        if self.at != 0 {
            o = o.num("at", self.at);
        }
        if self.stopped {
            o = o.bool("stopped", true);
        }
        if self.cut {
            o = o.bool("cut", true);
        }
        o.str_omit("error", &self.error).end()
    }

    fn from(v: &Value) -> Option<Msg> {
        let role = match json::str_of(v, "role") {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => return None,
        };
        let s = |k: &str| json::str_of(v, k).to_owned();
        let b = |k: &str| matches!(v.get(k), Some(Value::Bool(true)));
        let tool_calls = match v.get("tool_calls") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|c| ToolCall {
                    id: json::str_of(c, "id").to_owned(),
                    name: json::str_of(c, "name").to_owned(),
                    arguments: c
                        .get("arguments")
                        .map_or_else(|| "{}".to_owned(), json::compact),
                    invalid: matches!(c.get("invalid"), Some(Value::Bool(true))),
                    signature: json::str_of(c, "signature").to_owned(),
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(Msg {
            message: Message {
                role,
                content: s("content"),
                thinking: s("thinking"),
                thinking_signature: s("thinking_signature"),
                tool_calls,
                tool_call_id: s("tool_call_id"),
                tool_name: s("tool_name"),
                is_error: b("is_error"),
            },
            at: int(v, "at"),
            stopped: b("stopped"),
            cut: b("cut"),
            error: s("error"),
        })
    }
}

fn int(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

/// A tool call as a kept message holds it: its arguments raw JSON, as the model gave them.
pub(crate) fn call_json(tc: &ToolCall) -> String {
    let args = if tc.arguments.is_empty() {
        "{}"
    } else {
        tc.arguments.as_str()
    };
    let mut o = Obj::new()
        .str("id", &tc.id)
        .str("name", &tc.name)
        .raw("arguments", args);
    if tc.invalid {
        o = o.bool("invalid", true);
    }
    o.str_omit("signature", &tc.signature).end()
}

/// A user's conversation with the assistant.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// Its ID.
    pub id: String,
    /// Its title, from its first message.
    pub title: String,
    /// When it was made and last changed, Unix milliseconds.
    pub created: i64,
    /// See [`created`](Conversation::created).
    pub updated: i64,
    /// Its messages.
    pub messages: Vec<Msg>,
    /// What stands for the messages before [`folded`](Conversation::folded) in the calls made
    /// from here: the early turns, summarized when the conversation outgrew its window.
    pub summary: String,
    /// See [`summary`](Conversation::summary).
    pub folded: usize,
    /// The skill it runs (I164), kept as read.
    pub skill: String,
    /// The members akit does not use yet (`proposal`, `proposal_note`, `skill_inputs`), kept
    /// as read.
    pub extra: Vec<(String, Value)>,
    /// Whether a chat is under way in it.
    pub busy: bool,
}

const OWN: &[&str] = &[
    "id", "title", "created", "updated", "messages", "summary", "folded", "skill",
];

impl Conversation {
    /// As fit writes it (`GET /api/conversations/{id}`, the store).
    #[must_use]
    pub fn json(&self, busy: Option<bool>) -> String {
        let msgs: Vec<String> = self.messages.iter().map(Msg::json).collect();
        let mut o = Obj::new()
            .str("id", &self.id)
            .str("title", &self.title)
            .num("created", self.created)
            .num("updated", self.updated)
            .raw("messages", &format!("[{}]", msgs.join(",")))
            .str_omit("summary", &self.summary);
        if self.folded > 0 {
            o = o.num("folded", self.folded);
        }
        // fit's members in fit's order: the proposal's before the skill, its inputs after.
        let extra = |o: Obj, k: &str| match self.extra.iter().find(|(n, _)| n == k) {
            Some((_, v)) => o.raw(k, &json::compact(v)),
            None => o,
        };
        o = extra(extra(o, "proposal"), "proposal_note");
        o = o.str_omit("skill", &self.skill);
        o = extra(o, "skill_inputs");
        for (k, v) in &self.extra {
            if !["proposal", "proposal_note", "skill_inputs"].contains(&k.as_str()) {
                o = o.raw(k, &json::compact(v));
            }
        }
        if let Some(b) = busy {
            o = o.bool("busy", b);
        }
        o.end()
    }

    fn from(v: &Value) -> Option<Conversation> {
        let id = json::str_of(v, "id");
        if id.is_empty() {
            return None;
        }
        let messages = match v.get("messages") {
            Some(Value::Array(a)) => a.iter().filter_map(Msg::from).collect(),
            _ => Vec::new(),
        };
        let extra = match v {
            Value::Object(ms) => ms
                .iter()
                .filter(|(k, x)| !OWN.contains(&k.as_str()) && *x != Value::Null)
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let folded = usize::try_from(int(v, "folded")).unwrap_or(0);
        Some(Conversation {
            id: id.to_owned(),
            title: json::str_of(v, "title").to_owned(),
            created: int(v, "created"),
            updated: int(v, "updated"),
            folded: folded.min(messages.len()),
            messages,
            summary: json::str_of(v, "summary").to_owned(),
            skill: json::str_of(v, "skill").to_owned(),
            extra,
            busy: false,
        })
    }

    /// The person's messages.
    #[must_use]
    pub fn turns(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.message.role == Role::User)
            .count()
    }

    /// About its size as it is kept.
    #[must_use]
    pub fn bytes(&self) -> usize {
        let sum = |xs: &[usize]| xs.iter().fold(0usize, |a, b| a.saturating_add(*b));
        let mut n = sum(&[
            128,
            self.id.len(),
            self.title.len(),
            self.summary.len(),
            self.skill.len(),
        ]);
        for (k, v) in &self.extra {
            n = sum(&[n, k.len(), json::compact(v).len()]);
        }
        for m in &self.messages {
            let mm = &m.message;
            n = sum(&[
                n,
                64,
                mm.content.len(),
                mm.thinking.len(),
                mm.thinking_signature.len(),
                mm.tool_call_id.len(),
                mm.tool_name.len(),
                m.error.len(),
            ]);
            for tc in &mm.tool_calls {
                n = sum(&[
                    n,
                    48,
                    tc.id.len(),
                    tc.name.len(),
                    tc.arguments.len(),
                    tc.signature.len(),
                ]);
            }
        }
        n
    }

    /// Keeps it within [`MAX_CONVERSATION_BYTES`]: the messages a summary stands for go first,
    /// then the earliest turns. fit's `trimLocked`.
    pub fn trim(&mut self) {
        while self.bytes() > MAX_CONVERSATION_BYTES && self.messages.len() > 1 {
            let mut n = self.folded;
            if n == 0 {
                n = 1;
                while self
                    .messages
                    .get(n)
                    .is_some_and(|m| m.message.role != Role::User)
                {
                    n = n.saturating_add(1);
                }
                if n >= self.messages.len() {
                    return;
                }
                if self.summary.is_empty() {
                    "(The earliest turns of this conversation were dropped to keep it within its \
                     bound.)"
                        .clone_into(&mut self.summary);
                }
            }
            self.messages.drain(..n.min(self.messages.len()));
            self.folded = 0;
        }
    }

    /// The calls of the last answer with tools that have no result yet.
    #[must_use]
    pub fn missing_results(&self) -> Vec<ToolCall> {
        for (i, m) in self.messages.iter().enumerate().rev() {
            if m.message.role == Role::User {
                return Vec::new();
            }
            if m.message.role != Role::Assistant || m.message.tool_calls.is_empty() {
                continue;
            }
            let later = self.messages.get(i.saturating_add(1)..).unwrap_or(&[]);
            return m
                .message
                .tool_calls
                .iter()
                .filter(|tc| {
                    !later
                        .iter()
                        .any(|r| r.message.role == Role::Tool && r.message.tool_call_id == tc.id)
                })
                .cloned()
                .collect();
        }
        Vec::new()
    }

    /// The `ask_user` call the conversation waits on.
    #[must_use]
    pub fn pending_ask(&self) -> Option<ToolCall> {
        self.missing_results()
            .into_iter()
            .find(|tc| tc.name == "ask_user")
    }

    /// Gives every call of the last answer without a result the result `why`.
    pub fn close_pending(&mut self, why: &str) {
        for tc in self.missing_results() {
            let mut m = Message::tool(&tc, why, tc.name != "ask_user");
            m.tool_name.clone_from(&tc.name);
            self.messages.push(Msg::of(m));
        }
    }
}

/// A user's conversations, and the tokens they spent today.
#[derive(Debug, Default)]
pub struct UserChats {
    /// Read from the store.
    pub loaded: bool,
    /// Oldest first, by when they changed.
    pub convs: Vec<Conversation>,
    /// The UTC day `tokens` counts.
    pub day: String,
    /// The tokens spent that day.
    pub tokens: u64,
}

impl UserChats {
    /// Reads a section as fit writes it.
    #[must_use]
    pub fn read(data: &[u8]) -> Option<UserChats> {
        let v = akit_config::value::parse(std::str::from_utf8(data).ok()?).ok()?;
        let convs = match v.get("conversations") {
            Some(Value::Array(a)) => a.iter().filter_map(Conversation::from).collect(),
            _ => Vec::new(),
        };
        let mut u = UserChats {
            loaded: true,
            convs,
            day: json::str_of(&v, "day").to_owned(),
            tokens: u64::try_from(int(&v, "tokens")).unwrap_or(0),
        };
        u.convs.sort_by_key(|c| c.updated);
        Some(u)
    }

    /// The section, as fit writes it.
    #[must_use]
    pub fn section(&self) -> String {
        let convs: Vec<String> = self.convs.iter().map(|c| c.json(None)).collect();
        let mut o = Obj::new().raw("conversations", &format!("[{}]", convs.join(",")));
        o = o.str_omit("day", &self.day);
        if self.tokens > 0 {
            o = o.num("tokens", self.tokens);
        }
        o.end()
    }

    /// The tokens spent today.
    pub fn used_today(&mut self) -> u64 {
        let d = today();
        if self.day != d {
            self.day = d;
            self.tokens = 0;
        }
        self.tokens
    }

    /// The conversation `id`.
    pub fn get(&mut self, id: &str) -> Option<&mut Conversation> {
        self.convs.iter_mut().find(|c| c.id == id)
    }

    /// Drops what is past the bounds: the oldest conversations not under way past `max`, or
    /// past [`MAX_USER_BYTES`] together.
    pub fn bound(&mut self, max: usize) {
        self.convs.sort_by_key(|c| c.updated);
        let mut total: usize = self.convs.iter().map(Conversation::bytes).sum();
        let mut i = 0;
        while i < self.convs.len() && (self.convs.len() > max || total > MAX_USER_BYTES) {
            if self.convs.get(i).is_some_and(|c| c.busy) {
                i = i.saturating_add(1);
                continue;
            }
            let c = self.convs.remove(i);
            total = total.saturating_sub(c.bytes());
        }
    }
}

/// Every user's conversations, and those to write back.
#[derive(Debug, Default)]
pub struct Chats {
    /// By user.
    pub users: BTreeMap<String, UserChats>,
    /// The users whose conversations changed since they were written.
    pub dirty: BTreeSet<String>,
    /// Whether a write is on the pool.
    pub saving: bool,
}

/// Now, Unix milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The UTC day, `2006-01-02`.
#[must_use]
pub fn today() -> String {
    let secs = now_ms() / 1000;
    let t = akit_roles::linktoken::rfc3339(secs);
    t.get(..10).unwrap_or("").to_owned()
}

/// A new ID: 18 hex digits, as fit's `newID`.
#[must_use]
pub fn new_id() -> String {
    let mut b = [0u8; 9];
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        for (d, s) in b.iter_mut().zip(now_ms().to_le_bytes()) {
            *d = s;
        }
    }
    akit_roles::linktoken::hex(&b)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    /// A conversation as fit's store holds it reads back the same, its proposal kept.
    #[test]
    fn fit_documents() {
        let fit = r#"{"conversations":[{"id":"abc","title":"t","created":1,"updated":2,"messages":[{"role":"user","content":"hi","at":1},{"role":"assistant","tool_calls":[{"id":"c1","name":"status_get","arguments":{}}],"at":2},{"role":"tool","content":"{}","tool_call_id":"c1","tool_name":"status_get","at":3,"cut":true}],"proposal":{"id":"p1","steps":[]},"skill":"explain"}],"day":"2026-11-28","tokens":42}"#;
        let u = UserChats::read(fit.as_bytes()).unwrap();
        assert_eq!(u.tokens, 42);
        let c = &u.convs[0];
        assert_eq!(c.messages.len(), 3);
        assert!(c.messages[2].cut);
        assert_eq!(c.messages[1].message.tool_calls[0].arguments, "{}");
        assert_eq!(u.section(), fit);
    }

    #[test]
    fn bounds() {
        let mut c = Conversation {
            id: "x".into(),
            ..Conversation::default()
        };
        for i in 0..40 {
            c.messages
                .push(Msg::of(Message::new(Role::User, &format!("q{i}"))));
            c.messages
                .push(Msg::of(Message::new(Role::Assistant, &"a".repeat(20_000))));
        }
        c.trim();
        assert!(c.bytes() <= MAX_CONVERSATION_BYTES);
        assert_eq!(c.messages[0].message.role, Role::User);
        assert!(c.summary.contains("dropped"));
        let mut u = UserChats::default();
        for i in 0..5 {
            u.convs.push(Conversation {
                id: i.to_string(),
                updated: i,
                busy: i == 0,
                ..Conversation::default()
            });
        }
        u.bound(3);
        let ids: Vec<&str> = u.convs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["0", "3", "4"]);
        assert!(
            section_of("ada").starts_with("assistant.chats.") && section_of("ada").len() == 16 + 24
        );
        assert_eq!(today().len(), 10);
    }
}
