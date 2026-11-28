//! The audit log (I163, fit's `assistant/audit.go`): what the assistant did for each user, the
//! tools an answer called and the proposals applied or discarded, kept in the store's section
//! [`SECTION`], bounded by `limits.max_audit_entries`, numbered across restarts. `GET
//! /api/audit` reads it; every console user may, as the console's log of changes.

use akit_config::value::Value;
use akit_roles::pages;

use crate::Assistant;
use crate::json::{self, Obj};
use crate::prompt::clip;
use crate::proposal::{MAX_STEPS, MAX_SUMMARY_BYTES, Proposal};

/// The store's section the log is kept in.
pub const SECTION: &str = "assistant.audit";

/// An answer of the assistant's that called tools.
pub const KIND_CHAT: &str = "chat";
/// An apply of a proposal, or a try at one.
pub const KIND_APPLY: &str = "apply";
/// A proposal discarded.
pub const KIND_DISCARD: &str = "discard";

/// The config and the purges were applied.
pub const APPLIED: &str = "applied";
/// The config was applied, a purge failed.
pub const PARTIAL: &str = "partial";
/// The Admin API refused the change: nothing was applied.
pub const FAILED: &str = "failed";
/// The config changed since: offered again on the new one.
pub const STALE: &str = "stale";
/// The user's level, or a proposal that does not check.
pub const REFUSED: &str = "refused";
/// Discarded, or deleted with its conversation.
pub const DISCARDED: &str = "discarded";

/// The source of an entry from the panel; I166's MCP server adds its own.
pub const SOURCE_PANEL: &str = "panel";

/// The bounds of an entry, so that the log's bound in entries bounds its bytes too.
pub const MAX_CALLS: usize = 32;
/// See [`MAX_CALLS`].
pub const MAX_ARGS: usize = 512;
/// See [`MAX_CALLS`].
pub const MAX_DETAIL: usize = 1024;
/// See [`MAX_CALLS`].
pub const MAX_NAMES: usize = 64;

/// A page of the log, and the most of one.
const DEFAULT_PAGE: usize = 100;
const MAX_PAGE: usize = 500;

/// A tool call: its tool, its arguments (redacted, cut), and whether it failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Call {
    /// The tool.
    pub tool: String,
    /// Its arguments.
    pub args: String,
    /// Whether it failed.
    pub error: bool,
}

impl Call {
    fn json(&self) -> String {
        let mut o = Obj::new()
            .str("tool", &self.tool)
            .str_omit("args", &self.args);
        if self.error {
            o = o.bool("error", true);
        }
        o.end()
    }

    fn from(v: &Value) -> Call {
        Call {
            tool: json::str_of(v, "tool").to_owned(),
            args: json::str_of(v, "args").to_owned(),
            error: matches!(v.get("error"), Some(Value::Bool(true))),
        }
    }
}

/// A proposal as the log keeps it: its steps' tools and summaries, and the members it changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prop {
    /// Its ID.
    pub id: String,
    /// Its revision.
    pub revision: u64,
    /// The config version it was checked on.
    pub base_version: u64,
    /// Its steps' tools and summaries.
    pub steps: Vec<(String, String)>,
    /// The members it changes.
    pub changed: Vec<String>,
}

/// The steps of a proposal as the log and the list of pending ones write them.
#[must_use]
pub fn steps_json(steps: &[(String, String)]) -> String {
    let items: Vec<String> = steps
        .iter()
        .map(|(t, s)| Obj::new().str("tool", t).str("summary", s).end())
        .collect();
    format!("[{}]", items.join(","))
}

impl Prop {
    /// `p` as the log keeps it, with the members `changed`, bounded.
    #[must_use]
    pub fn of(p: &Proposal, changed: &[String]) -> Prop {
        Prop {
            id: p.id.clone(),
            revision: p.revision,
            base_version: p.base_version,
            steps: p
                .steps
                .iter()
                .take(MAX_STEPS)
                .map(|s| (s.tool.clone(), clip(&s.summary, MAX_SUMMARY_BYTES)))
                .collect(),
            changed: changed.iter().take(MAX_NAMES).cloned().collect(),
        }
    }

    fn json(&self) -> String {
        let mut o = Obj::new()
            .str("id", &self.id)
            .num("revision", self.revision);
        if self.base_version > 0 {
            o = o.num("base_version", self.base_version);
        }
        if !self.steps.is_empty() {
            o = o.raw("steps", &steps_json(&self.steps));
        }
        if !self.changed.is_empty() {
            o = o.raw("changed", &json::strings(&self.changed));
        }
        o.end()
    }

