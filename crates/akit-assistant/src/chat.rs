//! A chat (I162, fit's `assistant/chat.go`): one request, `POST /api/chat`, answered as
//! Server-Sent Events, during which the assistant calls the model, runs the tools it asks for
//! and calls it again, until an answer, a question to the person (`ask_user`) or a limit.
//!
//! The events are fit's: `start`, `text`, `thinking`, `tool_call`, `tool_result`, `ask`, `usage`,
//! `summary`, `proposal` (a step joined the conversation's proposal, I163), and last the SSE event
//! `end` (`done` with its reason) or `error`. An answer that called tools is in the audit log. The tools run on a
//! pool of the assistant's own, each within 30 s; nothing blocks an I/O thread.
//!
//! The browser's stream and the model's are linked (rule 8, decision row 102): once more than
//! [`WATERMARK`] bytes of events wait to be sent, the model's answer is held, and resumed when
//! they have drained; a browser that takes nothing for [`STALL_TIMEOUT`] has its stream cut, which
//! stops the chat, as fit's. A quiet stream gets a comment every [`PING_INTERVAL`], for proxies.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use akio::timer::SendTimer;
use akio_http::BodyStream;
use akit_config::assistant::{Assistant as Settings, Level};
use akit_config::value::Value;

use crate::conv::{Msg, now_ms};
use crate::json::{self, Obj};
use crate::model::{self, Answer, Event as ModelEvent, Message, Request, Role, ToolCall};
use crate::prompt::{self, Budget};
use crate::redact::Redactor;
use crate::registry::{Call, Kind, Tool};
use crate::tools::MAX_ASK_OPTIONS;
use crate::{Assistant, audit};

/// A tool's call is given up on past it: the model is told it failed.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a quiet stream gets a comment.
pub const PING_INTERVAL: Duration = Duration::from_secs(15);
/// How long a browser that takes nothing is waited for.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// The bytes of events waiting to be sent past which the model's answer is held.
pub const WATERMARK: usize = 64 << 10;

/// The reasons a chat ends with.
pub const REASON_ANSWER: &str = "answer";
/// See [`REASON_ANSWER`].
pub const REASON_ASK: &str = "ask";
/// See [`REASON_ANSWER`].
pub const REASON_TOOL_CALLS: &str = "max_tool_calls";
/// See [`REASON_ANSWER`].
pub const REASON_TOKENS: &str = "max_tokens_per_day";
/// See [`REASON_ANSWER`].
pub const REASON_STOPPED: &str = "stopped";
/// See [`REASON_ANSWER`]: a model call failed.
pub const REASON_ERROR: &str = "error";

/// An event of a chat's stream, as fit's `Event` writes it (members left out when empty).
#[derive(Debug, Default)]
pub(crate) struct Ev<'a> {
    pub(crate) typ: &'a str,
    pub(crate) text: &'a str,
    pub(crate) id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) arguments: &'a str,
    pub(crate) content: &'a str,
    pub(crate) error: bool,
    pub(crate) cut: bool,
    pub(crate) question: &'a str,
    pub(crate) options: Option<&'a [String]>,
    pub(crate) usage: Option<&'a model::Usage>,
    pub(crate) today: u64,
    pub(crate) conversation: &'a str,
    pub(crate) title: &'a str,
    pub(crate) model: &'a str,
    pub(crate) folded: usize,
    pub(crate) revision: u64,
    pub(crate) reason: &'a str,
}

impl Ev<'_> {
    pub(crate) fn json(&self) -> String {
        let mut o = Obj::new()
            .str("type", self.typ)
            .str_omit("text", self.text)
            .str_omit("id", self.id)
            .str_omit("name", self.name);
        if !self.arguments.is_empty() {
            o = o.raw("arguments", self.arguments);
        }
        o = o.str_omit("content", self.content);
        if self.error {
            o = o.bool("error", true);
        }
        if self.cut {
            o = o.bool("cut", true);
        }
        o = o.str_omit("question", self.question);
        if let Some(opts) = self.options {
            o = o.raw("options", &json::strings(opts));
        }
        if let Some(u) = self.usage {
            o = o.raw("usage", &u.json());
        }
        if self.today > 0 {
            o = o.num("today", self.today);
        }
        o = o
            .str_omit("conversation", self.conversation)
            .str_omit("title", self.title)
            .str_omit("model", self.model);
        if self.folded > 0 {
            o = o.num("folded", self.folded);
        }
        if self.revision > 0 {
            o = o.num("revision", self.revision);
        }
        o.str_omit("reason", self.reason).end()
    }

    fn sse(&self) -> String {
        format!("data: {}\n\n", self.json())
    }
}

