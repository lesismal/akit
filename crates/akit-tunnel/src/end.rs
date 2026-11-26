//! The end of a tunnel that nodes and proxies share (fit's `tunnel/internal/tunnel`): a TCP
//! connection of their own, joined to the link to the hub that carries the tunnel's frames.
//!
//! Each end sends no more than [`WINDOW`] bytes the other has not acknowledged, and holds its TCP
//! connection's reads once it has; the other end acknowledges what its own connection has taken,
//! once that connection's queue is short again (`on_drain`). A slow reader at one end holds back
//! the writer at the other, one tunnel at a time (rule 8). A link whose queue is past its high
//! water holds every end that feeds it until it drains.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use crate::link::Link;
use crate::msg::{ACK_EVERY, MAX_CHUNK, MAX_IN_FLIGHT, WINDOW, method};

/// How much of what an end received may wait for its TCP connection before it stops
/// acknowledging.
const ACK_LIMIT: usize = 1 << 20;

fn now_ms() -> i64 {
    akit_roles::linktoken::unix_now().saturating_mul(1000)
}

struct EndState {
    conn: Option<akio::Conn<EndHandler>>,
    window: i64,
    ready: bool,
    held: bool,
    link_held: bool,
    unacked: i64,
    ack_waiting: bool,
    closed: bool,
}

/// One end of a tunnel.
pub struct End {
    /// The tunnel's ID.
    pub id: u64,
    /// The node, service and the address the node connected to.
    pub node: String,
    /// The service.
    pub service: String,
    /// The proxy (on a node) or hub (on a proxy).
    pub peer: String,
    link: Arc<Link>,
    table: Weak<Table>,
    st: Mutex<EndState>,
    /// Bytes taken from the link, and sent onto it.
    pub bytes_in: AtomicI64,
    /// Bytes sent onto the link.
    pub bytes_out: AtomicI64,
    active: AtomicI64,
}

impl std::fmt::Debug for End {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("End")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn lock(m: &Mutex<EndState>) -> MutexGuard<'_, EndState> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl End {
    /// The link that carries the tunnel.
    #[must_use]
    pub fn link(&self) -> &Arc<Link> {
        &self.link
    }

    fn update_hold(st: &mut EndState) {
        let hold = !st.ready || st.window <= 0 || st.link_held;
        if hold != st.held {
            st.held = hold;
            if let Some(c) = &st.conn {
                c.hold_reads(hold);
            }
        }
    }

    /// Attaches `c`, the end's TCP connection.
    pub fn attach(&self, c: &akio::Conn<EndHandler>) {
        // Paced by the window, not by its own output (fit's SetWriteBackpressure(false)).
        c.set_relayed(true);
        let mut st = lock(&self.st);
        st.conn = Some(c.clone());
        // The first hold, whatever `held` says: a connection reads until told.
        st.held = !st.ready;
        c.hold_reads(st.held);
        Self::update_hold(&mut st);
    }

    /// The tunnel is open: its connection is read from now on.
    pub fn set_ready(&self) {
        let mut st = lock(&self.st);
        st.ready = true;
        Self::update_hold(&mut st);
    }

    /// Bytes read from the end's connection: sent on the link, within the window.
    fn local(self: &Arc<Self>, data: &[u8]) {
        let total = i64::try_from(data.len()).unwrap_or(i64::MAX);
        {
            let mut st = lock(&self.st);
            st.window = st.window.saturating_sub(total);
            Self::update_hold(&mut st);
        }
        for chunk in data.chunks(MAX_CHUNK) {
            if !self.link.send_tunnel(method::TUNNEL_DATA, self.id, chunk) {
                self.close("link: closed");
                return;
            }
        }
        self.bytes_out.fetch_add(total, Ordering::Relaxed);
        self.active.store(now_ms(), Ordering::Relaxed);
        if self.link.congested() {
            {
                let mut st = lock(&self.st);
                st.link_held = true;
                Self::update_hold(&mut st);
            }
            let me = Arc::clone(self);
            self.link.when_drained(Box::new(move || {
                let mut st = lock(&me.st);
                st.link_held = false;
                Self::update_hold(&mut st);
            }));
        }
    }

