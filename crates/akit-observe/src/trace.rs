//! Traces (I64), as fit's `observe/trace.go`.
//!
//! Traces follow W3C Trace Context: a request's `traceparent` field names its trace and the
//! caller's span, and says whether the caller sampled it. akit decides by it, or by
//! `telemetry.traces.sample_ratio` for a request without one, and for a request it samples it
//! begins a server span of its own, sends a `traceparent` naming that span on to the upstream,
//! and, once the response is sent, writes the span as OTLP JSON into a buffer that a thread of the
//! tracer's own exports ([`Otlp`]). A request it does not sample costs a field lookup: its
//! `traceparent`, if any, goes on to the upstream as it came, with the rest of its fields.
//! `tracestate`, which akit adds nothing to, goes on as it came either way.
//!
//! As the access log's, the buffer is split in shards, picked by the thread that ends the span,
//! each with its own lock, held for a copy; a shard hands what it holds to the exporter once it
//! holds [`SPAN_FLUSH_BYTES`], and the exporter takes it every `interval` otherwise. Spans past
//! `max_queue_bytes` are dropped and counted rather than waited for (rules 1 and 11). A span is
//! encoded into a buffer of its thread's own: a sampled request allocates its `traceparent` (the
//! upstream's field) and nothing else once the thread has encoded a span as long.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::json_string;
use crate::metrics::thread_index;
use crate::otlp::{
    Otlp, Reporter, SCOPE, Text, append_attr, append_attr_bytes, append_int_attr, unix_nanos,
};

/// How many bytes of spans make an export before the interval is up.
pub const SPAN_FLUSH_BYTES: usize = 256 << 10;

/// The largest buffer an export gives back to its shard, for the spans to come.
const MAX_KEPT_BYTES: usize = 2 * SPAN_FLUSH_BYTES;

/// The shards of a tracer's buffer.
const SHARDS: usize = 16;

/// A request's place in a trace: the trace, the span akit began for it, and the caller's span,
/// whose bytes are all zero when it had none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    /// The trace's ID.
    pub trace_id: [u8; 16],
    /// The ID of akit's span.
    pub span_id: [u8; 8],
    /// The ID of the caller's span; zero without one.
    pub parent: [u8; 8],
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn append_hex(b: &mut Vec<u8>, bytes: &[u8]) {
    for &c in bytes {
        b.push(HEX.get(usize::from(c >> 4)).copied().unwrap_or(b'0'));
        b.push(HEX.get(usize::from(c & 15)).copied().unwrap_or(b'0'));
    }
}

impl Trace {
    /// The `traceparent` field naming this span, sampled: version 00.
    #[must_use]
    pub fn traceparent(&self) -> String {
        let mut b = Vec::with_capacity(55);
        b.extend_from_slice(b"00-");
        append_hex(&mut b, &self.trace_id);
        b.push(b'-');
        append_hex(&mut b, &self.span_id);
        b.extend_from_slice(b"-01");
        String::from_utf8(b).unwrap_or_default()
    }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(c.wrapping_sub(b'a').wrapping_add(10)),
        _ => None,
    }
}

/// Decodes lowercase hex into `out`, which takes half of `s`'s bytes.
fn decode_hex(s: &[u8], out: &mut [u8]) -> bool {
    if s.len() != out.len().saturating_mul(2) {
        return false;
    }
    for (pair, o) in s.chunks_exact(2).zip(out.iter_mut()) {
        match (
            pair.first().copied().and_then(hex_value),
            pair.get(1).copied().and_then(hex_value),
        ) {
            (Some(h), Some(l)) => *o = h.wrapping_shl(4) | l,
            _ => return false,
        }
    }
    true
}

/// A `traceparent` field read: the trace ID, the parent's ID, and whether the caller sampled it.
/// `None` for one that is not valid, which is then not followed: version, trace ID, parent ID and
/// flags, as version 00 has them, and as a later version begins with them (fit's
/// `parseTraceparent`).
#[must_use]
pub fn parse_traceparent(v: &str) -> Option<([u8; 16], [u8; 8], bool)> {
    let b = v.as_bytes();
    if b.len() < 55
        || b.get(2) != Some(&b'-')
        || b.get(35) != Some(&b'-')
        || b.get(52) != Some(&b'-')
        || (b.len() > 55 && (b.get(..2) == Some(b"00") || b.get(55) != Some(&b'-')))
    {
        return None;
    }
    let (mut version, mut trace, mut parent, mut flags) = ([0u8; 1], [0u8; 16], [0u8; 8], [0u8; 1]);
    if !decode_hex(b.get(..2)?, &mut version)
        || !decode_hex(b.get(3..35)?, &mut trace)
        || !decode_hex(b.get(36..52)?, &mut parent)
        || !decode_hex(b.get(53..55)?, &mut flags)
    {
        return None;
    }
    if version == [0xff] || trace == [0; 16] || parent == [0; 8] {
        return None;
    }
    let [flags] = flags;
    Some((trace, parent, flags & 1 == 1))
}

