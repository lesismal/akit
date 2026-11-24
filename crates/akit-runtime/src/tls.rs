//! A TLS listener's certificate manager across reloads (I30): each full handshake is served the
//! current graph's certificates for the listener, so that a reload which keeps the listener, its
//! sockets and its connections, still has its next handshakes get the new certificates.

use std::fmt;
use std::sync::Arc;

use akio::tls::{CertAlert, CertDone, CertResolver, ClientHelloInfo};
use akit_tls::{Certs, ListenerTls, ServerConfigs};

use crate::{Graph, GraphCell};

/// The certificate manager of the TLS listener named `listener`: an akio-tls
/// [`CertResolver`] that answers each full handshake from the certificates the cell's current
/// graph loaded for the listener, in the handshake's call (it never blocks).
///
/// A handshake under way keeps the certificate it was given; a reload changes what the next one
/// gets. Until the graph it was made for is published (a listener opened by a reload before the
/// swap), or when the current graph has no TLS listener of its name (one being drained after a
/// reload removed it), it serves the certificates of the graph it was made for.
///
/// Per handshake it reads the graph through the I/O thread's cache, as a request does (rule 13):
/// no lock and no shared reference count, but an `Arc` clone of the certificate chosen.
pub struct ListenerCerts {
    cell: GraphCell,
    listener: String,
    /// The epoch from which the cell's current graph is the one to serve from.
    since: u64,
    /// The certificates of the graph it was made for.
    own: Arc<Certs>,
}

impl fmt::Debug for ListenerCerts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenerCerts")
            .field("listener", &self.listener)
            .field("since", &self.since)
            .finish_non_exhaustive()
    }
}

impl ListenerCerts {
    /// The manager of the listener at `index` of `graph`, which is `cell`'s current graph, or,
    /// with `pending`, the graph a reload is about to publish in it. `None` when the listener has
    /// no `tls`.
    #[must_use]
    pub fn new(cell: &GraphCell, graph: &Graph, index: usize, pending: bool) -> Option<Self> {
        let l = graph.listeners().get(index)?;
        let tls = graph.tls(index)?;
        let epoch = cell.epoch();
        Some(ListenerCerts {
            cell: cell.clone(),
            listener: l.name.clone(),
            since: if pending {
                epoch.saturating_add(1)
            } else {
                epoch
            },
            own: Arc::clone(tls.certs()),
        })
    }

    fn get(&self, hello: &ClientHelloInfo<'_>) -> Option<akio::tls::CertifiedKey> {
        if self.cell.epoch() >= self.since {
            if let Some(k) = self
                .cell
                .with(|g| g.certs(&self.listener).map(|c| c.get(hello)))
            {
                return k;
            }
        }
        self.own.get(hello)
    }
}

impl CertResolver for ListenerCerts {
    fn resolve(&self, hello: &ClientHelloInfo<'_>, done: CertDone) {
        match self.get(hello) {
            Some(k) => done.complete(k),
            None => done.fail(CertAlert::UnrecognizedName),
        }
    }
}

/// The configurations the TLS listener at `index` of `graph` serves, its certificates managed by
/// a [`ListenerCerts`] (see [`ListenerCerts::new`] for `pending`). `None` for a listener in the
/// clear.
///
/// # Errors
///
/// When akio-tls refuses the configuration (the graph built one when it was compiled, so this is
/// not expected).
pub fn server_configs(
    cell: &GraphCell,
    graph: &Graph,
    index: usize,
    pending: bool,
) -> Option<Result<ServerConfigs, akit_tls::Error>> {
    let tls: &ListenerTls = graph.tls(index)?;
    let manager = ListenerCerts::new(cell, graph, index, pending)?;
    Some(tls.server_configs(Arc::new(manager)))
}
