//! The Redis store (fit's `redisBackend`): RESP2 to one server or primary, over at most
//! [`REDIS_CONNS`] connections, each with one command at a time, made through akio (with TLS
//! when `tls` is set) and authenticated and its database selected when made. Their I/O thread
//! only appends what arrives to the connection's inbox; the caller's thread parses the reply
//! from it, within its deadline. Locks are keys set with `SET NX PX` and a random token, renewed
//! and deleted by scripts that act only while the key holds the token.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use akio::dns::HappyEyeballs;
use akio::tls::{ClientConfig, Tls};
use akio::{Acceptor, Conn, Handler};
use akit_config::RedisStorage;

use super::net::NetEngine;
use super::{Backend, CallError, Lease, MAX_KEYS};
use crate::storage::MAX_VALUE_BYTES;

/// The most connections kept to Redis, idle or not (fit's `redisConns`).
pub const REDIS_CONNS: usize = 4;

/// The most bytes a connection's inbox holds: a reply as large as a value may be, and more.
const MAX_INBOX: usize = 2 * (4 << 20) + (64 << 10);
const _: () = assert!(MAX_VALUE_BYTES == 4 << 20);

/// Scripts that act on a lock only while its value is the holder's token.
const RENEW: &str = r#"if redis.call("get", KEYS[1]) == ARGV[1] then return redis.call("pexpire", KEYS[1], ARGV[2]) else return 0 end"#;
const RELEASE: &str = r#"if redis.call("get", KEYS[1]) == ARGV[1] then return redis.call("del", KEYS[1]) else return 0 end"#;

/// A RESP2 reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    Simple(String),
    /// An error Redis answered with, inside an array (one at the top is an `Err`).
    Error(String),
    Int(i64),
    Bulk(Vec<u8>),
    Nil,
    Array(Vec<Reply>),
}

/// Why a command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RedisError {
    /// An error Redis answered with: the connection is kept.
    Answered(String),
    /// Anything else: the connection is in an unknown state, and closed.
    Conn(String),
}

impl RedisError {
    pub(crate) fn text(self) -> String {
        match self {
            RedisError::Answered(s) | RedisError::Conn(s) => s,
        }
    }
}

/// One RESP2 reply at the start of `buf`, its bulk strings at most [`MAX_VALUE_BYTES`] and its
/// arrays at most [`MAX_KEYS`] long and two deep (fit's `readReply`): `Ok(None)` while it is not
/// all there; the reply and the bytes it took; or the error, a top-level error reply as
/// [`RedisError::Answered`].
pub(crate) fn parse(buf: &[u8], depth: usize) -> Result<Option<(Reply, usize)>, RedisError> {
    let bad = |s: String| RedisError::Conn(s);
    let Some(nl) = buf.iter().position(|b| *b == b'\n') else {
        return Ok(None);
    };
    let line = buf.get(..nl).unwrap_or_default();
    let Some((&kind, rest)) = line
        .strip_suffix(b"\r")
        .filter(|l| !l.is_empty())
        .and_then(<[u8]>::split_first)
    else {
        return Err(bad("redis: a reply that is not RESP".to_owned()));
    };
    let rest = String::from_utf8_lossy(rest).into_owned();
    let used = nl.saturating_add(1);
    match kind {
        b'+' => Ok(Some((Reply::Simple(rest), used))),
        b'-' if depth == 0 => Err(RedisError::Answered(rest)),
        b'-' => Ok(Some((Reply::Error(rest), used))),
        b':' => rest
            .parse::<i64>()
            .map(|n| Some((Reply::Int(n), used)))
            .map_err(|_| bad(format!("redis: integer {rest:?}"))),
        b'$' => {
            let n: i64 = rest
                .parse()
                .ok()
                .filter(|n| (-1..=i64::try_from(MAX_VALUE_BYTES).unwrap_or(i64::MAX)).contains(n))
                .ok_or_else(|| bad(format!("redis: bulk string of length {rest:?}")))?;
            let Ok(n) = usize::try_from(n) else {
                return Ok(Some((Reply::Nil, used)));
            };
            let end = used.saturating_add(n);
            if buf.len() < end.saturating_add(2) {
                return Ok(None);
            }
            let data = buf.get(used..end).unwrap_or_default().to_vec();
            Ok(Some((Reply::Bulk(data), end.saturating_add(2))))
        }
        b'*' => {
            let n: i64 = rest
                .parse()
                .ok()
                .filter(|n| {
                    (-1..=i64::try_from(MAX_KEYS).unwrap_or(i64::MAX)).contains(n) && depth <= 1
                })
                .ok_or_else(|| bad(format!("redis: array of length {rest:?}")))?;
            let Ok(n) = usize::try_from(n) else {
                return Ok(Some((Reply::Nil, used)));
            };
            let mut items = Vec::with_capacity(n.min(1024));
            let mut at = used;
            for _ in 0..n {
                let Some((item, took)) =
                    parse(buf.get(at..).unwrap_or_default(), depth.saturating_add(1))?
                else {
                    return Ok(None);
                };
                items.push(item);
                at = at.saturating_add(took);
            }
            Ok(Some((Reply::Array(items), at)))
        }
        k => Err(bad(format!("redis: a reply of type {:?}", char::from(k)))),
    }
}

