//! The room API, below `<prefix>/api/rooms/<room>/` (fit's `rooms.go`):
//!
//! - `POST join`: `{name, key?, audio, video, audioDevice, videoDevice, screen, publishing}`
//!   joins the room as `name`, or takes the name back with the key it was given; answered
//!   `{heartbeatMs, key, publish, stream, ttlMs}`. It needs play on `<room>-*`, and announcing
//!   publishing needs publish on `<room>-<name>`.
//! - `POST members/<name>`: `{key, ...the state}` changes the member's state and is its
//!   heartbeat; 404 once the member is gone, which the page answers by joining again.
//! - `POST leave`: `{name, key}` leaves; a page's unload sends it as a beacon.
//! - `POST chat`, `POST files?file=<name>`, `GET files/<id>`: the chat ([`super::chat`]).
//! - `GET events`: Server-Sent Events, `history` with the room's chat messages when the chat is
//!   on, `room` with the room's members whenever they change, `chat` with each new message, and a
//!   comment as a ping every third of `member_ttl`. It needs play on `<room>-*`.
//!
//! The key, 128 random bits only its member knows, is what lets it change its state, as a WHIP
//! session's URL lets its peer end it; the chat asks for it too, so that only a member speaks as
//! itself.
//!
//! The rooms of a prefix on an engine are shared by the configs that serve it ([`acquire`]), so
//! that a reload keeps the rooms and their members; the last config to let go ends their event
//! streams. Each room has a timer of its own, every third of `member_ttl` (at least a second),
//! which drops the members gone quiet, pings the watchers and forgets the room once it has
//! neither members nor watchers.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use akio_http::{BodyStream, Context, Method};
use akit_config::media::MediaAction as Action;

use super::chat::{self, Message};
use super::{Settings, body, bool_member, json, lock, object, plain, quote, str_member};
use crate::http::HandlerAuth;

/// The most bytes of files the rooms of a hub keep together (fit's `chatFileBytes`); past it the
/// oldest file messages go.
static CHAT_FILE_BYTES: AtomicU64 = AtomicU64::new(64 << 20);

/// Sets the most bytes of files the rooms of a hub keep (64 MiB by default), for tests that fill
/// it.
#[doc(hidden)]
pub fn set_chat_file_bytes(n: u64) {
    CHAT_FILE_BYTES.store(n, Ordering::SeqCst);
}

/// What the rooms of a prefix on an engine hold, for tests and diagnostics ([`stats`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Rooms with members or watchers.
    pub rooms: usize,
    /// Event streams.
    pub watchers: usize,
    /// Bytes of the chat's files.
    pub file_bytes: u64,
}

/// What the rooms of `prefix` on the engine `key` hold, or `None` while no config serves them.
#[must_use]
pub fn stats(key: usize, prefix: &str) -> Option<Stats> {
    let h = lock(hubs())
        .get(&(key, prefix.to_owned()))
        .map(|e| Arc::clone(&e.1))?;
    let i = lock(&h.inner);
    Some(Stats {
        rooms: i.rooms.len(),
        watchers: i.watchers,
        file_bytes: i.file_bytes,
    })
}

pub(crate) fn chat_file_bytes() -> u64 {
    CHAT_FILE_BYTES.load(Ordering::SeqCst)
}

/// What a watcher's queue of chat events may hold before its stream is cut.
const MAX_WATCHER_QUEUE_EVENTS: usize = 256;
const MAX_WATCHER_QUEUE_BYTES: usize = 1 << 20;
/// Bytes a stream may have queued before its watcher waits for the browser to take them.
const WATERMARK: usize = 64 << 10;

/// Whether `s` is a room's name: 1 to 64 letters, digits, '.', '_', '~' and '-'.
fn room_name(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(name_byte)
}

/// Whether `s` is a member's name: 1 to 63 of the same.
pub(crate) fn member_name(s: &str) -> bool {
    (1..=63).contains(&s.len()) && s.bytes().all(name_byte)
}

fn name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-')
}