    /// Bytes of the tunnel from the link: written to the end's connection.
    pub fn remote(self: &Arc<Self>, payload: &[u8]) {
        let c = lock(&self.st).conn.clone();
        let Some(c) = c else {
            self.close("data before the tunnel was open");
            return;
        };
        if c.send(payload).is_err() {
            self.close("write: the connection would not take it");
            return;
        }
        let n = i64::try_from(payload.len()).unwrap_or(i64::MAX);
        self.bytes_in.fetch_add(n, Ordering::Relaxed);
        self.active.store(now_ms(), Ordering::Relaxed);
        let ack = {
            let mut st = lock(&self.st);
            st.unacked = st.unacked.saturating_add(n);
            if st.unacked > MAX_IN_FLIGHT {
                drop(st);
                self.close("the other end sent past its window");
                return;
            }
            if st.unacked < ACK_EVERY || st.ack_waiting {
                return;
            }
            if c.queued_bytes() > ACK_LIMIT {
                st.ack_waiting = true;
                None
            } else {
                Some(std::mem::take(&mut st.unacked))
            }
        };
        match ack {
            Some(n) => self.send_ack(n),
            None => {
                let me = Arc::clone(self);
                c.on_drain(move || me.flush_ack());
            }
        }
    }

    fn flush_ack(&self) {
        let n = {
            let mut st = lock(&self.st);
            st.ack_waiting = false;
            std::mem::take(&mut st.unacked)
        };
        if n > 0 {
            self.send_ack(n);
        }
    }

    fn send_ack(&self, n: i64) {
        self.link
            .send_tunnel(method::TUNNEL_ACK, self.id, &n.to_be_bytes());
    }

    /// The other end acknowledged `n` bytes.
    pub fn acked(&self, n: i64) {
        let mut st = lock(&self.st);
        if n <= 0 || n > WINDOW.saturating_sub(st.window) {
            drop(st);
            self.close("the other end acknowledged what was not sent to it");
            return;
        }
        st.window = st.window.saturating_add(n);
        Self::update_hold(&mut st);
    }

    fn mark_closed(&self) -> bool {
        let mut st = lock(&self.st);
        !std::mem::replace(&mut st.closed, true)
    }

    fn forget(&self) {
        if let Some(t) = self.table.upgrade() {
            t.remove(self.id, self);
        }
    }

    /// Closes the tunnel: the other end is told why, and the connection closed.
    pub fn close(&self, reason: &str) {
        if self.mark_closed() {
            self.link
                .send_tunnel(method::TUNNEL_CLOSE, self.id, reason.as_bytes());
        }
        self.forget();
        if let Some(c) = lock(&self.st).conn.clone() {
            c.close();
        }
    }

    /// The other end closed it: the connection closes once what it was sent is written.
    pub fn remote_close(&self) {
        self.mark_closed();
        self.forget();
        if let Some(c) = lock(&self.st).conn.clone() {
            c.close_after_send();
        }
    }

    /// The end's connection closed.
    fn local_closed(&self, err: Option<String>) {
        if self.mark_closed() {
            let reason = err.unwrap_or_else(|| "closed".to_owned());
            self.link
                .send_tunnel(method::TUNNEL_CLOSE, self.id, reason.as_bytes());
        }
        self.forget();
    }

    /// Milliseconds since 1970 of the end's last byte either way.
    #[must_use]
    pub fn active(&self) -> i64 {
        self.active.load(Ordering::Relaxed)
    }
}

/// akio's handler of a tunnel's TCP connection.
pub struct EndHandler(pub Option<Arc<End>>);

impl std::fmt::Debug for EndHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EndHandler").finish_non_exhaustive()
    }
}

impl akio::Handler for EndHandler {
    fn on_data(&mut self, _c: &akio::Conn<Self>, data: &[u8]) {
        if let Some(e) = &self.0 {
            e.local(data);
        }
    }

    fn on_close(&mut self, _c: &akio::Conn<Self>, err: Option<akio::Error>) {
        if let Some(e) = self.0.take() {
            e.local_closed(err.map(|e| e.to_string()));
        }
    }
}

/// The tunnels open at once over every hub of a role, bounded.
#[derive(Debug)]
pub struct Limit {
    n: AtomicUsize,
    max: AtomicUsize,
}

impl Limit {
    /// A limit of `max`.
    #[must_use]
    pub fn new(max: usize) -> Arc<Limit> {
        Arc::new(Limit {
            n: AtomicUsize::new(0),
            max: AtomicUsize::new(max),
        })
    }
    /// Changes the bound.
    pub fn set_max(&self, max: usize) {
        self.max.store(max, Ordering::Relaxed);
    }
    /// The tunnels open.
    #[must_use]
    pub fn open(&self) -> usize {
        self.n.load(Ordering::Relaxed)
    }
    fn take(&self) -> bool {
        let max = self.max.load(Ordering::Relaxed);
        self.n
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max).then(|| n.saturating_add(1))
            })
            .is_ok()
    }
    fn give(&self) {
        let _ = self
            .n
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                Some(n.saturating_sub(1))
            });
    }
}

