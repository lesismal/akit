//! The access log: lines formatted on the thread that finished a request, buffered per thread,
//! and written by a thread of the log's own, as fit's `observe.AccessLog`.
//!
//! The buffer is split in shards, one for each thread that logs (up to [`MAX_SHARDS`]; past
//! that, threads share them), each with its own lock: a thread takes its own shard's, which only
//! the writer takes too, once a flush, so threads finishing requests at once never wait for one
//! another. A shard hands its buffer to the writer as soon as it holds [`FLUSH_BYTES`], and the
//! writer takes what each holds every flush interval. `buffer_bytes` bounds what waits for the
//! writer together with what the shards hold, though a shard may hold up to a line more than
//! it; a line that does not fit is dropped and counted, never waited for (rules 1 and 11), and
//! the writer reports the lines dropped once a flush. The lines of one connection, which one I/O
//! thread serves, keep their order; lines of different threads may be written out of time order
//! by up to the flush interval, as nginx's workers' are.

use std::cell::Cell;
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::files::LogFile;
use crate::format::{self, Format, Record};
use crate::time::Stamp;
use crate::zone::Zone;

/// How full a shard's buffer gets before it is handed to the writer without waiting for the
/// interval: fit's `flushBytes`.
pub const FLUSH_BYTES: usize = 64 << 10;

/// The most shards a log has, whatever the number of threads.
pub const MAX_SHARDS: usize = 64;

/// fit's default flush interval.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// fit's default buffer, 4 MiB.
pub const DEFAULT_BUFFER_BYTES: usize = 4 << 20;

/// Where lines go, and how they are written.
pub struct Options {
    /// How lines are written.
    pub format: Format,
    /// The zone lines' times are written in.
    pub zone: Zone,
    /// The least status of the responses whose requests are written.
    pub min_status: u16,
    /// How long a line may wait before it is written; [`DEFAULT_FLUSH_INTERVAL`] when zero.
    pub flush_interval: Duration,
    /// How many bytes of lines may wait to be written; [`DEFAULT_BUFFER_BYTES`] when zero.
    pub buffer_bytes: usize,
    /// How many shards; one for each thread that logs is best. Rounded up to a power of two,
    /// at most [`MAX_SHARDS`]; the number of CPUs when zero.
    pub shards: usize,
    /// Told how many lines were dropped since it was last told, from the writer thread, once a
    /// flush with any; by default a warning on standard error, as fit's `access_log` logger
    /// writes one.
    pub on_dropped: Box<dyn Fn(u64) + Send>,
}

impl Options {
    /// fit's defaults: JSON, UTC, every request, a second, 4 MiB, a shard for each CPU.
    #[must_use]
    pub fn new() -> Options {
        Options {
            format: Format::Json,
            zone: Zone::utc(),
            min_status: 0,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            buffer_bytes: DEFAULT_BUFFER_BYTES,
            shards: 0,
            on_dropped: Box::new(|n| {
                eprintln!("akit: access log: {n} lines dropped, the output was too slow");
            }),
        }
    }
}

impl Default for Options {
    fn default() -> Options {
        Options::new()
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("format", &self.format)
            .field("zone", &self.zone)
            .field("min_status", &self.min_status)
            .field("flush_interval", &self.flush_interval)
            .field("buffer_bytes", &self.buffer_bytes)
            .field("shards", &self.shards)
            .finish_non_exhaustive()
    }
}

/// What the writer is sent.
enum Msg {
    /// A shard's buffer, to write.
    Buf(Vec<u8>),
    /// Write what is left and stop.
    Stop,
}

/// What every shard reads and the writer shares with them.
struct Core {
    format: Format,
    max_bytes: usize,
    min_status: AtomicU16,
    /// The zone, and its generation, which a change bumps so each shard takes it up.
    zone: Mutex<Arc<Zone>>,
    zone_gen: AtomicU64,
    /// Bytes handed to the writer and not yet written: added once a buffer, not once a line.
    queued: AtomicUsize,
    /// Lines dropped since the writer last reported them, and in all.
    dropped: AtomicU64,
    dropped_total: AtomicU64,
    ready: SyncSender<Msg>,
}

/// One part of the buffer, with its own lock.
pub struct Shard {
    core: Arc<Core>,
    state: Mutex<State>,
}

impl fmt::Debug for Shard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shard").finish_non_exhaustive()
    }
}