/// What a member announces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct State {
    audio: bool,
    video: bool,
    audio_device: bool,
    video_device: bool,
    screen: bool,
    publishing: bool,
}

impl State {
    fn from(o: &[(String, akit_config::value::Value)]) -> Result<State, ()> {
        Ok(State {
            audio: bool_member(o, "audio")?,
            video: bool_member(o, "video")?,
            audio_device: bool_member(o, "audioDevice")?,
            video_device: bool_member(o, "videoDevice")?,
            screen: bool_member(o, "screen")?,
            publishing: bool_member(o, "publishing")?,
        })
    }
}

struct Member {
    name: String,
    key: String,
    /// May publish `<room>-<name>`.
    publish: bool,
    st: State,
    /// The room's join count when it joined: its place in the list.
    joined: u64,
    seen: Instant,
}

/// A room's members, as the event that tells them.
pub(crate) struct Snapshot {
    seq: u64,
    ev: Vec<u8>,
}

pub(crate) struct Room {
    name: String,
    /// Tells this room from one of the same name made after it was forgotten.
    id: u64,
    members: HashMap<String, Member>,
    watchers: Vec<Arc<Watcher>>,
    joins: u64,
    seq: u64,
    snap: Option<Arc<Snapshot>>,
    timer: Option<akio::timer::SendTimer>,
    /// The chat, oldest first.
    pub(crate) messages: VecDeque<Arc<Message>>,
    pub(crate) file_bytes: u64,
}

pub(crate) struct Inner {
    pub(crate) rooms: HashMap<String, Room>,
    watchers: usize,
    closed: bool,
    pub(crate) file_bytes: u64,
    pub(crate) chat_seq: u64,
    next_room: u64,
}

/// The rooms of a prefix of an engine's routes.
pub(crate) struct Hub {
    key: (usize, String),
    set: Mutex<Arc<Settings>>,
    pub(crate) inner: Mutex<Inner>,
    me: Weak<Hub>,
}

type Hubs = Mutex<HashMap<(usize, String), (usize, Arc<Hub>)>>;

fn hubs() -> &'static Hubs {
    static HUBS: LazyLock<Hubs> = LazyLock::new(|| Mutex::new(HashMap::new()));
    &HUBS
}

/// The hub of the engine `key` and `prefix`, made when there is none, with `set` its settings
/// from now on.
pub(crate) fn acquire(key: usize, prefix: &str, set: Arc<Settings>) -> Arc<Hub> {
    let k = (key, prefix.to_owned());
    let mut hs = lock(hubs());
    let e = hs.entry(k.clone()).or_insert_with(|| {
        (
            0,
            Arc::new_cyclic(|me| Hub {
                key: k,
                set: Mutex::new(Arc::clone(&set)),
                inner: Mutex::new(Inner {
                    rooms: HashMap::new(),
                    watchers: 0,
                    closed: false,
                    file_bytes: 0,
                    chat_seq: 0,
                    next_room: 0,
                }),
                me: me.clone(),
            }),
        )
    });
    e.0 = e.0.saturating_add(1);
    *lock(&e.1.set) = set;
    Arc::clone(&e.1)
}

