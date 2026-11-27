//! The room chat (fit's `chat.go`, its I118): text messages and small files, carried between the
//! members of a room and kept in memory with it. A message is immutable once stored: its text,
//! its JSON and its event are built once, and its file's bytes are never written to again.
//!
//! - `POST chat`: `{text}` adds a text message, from the member the `X-Akit-Member` and
//!   `X-Akit-Room-Key` fields name, answered with the message; the room's watchers get it as a
//!   `chat` event.
//! - `POST files?file=<name>`: the body is a file, answered with the file's message; its bytes
//!   are downloaded at `GET files/<message id>`, until the message goes.
//!
//! Everything is bounded: a text by `max_message_bytes`, a file by `max_file_bytes`, a room by
//! [`CHAT_HISTORY`] messages, a hub's file bytes by 64 MiB (past it the oldest files go, whose
//! downloads then 404), and what a watcher that does not take its events holds. A file's body is
//! read whole by the listener, within its `max_body_bytes`, before the member is checked, where
//! fit's checks the member and reserves the bytes before reading it.

use std::sync::Arc;

use akio_http::{Context, Response};

use super::rooms::{Hub, Inner, Watcher, chat_file_bytes, member_name};
use super::{Settings, body, json, object, plain, quote, str_member};

/// How many messages a room keeps, text and file messages together; past it the room's oldest
/// goes.
pub(crate) const CHAT_HISTORY: usize = 64;
/// The most bytes a file's name may have, after it was sanitized.
const MAX_FILE_NAME: usize = 200;
/// The most bytes of a `Content-Type` a file's message keeps; a longer one is not a type.
const MAX_FILE_TYPE: usize = 256;

/// A message's file.
pub(crate) struct FileInfo {
    name: String,
    size: u64,
    ctype: String,
}

/// A chat message, immutable once stored.
pub(crate) struct Message {
    pub(crate) id: String,
    /// The hub's order, which evicts the oldest file.
    pub(crate) seq: u64,
    pub(crate) file: Option<FileInfo>,
    /// The message's JSON.
    pub(crate) js: Vec<u8>,
    /// The event that tells it.
    pub(crate) ev: Vec<u8>,
    /// A file's bytes.
    data: Vec<u8>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Message {
    fn new(
        seq: u64,
        from: &str,
        text: Option<&str>,
        file: Option<FileInfo>,
        data: Vec<u8>,
    ) -> Message {
        let id = super::rooms::key();
        let mut js = format!(
            "{{\"id\":{},\"from\":{},\"at\":{}",
            quote(&id),
            quote(from),
            now_ms()
        );
        if let Some(t) = text.filter(|t| !t.is_empty()) {
            js.push_str(&format!(",\"text\":{}", quote(t)));
        }
        if let Some(f) = &file {
            js.push_str(&format!(
                ",\"file\":{{\"name\":{},\"size\":{},\"type\":{}}}",
                quote(&f.name),
                f.size,
                quote(&f.ctype)
            ));
        }
        js.push('}');
        let ev = format!("event: chat\ndata: {js}\n\n").into_bytes();
        Message {
            id,
            seq,
            file,
            js: js.into_bytes(),
            ev,
            data,
        }
    }
}

/// The event that fills a browser's chat window when it joins the room: its messages, oldest
/// first.
pub(crate) fn history_event(msgs: &[Arc<Message>]) -> Vec<u8> {
    let mut b = b"event: history\ndata: {\"messages\":[".to_vec();
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            b.push(b',');
        }
        b.extend_from_slice(&m.js);
    }
    b.extend_from_slice(b"]}\n\n");
    b
}

/// Stores `msg` in `room`: counts its file's bytes, and drops the room's oldest message past
/// [`CHAT_HISTORY`].
fn store(i: &mut Inner, room: &str, msg: &Arc<Message>) {
    let size = msg.file.as_ref().map_or(0, |f| f.size);
    i.file_bytes = i.file_bytes.saturating_add(size);
    let mut dropped = 0u64;
    if let Some(r) = i.rooms.get_mut(room) {
        r.file_bytes = r.file_bytes.saturating_add(size);
        r.messages.push_back(Arc::clone(msg));
        while r.messages.len() > CHAT_HISTORY {
            if let Some(old) = r.messages.pop_front() {
                let s = old.file.as_ref().map_or(0, |f| f.size);
                r.file_bytes = r.file_bytes.saturating_sub(s);
                dropped = dropped.saturating_add(s);
            }
        }
    }
    i.file_bytes = i.file_bytes.saturating_sub(dropped);
}