/// A command as RESP2: an array of bulk strings.
fn encode(args: &[&[u8]]) -> Vec<u8> {
    let mut b = Vec::with_capacity(args.iter().map(|a| a.len().saturating_add(16)).sum());
    b.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
    for a in args {
        b.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
        b.extend_from_slice(a);
        b.extend_from_slice(b"\r\n");
    }
    b
}

/// What a connection's I/O thread and its caller share.
#[derive(Default)]
struct Inbox {
    data: Vec<u8>,
    conn: Option<Conn<RedisHandler>>,
    /// Why it closed, once it has.
    closed: Option<String>,
    /// Its caller gave up waiting for it to open: it is closed once it does.
    abandoned: bool,
}

#[derive(Default)]
struct Wire {
    inbox: Mutex<Inbox>,
    cv: Condvar,
}

impl Wire {
    fn inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self, why: &str) {
        let mut ib = self.inbox();
        ib.closed.get_or_insert_with(|| why.to_owned());
        if let Some(c) = ib.conn.take() {
            c.close();
        }
        drop(ib);
        self.cv.notify_all();
    }
}

/// A connection's handler, on its I/O thread: never blocks, holds the inbox's lock only to
/// append.
pub(crate) struct RedisHandler {
    wire: Arc<Wire>,
}

impl Handler for RedisHandler {
    fn on_data(&mut self, c: &Conn<Self>, data: &[u8]) {
        let mut ib = self.wire.inbox();
        if ib.data.len().saturating_add(data.len()) > MAX_INBOX {
            ib.closed
                .get_or_insert_with(|| format!("redis: a reply larger than {MAX_INBOX} bytes"));
            drop(ib);
            c.close();
        } else {
            ib.data.extend_from_slice(data);
            drop(ib);
        }
        self.wire.cv.notify_all();
    }

    fn on_close(&mut self, _c: &Conn<Self>, err: Option<akio::Error>) {
        let mut ib = self.wire.inbox();
        ib.conn = None;
        ib.closed.get_or_insert_with(|| match err {
            Some(e) => e.to_string(),
            None => "EOF".to_owned(),
        });
        drop(ib);
        self.wire.cv.notify_all();
    }
}

struct Dial {
    wire: Arc<Wire>,
}

impl Acceptor for Dial {
    type Handler = RedisHandler;

    fn on_open(&self, c: &Conn<RedisHandler>) -> RedisHandler {
        let mut ib = self.wire.inbox();
        if ib.abandoned {
            c.close();
        } else {
            ib.conn = Some(c.clone());
        }
        drop(ib);
        self.wire.cv.notify_all();
        RedisHandler {
            wire: Arc::clone(&self.wire),
        }
    }
}

/// A connection made, with its inbox.
pub(crate) struct RedisConn {
    wire: Arc<Wire>,
}

