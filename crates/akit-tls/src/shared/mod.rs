//! A [`Storage`] that instances of akit on several hosts share (I62, `acme.shared_storage`), as
//! fit's `tlsmgr/sharedstore` (fit's decision 014): Redis, etcd or Consul's KV store, each under
//! a key prefix.
//!
//! - **Its locks are leased**: a Redis key set with `SET NX PX` and a random token, renewed and
//!   deleted by scripts that check the token; an etcd lease, the key put in a transaction only
//!   if it was never made, renewed with a keepalive and revoked to give it back; a Consul session
//!   with the lock's TTL, which deletes the key when it is destroyed or expires. Each lasts the
//!   lock TTL and is renewed every third of it while held, by a thread of the lock's own; an
//!   instance that stops without giving one back loses it once the TTL passes. A waiter tries
//!   again every second, within its deadline.
//! - **The network**: Redis is spoken in RESP2 over at most [`REDIS_CONNS`] connections, with
//!   optional TLS; etcd (its v3 JSON gateway) and Consul over akio-http's client. All of them on
//!   an akio engine of the store's own with one client thread, whose callbacks only hand data
//!   over: the calls here wait for it on the caller's thread, which is the control plane's,
//!   never an I/O thread (rules.md rule 1), each within `request_timeout` and its deadline.
//! - **Bounds** (rule 11): a value is at most [`MAX_VALUE_BYTES`], a listing at most
//!   [`MAX_KEYS`] keys, the locks held at most [`MAX_HELD_LOCKS`].

mod consul;
mod etcd;
mod net;
mod redis;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use akit_config::{SharedStorage, SharedStorageKind};

use crate::Log;
use crate::storage::{MAX_HELD_LOCKS, MAX_VALUE_BYTES, Storage, StorageError};

pub use redis::REDIS_CONNS;

/// The most keys a listing returns (fit's `maxKeys`).
pub const MAX_KEYS: usize = 100_000;

/// How often a waiter for a lock tries again, by default (fit's `lockPoll`).
pub const DEFAULT_LOCK_POLL: Duration = Duration::from_secs(1);

/// Where locks are kept, under the prefix: not a key [`list`](Storage::list) returns.
const LOCKS_PART: &str = "locks/";

/// What a call to a store failed with: a message, or the key not there.
#[derive(Debug)]
pub(crate) enum CallError {
    /// No value (a `get` only).
    NotFound,
    /// Anything else, as a line.
    Failed(String),
}

impl From<String> for CallError {
    fn from(s: String) -> CallError {
        CallError::Failed(s)
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::NotFound => f.write_str("no such key"),
            CallError::Failed(s) => f.write_str(s),
        }
    }
}

/// What each kind of store does, on full keys; every call within `deadline`.
pub(crate) trait Backend: Send + Sync {
    /// The value of `key`, or [`CallError::NotFound`].
    fn get(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, CallError>;
    fn put(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), String>;
    fn del(&self, key: &str, deadline: Instant) -> Result<(), String>;
    /// The keys starting with `prefix`.
    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, String>;
    /// Takes the lock at `key` for `ttl` if nobody holds it; `None` when another does.
    fn try_lock(
        &self,
        key: &str,
        ttl: Duration,
        deadline: Instant,
    ) -> Result<Option<Box<dyn Lease>>, String>;
    /// Closes its connections.
    fn close(&self);
}

/// A lock held.
pub(crate) trait Lease: Send + Sync {
    fn renew(&self, deadline: Instant) -> Result<(), String>;
    fn release(&self, deadline: Instant) -> Result<(), String>;
}

/// A lock held, and what renews it: dropping `stop` ends its thread.
struct Holder {
    lease: Arc<dyn Lease>,
    stop: SyncSender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Holder {
    /// Stops renewing it, and waits for the renewing thread to end.
    fn stop(mut self) -> Arc<dyn Lease> {
        drop(self.stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.lease
    }
}

/// A shared storage of one kind (fit's `sharedstore.Store`): a [`Storage`] whose
/// [`shared`](Storage::shared) is true, so that ACME keeps challenge answers in it too. Made by
/// [`Acme::new`](crate::Acme::new) from `acme.shared_storage`, or by [`new`](Self::new).
pub struct SharedStore {
    b: Box<dyn Backend>,
    kind: &'static str,
    prefix: String,
    ttl: Duration,
    timeout: Duration,
    poll: Duration,
    held: Mutex<HashMap<String, Holder>>,
    log: Arc<Mutex<Log>>,
}

impl fmt::Debug for SharedStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedStore")
            .field("kind", &self.kind)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `d` from now, or now when that cannot be told.
pub(crate) fn after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d).unwrap_or(now)
}

/// Refuses a key that is empty, absolute, or has an empty, `.` or `..` part, a backslash or a
/// NUL, as [`FileStorage`](crate::FileStorage) does.
fn check_key(key: &str) -> Result<(), StorageError> {
    let bad = key.is_empty()
        || key.starts_with('/')
        || key.contains('\\')
        || key.contains('\0')
        || key
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..");
    if bad {
        return Err(StorageError::BadKey(key.to_owned()));
    }
    Ok(())
}

impl SharedStore {
    /// The store `c` names, its files (passwords, tokens, TLS) read now. It connects when first
    /// used.
    ///
    /// # Errors
    ///
    /// A file that cannot be read, TLS files that do not hold what they should, or an engine
    /// that does not start; each as `shared_storage.<kind>: ...`.
    pub fn new(c: &SharedStorage) -> Result<SharedStore, String> {
        let kind = c.kind.name();
        let wrap = |e: String| format!("shared_storage.{kind}: {e}");
        let timeout = c.request_timeout;
        let b: Box<dyn Backend> = match &c.kind {
            SharedStorageKind::Redis(r) => {
                Box::new(redis::RedisBackend::new(r, timeout).map_err(wrap)?)
            }
            SharedStorageKind::Etcd(e) => Box::new(etcd::EtcdBackend::new(e).map_err(wrap)?),
            SharedStorageKind::Consul(cs) => {
                Box::new(consul::ConsulBackend::new(cs).map_err(wrap)?)
            }
        };
        Ok(SharedStore::with_backend(b, kind, c, DEFAULT_LOCK_POLL))
    }