/// Drops the hub's oldest file message, the rooms' messages being in order: false when there is
/// none.
fn evict_oldest_file(i: &mut Inner) -> bool {
    let oldest = i
        .rooms
        .iter()
        .filter_map(|(name, r)| {
            r.messages
                .iter()
                .find(|m| m.file.is_some())
                .map(|m| (m.seq, name.clone(), m.id.clone()))
        })
        .min_by_key(|(seq, _, _)| *seq);
    let Some((_, room, id)) = oldest else {
        return false;
    };
    let mut size = 0u64;
    if let Some(r) = i.rooms.get_mut(&room) {
        if let Some(pos) = r.messages.iter().position(|m| m.id == id) {
            if let Some(m) = r.messages.remove(pos) {
                size = m.file.as_ref().map_or(0, |f| f.size);
                r.file_bytes = r.file_bytes.saturating_sub(size);
            }
        }
    }
    i.file_bytes = i.file_bytes.saturating_sub(size);
    true
}

fn broadcast(ws: &[Arc<Watcher>], msg: &Arc<Message>) {
    for w in ws {
        w.post(msg);
    }
}

/// The member a chat request says it is.
fn member(ctx: &Context) -> (String, String) {
    let get = |n: &str| ctx.header(n).unwrap_or("").trim().to_owned();
    (get("x-akit-member"), get("x-akit-room-key"))
}

/// Answers a text message: `{text}`, as the member `X-Akit-Member` and `X-Akit-Room-Key` name.
pub(crate) fn chat(ctx: &mut Context, hub: &Hub, set: &Settings, room: &str) {
    let (name, key) = member(ctx);
    if !member_name(&name) || key.is_empty() {
        return plain(ctx, 404, "no such member", &[]);
    }
    let Some(b) = body(ctx, set.max_message) else {
        return;
    };
    if std::str::from_utf8(&b).is_err() {
        return plain(ctx, 400, "the message is not UTF-8", &[]);
    }
    let Some(text) = object(&b).and_then(|o| str_member(&o, "text").ok()) else {
        return plain(ctx, 400, "the body must be JSON: {\"text\"}", &[]);
    };
    if text.trim().is_empty() {
        return plain(ctx, 400, "the message is empty", &[]);
    }
    if text.len() > set.max_message {
        return plain(
            ctx,
            400,
            "the message is longer than max_message_bytes",
            &[],
        );
    }
    let r = hub.with_member(room, &name, &key, |i, from| {
        i.chat_seq = i.chat_seq.saturating_add(1);
        let msg = Arc::new(Message::new(
            i.chat_seq,
            from,
            Some(&text),
            None,
            Vec::new(),
        ));
        store(i, room, &msg);
        msg
    });
    let Some((msg, ws)) = r else {
        return plain(ctx, 404, "no such member", &[]);
    };
    broadcast(&ws, &msg);
    json(ctx, 200, msg.js.clone(), &[]);
}

/// A file's name without what it may not have: control characters, and the path separators `/`
/// and `\`; empty for one that cannot come back short enough.
fn sanitize(name: &str) -> String {
    if name.len() > MAX_FILE_NAME.saturating_mul(4) {
        return String::new();
    }
    name.chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Whether `t` is a media type, as Go's `mime.ParseMediaType` takes one: a type and a subtype of
/// token characters, and parameters.
fn media_type(t: &str) -> bool {
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    };
    let mut parts = t.split(';');
    let main = parts.next().unwrap_or("").trim();
    let Some((a, b)) = main.split_once('/') else {
        return false;
    };
    if !token(a) || !token(b) {
        return false;
    }
    parts.all(|p| {
        let p = p.trim();
        p.is_empty()
            || p.split_once('=').is_some_and(|(k, v)| {
                let v = v.trim();
                token(k.trim())
                    && (token(v) || (v.len() >= 2 && v.starts_with('"') && v.ends_with('"')))
            })
    })
}

