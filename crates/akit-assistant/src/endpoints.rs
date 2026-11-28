//! The chat's endpoints (I162, fit's `chat.go` and `endpoints.go`): `POST /api/chat` and
//! `/api/chat/stop`, `GET /api/tools`, `GET /api/conversations`, and `GET` and `DELETE
//! /api/conversations/<id>`.

use std::sync::Arc;

use akio_http::{Context, Headers};
use akit_config::assistant::Level;
use akit_config::value::Value;
use akit_roles::pages;

use crate::Assistant;
use crate::chat::{Chat, Ev, parse_ask};
use crate::conv::{Conversation, MAX_MESSAGE_BYTES, Msg, new_id, now_ms};
use crate::json::{self, Obj};
use crate::model::{Message, Role};
use crate::prompt::{self, DEFAULT_CONTEXT_TOKENS, MAX_PAGE_BYTES};
use crate::redact::HIDDEN;

/// What the person sent, checked: a message, a retry of the last answer, or the choice of an
/// `ask_user` question.
enum Sent {
    Message(String),
    Retry,
    Answer(String, String),
}

/// The members of the page context the prompt carries, fit's `PageContext`.
const PAGE_MEMBERS: &[&str] = &[
    "module",
    "page",
    "title",
    "selection",
    "text",
    "editable",
    "errors",
];

impl Assistant {
    /// `POST /api/chat`: the checks that need nothing kept here, on the I/O thread; the rest on
    /// the pool, which starts the stream and the loop.
    pub(crate) fn chat(&self, ctx: &mut Context, user: &str) {
        let body = match pages::decode(ctx) {
            Ok(b) => b.0,
            Err(r) => return ctx.respond(r),
        };
        let s = self.settings();
        let level = self.level(user);
        let message = match body.get("message") {
            Some(Value::String(m)) => Some(m.clone()),
            _ => None,
        };
        let retry = matches!(body.get("retry"), Some(Value::Bool(true)));
        let answer = body.get("answer").filter(|a| **a != Value::Null);
        let conv_id = json::str_of(&body, "conversation").to_owned();
        let modes = [message.is_some(), retry, answer.is_some()]
            .iter()
            .filter(|x| **x)
            .count();
        if level == Level::Off {
            return ctx.respond(pages::fail(
                403,
                &format!("the assistant is off for {user}"),
            ));
        }
        if modes != 1 {
            return ctx.respond(pages::fail(400, "send one of message, retry and answer"));
        }
        if let Some(m) = &message {
            if m.is_empty() || m.len() > MAX_MESSAGE_BYTES {
                return ctx.respond(pages::fail(
                    400,
                    &format!("the message is 1 to {MAX_MESSAGE_BYTES} bytes of text"),
                ));
            }
        }
        if conv_id.is_empty() && message.is_none() {
            return ctx.respond(pages::fail(
                400,
                "a retry or an answer names its conversation",
            ));
        }
        let sent = match (message, answer) {
            (Some(m), _) => Sent::Message(m),
            (None, Some(a)) => Sent::Answer(
                json::str_of(a, "id").to_owned(),
                json::str_of(a, "choice").to_owned(),
            ),
            (None, None) => Sent::Retry,
        };
        let (prov, model) = s.default_model();
        let (prov, model) = (prov.to_owned(), model.to_owned());
        if prov.is_empty() || model.is_empty() {
            return ctx.respond(pages::fail(
                409,
                "no model is set: add a provider with a model on the Settings page",
            ));
        }
        if !self.take_slot(s.limits.concurrent_chats()) {
            return ctx.respond(pages::fail(
                429,
                "the assistant is answering as many chats as it may at once; try again shortly",
            ));
        }
        let page = self.page_json(body.get("page"));
        let lang = json::str_of(&body, "lang").to_owned();
        let Some(mut d) = ctx.detach() else {
            return self.give_slot();
        };
        let gone: Arc<std::sync::Mutex<Option<Arc<Chat>>>> = Arc::default();
        let g2 = Arc::clone(&gone);
        d.on_gone(move || {
            let c = g2.lock().ok().and_then(|g| g.clone());
            if let Some(c) = c {
                c.stop();
            }
        });
        let me = self.clone();
        let user = user.to_owned();
        self.run(d, move |d| {
            let Some(p) = s.provider(&prov) else {
                me.give_slot();
                return d.respond(pages::fail(409, &format!("no provider is named {prov:?}")));
            };
            let key = me.key(&prov).map(|(k, _)| k).unwrap_or_default();
            let mp = match me.resolve(p, key) {
                Ok(mp) => mp,
                Err(why) => {
                    me.give_slot();
                    return d.respond(pages::fail(409, &why));
                }
            };
            let (mut window, mut use_tools) = (DEFAULT_CONTEXT_TOKENS, true);
            if let Some(m) = p.models.iter().find(|m| m.name == model) {
                use_tools = m.calls_tools();
                if m.context > 0 {
                    window = m.context;
                }
            }
            let tools = if use_tools {
                me.0.registry.for_level(level, &s)
            } else {
                Vec::new()
            };
            let red = me.redactor(&s);
            let (conv, title) = match me.prepare(&user, &conv_id, sent, &s) {
                Ok(x) => x,
                Err((status, msg)) => {
                    me.give_slot();
                    return d.respond(pages::fail(status, &msg));
                }
            };
            let c = Arc::new(Chat::new(
                me.clone(),
                user.clone(),
                level,
                Arc::clone(&s),
                mp,
                model,
                window,
                tools,
                red,
                page,
                lang,
                conv.clone(),
            ));
            me.start_chat(&user, &conv, &c);
            let mut h = Headers::new();
            h.set("Content-Type", "text/event-stream; charset=utf-8");
            h.set("X-Accel-Buffering", "no");
            h.set("Cache-Control", "no-store");
            let Some(stream) = d.stream_response(200, h) else {
                return c.finish(crate::chat::REASON_STOPPED, "");
            };
            if let Ok(mut g) = gone.lock() {
                *g = Some(Arc::clone(&c));
            }
            c.open(stream, &title);
            c.step();
        });
    }