impl RedisConn {
    /// Sends `args` and waits for the reply, until `deadline`.
    fn round_trip(&self, args: &[&[u8]], deadline: Instant) -> Result<Reply, RedisError> {
        let conn = self.wire.inbox().conn.clone();
        let Some(conn) = conn else {
            return Err(RedisError::Conn(
                "redis: the connection is closed".to_owned(),
            ));
        };
        conn.send(&encode(args))
            .map_err(|e| RedisError::Conn(format!("redis: {e}")))?;
        let mut ib = self.wire.inbox();
        loop {
            match parse(&ib.data, 0) {
                Ok(Some((reply, took))) => {
                    ib.data.drain(..took);
                    return Ok(reply);
                }
                Ok(None) => {}
                Err(e) => {
                    if matches!(e, RedisError::Answered(_)) {
                        if let Some(nl) = ib.data.iter().position(|b| *b == b'\n') {
                            ib.data.drain(..=nl);
                        }
                    }
                    return Err(e);
                }
            }
            if let Some(why) = &ib.closed {
                return Err(RedisError::Conn(format!("redis: {why}")));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(RedisError::Conn("redis: i/o timeout".to_owned()));
            }
            ib = self
                .wire
                .cv
                .wait_timeout(ib, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn close(&self) {
        self.wire.close("closed");
    }
}

#[derive(Default)]
struct Pool {
    /// Connections open or being opened.
    open: usize,
    idle: Vec<RedisConn>,
}

pub(crate) struct Redis {
    opts: RedisStorage,
    tls: Option<ClientConfig>,
    timeout: Duration,
    pool: Mutex<Pool>,
    free: Condvar,
    engine: NetEngine,
}

/// `address`'s host (without brackets) and port.
fn host_port(address: &str) -> Result<(String, u16), String> {
    let bad = || format!("{address:?} is not a host and port");
    let (host, port) = match address.strip_prefix('[') {
        Some(r) => {
            let (h, p) = r.split_once("]:").ok_or_else(bad)?;
            (h, p)
        }
        None => address.rsplit_once(':').ok_or_else(bad)?,
    };
    Ok((host.to_owned(), port.parse().map_err(|_| bad())?))
}

/// The Redis store; its locks share its connections.
pub(crate) struct RedisBackend(pub(crate) Arc<Redis>);

impl std::ops::Deref for RedisBackend {
    type Target = Redis;
    fn deref(&self) -> &Redis {
        &self.0
    }
}

impl RedisBackend {
    pub(crate) fn new(c: &RedisStorage, timeout: Duration) -> Result<RedisBackend, String> {
        Redis::new(c, timeout).map(|r| RedisBackend(Arc::new(r)))
    }
}

impl Redis {
    fn new(c: &RedisStorage, timeout: Duration) -> Result<Redis, String> {
        let tls = if c.tls {
            super::net::tls(&c.tls_files, true)?
        } else {
            None
        };
        Ok(Redis {
            opts: c.clone(),
            tls,
            timeout,
            pool: Mutex::new(Pool::default()),
            free: Condvar::new(),
            engine: NetEngine::start()?,
        })
    }

    fn pool(&self) -> MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The connections idle, and open or being opened.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let p = self.pool();
        (p.idle.len(), p.open)
    }

    /// Takes every slot, as if every connection were busy.
    #[cfg(test)]
    pub(crate) fn fill(&self) {
        self.pool().open = REDIS_CONNS;
    }

    /// Sends a command and reads its reply, on an idle connection or a new one, waiting for
    /// one within `deadline` while every slot is taken.
    pub(crate) fn run(&self, args: &[&[u8]], deadline: Instant) -> Result<Reply, RedisError> {
        let conn = {
            let mut p = self.pool();
            loop {
                if let Some(c) = p.idle.pop() {
                    break Some(c);
                }
                if p.open < REDIS_CONNS {
                    p.open = p.open.saturating_add(1);
                    break None;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(RedisError::Conn(
                        "redis: every connection is busy: timed out".to_owned(),
                    ));
                }
                p = self
                    .free
                    .wait_timeout(p, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        };
        let conn = match conn {
            Some(c) => c,
            None => match self.dial(deadline) {
                Ok(c) => c,
                Err(e) => {
                    let mut p = self.pool();
                    p.open = p.open.saturating_sub(1);
                    drop(p);
                    self.free.notify_one();
                    return Err(RedisError::Conn(e));
                }
            },
        };
        let r = conn.round_trip(args, deadline);
        let mut p = self.pool();
        if matches!(r, Err(RedisError::Conn(_))) {
            conn.close();
            p.open = p.open.saturating_sub(1);
        } else {
            p.idle.push(conn);
        }
        drop(p);
        self.free.notify_one();
        r
    }

    /// Connects, authenticates and selects the database.
    fn dial(&self, deadline: Instant) -> Result<RedisConn, String> {
        let (host, port) = host_port(&self.opts.address)?;
        let deadline = deadline.min(super::after(self.timeout));
        let wire = Arc::new(Wire::default());
        let failed = Arc::clone(&wire);
        let on_error = move |e: std::io::Error| failed.close(&e.to_string());
        let mut he = HappyEyeballs::default();
        he.connect_timeout = self
            .timeout
            .min(deadline.saturating_duration_since(Instant::now()))
            .max(Duration::from_millis(1));
        let dial = Dial {
            wire: Arc::clone(&wire),
        };
        match &self.tls {
            Some(cfg) => {
                let a = Tls::client(cfg.clone(), &host, dial).map_err(|e| e.to_string())?;
                he.connect(self.engine.engine(), &host, port, a, on_error);
            }
            None => he.connect(self.engine.engine(), &host, port, dial, on_error),
        }
        // Open, or failed, or the deadline.
        {
            let mut ib = wire.inbox();
            loop {
                if ib.conn.is_some() {
                    break;
                }
                if let Some(why) = &ib.closed {
                    return Err(why.clone());
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    ib.abandoned = true;
                    return Err("connecting: i/o timeout".to_owned());
                }
                ib = wire
                    .cv
                    .wait_timeout(ib, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
        let c = RedisConn { wire };
        let setup = || -> Result<(), String> {
            if !self.opts.password_file.is_empty() {
                let pw = std::fs::read(&self.opts.password_file)
                    .map_err(|e| format!("{}: {e}", self.opts.password_file))?;
                let pw = String::from_utf8_lossy(&pw);
                let pw = pw.trim_end_matches(['\r', '\n']);
                let mut args: Vec<&[u8]> = vec![b"AUTH"];
                if !self.opts.username.is_empty() {
                    args.push(self.opts.username.as_bytes());
                }
                args.push(pw.as_bytes());
                c.round_trip(&args, deadline)
                    .map_err(|e| format!("AUTH: {}", e.text()))?;
            }
            if self.opts.db != 0 {
                let db = self.opts.db.to_string();
                c.round_trip(&[b"SELECT", db.as_bytes()], deadline)
                    .map_err(|e| format!("SELECT: {}", e.text()))?;
            }
            Ok(())
        };
        if let Err(e) = setup() {
            c.close();
            return Err(e);
        }
        Ok(c)
    }

    fn token() -> String {
        let mut b = [0u8; 16];
        let _ = akio_crypto::rand::fill_random(&mut b);
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}

/// A reply that is not what a command answers.
fn shape(cmd: &str, r: &Reply) -> String {
    let what = match r {
        Reply::Simple(_) => "a simple string",
        Reply::Error(_) => "an error",
        Reply::Int(_) => "an integer",
        Reply::Bulk(_) => "a bulk string",
        Reply::Nil => "nil",
        Reply::Array(_) => "an array",
    };
    format!("{cmd}: a reply of {what}")
}

impl Backend for RedisBackend {
    fn get(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, CallError> {
        match self
            .run(&[b"GET", key.as_bytes()], deadline)
            .map_err(RedisError::text)?
        {
            Reply::Nil => Err(CallError::NotFound),
            Reply::Bulk(v) => Ok(v),
            Reply::Simple(s) => Ok(s.into_bytes()),
            r => Err(CallError::Failed(shape("GET", &r))),
        }
    }

    fn put(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), String> {
        self.run(&[b"SET", key.as_bytes(), value], deadline)
            .map(drop)
            .map_err(RedisError::text)
    }

    fn del(&self, key: &str, deadline: Instant) -> Result<(), String> {
        self.run(&[b"DEL", key.as_bytes()], deadline)
            .map(drop)
            .map_err(RedisError::text)
    }

    /// Scans the keys starting with `prefix`, its glob characters escaped.
    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, String> {
        let mut pattern = String::with_capacity(prefix.len().saturating_add(1));
        for c in prefix.chars() {
            if "*?[]\\".contains(c) {
                pattern.push('\\');
            }
            pattern.push(c);
        }
        pattern.push('*');
        let mut keys = Vec::new();
        let mut cursor = b"0".to_vec();
        loop {
            let r = self
                .run(
                    &[
                        b"SCAN",
                        &cursor,
                        b"MATCH",
                        pattern.as_bytes(),
                        b"COUNT",
                        b"1000",
                    ],
                    deadline,
                )
                .map_err(RedisError::text)?;
            let Reply::Array(a) = r else {
                return Err("SCAN: a reply that is not a cursor and keys".to_owned());
            };
            let [next, batch] = a.as_slice() else {
                return Err("SCAN: a reply that is not a cursor and keys".to_owned());
            };
            cursor = match next {
                Reply::Bulk(c) => c.clone(),
                Reply::Simple(c) => c.clone().into_bytes(),
                _ => Vec::new(),
            };
            if let Reply::Array(batch) = batch {
                for k in batch {
                    if let Reply::Bulk(k) = k {
                        keys.push(String::from_utf8_lossy(k).into_owned());
                    }
                }
            }
            if keys.len() > MAX_KEYS {
                return Err(format!("more than {MAX_KEYS} keys"));
            }
            if cursor.is_empty() || cursor == b"0" {
                return Ok(keys);
            }
        }
    }

    fn try_lock(
        &self,
        key: &str,
        ttl: Duration,
        deadline: Instant,
    ) -> Result<Option<Box<dyn Lease>>, String> {
        let token = Redis::token();
        let ms = ttl.as_millis().to_string();
        let r = self
            .run(
                &[
                    b"SET",
                    key.as_bytes(),
                    token.as_bytes(),
                    b"NX",
                    b"PX",
                    ms.as_bytes(),
                ],
                deadline,
            )
            .map_err(RedisError::text)?;
        if r == Reply::Nil {
            return Ok(None);
        }
        Ok(Some(Box::new(RedisLease {
            r: Arc::clone(&self.0),
            key: key.to_owned(),
            token,
            ttl_ms: ms,
        })))
    }

    fn close(&self) {
        let idle: Vec<RedisConn> = {
            let mut p = self.pool();
            let idle: Vec<RedisConn> = std::mem::take(&mut p.idle);
            p.open = p.open.saturating_sub(idle.len());
            idle
        };
        for c in idle {
            c.close();
        }
    }
}

/// A lock held: its key holds `token` until it expires.
struct RedisLease {
    r: Arc<Redis>,
    key: String,
    token: String,
    ttl_ms: String,
}

impl Lease for RedisLease {
    fn renew(&self, deadline: Instant) -> Result<(), String> {
        let r = self
            .r
            .run(
                &[
                    b"EVAL",
                    RENEW.as_bytes(),
                    b"1",
                    self.key.as_bytes(),
                    self.token.as_bytes(),
                    self.ttl_ms.as_bytes(),
                ],
                deadline,
            )
            .map_err(RedisError::text)?;
        if r != Reply::Int(1) {
            return Err("the lock was lost: another holder has it, or it expired".to_owned());
        }
        Ok(())
    }

    fn release(&self, deadline: Instant) -> Result<(), String> {
        self.r
            .run(
                &[
                    b"EVAL",
                    RELEASE.as_bytes(),
                    b"1",
                    self.key.as_bytes(),
                    self.token.as_bytes(),
                ],
                deadline,
            )
            .map(drop)
            .map_err(RedisError::text)
    }
}