/// `ask_user`'s arguments, checked.
pub(crate) fn parse_ask(args: &str) -> Result<(String, Vec<String>), String> {
    let bad = || format!("ask_user takes a question and 1 to {MAX_ASK_OPTIONS} options");
    let v = akit_config::value::parse(args).map_err(|_| bad())?;
    let q = json::str_of(&v, "question").trim().to_owned();
    let opts: Vec<String> = match v.get("options") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|o| match o {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if q.is_empty() || opts.is_empty() || opts.len() > MAX_ASK_OPTIONS {
        return Err(bad());
    }
    Ok((q, opts))
}

/// What runs once a tool's call has its result.
type Next = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A chat under way: the loop between the model and the tools for one message of the person's,
/// streamed to the browser that sent it.
pub(crate) struct Chat {
    pub(crate) a: Assistant,
    pub(crate) user: String,
    pub(crate) level: Level,
    pub(crate) s: Arc<Settings>,
    pub(crate) p: model::Provider,
    pub(crate) model: String,
    pub(crate) window: u64,
    pub(crate) tools: Vec<Tool>,
    pub(crate) red: Redactor,
    pub(crate) page: String,
    pub(crate) lang: String,
    pub(crate) conv: String,
    calls: AtomicU64,
    limited: AtomicBool,
    summarized: AtomicBool,
    st: Mutex<State>,
    /// The browser's stream, behind a lock of its own: what it calls back takes `st`.
    stream: Mutex<Option<BodyStream>>,
    /// The tools called, for the chat's audit entry, and the proposal its propose tools left.
    audit: Mutex<(Vec<audit::Call>, Option<audit::Prop>)>,
}

#[derive(Default)]
struct State {
    ended: bool,
    call: Option<model::Call>,
    /// The answer streaming in, kept when the chat is stopped.
    partial: String,
    /// Whether the model's answer is held while the browser catches up.
    held: bool,
    stall: Option<SendTimer>,
    ping: Option<SendTimer>,
}

impl std::fmt::Debug for Chat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chat")
            .field("user", &self.user)
            .field("conv", &self.conv)
            .finish_non_exhaustive()
    }
}

