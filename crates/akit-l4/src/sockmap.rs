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
//!   before the kernel relays any byte (akio's [`akio::ebpf::Relay::tunnel`]). The upstream is
//!   dialed once the callback that took the connection is over (on the client's thread), so
//!   what a layer below hands over after it (the rest of the read that brought a PROXY header)
//!   is among them;
//! - neither connection has a transport layer (TLS, which a `tcp` listener never terminates);
//! - the relay has room ([`akio::ebpf::DEFAULT_PAIRS`] pairs).
//!
//! A connection that began with a PROXY header the listener read is spliced too: the tunnel's
//! base counters count what userspace read from the client socket, whatever layer read it (the
//! header, a ClientHello), and the kernel relays only what comes after. akio's ordering contract
//! asks that userspace have read all of what had arrived or none of it (the kernel relays a
//! partly read segment again from its start): the client's reads are held from the accept, or
//! from the read that brought its ClientHello, and what akio reads before a hold takes effect is
//! everything the socket had, unless a read filled akio's read buffer (64 KiB), which a
//! ClientHello and a PROXY header do not.
//!
//! Otherwise, or when the splice fails, nothing is lost: what was not written first is sent
//! through the engine (akio's `SpliceError::written` says how much was), and the connection is
//! relayed in userspace.
//!
//! A spliced connection's bytes never reach its handlers, nor akio's idle timer (its idle
//! timeout is turned off), so its progress is read from the sockets' TCP counters, through
//! akio's [`akio::ebpf::Tunnel`] (the client is its side A, the upstream B): its idle timeout is
//! kept by [`Tunnel::moved`](akio::ebpf::Tunnel::moved), read by each sweep of the service, and a
//! side that ends has the other closed only once everything it received has reached the other's
//! send queue (closing a socket drops what the kernel's relay still holds for it), or once that
//! makes no progress for the service's idle timeout
//! ([`Tunnel::close_after_forward`](akio::ebpf::Tunnel::close_after_forward), on akio-ebpf's
//! drain thread). A payload a handler still reads from a spliced connection while both sides
//! are open can only be a read that completed before the splice took over (io_uring), which the
//! kernel's newer bytes may have overtaken: both sides are closed rather than the stream
//! reordered ([`Tunnel::late_payload`](akio::ebpf::Tunnel::late_payload)).
//!
//! [`MIN_KERNEL`]: akio::ebpf::MIN_KERNEL

use std::sync::OnceLock;

use akio::ebpf::Relay;

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
                akit_observe::warn!(
                    Engine,
                    "the sockmap relay cannot be loaded ({e}): listeners with sockmap relay \
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