struct State {
    buf: Vec<u8>,
    closed: bool,
    /// The zone lines are written in, as of `zone_gen`, and the offset of `stamp_sec`.
    zone: Arc<Zone>,
    zone_gen: u64,
    stamp_sec: i64,
    stamp_off: i32,
}

/// An access log. Cloning it is cheap: the clones are the same log. Safe for concurrent use.
#[derive(Clone)]
pub struct AccessLog(Arc<Shared>);

struct Shared {
    core: Arc<Core>,
    shards: Vec<Arc<Shard>>,
    /// The first shard: the one a thread would get if the shards could be empty.
    first: Arc<Shard>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for AccessLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessLog")
            .field("format", &self.0.core.format)
            .field("shards", &self.0.shards.len())
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

thread_local! {
    /// This thread's shard index, taken once, in the order threads first log.
    static SLOT: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// The next thread's shard index.
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding it leaves a buffer at worst half a line longer: still sound.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AccessLog {
    /// A log writing to `out` from a thread of its own, started here.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn new(out: Box<dyn Write + Send>, opts: Options) -> io::Result<AccessLog> {
        AccessLog::start(Out::Stream(out), opts)
    }

    fn start(out: Out, opts: Options) -> io::Result<AccessLog> {
        let interval = if opts.flush_interval.is_zero() {
            DEFAULT_FLUSH_INTERVAL
        } else {
            opts.flush_interval
        };
        let max_bytes = if opts.buffer_bytes == 0 {
            DEFAULT_BUFFER_BYTES
        } else {
            opts.buffer_bytes
        };
        let wanted = if opts.shards == 0 {
            std::thread::available_parallelism().map_or(1, usize::from)
        } else {
            opts.shards
        };
        let n = wanted.clamp(1, MAX_SHARDS).next_power_of_two();
        // Room for every full buffer the bound lets wait (each holds FLUSH_BYTES or more), one
        // from each shard at a flush, and the stop: a shard never has to wait to hand one over.
        let (ready, rx) = mpsc::sync_channel(
            (max_bytes / FLUSH_BYTES)
                .saturating_add(n)
                .saturating_add(2),
        );
        let zone = Arc::new(opts.zone);
        let core = Arc::new(Core {
            format: opts.format,
            max_bytes,
            min_status: AtomicU16::new(opts.min_status),
            zone: Mutex::new(Arc::clone(&zone)),
            zone_gen: AtomicU64::new(0),
            queued: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            ready,
        });
        let shards: Vec<Arc<Shard>> = (0..n)
            .map(|_| {
                Arc::new(Shard {
                    core: Arc::clone(&core),
                    state: Mutex::new(State {
                        buf: Vec::new(),
                        closed: false,
                        zone: Arc::clone(&zone),
                        zone_gen: 0,
                        stamp_sec: i64::MIN,
                        stamp_off: 0,
                    }),
                })
            })
            .collect();
        let first = Arc::new(Shard {
            core: Arc::clone(&core),
            state: Mutex::new(State {
                buf: Vec::new(),
                closed: false,
                zone: Arc::clone(&zone),
                zone_gen: 0,
                stamp_sec: i64::MIN,
                stamp_off: 0,
            }),
        });
        let first = shards.first().cloned().unwrap_or(first);
        let writer = Writer {
            core: Arc::clone(&core),
            shards: shards.clone(),
            out,
            on_dropped: opts.on_dropped,
            interval,
        };
        let handle = std::thread::Builder::new()
            .name("akit-access-log".into())
            .spawn(move || writer.run(&rx))?;
        Ok(AccessLog(Arc::new(Shared {
            core,
            shards,
            first,
            writer: Mutex::new(Some(handle)),
        })))
    }

    /// This thread's shard, for a request it serves to write its line to once its response is
    /// done (from whichever thread finishes it). Taking it is a clone of a handle only this
    /// thread (and the threads that outnumber the shards) takes.
    #[must_use]
    pub fn shard(&self) -> Arc<Shard> {
        let shards = &self.0.shards;
        let slot = SLOT.with(|s| {
            let mut v = s.get();
            if v == usize::MAX {
                v = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
                s.set(v);
            }
            v
        });
        // A power of two, never empty: the mask picks one, and `first` is never taken.
        let i = slot & shards.len().wrapping_sub(1);
        Arc::clone(shards.get(i).unwrap_or(&self.0.first))
    }

    /// Writes the line for `r` to this thread's shard. A line that does not fit is dropped.
    pub fn log(&self, r: &Record<'_>) {
        self.shard().log(r);
    }

    /// Has the log write the requests answered with `min_status` or more from now on, as a
    /// reload changes the level.
    pub fn set_min_status(&self, min_status: u16) {
        self.0.core.min_status.store(min_status, Ordering::Relaxed);
    }

    /// Has the log write times in `zone` from now on, as a reload changes the time zone.
    pub fn set_zone(&self, zone: Zone) {
        *lock(&self.0.core.zone) = Arc::new(zone);
        self.0.core.zone_gen.fetch_add(1, Ordering::Release);
    }

    /// How many lines have been dropped since the log was opened.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.0.core.dropped_total.load(Ordering::Relaxed)
    }