/// Lets go of `h`; the last config to let go closes it, ending its watchers' streams.
pub(crate) fn release(h: &Arc<Hub>) {
    let last = {
        let mut hs = lock(hubs());
        match hs.get_mut(&h.key) {
            Some(e) if Arc::ptr_eq(&e.1, h) => {
                e.0 = e.0.saturating_sub(1);
                if e.0 == 0 {
                    hs.remove(&h.key);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    };
    if !last {
        return;
    }
    let ws: Vec<Arc<Watcher>> = {
        let mut i = lock(&h.inner);
        i.closed = true;
        let mut ws = Vec::new();
        for r in i.rooms.values_mut() {
            if let Some(t) = r.timer.take() {
                t.cancel();
            }
            ws.append(&mut r.watchers);
        }
        i.rooms.clear();
        i.watchers = 0;
        i.file_bytes = 0;
        ws
    };
    for w in ws {
        w.end(true);
    }
}

fn new_key() -> String {
    let mut b = [0u8; 16];
    let _ = akio_crypto::rand::fill_random(&mut b);
    b.iter().fold(String::with_capacity(32), |mut s, x| {
        use std::fmt::Write as _;
        let _ = write!(s, "{x:02x}");
        s
    })
}

pub(crate) fn key() -> String {
    new_key()
}

/// Whether `a` and `b` are the same key, in constant time.
pub(crate) fn same_key(a: &str, b: &str) -> bool {
    akio_crypto::ct_eq(a.as_bytes(), b.as_bytes())
}

impl Hub {
    pub(crate) fn settings(&self) -> Arc<Settings> {
        Arc::clone(&lock(&self.set))
    }

    /// The room named `name`, made when `create` is set and the hub has room for it.
    fn room_locked<'i>(&self, i: &'i mut Inner, name: &str, create: bool) -> Option<&'i mut Room> {
        if !i.rooms.contains_key(name) {
            if !create || i.closed || i.rooms.len() >= self.settings().max_rooms {
                return None;
            }
            i.next_room = i.next_room.saturating_add(1);
            let id = i.next_room;
            let timer = self.schedule(name, id);
            i.rooms.insert(
                name.to_owned(),
                Room {
                    name: name.to_owned(),
                    id,
                    members: HashMap::new(),
                    watchers: Vec::new(),
                    joins: 0,
                    seq: 0,
                    snap: None,
                    timer,
                    messages: VecDeque::new(),
                    file_bytes: 0,
                },
            );
        }
        i.rooms.get_mut(name)
    }

    /// The room's next sweep and ping.
    fn schedule(&self, name: &str, id: u64) -> Option<akio::timer::SendTimer> {
        let every = self
            .settings()
            .member_ttl
            .checked_div(3)
            .unwrap_or_default()
            .max(Duration::from_secs(1));
        let (me, name) = (self.me.clone(), name.to_owned());
        akio::timer::after_send(every, move || {
            if let Some(h) = me.upgrade() {
                h.tick(&name, id);
            }
        })
        .ok()
    }

    /// Sweeps the room's members gone quiet, pings its watchers, and forgets the room once it
    /// has neither members nor watchers.
    fn tick(&self, name: &str, id: u64) {
        let ttl = self.settings().member_ttl;
        let now = Instant::now();
        let (snap, ws) = {
            let mut i = lock(&self.inner);
            if i.closed {
                return;
            }
            let Some(r) = i.rooms.get_mut(name).filter(|r| r.id == id) else {
                return;
            };
            let before = r.members.len();
            r.members
                .retain(|_, m| now.saturating_duration_since(m.seen) <= ttl);
            let changed = r.members.len() != before;
            if r.members.is_empty() && r.watchers.is_empty() {
                if let Some(r) = i.rooms.remove(name) {
                    i.file_bytes = i.file_bytes.saturating_sub(r.file_bytes);
                }
                return;
            }
            let snap = changed.then(|| r.changed());
            let ws = r.watchers.clone();
            let timer = self.schedule(name, id);
            if let Some(r) = i.rooms.get_mut(name) {
                r.timer = timer;
            }
            (snap, ws)
        };
        for w in ws {
            match &snap {
                Some(s) => w.push(s),
                None => w.ping(),
            }
        }
    }

    /// Adds `name` to the room, or gives the name back to the member that has its key: its key.
    fn join(
        &self,
        room: &str,
        name: &str,
        key: &str,
        st: State,
        publish: bool,
    ) -> Result<String, (u16, &'static str)> {
        let set = self.settings();
        let (key, snap, ws) = {
            let mut i = lock(&self.inner);
            let Some(r) = self.room_locked(&mut i, room, true) else {
                return Err((503, "too many rooms"));
            };
            let now = Instant::now();
            let taken = r.members.get(name).is_some_and(|m| {
                (key.is_empty() || !same_key(key, &m.key))
                    && now.saturating_duration_since(m.seen) <= set.member_ttl
            });
            if taken {
                return Err((409, "the name is taken in the room"));
            }
            if !r.members.contains_key(name) && r.members.len() >= set.max_members {
                return Err((503, "the room is full"));
            }
            let keep = r.members.get(name).is_some_and(|m| m.key == key);
            if !keep {
                r.joins = r.joins.saturating_add(1);
                r.members.insert(
                    name.to_owned(),
                    Member {
                        name: name.to_owned(),
                        key: new_key(),
                        publish,
                        st,
                        joined: r.joins,
                        seen: now,
                    },
                );
            }
            let m = r.members.get_mut(name).ok_or((500, "the member went"))?;
            m.publish = publish;
            m.st = st;
            m.seen = now;
            let key = m.key.clone();
            let snap = r.changed();
            (key, snap, r.watchers.clone())
        };
        broadcast(&ws, &snap);
        Ok(key)
    }

    /// Changes a member's state, the member found by its key: whether there is such a member,
    /// and whether it may publish what it says it does.
    fn update(&self, room: &str, name: &str, key: &str, st: State) -> (bool, bool) {
        let (snap, ws) = {
            let mut i = lock(&self.inner);
            let Some(r) = self.room_locked(&mut i, room, false) else {
                return (false, false);
            };
            let Some(m) = r.members.get_mut(name).filter(|m| same_key(key, &m.key)) else {
                return (false, false);
            };
            if st.publishing && !m.publish {
                return (true, false);
            }
            m.seen = Instant::now();
            if m.st == st {
                return (true, true);
            }
            m.st = st;
            (r.changed(), r.watchers.clone())
        };
        broadcast(&ws, &snap);
        (true, true)
    }

    /// Removes a member, found by its key.
    fn leave(&self, room: &str, name: &str, key: &str) -> bool {
        let (snap, ws) = {
            let mut i = lock(&self.inner);
            let Some(r) = self.room_locked(&mut i, room, false) else {
                return false;
            };
            if !r.members.get(name).is_some_and(|m| same_key(key, &m.key)) {
                return false;
            }
            r.members.remove(name);
            (r.changed(), r.watchers.clone())
        };
        broadcast(&ws, &snap);
        true
    }

    /// The watchers of `room` and a member named `name` with `key`, found under the lock, for
    /// the chat: `f` runs with the room, its member's name, under the lock.
    pub(crate) fn with_member<R>(
        &self,
        room: &str,
        name: &str,
        key: &str,
        f: impl FnOnce(&mut Inner, &str) -> R,
    ) -> Option<(R, Vec<Arc<Watcher>>)> {
        let mut i = lock(&self.inner);
        let r = i.rooms.get(room)?;
        let m = r.members.get(name).filter(|m| same_key(key, &m.key))?;
        let (name, ws) = (m.name.clone(), r.watchers.clone());
        Some((f(&mut i, &name), ws))
    }

    /// Takes `w` out of its room's watchers.
    fn detach(&self, w: &Watcher) {
        let mut i = lock(&self.inner);
        let gone = i
            .rooms
            .get_mut(&w.room)
            .filter(|r| r.id == w.room_id)
            .and_then(|r| {
                let n = r.watchers.len();
                r.watchers.retain(|x| x.id != w.id);
                (r.watchers.len() != n).then_some(())
            })
            .is_some();
        if gone {
            i.watchers = i.watchers.saturating_sub(1);
        }
    }
}

impl Room {
    /// The room's snapshot made anew.
    fn changed(&mut self) -> Arc<Snapshot> {
        self.seq = self.seq.saturating_add(1);
        let mut list: Vec<&Member> = self.members.values().collect();
        list.sort_by_key(|m| m.joined);
        let members: Vec<String> = list
            .iter()
            .map(|m| {
                format!(
                    "{{\"name\":{},\"audio\":{},\"video\":{},\"audioDevice\":{},\"videoDevice\":{},\"screen\":{},\"publishing\":{}}}",
                    quote(&m.name),
                    m.st.audio,
                    m.st.video,
                    m.st.audio_device,
                    m.st.video_device,
                    m.st.screen,
                    m.st.publishing
                )
            })
            .collect();
        let data = format!(
            "{{\"room\":{},\"seq\":{},\"members\":[{}]}}",
            quote(&self.name),
            self.seq,
            members.join(",")
        );
        let snap = Arc::new(Snapshot {
            seq: self.seq,
            ev: format!("event: room\ndata: {data}\n\n").into_bytes(),
        });
        self.snap = Some(Arc::clone(&snap));
        snap
    }

    fn snapshot(&mut self) -> Arc<Snapshot> {
        match &self.snap {
            Some(s) => Arc::clone(s),
            None => self.changed(),
        }
    }
}

fn broadcast(ws: &[Arc<Watcher>], snap: &Arc<Snapshot>) {
    for w in ws {
        w.push(snap);
    }
}

/// Serves the room API: `rest` is what follows `rooms/`.
pub(crate) fn serve(
    ctx: &mut Context,
    hub: &Arc<Hub>,
    auth: &HandlerAuth,
    set: &Arc<Settings>,
    rest: &str,
) {
    let (room, op) = rest.split_once('/').unwrap_or((rest, ""));
    if !room_name(room) {
        return plain(
            ctx,
            404,
            "not a room's name: 1 to 64 letters, digits, '.', '_', '~' and '-'",
            &[],
        );
    }
    let chat_api = op == "chat" || op == "files" || op.starts_with("files/");
    if chat_api && !hub.settings().chat {
        return plain(ctx, 404, "no such API", &[]);
    }
    let get = op == "events" || op.starts_with("files/");
    let want = if get { Method::Get } else { Method::Post };
    if *ctx.method() != want {
        let w = if get { "GET" } else { "POST" };
        return plain(
            ctx,
            405,
            &format!("the room API's {op} takes {w}"),
            &[("Allow", w)],
        );
    }
    let (room, op) = (room.to_owned(), op.to_owned());
    let pattern = format!("{room}-*");
    let (hub, set) = (Arc::clone(hub), Arc::clone(set));
    match op.as_str() {
        "events" => auth.guard(ctx, Action::Play, &pattern, "", move |ctx, _| {
            watch(ctx, &hub, &room);
        }),
        "join" => {
            let a = auth.clone();
            auth.guard(ctx, Action::Play, &pattern, "", move |ctx, _| {
                join(ctx, &hub, &a, &set, &room);
            });
        }
        "leave" => {
            let Some(b) = body(ctx, set.max_message) else {
                return;
            };
            let ok = object(&b).is_some_and(|o| {
                let (n, k) = (str_member(&o, "name"), str_member(&o, "key"));
                matches!((n, k), (Ok(n), Ok(k)) if hub.leave(&room, &n, &k))
            });
            if ok {
                json(ctx, 200, b"{}".to_vec(), &[]);
            } else {
                plain(ctx, 404, "no such member", &[]);
            }
        }
        "chat" => auth.guard(ctx, Action::Play, &pattern, "", move |ctx, _| {
            chat::chat(ctx, &hub, &set, &room);
        }),
        "files" => auth.guard(ctx, Action::Play, &pattern, "", move |ctx, _| {
            chat::upload(ctx, &hub, &set, &room);
        }),
        o if o.starts_with("files/") => {
            let id = o.strip_prefix("files/").unwrap_or("").to_owned();
            auth.guard(ctx, Action::Play, &pattern, "", move |ctx, _| {
                chat::download(ctx, &hub, &room, &id);
            });
        }
        o if o.starts_with("members/") => {
            let name = o.strip_prefix("members/").unwrap_or("").to_owned();
            let Some(b) = body(ctx, set.max_message) else {
                return;
            };
            let parsed = object(&b).and_then(|o| {
                let key = str_member(&o, "key").ok()?;
                Some((key, State::from(&o).ok()?))
            });
            let Some((key, st)) = parsed else {
                return plain(
                    ctx,
                    400,
                    "the body must be JSON: {key, audio, video, audioDevice, videoDevice, screen, publishing}",
                    &[],
                );
            };
            match hub.update(&room, &name, &key, st) {
                (false, _) => plain(ctx, 404, "no such member: join again", &[]),
                (true, false) => plain(
                    ctx,
                    403,
                    &format!(
                        "the credentials the member joined with do not allow publishing {room}-{name}"
                    ),
                    &[],
                ),
                (true, true) => json(ctx, 200, b"{}".to_vec(), &[]),
            }
        }
        _ => plain(ctx, 404, "no such API", &[]),
    }
}

/// Joins a member, its credentials checked for play on `<room>-*` already.
fn join(ctx: &mut Context, hub: &Hub, auth: &HandlerAuth, set: &Settings, room: &str) {
    let Some(b) = body(ctx, set.max_message) else {
        return;
    };
    let parsed = object(&b).and_then(|o| {
        Some((
            str_member(&o, "name").ok()?,
            str_member(&o, "key").ok()?,
            State::from(&o).ok()?,
        ))
    });
    let Some((name, key, st)) = parsed else {
        return plain(
            ctx,
            400,
            "the body must be JSON: {name, key, audio, video, audioDevice, videoDevice, screen, publishing}",
            &[],
        );
    };
    if !member_name(&name) {
        return plain(
            ctx,
            400,
            "not a member's name: 1 to 63 letters, digits, '.', '_', '~' and '-'",
            &[],
        );
    }
    let stream = format!("{room}-{name}");
    // The credentials allowed play already: a password among them was checked and is
    // remembered, so this does not wait.
    let publish = if auth.named() {
        auth.policy().is_some_and(|p| {
            p.allow(
                Action::Publish,
                &stream,
                &crate::http::credentials(ctx.req()),
            )
            .is_ok()
        })
    } else {
        true
    };
    if st.publishing && !publish {
        return plain(
            ctx,
            403,
            &format!("the credentials do not allow publishing {stream}"),
            &[],
        );
    }
    match hub.join(room, &name, &key, st, publish) {
        Err((status, msg)) => {
            let retry: &[(&str, &str)] = if status == 503 {
                &[("Retry-After", "5")]
            } else {
                &[]
            };
            plain(ctx, status, msg, retry);
        }
        Ok(key) => {
            let set = hub.settings();
            let ttl = u64::try_from(set.member_ttl.as_millis()).unwrap_or(u64::MAX);
            let body = format!(
                "{{\"heartbeatMs\":{},\"key\":{},\"publish\":{publish},\"stream\":{},\"ttlMs\":{ttl}}}",
                ttl / 3,
                quote(&key),
                quote(&stream)
            );
            json(ctx, 200, body.into_bytes(), &[]);
        }
    }
}

// ---- the event stream ----

/// The drain waiter's states: none; being registered (a drain while so is the registrant's to
/// handle); registered, waiting.
const IDLE: u8 = 0;
const REGISTERING: u8 = 1;
const FIRED: u8 = 2;
const WAITING: u8 = 3;

static WATCHER_IDS: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct WState {
    ended: bool,
    /// The stream's first events are written.
    started: bool,
    /// Above the watermark: waiting for the browser.
    waiting: bool,
    /// A drain waiter to register once what is under way is written.
    need_drain: bool,
    /// The latest snapshot while waiting.
    pending: Option<Arc<Snapshot>>,
    /// The seq of the last snapshot written.
    sent: u64,
    /// Chat events the browser has not taken, in order.
    queue: VecDeque<Arc<Message>>,
    queued: usize,
    body: Option<BodyStream>,
    stall: Option<akio::timer::SendTimer>,
}

/// A room's event stream to a browser.
pub(crate) struct Watcher {
    id: u64,
    hub: Weak<Hub>,
    room: String,
    room_id: u64,
    stall: Duration,
    st: Mutex<WState>,
    drain: AtomicU8,
    me: Weak<Watcher>,
}

/// What a write under the lock leaves to do once the lock is let go.
enum After {
    Nothing,
    /// The stream failed, or fell behind: end it, finishing the response when set.
    End(bool),
}

/// Starts a room's event stream.
fn watch(ctx: &mut Context, hub: &Arc<Hub>, room: &str) {
    let set = hub.settings();
    let (w, snap, msgs) = {
        let mut i = lock(&hub.inner);
        let Some(r) = hub.room_locked(&mut i, room, true) else {
            drop(i);
            return plain(ctx, 503, "too many rooms", &[("Retry-After", "5")]);
        };
        if r.watchers.len() >= set.max_watchers {
            drop(i);
            return plain(
                ctx,
                503,
                "the room has too many watchers",
                &[("Retry-After", "5")],
            );
        }
        let w = Arc::new_cyclic(|me| Watcher {
            id: WATCHER_IDS.fetch_add(1, Ordering::Relaxed),
            hub: Arc::downgrade(hub),
            room: room.to_owned(),
            room_id: r.id,
            stall: set.stall,
            st: Mutex::new(WState::default()),
            drain: AtomicU8::new(IDLE),
            me: me.clone(),
        });
        r.watchers.push(Arc::clone(&w));
        let snap = r.snapshot();
        let msgs: Vec<Arc<Message>> = if set.chat {
            r.messages.iter().cloned().collect()
        } else {
            Vec::new()
        };
        i.watchers = i.watchers.saturating_add(1);
        (w, snap, msgs)
    };
    let history = set.chat.then(|| chat::history_event(&msgs));
    let gone = Arc::downgrade(&w);
    ctx.on_gone(move || {
        if let Some(w) = gone.upgrade() {
            w.end(false);
        }
    });
    let Some(body) = ctx.stream_response(200, super::sse_headers()) else {
        hub.detach(&w);
        return;
    };
    w.start(body, history, &snap);
}

impl Watcher {
    /// Writes the stream's first events: the retry hint, the chat's history when the chat is
    /// on, and the room's snapshot; then what was held back meanwhile.
    fn start(&self, body: BodyStream, history: Option<Vec<u8>>, snap: &Arc<Snapshot>) {
        let after = {
            let mut st = lock(&self.st);
            if st.ended {
                drop(st);
                // Ended while it was being started: the response is finished.
                body.finish();
                return;
            }
            st.body = Some(body);
            let mut ok = self.write(&mut st, b"retry: 2000\n\n");
            if let Some(h) = history.filter(|h| !h.is_empty()) {
                ok = ok && self.write(&mut st, &h);
            }
            st.sent = snap.seq;
            ok = ok && self.write(&mut st, &snap.ev);
            st.started = true;
            ok = ok && self.flush(&mut st);
            if ok {
                self.settle(&mut st)
            } else {
                After::End(false)
            }
        };
        self.after(after);
    }

    /// Writes `b` without waiting; past the watermark it waits for the browser, at most the
    /// stall timeout. False when the stream failed.
    fn write(&self, st: &mut WState, b: &[u8]) -> bool {
        let Some(body) = &st.body else {
            return false;
        };
        if body.write(b).is_err() {
            return false;
        }
        if !st.waiting && body.queued_bytes() > WATERMARK {
            st.waiting = true;
            st.need_drain = true;
            let me = self.me.clone();
            st.stall = akio::timer::after_send(self.stall, move || {
                if let Some(w) = me.upgrade() {
                    w.end(false);
                }
            })
            .ok();
        }
        true
    }

    /// Writes, in order, what the browser has not taken while it takes it. False when the
    /// stream failed.
    fn flush(&self, st: &mut WState) -> bool {
        while !st.waiting {
            let Some(m) = st.queue.pop_front() else {
                break;
            };
            st.queued = st.queued.saturating_sub(m.ev.len());
            if !self.write(st, &m.ev) {
                return false;
            }
        }
        if st.waiting {
            return true;
        }
        if let Some(s) = st.pending.take() {
            if s.seq > st.sent {
                st.sent = s.seq;
                return self.write(st, &s.ev);
            }
        }
        true
    }

    /// Registers the drain waiter the writes under way asked for. A drain that comes while it
    /// is registered (the body was empty already, or emptied meanwhile) is handled here, under
    /// the lock: what was held back is written, which may ask for a waiter again.
    fn settle(&self, st: &mut WState) -> After {
        while st.need_drain && !st.ended {
            st.need_drain = false;
            self.drain.store(REGISTERING, Ordering::SeqCst);
            let me = self.me.clone();
            if let Some(body) = &st.body {
                body.on_drain(move || {
                    if let Some(w) = me.upgrade() {
                        w.drained();
                    }
                });
            }
            if self
                .drain
                .compare_exchange(REGISTERING, WAITING, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return After::Nothing;
            }
            self.drain.store(IDLE, Ordering::SeqCst);
            st.waiting = false;
            if let Some(t) = st.stall.take() {
                t.cancel();
            }
            if !self.flush(st) {
                return After::End(false);
            }
        }
        After::Nothing
    }

    /// The browser took what was sent: what was held back goes.
    fn drained(&self) {
        if self
            .drain
            .compare_exchange(REGISTERING, FIRED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }
        if self
            .drain
            .compare_exchange(WAITING, IDLE, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let after = {
            let mut st = lock(&self.st);
            if st.ended {
                return;
            }
            st.waiting = false;
            if let Some(t) = st.stall.take() {
                t.cancel();
            }
            if self.flush(&mut st) {
                self.settle(&mut st)
            } else {
                After::End(false)
            }
        };
        self.after(after);
    }

    fn after(&self, a: After) {
        if let After::End(finish) = a {
            self.end(finish);
        }
    }

    /// Sends `snap`, unless a later one was sent; while the browser has not taken what was
    /// sent, only the latest is kept, to send once it has.
    pub(crate) fn push(&self, snap: &Arc<Snapshot>) {
        let after = {
            let mut st = lock(&self.st);
            if st.ended || snap.seq <= st.sent {
                return;
            }
            if !st.started || st.waiting {
                if st.pending.as_ref().is_none_or(|p| snap.seq > p.seq) {
                    st.pending = Some(Arc::clone(snap));
                }
                return;
            }
            st.sent = snap.seq;
            if self.write(&mut st, &snap.ev) {
                self.settle(&mut st)
            } else {
                After::End(false)
            }
        };
        self.after(after);
    }

    /// Sends a chat message's event, held in order while the browser has not taken what it was
    /// sent. Past the queue's bounds the stream is cut, and the browser reconnects and refills
    /// its window from the history.
    pub(crate) fn post(&self, msg: &Arc<Message>) {
        let after = {
            let mut st = lock(&self.st);
            if st.ended {
                return;
            }
            if !st.started || st.waiting {
                st.queued = st.queued.saturating_add(msg.ev.len());
                st.queue.push_back(Arc::clone(msg));
                if st.queue.len() > MAX_WATCHER_QUEUE_EVENTS || st.queued > MAX_WATCHER_QUEUE_BYTES
                {
                    After::End(true)
                } else {
                    After::Nothing
                }
            } else if self.write(&mut st, &msg.ev) {
                self.settle(&mut st)
            } else {
                After::End(false)
            }
        };
        self.after(after);
    }

    /// Sends a comment, which keeps proxies from closing a quiet stream.
    fn ping(&self) {
        let after = {
            let mut st = lock(&self.st);
            if st.ended || st.waiting || !st.started {
                return;
            }
            if self.write(&mut st, b": ping\n\n") {
                self.settle(&mut st)
            } else {
                After::End(false)
            }
        };
        self.after(after);
    }

    /// Ends the stream once: it leaves its room, finishes the response when `finish` is set (a
    /// stream cut, or the hub closed) or drops it (the browser gone), and lets go of the request.
    pub(crate) fn end(&self, finish: bool) {
        let body = {
            let mut st = lock(&self.st);
            if st.ended {
                return;
            }
            st.ended = true;
            st.pending = None;
            st.queue.clear();
            st.queued = 0;
            if let Some(t) = st.stall.take() {
                t.cancel();
            }
            st.body.take()
        };
        if let Some(h) = self.hub.upgrade() {
            h.detach(self);
        }
        if let Some(b) = body {
            if finish {
                b.finish();
            }
        }
    }
}