    /// The page context as the prompt carries it: its known members, redacted, bounded.
    fn page_json(&self, page: Option<&Value>) -> String {
        let Some(Value::Object(ms)) = page else {
            return String::new();
        };
        let kept: Vec<(String, Value)> = ms
            .iter()
            .filter(|(k, v)| PAGE_MEMBERS.contains(&k.as_str()) && *v != Value::Null)
            .cloned()
            .collect();
        if kept.is_empty() {
            return String::new();
        }
        let red = self
            .redactor(&self.settings())
            .json(&Value::Object(kept), HIDDEN);
        if red.len() > MAX_PAGE_BYTES {
            prompt::cut_text(&red, MAX_PAGE_BYTES).0
        } else {
            red
        }
    }

    /// Finds or makes the conversation, adds what the person sent, and marks it under way: its
    /// ID and title, or the status and why it cannot be.
    fn prepare(
        &self,
        user: &str,
        id: &str,
        sent: Sent,
        s: &akit_config::assistant::Assistant,
    ) -> Result<(String, String), (u16, String)> {
        let mut chats = self.chats();
        let u = self.user_chats(&mut chats, user);
        let exists = !id.is_empty() && u.get(id).is_some();
        if !id.is_empty() && !exists {
            return Err((404, format!("no conversation {id}")));
        }
        if u.get(id).is_some_and(|c| c.busy) {
            return Err((
                409,
                "the conversation is answering already: stop it first".to_owned(),
            ));
        }
        let max_turns = usize::try_from(s.limits.turns()).unwrap_or(usize::MAX);
        if matches!(sent, Sent::Message(_)) && u.get(id).is_some_and(|c| c.turns() >= max_turns) {
            return Err((
                409,
                format!("the conversation has its {max_turns} turns: start a new one"),
            ));
        }
        if u.used_today() >= s.limits.tokens_per_day() {
            return Err((
                429,
                format!(
                    "{user} has spent today's {} tokens",
                    s.limits.tokens_per_day()
                ),
            ));
        }
        let now = now_ms();
        let conv_id = match sent {
            Sent::Message(m) => {
                let cid = if exists {
                    id.to_owned()
                } else {
                    let c = Conversation {
                        id: new_id(),
                        title: prompt::title(&m),
                        created: now,
                        updated: now,
                        ..Conversation::default()
                    };
                    let cid = c.id.clone();
                    u.convs.push(c);
                    cid
                };
                if let Some(c) = u.get(&cid) {
                    c.close_pending(
                        "(no choice was made: the person wrote the next message instead)",
                    );
                    c.messages.push(Msg::of(Message::new(Role::User, &m)));
                }
                cid
            }
            Sent::Retry => {
                let Some(c) = u.get(id) else {
                    return Err((404, format!("no conversation {id}")));
                };
                let Some(last) = c
                    .messages
                    .iter()
                    .rposition(|m| m.message.role == Role::User)
                else {
                    return Err((
                        409,
                        "the conversation has no message to answer again".to_owned(),
                    ));
                };
                c.messages.truncate(last.saturating_add(1));
                id.to_owned()
            }
            Sent::Answer(ask_id, choice) => {
                let Some(c) = u.get(id) else {
                    return Err((404, format!("no conversation {id}")));
                };
                let Some(tc) = c.pending_ask().filter(|tc| tc.id == ask_id) else {
                    return Err((
                        409,
                        "the conversation is not waiting for that answer".to_owned(),
                    ));
                };
                let ok = parse_ask(&tc.arguments).is_ok_and(|(_, o)| o.contains(&choice));
                if !ok {
                    return Err((
                        400,
                        "the choice is not one of the question's options".to_owned(),
                    ));
                }
                let mut m = Message::tool(&tc, &choice, false);
                m.tool_name.clone_from(&tc.name);
                c.messages.push(Msg::of(m));
                id.to_owned()
            }
        };
        let title = match u.get(&conv_id) {
            Some(c) => {
                c.updated = now;
                c.busy = true;
                c.title.clone()
            }
            None => String::new(),
        };
        u.bound(usize::try_from(s.limits.conversations()).unwrap_or(usize::MAX));
        Ok((conv_id, title))
    }