/// Whether a `traceparent`'s flags, where version 00 has them, say sampled, without checking the
/// rest: fit's `sampledFlag`.
fn sampled_flag(v: &str) -> bool {
    v.as_bytes()
        .get(54)
        .is_some_and(|c| matches!(c, b'1' | b'3' | b'5' | b'7' | b'9' | b'b' | b'd' | b'f'))
}

thread_local! {
    /// This thread's random state: wyrand, seeded by std's per-thread random keys.
    static RNG: Cell<u64> = Cell::new({
        let mut h = RandomState::new().build_hasher();
        h.write_u64(0x5eed);
        h.finish()
    });
    /// The buffer this thread encodes its spans into.
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// A random `u64`, from this thread's state: wyrand.
#[allow(clippy::cast_possible_truncation)]
fn random() -> u64 {
    RNG.with(|s| {
        let v = s.get().wrapping_add(0xa076_1d64_78bd_642f);
        s.set(v);
        let t = u128::from(v).wrapping_mul(u128::from(v ^ 0xe703_7ed1_a0b4_28db));
        (t >> 64) as u64 ^ t as u64
    })
}

fn non_zero() -> u64 {
    loop {
        let v = random();
        if v != 0 {
            return v;
        }
    }
}

/// What a request's span records, once its response is sent: the request's and the response's,
/// as fit's `appendSpan` writes them.
#[derive(Debug, Clone, Copy)]
pub struct SpanEnd<'a> {
    /// The request's method.
    pub method: &'a str,
    /// Its route's name; empty for none, or a route without a name.
    pub route: &'a str,
    /// Its target's path, as sent: percent-decoded when the span is written (`url.path`).
    pub path: &'a str,
    /// Whether it came over TLS (`url.scheme`).
    pub tls: bool,
    /// Its `Host` (`server.address`).
    pub host: &'a str,
    /// The client's address.
    pub client: IpAddr,
    /// Its protocol's version: `1.0`, `1.1`, `2` or `3`.
    pub version: &'a str,
    /// The response's status.
    pub status: u16,
    /// The response body's bytes.
    pub size: u64,
    /// The listener's name.
    pub listener: &'a str,
    /// The upstream's address the request was proxied to, or empty.
    pub upstream: &'a str,
    /// When the request began.
    pub start: SystemTime,
    /// When its response was sent.
    pub end: SystemTime,
}

/// Appends `span`, of the request traced `t`, as an OTLP JSON object, and a comma: fit's
/// `appendSpan`.
pub fn append_span(b: &mut Vec<u8>, t: &Trace, span: &SpanEnd<'_>) {
    b.extend_from_slice(b"{\"traceId\":\"");
    append_hex(b, &t.trace_id);
    b.extend_from_slice(b"\",\"spanId\":\"");
    append_hex(b, &t.span_id);
    if t.parent != [0; 8] {
        b.extend_from_slice(b"\",\"parentSpanId\":\"");
        append_hex(b, &t.parent);
    }
    b.extend_from_slice(b"\",\"name\":");
    if span.route.is_empty() {
        json_string(b, span.method.as_bytes());
    } else {
        // "METHOD route", as one string: the method's closing quote and the route's opening one
        // taken out.
        json_string(b, span.method.as_bytes());
        b.pop();
        b.push(b' ');
        let at = b.len();
        json_string(b, span.route.as_bytes());
        b.remove(at);
    }
    let _ = fmt::Write::write_fmt(
        &mut Text(b),
        format_args!(
            ",\"kind\":2,\"startTimeUnixNano\":\"{}\",\"endTimeUnixNano\":\"{}\",\"attributes\":[",
            unix_nanos(span.start),
            unix_nanos(span.end)
        ),
    );
    append_attr(b, "http.request.method", span.method, true);
    append_path(b, span.path);
    append_attr(
        b,
        "url.scheme",
        if span.tls { "https" } else { "http" },
        false,
    );
    append_attr(b, "server.address", span.host, false);
    let mut ip = [0u8; 64];
    append_attr(b, "client.address", ip_str(span.client, &mut ip), false);
    append_attr(b, "network.protocol.version", span.version, false);
    append_int_attr(b, "http.response.status_code", i64::from(span.status));
    append_int_attr(
        b,
        "http.response.body.size",
        i64::try_from(span.size).unwrap_or(i64::MAX),
    );
    append_attr(b, "akit.listener", span.listener, false);
    if !span.route.is_empty() {
        append_attr(b, "akit.route", span.route, false);
    }
    if !span.upstream.is_empty() {
        append_attr(b, "akit.upstream.address", span.upstream, false);
    }
    b.push(b']');
    if span.status >= 500 {
        b.extend_from_slice(b",\"status\":{\"code\":2}");
    }
    b.extend_from_slice(b"},");
}

