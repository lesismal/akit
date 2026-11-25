//! The kernel relay of `tcp` services (I51b): a connection a listener with `sockmap` accepts is
//! spliced to its upstream's with akio's eBPF sockmap relay ([`akio::ebpf::Relay`]) once the
//! upstream's connection opens, and its bytes stop passing through userspace. An akit extension
//! (decision 010): fit has no such field.
//!
//! The relay is loaded once per process, by [`load`], where listeners are bound or reloaded (on
//! the control thread: loading runs the kernel's verifier), never on an I/O thread; a connection
//! reads it with [`relay`], which never waits. Loading fails without `CAP_BPF` and
//! `CAP_NET_ADMIN` (or `CAP_SYS_ADMIN`), on kernels older than akio's [`MIN_KERNEL`] (6.15) and
//! off Linux; then, and for each connection the relay cannot take, the service relays in
//! userspace as I51 built it.
//!
//! A connection is spliced when, as the upstream's connection opens:
//!
//! - the client's reads are held (they are from the accept) and nothing waits in userspace to go
//!   to either side but what the client sent before (its ClientHello on a listener routing by
//!   server name, a read that came before the hold) and the service's PROXY header: those are
//!   written to the upstream's socket first, directly, once both sockets are in the map and
//!   before the kernel relays any byte ([`akio::ebpf::Relay::splice_inner_with`]);
//! - neither connection has a transport layer (TLS, which a `tcp` listener never terminates);
//! - the client's connection did not begin with a PROXY header read by the listener: akio-mux
//!   reads it in userspace without saying how long it was, which the byte counts below need
//!   (akio API request A170);
//! - the relay has room ([`akio::ebpf::DEFAULT_PAIRS`] pairs).
//!
//! Otherwise, or when the splice fails, nothing is lost: what was to be written first is sent
//! as before, and the connection is relayed in userspace.
//!
//! A spliced connection's bytes never reach its handlers, so its progress is read from the
//! sockets' TCP counters ([`akio::ebpf::byte_counts`]): its idle timeout is kept by the bytes
//! the two sockets received, read by each sweep of the service, and a side that ends has the
//! other closed only once everything it received has reached the other's send queue (closing a
//! socket drops what the kernel's relay still holds for it), or once that makes no progress for
//! the service's idle timeout. A payload a handler still reads from a spliced connection while
//! both sides are open can only be a read that completed before the splice took over (io_uring),
//! which the kernel's newer bytes may have overtaken: both sides are closed rather than the
//! stream reordered.
//!
//! [`MIN_KERNEL`]: akio::ebpf::MIN_KERNEL

use std::io::{self, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use akio::ebpf::{ByteCounts, Relay, byte_counts};
use akio::{Conn, Handler};

/// The process's relay, or why it could not be loaded.
static RELAY: OnceLock<Result<Relay, String>> = OnceLock::new();

/// Loads the relay, once per process; later calls return what the first one did. Call it where
/// listeners are bound or reloaded, before a listener with `sockmap` takes connections: it runs
/// the kernel's verifier, which must not run on an I/O thread. The first failure is written to
/// standard error, once.
///
/// # Errors
///
/// Why the relay cannot be loaded: no privileges, an old kernel, not Linux.
pub fn load() -> Result<(), String> {
    let mut first = false;
    let r = RELAY.get_or_init(|| {
        first = true;
        Relay::new(0).map_err(|e| e.to_string())
    });
    match r {
        Ok(_) => Ok(()),
        Err(e) => {
            if first {
                eprintln!(
                    "akit: the sockmap relay cannot be loaded ({e}): listeners with sockmap relay \
                     their tcp services in userspace"
                );
            }
            Err(e.clone())
        }
    }
}

/// The relay, when [`load`] loaded it. Never waits.
#[must_use]
pub fn relay() -> Option<&'static Relay> {
    RELAY.get().and_then(|r| r.as_ref().ok())
}

/// Whether [`load`] loaded the relay: connections of listeners with `sockmap` may be spliced.
#[must_use]
pub fn available() -> bool {
    relay().is_some()
}

/// What a spliced connection's counters are read against: what userspace took from each socket,
/// and what had been written to each, as the kernel began relaying.
#[derive(Debug)]
pub(crate) struct Spliced {
    /// Bytes userspace read from the client, all written to the upstream before splicing.
    client_read: u64,
    /// The upstream's written count once those (and the PROXY header) were written.
    up_written: u64,
    /// The client's written count as it was spliced (userspace read nothing of the upstream's).
    client_written: u64,
    /// Bytes both sockets have received, as the last sweep read them.
    activity: AtomicU64,
}