impl Chat {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        a: Assistant,
        user: String,
        level: Level,
        s: Arc<Settings>,
        p: model::Provider,
        model: String,
        window: u64,
        tools: Vec<Tool>,
        red: Redactor,
        page: String,
        lang: String,
        conv: String,
    ) -> Chat {
        Chat {
            a,
            user,
            level,
            s,
            p,
            model,
            window,
            tools,
            red,
            page,
            lang,
            conv,
            calls: AtomicU64::new(0),
            limited: AtomicBool::new(false),
            summarized: AtomicBool::new(false),
            st: Mutex::new(State::default()),
            stream: Mutex::new(None),
            audit: Mutex::new((Vec::new(), None)),
        }
    }

    /// Starts the stream on `stream`: its first event, and the ping.
    pub(crate) fn open(self: &Arc<Self>, stream: BodyStream, title: &str) {
        *lock(&self.stream) = Some(stream);
        self.emit(&Ev {
            typ: "start",
            conversation: &self.conv,
            title,
            model: &self.model,
            ..Ev::default()
        });
        self.arm_ping();
    }

    fn arm_ping(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let t = akio::timer::after_send(PING_INTERVAL, move || {
            if let Some(c) = me.upgrade() {
                if !c.ended() {
                    c.write(": ping\n\n");
                    c.arm_ping();
                }
            }
        })
        .ok();
        let old = std::mem::replace(&mut lock(&self.st).ping, t);
        if let Some(o) = old {
            let _ = o.cancel();
        }
    }

    pub(crate) fn ended(&self) -> bool {
        lock(&self.st).ended
    }

    fn emit(self: &Arc<Self>, ev: &Ev<'_>) {
        self.write(&ev.sse());
    }

    /// Writes `data` to the browser; past the watermark, holds the model's answer until it
    /// drains (rule 8).
    fn write(self: &Arc<Self>, data: &str) {
        if self.ended() {
            return;
        }
        let queued = {
            let s = lock(&self.stream);
            let Some(s) = s.as_ref() else { return };
            if s.write(data.as_bytes()).is_err() {
                None
            } else {
                Some(s.queued_bytes())
            }
        };
        let Some(queued) = queued else {
            // The browser is gone, or too far behind: the chat stops.
            return self.stop();
        };
        if queued <= WATERMARK {
            return;
        }
        let call = {
            let mut st = lock(&self.st);
            if st.held || st.ended {
                return;
            }
            st.held = true;
            st.call.clone()
        };
        self.hold(call);
    }

    /// Holds the model's answer while the browser catches up, a stall timer running; resumes it
    /// once what waits has drained.
    fn hold(self: &Arc<Self>, call: Option<model::Call>) {
        let me = Arc::downgrade(self);
        let stall = akio::timer::after_send(STALL_TIMEOUT, move || {
            if let Some(c) = me.upgrade() {
                c.stop();
            }
        })
        .ok();
        lock(&self.st).stall = stall;
        if let Some(c) = &call {
            c.hold(true);
        }
        let me = Arc::clone(self);
        let drained = move || {
            let (call, stall) = {
                let mut st = lock(&me.st);
                st.held = false;
                (st.call.clone(), st.stall.take())
            };
            if let Some(t) = stall {
                let _ = t.cancel();
            }
            if let Some(c) = call {
                c.hold(false);
            }
        };
        // `on_drain` may run `drained` at once, on this thread: only the stream's lock is held,
        // which `drained` does not take.
        let s = lock(&self.stream);
        match s.as_ref() {
            Some(s) => s.on_drain(drained),
            None => {
                drop(s);
                drained();
            }
        }
    }

    /// Keeps the model call under way, cancelling it when the chat has ended; held when the
    /// browser is behind.
    fn set_call(&self, k: model::Call) {
        let (ended, held) = {
            let mut st = lock(&self.st);
            st.call = Some(k.clone());
            (st.ended, st.held)
        };
        if ended {
            k.cancel("the chat was stopped");
        } else if held {
            k.hold(true);
        }
    }

    /// Ends the chat as the person stopped it, or left: the model call is cancelled, and what
    /// the answer had come to is kept.
    pub(crate) fn stop(self: &Arc<Self>) {
        let call = lock(&self.st).call.clone();
        self.finish(REASON_STOPPED, "");
        if let Some(c) = call {
            c.cancel("the chat was stopped");
        }
    }

    /// Ends the chat once: the conversation kept and let go, the slot given back, then the
    /// last event and the stream's end.
    pub(crate) fn finish(self: &Arc<Self>, reason: &str, err: &str) {
        let (partial, ping, stall) = {
            let mut st = lock(&self.st);
            if st.ended {
                return;
            }
            st.ended = true;
            (
                std::mem::take(&mut st.partial),
                st.ping.take(),
                st.stall.take(),
            )
        };
        for t in [ping, stall].into_iter().flatten() {
            let _ = t.cancel();
        }
        let (calls, prop) = std::mem::take(&mut *lock(&self.audit));
        if !calls.is_empty() {
            self.a.audit(audit::Entry {
                user: self.user.clone(),
                kind: audit::KIND_CHAT.to_owned(),
                conversation: self.conv.clone(),
                calls,
                proposal: prop,
                outcome: if err.is_empty() { reason } else { REASON_ERROR }.to_owned(),
                detail: err.to_owned(),
                ..audit::Entry::default()
            });
        }
        {
            let mut chats = self.a.chats();
            let u = chats.users.entry(self.user.clone()).or_default();
            if let Some(conv) = u.get(&self.conv) {
                if !partial.is_empty() && reason != REASON_ANSWER {
                    let mut m = Msg::of(Message::new(Role::Assistant, &partial));
                    m.stopped = true;
                    conv.messages.push(m);
                }
                if !err.is_empty() {
                    let mut m = Msg::of(Message::new(Role::Assistant, ""));
                    m.error = err.to_owned();
                    conv.messages.push(m);
                }
                if reason != REASON_ASK {
                    conv.close_pending("error: not run: the answer was stopped");
                }
                conv.busy = false;
                conv.updated = now_ms();
                conv.trim();
            }
            u.bound(usize::try_from(self.s.limits.conversations()).unwrap_or(usize::MAX));
        }
        self.a.end_chat(&self.user, &self.conv);
        self.a.save_chats(&self.user);
        let last = if err.is_empty() {
            format!(
                "event: end\ndata: {}\n\n",
                Ev {
                    typ: "done",
                    reason,
                    conversation: &self.conv,
                    ..Ev::default()
                }
                .json()
            )
        } else {
            format!(
                "event: error\ndata: {}\n\n",
                Obj::new().str("message", err).end()
            )
        };
        let stream = lock(&self.stream).take();
        if let Some(s) = stream {
            let _ = s.write(last.as_bytes());
            s.finish();
        }
    }

    fn add_usage(self: &Arc<Self>, u: &model::Usage) {
        let today = {
            let mut chats = self.a.chats();
            let uc = chats.users.entry(self.user.clone()).or_default();
            uc.used_today();
            uc.tokens = uc
                .tokens
                .saturating_add(u.input_tokens)
                .saturating_add(u.output_tokens);
            uc.tokens
        };
        self.emit(&Ev {
            typ: "usage",
            usage: Some(u),
            today,
            ..Ev::default()
        });
    }

    fn append(&self, m: Msg) {
        let mut chats = self.a.chats();
        if let Some(c) = chats
            .users
            .get_mut(&self.user)
            .and_then(|u| u.get(&self.conv))
        {
            c.messages.push(m);
        }
    }

    /// The next call, or, when the conversation outgrew its window and its early turns may be
    /// summarized, up to where.
    fn request(&self) -> Result<Request, usize> {
        let (msgs, from, summary, proposal) = {
            let mut chats = self.a.chats();
            match chats
                .users
                .get_mut(&self.user)
                .and_then(|u| u.get(&self.conv))
            {
                Some(c) => (
                    c.messages.clone(),
                    c.folded,
                    c.summary.clone(),
                    crate::proposal::state(c.proposal.as_ref(), &c.proposal_note),
                ),
                None => (Vec::new(), 0, String::new(), String::new()),
            }
        };
        let system = prompt::system(&prompt::Parts {
            level: self.level,
            lang: &self.lang,
            page: &self.page,
            summary: &summary,
            proposal: &self.red.text(&proposal),
        });
        let tools: Vec<model::Tool> = self.tools.iter().map(Tool::model).collect();
        let tools_text: String = tools
            .iter()
            .map(|t| format!("{}{}{}", t.name, t.description, t.parameters))
            .collect();
        let max = self.s.params.max_output();
        let b = Budget {
            window: self.window,
            output: max.min(self.window / 4),
            fixed: prompt::tokens(&system).saturating_add(prompt::tokens(&tools_text)),
        };
        let mut req = Request {
            model: self.model.clone(),
            system,
            messages: prompt::history(msgs.get(from..).unwrap_or(&[])),
            tools,
            max_tokens: max,
            temperature: self.s.params.temperature.clone(),
            thinking: self.s.params.thinking_level().to_owned(),
        };
        if prompt::fit_history(&mut req.messages, b.room())
            || self.summarized.load(Ordering::SeqCst)
        {
            return Ok(req);
        }
        let k = prompt::fold_point(&msgs, from, b.room());
        if k > from { Err(k) } else { Ok(req) }
    }

    /// The loop's next model call.
    pub(crate) fn step(self: &Arc<Self>) {
        if self.ended() {
            return;
        }
        let spent = {
            let mut chats = self.a.chats();
            chats
                .users
                .entry(self.user.clone())
                .or_default()
                .used_today()
                >= self.s.limits.tokens_per_day()
        };
        if spent {
            return self.finish(REASON_TOKENS, "");
        }
        let req = match self.request() {
            Ok(r) => r,
            Err(fold) => return self.summarize(fold),
        };
        lock(&self.st).partial.clear();
        let Some(client) = self.a.client() else {
            return self.finish(REASON_ERROR, "the assistant has no engine to call with");
        };
        let (on, done) = (Arc::clone(self), Arc::clone(self));
        let k = client.stream(
            &self.p,
            &req,
            self.s.limits.stream_bytes(),
            move |e| on.on_event(e),
            move |r| done.on_done(r),
        );
        self.set_call(k);
    }

    /// Has the model summarize the turns up to `fold`, keeps the summary with the conversation,
    /// and goes on; a summary that fails leaves the turns to be cut instead.
    fn summarize(self: &Arc<Self>, fold: usize) {
        self.summarized.store(true, Ordering::SeqCst);
        let (msgs, prev) = {
            let mut chats = self.a.chats();
            match chats
                .users
                .get_mut(&self.user)
                .and_then(|u| u.get(&self.conv))
            {
                Some(c) => (
                    c.messages.get(c.folded..fold).unwrap_or(&[]).to_vec(),
                    c.summary.clone(),
                ),
                None => (Vec::new(), String::new()),
            }
        };
        let mut text = prompt::transcript(&prev, &msgs);
        let limit = usize::try_from(self.window.saturating_mul(3) / 2).unwrap_or(usize::MAX);
        if text.len() > limit {
            text = prompt::cut_text(&text, limit).0;
        }
        let req = Request {
            model: self.model.clone(),
            system: prompt::SUMMARY_PROMPT.to_owned(),
            max_tokens: 1024,
            messages: vec![Message::new(Role::User, &text)],
            ..Request::default()
        };
        let Some(client) = self.a.client() else {
            return self.finish(REASON_ERROR, "the assistant has no engine to call with");
        };
        let me = Arc::clone(self);
        let k = client.stream(
            &self.p,
            &req,
            self.s.limits.stream_bytes(),
            |_| {},
            move |r| {
                if me.ended() {
                    return;
                }
                if let Ok(a) = r {
                    let s = a.text.trim().to_owned();
                    if !s.is_empty() {
                        me.add_usage(&a.usage);
                        {
                            let mut chats = me.a.chats();
                            if let Some(c) =
                                chats.users.get_mut(&me.user).and_then(|u| u.get(&me.conv))
                            {
                                c.summary = s;
                                c.folded = fold;
                            }
                        }
                        me.emit(&Ev {
                            typ: "summary",
                            folded: fold,
                            ..Ev::default()
                        });
                    }
                }
                me.step();
            },
        );
        self.set_call(k);
    }

    fn on_event(self: &Arc<Self>, e: ModelEvent) {
        if self.ended() {
            return;
        }
        match e {
            ModelEvent::Text(t) => {
                lock(&self.st).partial.push_str(&t);
                self.emit(&Ev {
                    typ: "text",
                    text: &t,
                    ..Ev::default()
                });
            }
            ModelEvent::Thinking(t) => self.emit(&Ev {
                typ: "thinking",
                text: &t,
                ..Ev::default()
            }),
            ModelEvent::ToolCall(tc) => self.emit(&Ev {
                typ: "tool_call",
                id: &tc.id,
                name: &tc.name,
                arguments: &tc.arguments,
                ..Ev::default()
            }),
        }
    }

    fn on_done(self: &Arc<Self>, r: Result<Answer, model::Error>) {
        if self.ended() {
            return;
        }
        let a = match r {
            Ok(a) => a,
            Err(e) => return self.finish(REASON_ERROR, &self.red.text(&e.to_string())),
        };
        self.add_usage(&a.usage);
        lock(&self.st).partial.clear();
        self.append(Msg::of(a.message()));
        if a.tool_calls.is_empty() {
            return self.finish(REASON_ANSWER, "");
        }
        self.run_calls(Arc::new(a.tool_calls), 0, None);
    }

    fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Runs the calls from `i` on, one at a time, then goes on: the next step, the question
    /// asked, or the end at a limit.
    fn run_calls(
        self: &Arc<Self>,
        calls: Arc<Vec<ToolCall>>,
        mut i: usize,
        mut ask: Option<usize>,
    ) {
        let max_calls = self.s.limits.tool_calls();
        while let Some(tc) = calls.get(i) {
            if self.ended() {
                return;
            }
            let Some(t) = self.tool(&tc.name).cloned() else {
                self.result(
                    None,
                    tc,
                    &format!("error: there is no tool {} you may call", tc.name),
                    true,
                );
                i = i.saturating_add(1);
                continue;
            };
            let level_now = self.a.level(&self.user);
            if tc.invalid {
                self.result(
                    Some(&t),
                    tc,
                    &format!(
                        "error: the arguments are not JSON: {}",
                        prompt::clip(&tc.arguments, 200)
                    ),
                    true,
                );
            } else if self.calls.load(Ordering::SeqCst) >= max_calls {
                self.limited.store(true, Ordering::SeqCst);
                self.result(
                    Some(&t),
                    tc,
                    &format!(
                        "error: not run: the answer reached its limit of {max_calls} tool calls"
                    ),
                    true,
                );
            } else if let Err(why) = t.allowed(level_now, &self.a.settings()) {
                // The user's level, checked on every call against the settings in use now.
                self.result(
                    Some(&t),
                    tc,
                    &format!(
                        "error: not run: {}'s level is {} now, and the tool {why}",
                        self.user,
                        level_now.name()
                    ),
                    true,
                );
            } else if t.kind == Kind::Ask {
                self.calls.fetch_add(1, Ordering::SeqCst);
                match parse_ask(&tc.arguments) {
                    Ok(_) if ask.is_none() => ask = Some(i),
                    Ok(_) => self.result(Some(&t), tc, "error: one question at a time", true),
                    Err(e) => self.result(Some(&t), tc, &format!("error: {e}"), true),
                }
            } else {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let next = i.saturating_add(1);
                let me = Arc::clone(self);
                let calls2 = Arc::clone(&calls);
                self.run_pool(&t, tc.clone(), move || me.run_calls(calls2, next, ask));
                return;
            }
            i = i.saturating_add(1);
        }
        let asked = ask.and_then(|k| calls.get(k));
        if self.limited.load(Ordering::SeqCst) {
            if let Some(tc) = asked {
                let t = self.tool(&tc.name).cloned();
                self.result(
                    t.as_ref(),
                    tc,
                    "error: not asked: the answer reached its limit of tool calls",
                    true,
                );
            }
            return self.finish(REASON_TOOL_CALLS, "");
        }
        if let Some(tc) = asked {
            if let Ok((q, opts)) = parse_ask(&tc.arguments) {
                let opts: Vec<String> = opts.iter().map(|o| self.red.text(o)).collect();
                self.emit(&Ev {
                    typ: "ask",
                    id: &tc.id,
                    question: &self.red.text(&q),
                    options: Some(&opts),
                    ..Ev::default()
                });
            }
            return self.finish(REASON_ASK, "");
        }
        self.step();
    }

    /// Runs a call of `t` on the tools' pool, within [`TOOL_TIMEOUT`], and then `next`.
    fn run_pool(self: &Arc<Self>, t: &Tool, tc: ToolCall, next: impl FnOnce() + Send + 'static) {
        let done = Arc::new(AtomicBool::new(false));
        let next: Next = Arc::new(Mutex::new(Some(Box::new(next))));
        let deliver = {
            let (me, done, next, t, tc) = (
                Arc::clone(self),
                Arc::clone(&done),
                Arc::clone(&next),
                t.clone(),
                tc.clone(),
            );
            move |out: Result<String, String>| {
                if done.swap(true, Ordering::SeqCst) {
                    return;
                }
                match out {
                    Ok(s) => me.result(Some(&t), &tc, &s, false),
                    Err(e) => me.result(Some(&t), &tc, &format!("error: {e}"), true),
                }
                if let Some(f) = lock(&next).take() {
                    f();
                }
            }
        };
        let deliver = Arc::new(deliver);
        let d2 = Arc::clone(&deliver);
        let timer = akio::timer::after_send(TOOL_TIMEOUT, move || {
            d2(Err(format!(
                "the tool took longer than {} s",
                TOOL_TIMEOUT.as_secs()
            )));
        })
        .ok();
        let (me, d3, run, late) = (Arc::clone(self), Arc::clone(&deliver), t.run, done);
        let args = if tc.arguments.is_empty() {
            Value::Object(Vec::new())
        } else {
            akit_config::value::parse(&tc.arguments).unwrap_or(Value::Object(Vec::new()))
        };
        let job = Box::new(move || {
            let s = me.a.settings();
            let call = Call {
                a: &me.a,
                args,
                user: &me.user,
                settings: &s,
                conv: &me.conv,
                expired: Some(&*late),
                chat: Some(&me),
            };
            let out = match run {
                Some(f) => f(&call),
                None => Err("the tool has nothing to run".to_owned()),
            };
            if let Some(t) = timer {
                let _ = t.cancel();
            }
            d3(out);
        });
        if self.a.0.tools_pool.try_execute(job).is_err() {
            deliver(Err("the assistant is busy; try again shortly".to_owned()));
        }
    }

    /// A propose tool's step joined `p`, which changes `changed`: kept for the chat's audit entry,
    /// and shown.
    pub(crate) fn on_proposed(self: &Arc<Self>, p: &crate::proposal::Proposal, changed: &[String]) {
        lock(&self.audit).1 = Some(audit::Prop::of(p, changed));
        let (name, summary) = p.steps.last().map_or(("", String::new()), |s| {
            (s.tool.as_str(), self.red.text(&s.summary))
        });
        self.emit(&Ev {
            typ: "proposal",
            id: &p.id,
            revision: p.revision,
            name,
            text: &summary,
            ..Ev::default()
        });
    }

    /// Keeps a call's result, redacted and bounded, as the model gets it, and shows it to the
    /// browser.
    fn result(self: &Arc<Self>, t: Option<&Tool>, tc: &ToolCall, out: &str, is_err: bool) {
        if self.ended() {
            return;
        }
        let mut limit = self.s.limits.tool_result_bytes();
        if let Some(t) = t.filter(|t| t.max_bytes > 0 && t.max_bytes < limit) {
            limit = t.max_bytes;
        }
        let (out, cut) = prompt::cut_text(
            &self.red.text(out),
            usize::try_from(limit).unwrap_or(usize::MAX),
        );
        lock(&self.audit).0.push(audit::Call {
            tool: tc.name.clone(),
            args: prompt::clip(&self.red.text(&tc.arguments), audit::MAX_ARGS),
            error: is_err,
        });
        let mut m = Message::tool(tc, &out, is_err);
        m.tool_name.clone_from(&tc.name);
        let mut msg = Msg::of(m);
        msg.cut = cut;
        self.append(msg);
        self.emit(&Ev {
            typ: "tool_result",
            id: &tc.id,
            name: &tc.name,
            content: &out,
            error: is_err,
            cut,
            ..Ev::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_asks() {
        let e = Ev {
            typ: "tool_call",
            id: "c1",
            name: "status_get",
            arguments: "{}",
            ..Ev::default()
        };
        assert_eq!(
            e.json(),
            r#"{"type":"tool_call","id":"c1","name":"status_get","arguments":{}}"#
        );
        assert!(parse_ask(r#"{"question": "Which?", "options": ["a", "b"]}"#).is_ok());
        assert!(parse_ask(r#"{"question": " ", "options": ["a"]}"#).is_err());
        assert!(parse_ask(r#"{"question": "q", "options": []}"#).is_err());
    }
}