    /// A log writing to `file`, a log file's sink (I81) it shares with the logs writing there, from
    /// a thread of its own: each buffer the shards hand over is queued to the sink as it is,
    /// and one the sink refuses counts its lines as dropped.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn to_file(file: LogFile, opts: Options) -> io::Result<AccessLog> {
        AccessLog::start(Out::File(file), opts)
    }

    /// Writes out what is buffered, stops the writer and waits for it. Lines logged afterwards
    /// are dropped. Blocks: call it on the control plane, after the listeners have stopped.
    pub fn close(&self) {
        let Some(handle) = lock(&self.0.writer).take() else {
            return;
        };
        for sh in &self.0.shards {
            lock(&sh.state).closed = true;
        }
        // The writer is the only receiver, and running: this waits only for it to make room.
        let _ = self.0.core.ready.send(Msg::Stop);
        let _ = handle.join();
    }
}

impl Shard {
    /// Writes the line for `r`, at the time now, unless its status is below the log's level. A
    /// line that does not fit in the buffer is dropped and counted: this never waits for the
    /// writer.
    pub fn log(&self, r: &Record<'_>) {
        let core = &*self.core;
        if r.status < core.min_status.load(Ordering::Relaxed) {
            return;
        }
        let now = Stamp::of(SystemTime::now());
        let mut st = lock(&self.state);
        if st.closed {
            drop(st);
            core.drop_lines(1);
            return;
        }
        let generation = core.zone_gen.load(Ordering::Acquire);
        if generation != st.zone_gen {
            st.zone = Arc::clone(&lock(&core.zone));
            st.zone_gen = generation;
            st.stamp_sec = i64::MIN;
        }
        if st.stamp_sec != now.secs {
            st.stamp_off = st.zone.offset(now.secs);
            st.stamp_sec = now.secs;
        }
        let offset = st.stamp_off;
        if st.buf.capacity() == 0 {
            st.buf.reserve(FLUSH_BYTES + (4 << 10));
        }
        let start = st.buf.len();
        format::append(&mut st.buf, core.format, r, now, offset);
        if core
            .queued
            .load(Ordering::Relaxed)
            .saturating_add(st.buf.len())
            > core.max_bytes
        {
            st.buf.truncate(start);
            drop(st);
            core.drop_lines(1);
            return;
        }
        if st.buf.len() >= FLUSH_BYTES {
            // Handed over under the lock, so the writer cannot take what the shard holds next
            // before it. The channel has room for it, as `queued` says; try_send only keeps
            // that from ever waiting.
            let buf = std::mem::take(&mut st.buf);
            let len = buf.len();
            core.queued.fetch_add(len, Ordering::Relaxed);
            if let Err(
                TrySendError::Full(Msg::Buf(buf)) | TrySendError::Disconnected(Msg::Buf(buf)),
            ) = core.ready.try_send(Msg::Buf(buf))
            {
                core.queued.fetch_sub(len, Ordering::Relaxed);
                st.buf = buf;
            }
        }
    }
}

impl Core {
    fn drop_lines(&self, n: u64) {
        self.dropped.fetch_add(n, Ordering::Relaxed);
        self.dropped_total.fetch_add(n, Ordering::Relaxed);
    }
}

/// The instant `d` from now, or now when that is past what an instant holds.
fn after(d: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(d).unwrap_or(now)
}

/// Where the writer writes.
enum Out {
    /// A stream: standard output or error, or a test's.
    Stream(Box<dyn Write + Send>),
    /// A log file's sink (I81).
    File(LogFile),
}