/// Splices `client` and `up`, first writing `first` (the PROXY header and what the client sent
/// before, `client_read` bytes of it the client's) to the upstream's socket directly. What of
/// `first` is left unwritten when the splice fails is returned, for userspace to send.
///
/// The sockets are reached through akio's `Conn::inner`, hidden in its docs: akio has no public
/// way to read a connection's counters yet (akio API request A170).
pub(crate) fn splice<A: Handler, B: Handler>(
    relay: &Relay,
    client: &Conn<A>,
    up: &Conn<B>,
    first: Vec<u8>,
    client_read: u64,
) -> Result<Spliced, Vec<u8>> {
    let (client, up) = (client.inner(), up.inner());
    let (Some(cs), Some(us)) = (client.tcp_stream(), up.tcp_stream()) else {
        return Err(first);
    };
    let (Ok(c), Ok(u)) = (byte_counts(cs), byte_counts(us)) else {
        return Err(first);
    };
    let mut written = 0;
    let r = relay.splice_inner_with(client, up, || {
        if first.is_empty() {
            return Ok(());
        }
        // The socket is non-blocking: a full send buffer fails the splice, which is undone.
        written = (&*us).write(&first)?;
        if written == first.len() {
            Ok(())
        } else {
            Err(io::ErrorKind::WriteZero.into())
        }
    });
    match r {
        // Dropping the splice leaves it in place, until either socket closes.
        Ok(_) => Ok(Spliced {
            client_read,
            up_written: u.written.saturating_add(first.len() as u64),
            client_written: c.written,
            activity: AtomicU64::new(c.received.wrapping_add(u.received)),
        }),
        Err(_) => Err(first.get(written..).map(<[u8]>::to_vec).unwrap_or_default()),
    }
}

/// The counters of `c`'s socket, or `None` when they cannot be read.
fn counts<H: Handler>(c: &Conn<H>) -> Option<ByteCounts> {
    c.inner().tcp_stream().and_then(|s| byte_counts(s).ok())
}

impl Spliced {
    /// Reads the sockets' counters: true when either received anything since the last call.
    /// Off the I/O threads, by the service's sweep; two `getsockopt`s and an `ioctl` a socket.
    pub(crate) fn moved<A: Handler, B: Handler>(&self, client: &Conn<A>, up: &Conn<B>) -> bool {
        if !client.is_open() || !up.is_open() {
            return false;
        }
        let (Some(c), Some(u)) = (counts(client), counts(up)) else {
            return false;
        };
        let now = c.received.wrapping_add(u.received);
        self.activity.swap(now, Ordering::Relaxed) != now
    }

    /// `from` ended (`from_client`: the client did): `to` is closed once everything `from`
    /// received has been written to `to`'s send queue, or once that makes no progress for
    /// `stall`.
    pub(crate) fn close_after_forward<A: Handler, B: Handler>(
        &self,
        from: &Conn<A>,
        from_client: bool,
        to: Conn<B>,
        stall: Duration,
    ) {
        let Some(rx) = counts(from) else {
            return to.close_after_send();
        };
        let target = if from_client {
            self.up_written
                .saturating_add(rx.received.saturating_sub(self.client_read))
        } else {
            self.client_written.saturating_add(rx.received)
        };
        poll(Drain {
            to,
            target,
            last: 0,
            since: Instant::now(),
            stall,
            wait: FIRST_POLL,
        });
    }
}

/// How long a side waiting for the kernel's relay to forward to it waits first, and at most,
/// between two reads of its counters.
const FIRST_POLL: Duration = Duration::from_millis(1);
const MAX_POLL: Duration = Duration::from_millis(100);

/// A side to be closed once the kernel's relay has written `target` bytes to it.
struct Drain<H: Handler> {
    to: Conn<H>,
    target: u64,
    last: u64,
    since: Instant,
    stall: Duration,
    wait: Duration,
}

/// Closes the side once it has been written everything, or has stalled; else reads it again
/// later, on akit's timer thread. Never waits: a read of its counters returns at once.
fn poll<H: Handler>(mut d: Drain<H>) {
    if !d.to.is_open() {
        return;
    }
    let Some(b) = counts(&d.to) else {
        return d.to.close_after_send();
    };
    if b.written >= d.target {
        return d.to.close_after_send();
    }
    if b.written != d.last {
        d.last = b.written;
        d.since = Instant::now();
    } else if d.since.elapsed() >= d.stall {
        return d.to.close();
    }
    d.wait = d.wait.saturating_mul(2).min(MAX_POLL);
    let to = d.to.clone();
    if !akit_upstream::timer::after(d.wait, move || poll(d)) {
        // The timers are full: closed as a userspace relay closes it.
        to.close_after_send();
    }
}