/// The ends of a role's tunnels, by ID.
#[derive(Debug)]
pub struct Table {
    limit: Arc<Limit>,
    m: Mutex<HashMap<u64, Arc<End>>>,
}

impl Table {
    /// A table under `limit`.
    #[must_use]
    pub fn new(limit: Arc<Limit>) -> Arc<Table> {
        Arc::new(Table {
            limit,
            m: Mutex::new(HashMap::new()),
        })
    }

    fn map(&self) -> MutexGuard<'_, HashMap<u64, Arc<End>>> {
        self.m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a new end of tunnel `id` on `link`, not ready (its connection not read) unless
    /// `ready`.
    ///
    /// # Errors
    ///
    /// The ID in use, or the limit reached.
    pub fn add(
        self: &Arc<Self>,
        link: &Arc<Link>,
        id: u64,
        ready: bool,
        node: &str,
        service: &str,
        peer: &str,
    ) -> Result<Arc<End>, &'static str> {
        let mut m = self.map();
        if m.contains_key(&id) {
            return Err("tunnel: tunnel ID in use");
        }
        if !self.limit.take() {
            return Err("tunnel: too many tunnels open");
        }
        let e = Arc::new(End {
            id,
            node: node.to_owned(),
            service: service.to_owned(),
            peer: peer.to_owned(),
            link: Arc::clone(link),
            table: Arc::downgrade(self),
            st: Mutex::new(EndState {
                conn: None,
                window: WINDOW,
                ready,
                held: false,
                link_held: false,
                unacked: 0,
                ack_waiting: false,
                closed: false,
            }),
            bytes_in: AtomicI64::new(0),
            bytes_out: AtomicI64::new(0),
            active: AtomicI64::new(now_ms()),
        });
        m.insert(id, Arc::clone(&e));
        Ok(e)
    }

    fn remove(&self, id: u64, e: &End) {
        let mut m = self.map();
        if m.get(&id).is_some_and(|x| std::ptr::eq(Arc::as_ptr(x), e)) {
            m.remove(&id);
            self.limit.give();
        }
    }

    /// The end of tunnel `id`.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<Arc<End>> {
        self.map().get(&id).cloned()
    }

    /// Serves a tunnel frame from `link`.
    pub fn frame(&self, link: &Arc<Link>, m: u8, body: &[u8]) {
        let Some((id, payload)) = crate::msg::parse_frame(body) else {
            return;
        };
        let Some(e) = self.get(id) else {
            return;
        };
        if !Arc::ptr_eq(&e.link, link) {
            return;
        }
        match m {
            method::TUNNEL_DATA => e.remote(payload),
            method::TUNNEL_ACK => match crate::msg::parse_ack(payload) {
                Some(n) => e.acked(n),
                None => e.close("a bad acknowledgement"),
            },
            method::TUNNEL_CLOSE => e.remote_close(),
            _ => {}
        }
    }

    /// Closes the tunnels a link carried, which closed.
    pub fn close_link(&self, link: &Arc<Link>, reason: &str) {
        let ends: Vec<Arc<End>> = self
            .map()
            .values()
            .filter(|e| Arc::ptr_eq(&e.link, link))
            .cloned()
            .collect();
        for e in ends {
            e.mark_closed();
            e.forget();
            if let Some(c) = lock(&e.st).conn.clone() {
                c.close();
            }
            let _ = reason;
        }
    }

    /// Closes the tunnels that carried nothing for `idle`.
    pub fn close_idle(&self, idle: Duration) {
        let before = now_ms().saturating_sub(i64::try_from(idle.as_millis()).unwrap_or(i64::MAX));
        let ends: Vec<Arc<End>> = self
            .map()
            .values()
            .filter(|e| e.active() < before)
            .cloned()
            .collect();
        for e in ends {
            e.close(&format!("idle for {}", akit_config::duration::format(idle)));
        }
    }

    /// Closes every tunnel.
    pub fn close_all(&self, reason: &str) {
        let ends: Vec<Arc<End>> = self.map().values().cloned().collect();
        for e in ends {
            e.close(reason);
        }
    }

    /// How many tunnels are open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map().len()
    }

    /// Whether none is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many tunnels `link` carries.
    #[must_use]
    pub fn on_link(&self, link: &Arc<Link>) -> usize {
        self.map()
            .values()
            .filter(|e| Arc::ptr_eq(&e.link, link))
            .count()
    }
}