/// The writer thread's state.
struct Writer {
    core: Arc<Core>,
    shards: Vec<Arc<Shard>>,
    out: Out,
    on_dropped: Box<dyn Fn(u64) + Send>,
    interval: Duration,
}

impl Writer {
    /// Writes the full buffers as they come, and every interval what the shards hold.
    fn run(mut self, rx: &Receiver<Msg>) {
        let mut next = after(self.interval);
        loop {
            let wait = next.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Msg::Buf(b)) => self.write(b),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    self.flush(rx);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.flush(rx) {
                        // A close sent its stop meanwhile: what it asks is done.
                        self.flush(rx);
                        return;
                    }
                    next = after(self.interval);
                }
            }
        }
    }

    /// Writes a buffer out. One the output refuses loses its lines, counted as dropped.
    fn write(&mut self, b: Vec<u8>) {
        let len = b.len();
        let lost = match &mut self.out {
            Out::Stream(out) => {
                let ok = out.write_all(&b).and_then(|()| out.flush()).is_ok();
                (!ok).then_some(b)
            }
            Out::File(f) => f.write_owned(b).err().map(|(_, b)| b),
        };
        if let Some(b) = lost {
            let lines = b.iter().filter(|&&c| c == b'\n').count();
            self.core.drop_lines(lines as u64);
        }
        self.core.queued.fetch_sub(len, Ordering::Relaxed);
    }

    /// Writes what the shards hold, after what they handed over before, and reports the lines
    /// dropped since the last flush. Each shard's buffer goes through the channel under its lock,
    /// as a full one does, so the channel has each shard's buffers in the order they filled.
    /// Returns whether a stop came meanwhile.
    fn flush(&mut self, rx: &Receiver<Msg>) -> bool {
        for sh in &self.shards {
            let mut st = lock(&sh.state);
            if st.buf.is_empty() {
                continue;
            }
            let buf = std::mem::take(&mut st.buf);
            let len = buf.len();
            self.core.queued.fetch_add(len, Ordering::Relaxed);
            if let Err(
                TrySendError::Full(Msg::Buf(buf)) | TrySendError::Disconnected(Msg::Buf(buf)),
            ) = self.core.ready.try_send(Msg::Buf(buf))
            {
                // Full of what came first: the rest waits for the next flush.
                self.core.queued.fetch_sub(len, Ordering::Relaxed);
                st.buf = buf;
            }
        }
        let mut stop = false;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Buf(b) => self.write(b),
                Msg::Stop => stop = true,
            }
        }
        let n = self.core.dropped.swap(0, Ordering::Relaxed);
        if n > 0 {
            (self.on_dropped)(n);
        }
        stop
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{AccessLog, Options};
    use crate::format::Record;

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn record(status: u16) -> Record<'static> {
        Record {
            listener: "web",
            route: "",
            client: IpAddr::V4(Ipv4Addr::LOCALHOST),
            user: "",
            proto: "HTTP/1.1",
            method: "GET",
            host: "h",
            uri: "/",
            status,
            bytes_in: 0,
            bytes_out: 0,
            duration: Duration::ZERO,
            upstream: None,
            user_agent: "",
            referer: "",
        }
    }

    #[test]
    fn close_writes_everything_and_drops_after() {
        let sink = Sink::default();
        let mut o = Options::new();
        o.flush_interval = Duration::from_secs(3600);
        let log = AccessLog::new(Box::new(sink.clone()), o).unwrap();
        for _ in 0..1000 {
            log.log(&record(200));
        }
        log.close();
        log.close();
        let out = sink.0.lock().unwrap().clone();
        assert_eq!(out.iter().filter(|&&c| c == b'\n').count(), 1000);
        log.log(&record(200));
        assert_eq!(log.dropped(), 1);
    }

    #[test]
    fn levels() {
        let sink = Sink::default();
        let mut o = Options::new();
        o.min_status = 400;
        let log = AccessLog::new(Box::new(sink.clone()), o).unwrap();
        log.log(&record(200));
        log.log(&record(404));
        log.set_min_status(500);
        log.log(&record(404));
        log.log(&record(503));
        log.close();
        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        let statuses: Vec<&str> = out
            .lines()
            .map(|l| l.split("\"status\":").nth(1).unwrap().get(..3).unwrap())
            .collect();
        assert_eq!(statuses, ["404", "503"]);
    }
}