/// `ip` as text, written into `buf`: an IPv4 address mapped into IPv6 as IPv4, as Go has it.
fn ip_str(ip: IpAddr, buf: &mut [u8; 64]) -> &str {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    };
    let mut w: &mut [u8] = buf.as_mut_slice();
    let _ = std::io::Write::write_fmt(&mut w, format_args!("{ip}"));
    let n = 64usize.saturating_sub(w.len());
    buf.get(..n)
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("")
}

/// The `url.path` attribute: the path percent-decoded, as Go's `URL.Path` has it, each byte
/// that is not UTF-8 then written U+FFFD.
fn append_path(b: &mut Vec<u8>, path: &str) {
    if !path.contains('%') {
        append_attr(b, "url.path", path, false);
        return;
    }
    let raw = path.as_bytes();
    let mut decoded = Vec::with_capacity(raw.len());
    let mut i = 0;
    while let Some(&c) = raw.get(i) {
        let pair = raw.get(i.saturating_add(1)..i.saturating_add(3));
        let mut v = [0u8; 1];
        if c == b'%' && pair.is_some_and(|p| decode_hex(&p.to_ascii_lowercase(), &mut v)) {
            decoded.push(v[0]);
            i = i.saturating_add(3);
        } else {
            decoded.push(c);
            i = i.saturating_add(1);
        }
    }
    append_attr_bytes(b, "url.path", &decoded, false);
}

/// A shard of the spans waiting.
#[derive(Default)]
struct Shard {
    /// Spans, each followed by a comma.
    buf: Vec<u8>,
    /// A buffer an export gave back, for the next.
    spare: Vec<u8>,
    spans: usize,
}

#[repr(align(128))]
#[derive(Default)]
struct Padded<T>(T);

struct Inner {
    /// A request without a sampled caller is sampled when a random `u64` is below it; 0 never,
    /// and every one when `all` is set.
    threshold: u64,
    all: bool,
    max_bytes: usize,
    shards: Box<[Padded<Mutex<Shard>>]>,
    /// Bytes in the shards.
    queued: AtomicUsize,
    dropped: AtomicU64,
    exported: AtomicU64,
    failed: AtomicU64,
    /// Wakes the exporter: an export, now, or a stop.
    tx: SyncSender<Msg>,
}

enum Msg {
    /// Enough spans wait.
    Ready,
    /// Export now, and say when done.
    Export(SyncSender<()>),
    /// Export what waits, then stop.
    Stop,
}

fn lock(m: &Mutex<Shard>) -> MutexGuard<'_, Shard> {
    // Every update under the lock leaves the shard sound.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Samples requests, records their spans and exports them over OTLP/HTTP, as fit's `Tracer`.
/// Cheap to clone; safe to use from any thread. [`close`](Self::close) exports what waits and
/// stops its thread.
#[derive(Clone)]
pub struct Tracer {
    inner: Arc<Inner>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl fmt::Debug for Tracer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tracer")
            .field("dropped", &self.dropped())
            .field("exported", &self.exported())
            .field("failed", &self.failed())
            .finish_non_exhaustive()
    }
}