/// The value of a `file` query parameter, decoded.
fn file_param(ctx: &Context) -> String {
    ctx.req()
        .query()
        .and_then(|q| crate::http::query_param(q, "file"))
        .unwrap_or_default()
}

/// Answers a file message: the body is the file, its name the `file` query parameter, and it is
/// from the member `X-Akit-Member` and `X-Akit-Room-Key` name.
pub(crate) fn upload(ctx: &mut Context, hub: &Hub, set: &Settings, room: &str) {
    let name = sanitize(&file_param(ctx));
    if name.is_empty() || name.len() > MAX_FILE_NAME {
        return plain(ctx, 400, "the file's name must be 1 to 200 bytes", &[]);
    }
    let (member, key) = member(ctx);
    if !member_name(&member) || key.is_empty() {
        return plain(ctx, 404, "no such member", &[]);
    }
    let mut ctype = ctx.header("content-type").unwrap_or("").trim().to_owned();
    if ctype.len() > MAX_FILE_TYPE || !media_type(&ctype) {
        ctype = "application/octet-stream".to_owned();
    }
    let max = usize::try_from(set.max_file).unwrap_or(usize::MAX);
    let declared = ctx
        .header("content-length")
        .and_then(|v| v.trim().parse::<u64>().ok());
    let n = declared.unwrap_or(ctx.body().len() as u64);
    if n > set.max_file || ctx.body().len() > max {
        return plain(ctx, 413, "the body is too large", &[]);
    }
    let limit = chat_file_bytes();
    if n > limit {
        return plain(ctx, 413, "the chat has no room for the file", &[]);
    }
    let data = ctx.body().to_vec();
    let size = data.len() as u64;
    let r = hub.with_member(room, &member, &key, |i, from| {
        while i.file_bytes.saturating_add(size) > limit {
            if !evict_oldest_file(i) {
                return None;
            }
        }
        i.chat_seq = i.chat_seq.saturating_add(1);
        let file = FileInfo {
            name: name.clone(),
            size,
            ctype: ctype.clone(),
        };
        let msg = Arc::new(Message::new(i.chat_seq, from, None, Some(file), data));
        store(i, room, &msg);
        Some(msg)
    });
    match r {
        None => plain(ctx, 404, "no such member", &[]),
        Some((None, _)) => plain(ctx, 413, "the chat has no room for the file", &[]),
        Some((Some(msg), ws)) => {
            broadcast(&ws, &msg);
            json(ctx, 200, msg.js.clone(), &[]);
        }
    }
}

/// `filename` as Go's `mime.FormatMediaType` writes it in a `Content-Disposition`.
fn disposition(name: &str) -> String {
    let token = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
    if token {
        return format!("attachment; filename={name}");
    }
    if name.is_ascii() {
        let mut q = String::with_capacity(name.len().saturating_add(2));
        for c in name.chars() {
            if c == '"' || c == '\\' {
                q.push('\\');
            }
            q.push(c);
        }
        return format!("attachment; filename=\"{q}\"");
    }
    let mut enc = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            enc.push(char::from(b));
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename*=utf-8''{enc}")
}

/// Answers a file message's bytes.
pub(crate) fn download(ctx: &mut Context, hub: &Hub, room: &str, id: &str) {
    let msg = {
        let i = super::lock(&hub.inner);
        i.rooms.get(room).and_then(|r| {
            r.messages
                .iter()
                .find(|m| m.id == id && m.file.is_some())
                .cloned()
        })
    };
    let Some(msg) = msg else {
        return plain(ctx, 404, "no such file", &[]);
    };
    let Some(f) = &msg.file else {
        return plain(ctx, 404, "no such file", &[]);
    };
    ctx.respond(
        Response::with_body(200, &f.ctype, msg.data.clone())
            .header("Content-Disposition", disposition(&f.name))
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "no-store"),
    );
}