    pub(crate) fn with_backend(
        b: Box<dyn Backend>,
        kind: &'static str,
        c: &SharedStorage,
        poll: Duration,
    ) -> SharedStore {
        let log: Log = Arc::new(|line: &str| eprintln!("akit: acme: {line}"));
        SharedStore {
            b,
            kind,
            prefix: c.prefix.clone(),
            ttl: c.lock_ttl,
            timeout: c.request_timeout,
            poll,
            held: Mutex::new(HashMap::new()),
            log: Arc::new(Mutex::new(log)),
        }
    }

    /// Sets how often a waiter for a lock tries again ([`DEFAULT_LOCK_POLL`] when zero).
    #[must_use]
    pub fn lock_poll(mut self, d: Duration) -> SharedStore {
        self.poll = if d.is_zero() { DEFAULT_LOCK_POLL } else { d };
        self
    }

    /// Has it report through `log` (standard error by default).
    pub fn set_log(&self, log: Log) {
        *lock(&self.log) = log;
    }

    /// Its kind: `redis`, `etcd` or `consul`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The deadline of one call: `request_timeout` from now, or the caller's if sooner.
    fn call_deadline(&self, deadline: Instant) -> Instant {
        after(self.timeout).min(deadline)
    }

    /// A call's failure, as the caller's deadline explains it: past it, it is the deadline.
    fn fail(&self, deadline: Instant, what: &str, e: impl fmt::Display) -> StorageError {
        if Instant::now() >= deadline {
            return StorageError::Timeout;
        }
        StorageError::Io(io::Error::other(format!("{}: {what}: {e}", self.kind)))
    }

    fn lock_key(&self, name: &str) -> String {
        format!("{}{LOCKS_PART}{}", self.prefix, name.replace('/', "_"))
    }

    fn held(&self) -> MutexGuard<'_, HashMap<String, Holder>> {
        lock(&self.held)
    }

    /// Renews `lease` every third of the TTL until the holder's sender is dropped.
    fn keep(&self, name: &str, lease: Arc<dyn Lease>) -> Result<Holder, StorageError> {
        let (tx, rx) = mpsc::sync_channel::<()>(0);
        let every = self.ttl.checked_div(3).unwrap_or(self.ttl);
        let (timeout, kind, log) = (self.timeout, self.kind, Arc::clone(&self.log));
        let (l, n) = (Arc::clone(&lease), name.to_owned());
        let thread = std::thread::Builder::new()
            .name("akit-tls-lease".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(every) {
                    if let Err(e) = l.renew(after(timeout)) {
                        let log = Arc::clone(&lock(&log));
                        log(&format!("{kind}: renewing lock {n}: {e}"));
                    }
                }
            })?;
        Ok(Holder {
            lease,
            stop: tx,
            thread: Some(thread),
        })
    }

    /// Stops renewing the lock `name` without giving it back, as an instance that stopped would:
    /// for tests.
    #[doc(hidden)]
    pub fn abandon_for_tests(&self, name: &str) {
        let h = self.held().remove(name);
        if let Some(h) = h {
            drop(h.stop());
        }
    }
}