impl Tracer {
    /// A tracer that samples and exports as `c` says, through `otlp`, and starts its thread.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn new(otlp: Arc<Otlp>, c: &akit_config::TracesExport) -> std::io::Result<Tracer> {
        let (tx, rx) = mpsc::sync_channel(1);
        let r = c.sample_ratio;
        let (all, threshold) = if r >= 1.0 {
            (true, 0)
        } else if r > 0.0 {
            // r * 2^64, below 2^64 since r < 1.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let t = (r * 18_446_744_073_709_551_616.0) as u64;
            (false, t)
        } else {
            (false, 0)
        };
        let inner = Arc::new(Inner {
            threshold,
            all,
            max_bytes: c.max_queue_bytes,
            shards: (0..SHARDS).map(|_| Padded::default()).collect(),
            queued: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            exported: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            tx,
        });
        let weak = Arc::downgrade(&inner);
        let interval = c.interval;
        let thread = std::thread::Builder::new()
            .name("akit-otlp-traces".into())
            .spawn(move || run(&weak, &otlp, interval, &rx))?;
        Ok(Tracer {
            inner,
            thread: Arc::new(Mutex::new(Some(thread))),
        })
    }

    /// Whether a request whose `traceparent` field is `traceparent` (`None` without one, or with
    /// more than one) is traced, and if so its place in the trace: under the caller's trace when
    /// its `traceparent` says it sampled it, or a new trace, as the sample ratio has it, when it
    /// has none or one that is not valid. A caller that did not sample it is followed. Allocates
    /// nothing.
    #[must_use]
    pub fn sample(&self, traceparent: Option<&str>) -> Option<Trace> {
        let i = &self.inner;
        if let Some(v) = traceparent {
            if !i.all && i.threshold == 0 && !sampled_flag(v) {
                // Not sampled whether it is valid or not: nothing to read.
                return None;
            }
            if let Some((trace_id, parent, sampled)) = parse_traceparent(v) {
                if !sampled {
                    return None;
                }
                return Some(Trace {
                    trace_id,
                    span_id: non_zero().to_be_bytes(),
                    parent,
                });
            }
        }
        if !i.all && (i.threshold == 0 || random() >= i.threshold) {
            return None;
        }
        let trace_id = (u128::from(random()) << 64) | u128::from(non_zero());
        Some(Trace {
            trace_id: trace_id.to_be_bytes(),
            span_id: non_zero().to_be_bytes(),
            parent: [0; 8],
        })
    }

    /// Records the span of the request traced `t`, as `span` has it. Never waits for the
    /// exporter: a span that does not fit is dropped and counted.
    pub fn end(&self, t: &Trace, span: &SpanEnd<'_>) {
        SCRATCH.with(|s| {
            // Not reentrant: nothing it calls ends a span.
            let Ok(mut b) = s.try_borrow_mut() else {
                return;
            };
            b.clear();
            append_span(&mut b, t, span);
            self.push(&b);
        });
    }

    fn push(&self, span: &[u8]) {
        let i = &self.inner;
        let Some(Padded(shard)) = i.shards.get(thread_index() % SHARDS) else {
            return;
        };
        let mut sh = lock(shard);
        if i.queued.load(Ordering::Relaxed).saturating_add(span.len()) > i.max_bytes {
            drop(sh);
            i.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if sh.buf.capacity() == 0 {
            sh.buf = std::mem::take(&mut sh.spare);
        }
        sh.buf.extend_from_slice(span);
        sh.spans = sh.spans.saturating_add(1);
        let queued = i
            .queued
            .fetch_add(span.len(), Ordering::Relaxed)
            .saturating_add(span.len());
        let full = sh.buf.len() >= SPAN_FLUSH_BYTES;
        drop(sh);
        if queued >= SPAN_FLUSH_BYTES || full {
            // One wake waiting is enough.
            let _ = i.tx.try_send(Msg::Ready);
        }
    }

    /// The bytes of spans waiting to be exported.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.inner.queued.load(Ordering::Relaxed)
    }

    /// Spans dropped for want of room.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Spans the receiver took.
    #[must_use]
    pub fn exported(&self) -> u64 {
        self.inner.exported.load(Ordering::Relaxed)
    }

    /// Spans in exports that failed.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.inner.failed.load(Ordering::Relaxed)
    }

    /// Exports what waits now, and returns once that is done. Blocks: control plane only.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::sync_channel(1);
        if self.inner.tx.send(Msg::Export(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// Exports what waits and stops the thread. Blocks until then; later calls do nothing.
    pub fn close(&self) {
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(t) = thread {
            let _ = self.inner.tx.send(Msg::Stop);
            let _ = t.join();
        }
    }
}

/// The exporter's thread: exports the spans every `interval`, and sooner when enough wait, until
/// stopped or every [`Tracer`] is gone.
fn run(inner: &Weak<Inner>, otlp: &Otlp, interval: Duration, rx: &mpsc::Receiver<Msg>) {
    let reporter = Reporter::default();
    let mut taken: Vec<Vec<u8>> = (0..SHARDS).map(|_| Vec::new()).collect();
    let mut next = Instant::now().checked_add(interval);
    loop {
        let wait = next.map_or(Duration::MAX, |n| {
            n.saturating_duration_since(Instant::now())
        });
        let (done, stop) = match rx.recv_timeout(wait) {
            Ok(Msg::Ready) => (None, false),
            Ok(Msg::Export(done)) => (Some(done), false),
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => (None, true),
            Err(RecvTimeoutError::Timeout) => {
                next = Instant::now().checked_add(interval);
                (None, false)
            }
        };
        let Some(i) = inner.upgrade() else {
            return;
        };
        export(&i, otlp, &reporter, &mut taken);
        drop(i);
        if let Some(done) = done {
            let _ = done.try_send(());
        }
        if stop {
            return;
        }
    }
}

/// Takes what the shards hold and sends it as one request; their buffers go back to them.
fn export(i: &Inner, otlp: &Otlp, reporter: &Reporter, taken: &mut [Vec<u8>]) {
    let mut spans = 0usize;
    let mut bytes = 0usize;
    for (Padded(shard), t) in i.shards.iter().zip(taken.iter_mut()) {
        let mut sh = lock(shard);
        *t = std::mem::take(&mut sh.buf);
        spans = spans.saturating_add(std::mem::take(&mut sh.spans));
        i.queued.fetch_sub(t.len(), Ordering::Relaxed);
        drop(sh);
        bytes = bytes.saturating_add(t.len());
    }
    let mut body = Vec::new();
    if spans > 0 {
        body.reserve(
            bytes
                .saturating_add(otlp.resource().len())
                .saturating_add(96),
        );
        body.extend_from_slice(b"{\"resourceSpans\":[{\"resource\":");
        body.extend_from_slice(otlp.resource());
        body.extend_from_slice(b",\"scopeSpans\":[{\"scope\":{\"name\":");
        json_string(&mut body, SCOPE.as_bytes());
        body.extend_from_slice(b"},\"spans\":[");
        for t in taken.iter() {
            body.extend_from_slice(t);
        }
        body.pop(); // the last span's comma
        body.extend_from_slice(b"]}]}]}");
    }
    // The buffers go back to their shards, for spans to come.
    for (Padded(shard), t) in i.shards.iter().zip(taken.iter_mut()) {
        let mut buf = std::mem::take(t);
        if buf.capacity() == 0 || buf.capacity() > MAX_KEPT_BYTES {
            continue;
        }
        buf.clear();
        let mut sh = lock(shard);
        if sh.spare.capacity() == 0 {
            sh.spare = buf;
        }
    }
    if spans == 0 {
        return;
    }
    let n = u64::try_from(spans).unwrap_or(u64::MAX);
    let r = otlp.post("/v1/traces", body);
    reporter.report(otlp.log(), "traces", &r);
    let counter = if r.is_ok() { &i.exported } else { &i.failed };
    counter.fetch_add(n, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::{Trace, parse_traceparent, sampled_flag};

    fn hex(b: &[u8]) -> String {
        b.iter().map(|c| format!("{c:02x}")).collect()
    }

    /// fit's `TestParseTraceparent`.
    #[test]
    fn parses_traceparent() {
        const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let (trace, parent, sampled) = parse_traceparent(VALID).unwrap();
        assert!(sampled);
        assert_eq!(hex(&trace), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(hex(&parent), "00f067aa0ba902b7");
        let base = VALID.strip_suffix("01").unwrap();
        assert!(!parse_traceparent(&format!("{base}00")).unwrap().2);
        // Flags other than sampled are passed over.
        assert!(parse_traceparent(&format!("{base}03")).unwrap().2);
        // A later version is read as far as version 00 goes.
        assert!(parse_traceparent(&format!("01{}-what-comes-next", &VALID[2..])).is_some());
        for bad in [
            String::new(),
            VALID[..54].to_owned(),
            format!("{VALID}-x"),
            format!("00{}0", &VALID[2..]),
            VALID.to_uppercase(),
            format!("ff{}", &VALID[2..]),
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01".to_owned(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01".to_owned(),
            "00_4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
            "00-4bf92f3577b34da6a3ce929d0e0e473g-00f067aa0ba902b7-01".to_owned(),
            format!("01{}x", &VALID[2..]),
        ] {
            assert!(parse_traceparent(&bad).is_none(), "{bad:?} was taken");
        }
        let t = Trace {
            trace_id: trace,
            span_id: [1, 2, 3, 4, 5, 6, 7, 8],
            parent: [0; 8],
        };
        assert_eq!(
            t.traceparent(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0102030405060708-01"
        );
    }

    /// fit's `TestSampledFlagShort`.
    #[test]
    fn short_traceparent_is_not_sampled() {
        assert!(!sampled_flag("00-4bf92f3577b34da6a3ce929d0e0e4736"));
    }
}