    fn from(v: &Value) -> Prop {
        Prop {
            id: json::str_of(v, "id").to_owned(),
            revision: uint(v, "revision"),
            base_version: uint(v, "base_version"),
            steps: match v.get("steps") {
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|s| {
                        (
                            json::str_of(s, "tool").to_owned(),
                            json::str_of(s, "summary").to_owned(),
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            },
            changed: match v.get("changed") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|x| match x {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
        }
    }
}

fn int(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn uint(v: &Value, k: &str) -> u64 {
    u64::try_from(int(v, k)).unwrap_or(0)
}

/// What the assistant did for a user: the tools an answer called, or a proposal applied or
/// discarded, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Numbers the entries, from 1, across restarts.
    pub seq: i64,
    /// When, Unix milliseconds.
    pub at: i64,
    /// The console user.
    pub user: String,
    /// Where it came from: `panel`, or (I166) `mcp`, with `token` the MCP token's name.
    pub source: String,
    /// See [`source`](Entry::source).
    pub token: String,
    /// What it is: [`KIND_CHAT`], [`KIND_APPLY`] or [`KIND_DISCARD`].
    pub kind: String,
    /// Whose conversation a proposal is of, when not `user`'s.
    pub owner: String,
    /// The conversation.
    pub conversation: String,
    /// The skill the answer ran (I164).
    pub skill: String,
    /// The tools an answer called, at most [`MAX_CALLS`].
    pub calls: Vec<Call>,
    /// The calls past those.
    pub more_calls: usize,
    /// The proposal.
    pub proposal: Option<Prop>,
    /// How it ended: a chat's reason, or an apply's outcome.
    pub outcome: String,
    /// Why, when it failed.
    pub detail: String,
    /// The config version an apply made.
    pub version: u64,
}

impl Entry {
    /// As fit writes it.
    #[must_use]
    pub fn json(&self) -> String {
        let mut o = Obj::new()
            .num("seq", self.seq)
            .num("at", self.at)
            .str("user", &self.user)
            .str("source", &self.source)
            .str_omit("token", &self.token)
            .str("kind", &self.kind)
            .str_omit("owner", &self.owner)
            .str_omit("conversation", &self.conversation)
            .str_omit("skill", &self.skill);
        if !self.calls.is_empty() {
            let calls: Vec<String> = self.calls.iter().map(Call::json).collect();
            o = o.raw("calls", &format!("[{}]", calls.join(",")));
        }
        if self.more_calls > 0 {
            o = o.num("more_calls", self.more_calls);
        }
        if let Some(p) = &self.proposal {
            o = o.raw("proposal", &p.json());
        }
        o = o
            .str("outcome", &self.outcome)
            .str_omit("detail", &self.detail);
        if self.version > 0 {
            o = o.num("version", self.version);
        }
        o.end()
    }

    fn from(v: &Value) -> Entry {
        Entry {
            seq: int(v, "seq"),
            at: int(v, "at"),
            user: json::str_of(v, "user").to_owned(),
            source: json::str_of(v, "source").to_owned(),
            token: json::str_of(v, "token").to_owned(),
            kind: json::str_of(v, "kind").to_owned(),
            owner: json::str_of(v, "owner").to_owned(),
            conversation: json::str_of(v, "conversation").to_owned(),
            skill: json::str_of(v, "skill").to_owned(),
            calls: match v.get("calls") {
                Some(Value::Array(a)) => a.iter().map(Call::from).collect(),
                _ => Vec::new(),
            },
            more_calls: usize::try_from(uint(v, "more_calls")).unwrap_or(0),
            proposal: v
                .get("proposal")
                .filter(|p| **p != Value::Null)
                .map(Prop::from),
            outcome: json::str_of(v, "outcome").to_owned(),
            detail: json::str_of(v, "detail").to_owned(),
            version: uint(v, "version"),
        }
    }
}

/// The entries kept, oldest first.
#[derive(Debug, Default)]
pub struct Log {
    /// The entries.
    pub entries: std::collections::VecDeque<Entry>,
    /// The last entry's number.
    pub seq: i64,
    /// Whether it changed since it was written.
    pub dirty: bool,
}

impl Log {
    /// The log as the store's section keeps it, fit's `keptAudit`.
    #[must_use]
    pub fn section(&self) -> String {
        let entries: Vec<String> = self.entries.iter().map(Entry::json).collect();
        Obj::new()
            .num("seq", self.seq)
            .raw("entries", &format!("[{}]", entries.join(",")))
            .end()
    }

    /// Reads the store's section.
    ///
    /// # Errors
    ///
    /// It is not the log.
    pub fn read(data: &[u8]) -> Result<Log, String> {
        let text = std::str::from_utf8(data).map_err(|_| "not UTF-8".to_owned())?;
        let v = akit_config::value::parse(text).map_err(|e| e.to_string())?;
        if !matches!(v, Value::Object(_)) {
            return Err("not a JSON object".to_owned());
        }
        let entries: std::collections::VecDeque<Entry> = match v.get("entries") {
            Some(Value::Array(a)) => a.iter().map(Entry::from).collect(),
            None | Some(Value::Null) => std::collections::VecDeque::new(),
            Some(_) => return Err("entries is not a list".to_owned()),
        };
        let seq = entries.iter().map(|e| e.seq).fold(int(&v, "seq"), i64::max);
        Ok(Log {
            entries,
            seq,
            dirty: false,
        })
    }

    /// Drops the oldest entries past `max`.
    pub fn bound(&mut self, max: u64) {
        let max = usize::try_from(max).unwrap_or(usize::MAX);
        while self.entries.len() > max {
            self.entries.pop_front();
        }
    }
}

/// What a page of the log picks: by user, conversation and kind when not empty, before the
/// entry `before` when not 0, the latest `limit`.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// See [`Filter`].
    pub user: String,
    /// See [`Filter`].
    pub conversation: String,
    /// See [`Filter`].
    pub kind: String,
    /// See [`Filter`].
    pub before: i64,
    /// See [`Filter`].
    pub limit: usize,
}

impl Assistant {
    /// Adds `e` to the log, bounded, and has the log written to the store. Its number is set,
    /// and when it is, when 0.
    pub fn audit(&self, mut e: Entry) {
        if e.at == 0 {
            e.at = crate::conv::now_ms();
        }
        if e.source.is_empty() {
            SOURCE_PANEL.clone_into(&mut e.source);
        }
        if e.calls.len() > MAX_CALLS {
            e.more_calls = e
                .more_calls
                .saturating_add(e.calls.len().saturating_sub(MAX_CALLS));
            e.calls.truncate(MAX_CALLS);
        }
        for c in &mut e.calls {
            c.args = clip(&c.args, MAX_ARGS);
        }
        e.detail = clip(&e.detail, MAX_DETAIL);
        let max = self.settings().limits.audit_entries();
        {
            let mut log = self.audit_log();
            log.seq = log.seq.saturating_add(1);
            e.seq = log.seq;
            log.entries.push_back(e);
            log.bound(max);
            log.dirty = true;
        }
        self.schedule_save();
    }

    /// The entries `f` picks, the latest first, and how many the log keeps.
    #[must_use]
    pub fn audit_entries(&self, f: &Filter) -> (Vec<Entry>, usize) {
        let limit = if f.limit == 0 { DEFAULT_PAGE } else { f.limit }.min(MAX_PAGE);
        let log = self.audit_log();
        let out = log
            .entries
            .iter()
            .rev()
            .filter(|e| {
                (f.before <= 0 || e.seq < f.before)
                    && (f.user.is_empty() || e.user == f.user)
                    && (f.conversation.is_empty() || e.conversation == f.conversation)
                    && (f.kind.is_empty() || e.kind == f.kind)
            })
            .take(limit)
            .cloned()
            .collect();
        (out, log.entries.len())
    }

    /// `GET /api/audit`: the log, the latest first, as `?user=`, `?conversation=`, `?kind=`,
    /// `?before=` and `?limit=` pick it.
    pub(crate) fn audit_view(&self, query: &str) -> akio_http::Response {
        let param = |name: &str| {
            query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == name)
                .map(|(_, v)| akio_http::message::percent_decode(v).unwrap_or_default())
                .unwrap_or_default()
        };
        let f = Filter {
            user: param("user"),
            conversation: param("conversation"),
            kind: param("kind"),
            before: param("before").parse().unwrap_or(0),
            limit: param("limit").parse().unwrap_or(0),
        };
        let (entries, total) = self.audit_entries(&f);
        let entries: Vec<String> = entries.iter().map(Entry::json).collect();
        pages::json(
            200,
            Obj::new()
                .raw("entries", &format!("[{}]", entries.join(",")))
                .num("total", total)
                .num("max", self.settings().limits.audit_entries())
                .bool("kept", self.0.o.store.is_some())
                .raw("users", &json::strings(&self.console_users()))
                .end(),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// An entry reads back as it was written, every member; one with nothing optional writes
    /// only what fit's does.
    #[test]
    fn entries_read_back() {
        let e = Entry {
            seq: 3,
            at: 9,
            user: "ada".into(),
            source: SOURCE_PANEL.into(),
            token: "t".into(),
            kind: KIND_APPLY.into(),
            owner: "bob".into(),
            conversation: "c".into(),
            skill: "s".into(),
            calls: vec![Call {
                tool: "status_get".into(),
                args: "{}".into(),
                error: true,
            }],
            more_calls: 2,
            proposal: Some(Prop {
                id: "p".into(),
                revision: 2,
                base_version: 4,
                steps: vec![("config_propose".into(), "x".into())],
                changed: vec!["routes".into()],
            }),
            outcome: APPLIED.into(),
            detail: "d".into(),
            version: 5,
        };
        let log = Log {
            entries: [e.clone()].into(),
            seq: 3,
            dirty: false,
        };
        let back = Log::read(log.section().as_bytes()).unwrap();
        assert_eq!(back.entries[0], e);
        assert_eq!(back.seq, 3);
        let bare = Entry {
            seq: 1,
            at: 2,
            user: "ada".into(),
            source: SOURCE_PANEL.into(),
            kind: KIND_CHAT.into(),
            outcome: "answer".into(),
            ..Entry::default()
        };
        assert_eq!(
            bare.json(),
            r#"{"seq":1,"at":2,"user":"ada","source":"panel","kind":"chat","outcome":"answer"}"#
        );
        assert!(Log::read(b"[1]").is_err());
    }
}
