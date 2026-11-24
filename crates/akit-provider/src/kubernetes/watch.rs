//! A kind's store, and the reading of its watch (fit's `informer.watch`): the events of the
//! stream, one JSON object each, decoded and applied to the store as they come, on the I/O
//! thread that reads them. Nothing waits there: a short lock, and a wake-up of the provider's
//! thread that never blocks.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use akit_config::value::{self, Value};

use super::Msg;
use super::client::{Failure, MAX_ERROR_BYTES, Stream, failure};
use super::types::{Object, Shape, decode, meta};

/// The largest watch event read: an object, at most what etcd holds, base64 and all.
pub(crate) const MAX_EVENT_BYTES: usize = 16 << 20;

/// The objects of one kind as the API server has them.
#[derive(Debug, Default)]
pub(crate) struct Store {
    pub(crate) objs: BTreeMap<String, Arc<Object>>,
    /// A list has been taken, or the kind is not served.
    pub(crate) synced: bool,
    /// The resource version reached.
    pub(crate) rv: String,
    /// Objects were left out past `max_objects`.
    pub(crate) full: bool,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How the I/O threads wake the provider's thread: at most one change waits in its queue, and
/// each request in flight ends with one message, so the queue, sized for them, never fills.
#[derive(Debug)]
pub(crate) struct Signal {
    pub(crate) tx: SyncSender<Msg>,
    pub(crate) changed: AtomicBool,
}

impl Signal {
    /// An object changed.
    pub(crate) fn changed(&self) {
        if !self.changed.swap(true, Ordering::AcqRel) {
            let _ = self.tx.try_send(Msg::Changed);
        }
    }

    pub(crate) fn post(&self, m: Msg) {
        let _ = self.tx.try_send(m);
    }
}

/// The watch of one kind, read as it comes.
pub(crate) struct Watch {
    pub(crate) kind: usize,
    pub(crate) seq: u64,
    pub(crate) name: &'static str,
    pub(crate) path: String,
    pub(crate) shape: Shape,
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) max: usize,
    pub(crate) signal: Arc<Signal>,
    /// The status, and the body of an error.
    status: u16,
    reason: String,
    err_body: Vec<u8>,
    /// The event being read.
    buf: Vec<u8>,
    depth: usize,
    in_str: bool,
    escaped: bool,
    /// The watch failed, and said so: what comes after is not read.
    done: bool,
}

impl Watch {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        kind: usize,
        seq: u64,
        name: &'static str,
        path: String,
        shape: Shape,
        store: Arc<Mutex<Store>>,
        max: usize,
        signal: Arc<Signal>,
    ) -> Watch {
        Watch {
            kind,
            seq,
            name,
            path,
            shape,
            store,
            max,
            signal,
            status: 0,
            reason: String::new(),
            err_body: Vec::new(),
            buf: Vec::new(),
            depth: 0,
            in_str: false,
            escaped: false,
            done: false,
        }
    }

    fn finish(&mut self, r: Result<(), Failure>) {
        if !self.done {
            self.done = true;
            self.signal.post(Msg::WatchEnded {
                kind: self.kind,
                seq: self.seq,
                r,
            });
        }
    }

    /// Applies one event; an error ends the watch.
    fn event(&mut self, data: &[u8]) -> Result<(), Failure> {
        let text = std::str::from_utf8(data)
            .map_err(|_| Failure::Other(format!("an event of {}: not UTF-8", self.name)))?;
        let e = value::parse(text)
            .map_err(|err| Failure::Other(format!("an event of {}: {err}", self.name)))?;
        let typ = match e.get("type") {
            Some(Value::String(t)) => t.as_str(),
            _ => "",
        };
        let obj = e.get("object").unwrap_or(&Value::Null);
        match typ {
            "ERROR" => {
                let code = match obj.get("code") {
                    Some(Value::Number(n)) => n.parse::<i64>().unwrap_or(0),
                    _ => 0,
                };
                if code == 410 {
                    return Err(Failure::Gone);
                }
                let msg = match obj.get("message") {
                    Some(Value::String(m)) => m.as_str(),
                    _ => "",
                };
                Err(Failure::Other(format!("the watch failed: {code} {msg}")))
            }
            "BOOKMARK" => {
                let rv = meta(obj).resource_version;
                if !rv.is_empty() {
                    lock(&self.store).rv = rv;
                }
                Ok(())
            }
            "ADDED" | "MODIFIED" | "DELETED" => {
                let o = decode(self.shape, obj)
                    .map_err(|err| Failure::Other(format!("an event of {}: {err}", self.name)))?;
                let m = o.meta();
                let key = m.key();
                let rv = m.resource_version.clone();
                let applied = {
                    let mut st = lock(&self.store);
                    if !rv.is_empty() {
                        st.rv = rv;
                    }
                    let had = st.objs.contains_key(&key);
                    if typ == "DELETED" {
                        st.objs.remove(&key);
                        true
                    } else if had || st.objs.len() < self.max {
                        st.objs.insert(key, Arc::new(o));
                        true
                    } else {
                        st.full = true;
                        false
                    }
                };
                if applied {
                    self.signal.changed();
                }
                Ok(())
            }
            // Types to come are passed over.
            _ => Ok(()),
        }
    }
}

impl Stream for Watch {
    fn head(&mut self, status: u16, reason: &str) {
        self.status = status;
        reason.clone_into(&mut self.reason);
    }

    fn data(&mut self, chunk: &[u8]) {
        if self.done {
            return;
        }
        if self.status != 200 {
            let room = MAX_ERROR_BYTES.saturating_sub(self.err_body.len());
            self.err_body
                .extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or_default());
            return;
        }
        // Events are JSON objects one after another, as the API server writes them, each on
        // its line: one ends where its outermost brace closes.
        let mut start = 0;
        for (i, &c) in chunk.iter().enumerate() {
            if self.depth == 0 {
                if c == b'{' {
                    self.depth = 1;
                    start = i;
                }
                continue;
            }
            if self.in_str {
                if self.escaped {
                    self.escaped = false;
                } else if c == b'\\' {
                    self.escaped = true;
                } else if c == b'"' {
                    self.in_str = false;
                }
                continue;
            }
            match c {
                b'"' => self.in_str = true,
                b'{' | b'[' => self.depth = self.depth.saturating_add(1),
                b'}' | b']' => {
                    self.depth = self.depth.saturating_sub(1);
                    if self.depth == 0 {
                        let piece = chunk.get(start..=i).unwrap_or_default();
                        let r = if self.buf.is_empty() {
                            self.event(piece)
                        } else {
                            let mut whole = std::mem::take(&mut self.buf);
                            whole.extend_from_slice(piece);
                            self.event(&whole)
                        };
                        if let Err(f) = r {
                            return self.finish(Err(f));
                        }
                    }
                }
                _ => {}
            }
        }
        if self.depth > 0 {
            let piece = chunk.get(start..).unwrap_or_default();
            if self.buf.len().saturating_add(piece.len()) > MAX_EVENT_BYTES {
                return self.finish(Err(Failure::Other(format!(
                    "an event of {} is larger than {MAX_EVENT_BYTES} bytes",
                    self.name
                ))));
            }
            self.buf.extend_from_slice(piece);
        }
    }

    fn end(&mut self, r: Result<(), String>) {
        let r = match r {
            Ok(()) if self.status == 200 => Ok(()),
            Ok(()) => Err(failure(
                "GET",
                &self.path,
                self.status,
                &self.reason,
                &self.err_body,
            )),
            Err(e) => Err(Failure::Other(e)),
        };
        self.finish(r);
    }
}