impl Storage for SharedStore {
    fn load(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, StorageError> {
        check_key(key)?;
        crate::storage::check(deadline)?;
        let full = format!("{}{key}", self.prefix);
        match self.b.get(&full, self.call_deadline(deadline)) {
            Ok(v) => Ok(v),
            Err(CallError::NotFound) => Err(StorageError::NotFound),
            Err(e) => Err(self.fail(deadline, &format!("loading {key}"), e)),
        }
    }

    fn store(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), StorageError> {
        check_key(key)?;
        if u64::try_from(value.len()).unwrap_or(u64::MAX) > MAX_VALUE_BYTES {
            return Err(StorageError::TooLarge(key.to_owned()));
        }
        crate::storage::check(deadline)?;
        let full = format!("{}{key}", self.prefix);
        self.b
            .put(&full, value, self.call_deadline(deadline))
            .map_err(|e| self.fail(deadline, &format!("storing {key}"), e))
    }

    fn delete(&self, key: &str, deadline: Instant) -> Result<(), StorageError> {
        check_key(key)?;
        crate::storage::check(deadline)?;
        let full = format!("{}{key}", self.prefix);
        self.b
            .del(&full, self.call_deadline(deadline))
            .map_err(|e| self.fail(deadline, &format!("deleting {key}"), e))
    }

    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, StorageError> {
        let mut prefix = prefix.trim_matches('/').to_owned();
        if !prefix.is_empty() {
            check_key(&prefix)?;
            prefix.push('/');
        }
        crate::storage::check(deadline)?;
        let full = self
            .b
            .list(
                &format!("{}{prefix}", self.prefix),
                self.call_deadline(deadline),
            )
            .map_err(|e| self.fail(deadline, &format!("listing {prefix}"), e))?;
        let mut keys: Vec<String> = full
            .into_iter()
            .filter_map(|k| k.strip_prefix(&self.prefix).map(str::to_owned))
            .filter(|k| !k.starts_with(LOCKS_PART) && !k.ends_with('/'))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        Ok(keys)
    }

    fn lock(&self, name: &str, deadline: Instant) -> Result<(), StorageError> {
        check_key(name)?;
        loop {
            let mine = {
                let held = self.held();
                if held.len() >= MAX_HELD_LOCKS {
                    return Err(StorageError::TooManyLocks);
                }
                held.contains_key(name)
            };
            if !mine {
                crate::storage::check(deadline)?;
                let lease = self
                    .b
                    .try_lock(&self.lock_key(name), self.ttl, self.call_deadline(deadline))
                    .map_err(|e| self.fail(deadline, &format!("taking lock {name}"), e))?;
                if let Some(lease) = lease {
                    let lease: Arc<dyn Lease> = Arc::from(lease);
                    let mut held = self.held();
                    if !held.contains_key(name) {
                        let h = match self.keep(name, Arc::clone(&lease)) {
                            Ok(h) => h,
                            Err(e) => {
                                drop(held);
                                let _ = lease.release(after(self.timeout));
                                return Err(e);
                            }
                        };
                        held.insert(name.to_owned(), h);
                        return Ok(());
                    }
                    drop(held);
                    // Taken by another thread of this process meanwhile, which cannot be, since
                    // the store held it; give it back.
                    let _ = lease.release(after(self.timeout));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(StorageError::Timeout);
            }
            // Off the I/O threads (see `Storage`): a wait here blocks only its caller.
            std::thread::sleep(self.poll.min(deadline.saturating_duration_since(now)));
        }
    }

    fn unlock(&self, name: &str) -> Result<(), StorageError> {
        let Some(h) = self.held().remove(name) else {
            return Err(StorageError::NotHeld(name.to_owned()));
        };
        h.stop().release(after(self.timeout)).map_err(|e| {
            StorageError::Io(io::Error::other(format!(
                "{}: giving back lock {name}: {e}",
                self.kind
            )))
        })
    }

    fn shared(&self) -> bool {
        true
    }

    fn close(&self) {
        let names: Vec<String> = self.held().keys().cloned().collect();
        for name in names {
            let _ = self.unlock(&name);
        }
        self.b.close();
    }
}

impl Drop for SharedStore {
    fn drop(&mut self) {
        // Its renewing threads end once their senders are dropped; the locks run out.
        for (_, h) in self.held().drain() {
            drop(h.stop());
        }
    }
}

#[cfg(test)]
mod tests;