    /// `POST /api/chat/stop`: the chat under way in the conversation is stopped, as when its
    /// browser leaves.
    pub(crate) fn stop_chat(&self, ctx: &mut Context, user: &str) {
        let body = match pages::decode(ctx) {
            Ok(b) => b.0,
            Err(r) => return ctx.respond(r),
        };
        let c = self.running(user, json::str_of(&body, "conversation"));
        if let Some(c) = &c {
            c.stop();
        }
        ctx.respond(pages::json(
            200,
            Obj::new().bool("stopped", c.is_some()).end(),
        ));
    }

    /// `GET /api/tools`: the user's level, the model the chat uses and whether it calls tools,
    /// and every tool with whether this user may call it under the settings in use.
    pub(crate) fn tools_view(&self, user: &str) -> String {
        let s = self.settings();
        let level = self.level(user);
        let (prov, model) = s.default_model();
        let use_tools = s
            .provider(prov)
            .and_then(|p| p.models.iter().find(|m| m.name == model))
            .is_none_or(akit_config::assistant::Model::calls_tools);
        let tools: Vec<String> = self
            .0
            .registry
            .tools()
            .iter()
            .map(|t| {
                let mut ok = t.allowed(level, &s);
                if ok.is_ok() && !use_tools {
                    ok = Err("the model does not call tools".to_owned());
                }
                t.view(ok)
            })
            .collect();
        Obj::new()
            .str("user", user)
            .str("level", level.name())
            .str("provider", prov)
            .str("model", model)
            .bool("use_tools", use_tools)
            .bool("store", self.0.o.store.is_some())
            .raw("tools", &format!("[{}]", tools.join(",")))
            .end()
    }

    /// `GET /api/conversations`: the user's conversations, the latest first.
    pub(crate) fn conversations(&self, ctx: &mut Context, user: &str) {
        let Some(d) = ctx.detach() else { return };
        let (me, user) = (self.clone(), user.to_owned());
        self.run(d, move |d| {
            let s = me.settings();
            let mut chats = me.chats();
            let u = me.user_chats(&mut chats, &user);
            let list: Vec<String> = u
                .convs
                .iter()
                .rev()
                .map(|c| {
                    Obj::new()
                        .str("id", &c.id)
                        .str("title", &c.title)
                        .num("created", c.created)
                        .num("updated", c.updated)
                        .num("messages", c.messages.len())
                        .bool("busy", c.busy)
                        .str_omit("skill", &c.skill)
                        .end()
                })
                .collect();
            let used = u.used_today();
            drop(chats);
            d.respond(pages::json(
                200,
                Obj::new()
                    .raw("conversations", &format!("[{}]", list.join(",")))
                    .num("max", s.limits.conversations())
                    .num("max_turns", s.limits.turns())
                    .num("tokens_today", used)
                    .num("max_tokens_per_day", s.limits.tokens_per_day())
                    .bool("kept", me.0.o.store.is_some())
                    .end(),
            ));
        });
    }

    /// `GET` and `DELETE /api/conversations/<id>`.
    pub(crate) fn conversation(&self, ctx: &mut Context, user: &str, id: &str, delete: bool) {
        let Some(d) = ctx.detach() else { return };
        let (me, user, id) = (self.clone(), user.to_owned(), id.to_owned());
        self.run(d, move |d| {
            let mut chats = me.chats();
            let u = me.user_chats(&mut chats, &user);
            let Some(c) = u.get(&id) else {
                drop(chats);
                return d.respond(pages::fail(404, &format!("no conversation {id}")));
            };
            if delete {
                if c.busy {
                    drop(chats);
                    return d.respond(pages::fail(
                        409,
                        "the conversation is answering: stop it first",
                    ));
                }
                u.convs.retain(|c| c.id != id);
                drop(chats);
                me.save_chats(&user);
                return d.respond(pages::json(200, Obj::new().str("deleted", &id).end()));
            }
            let mut body = c.json(Some(c.busy));
            if !c.busy {
                if let Some(tc) = c.pending_ask() {
                    if let Ok((q, opts)) = parse_ask(&tc.arguments) {
                        let ask = Ev {
                            typ: "ask",
                            id: &tc.id,
                            question: &q,
                            options: Some(&opts),
                            ..Ev::default()
                        }
                        .json();
                        body.pop();
                        body.push_str(&format!(",\"ask\":{ask}}}"));
                    }
                }
            }
            drop(chats);
            d.respond(pages::json(200, body));
        });
    }
}
