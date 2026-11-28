//! A role's own pages' way to its methods (fit's `Router.Invoke` and `Router.OpenLocal` with
//! the caller [`ROLE_LOCAL`]): every call and stream runs on one end of an in-memory session
//! ([`wire::pipe`]), so it behaves exactly as over the mesh (credit, cancel, the idle timeout,
//! errors as [`Error`]), and the methods are the ones peers reach, through one code path.
//!
//! [`Local`] is what a role's [`Role::api`](akit_roles::host::Role::api) hands its pages (I109)
//! and its tests: downcast `Arc<dyn Any>` to it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::mesh::WeakMesh;
use crate::model::{Identity, ROLE_LOCAL};
use crate::wire::{self, Error, Reply, Request, Session, Stream};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The caller a role's own pages are.
#[must_use]
pub fn local_caller() -> Identity {
    Identity {
        role: ROLE_LOCAL.to_owned(),
        id: "console".to_owned(),
        ..Identity::default()
    }
}

struct Serve(WeakMesh);

impl wire::Handler for Serve {
    fn request(&self, _s: &Session, req: Request, reply: Reply) {
        match self.0.upgrade() {
            Some(m) => m
                .router()
                .invoke(&local_caller(), &req.method, &req.payload, reply),
            None => reply.answer(Err(Error::Closed)),
        }
    }

    fn stream(&self, _s: &Session, st: Stream) {
        match self.0.upgrade() {
            Some(m) => m.router().invoke_stream(&local_caller(), st),
            None => st.end(Err(Error::Closed)),
        }
    }
}

struct Inner {
    mesh: WeakMesh,
    /// The caller's end of the pipe, made at the first call, again after it closed.
    pipe: Mutex<Option<(Session, Session)>>,
    /// Where the proxy role's agents' API listens, its port as bound (I161): what the console's
    /// assistant calls an `llm_proxy` provider at.
    api_addr: Mutex<Option<ApiAddr>>,
}

/// The address of the proxy role's agents' API, as bound; `None` while it does not run.
pub type ApiAddr = Arc<dyn Fn() -> Option<std::net::SocketAddr> + Send + Sync>;

/// A role's methods, called as its own pages ([`ROLE_LOCAL`]). Cheap to clone.
#[derive(Clone)]
pub struct Local(Arc<Inner>);

impl std::fmt::Debug for Local {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Local").finish_non_exhaustive()
    }
}

impl Local {
    /// The methods of `mesh`'s router.
    #[must_use]
    pub fn new(mesh: WeakMesh) -> Local {
        Local(Arc::new(Inner {
            mesh,
            pipe: Mutex::new(None),
            api_addr: Mutex::new(None),
        }))
    }

    /// Says where the role's agents' API listens (the proxy role, I161).
    pub fn set_api_addr(&self, f: ApiAddr) {
        *lock(&self.0.api_addr) = Some(f);
    }

    /// Where the proxy role's agents' API listens, its port as bound: `None` for another role,
    /// or while it does not run.
    #[must_use]
    pub fn api_addr(&self) -> Option<std::net::SocketAddr> {
        let f = lock(&self.0.api_addr).clone()?;
        f()
    }

    fn session(&self) -> Session {
        let mut g = lock(&self.0.pipe);
        if let Some((d, _)) = g.as_ref() {
            if !d.is_closed() {
                return d.clone();
            }
        }
        let (d, l) = wire::session::pipe(
            wire::Options {
                max_inbound: 0,
                ..wire::Options::default()
            },
            Arc::new(Serve(WeakMesh::default())),
            Arc::new(Serve(self.0.mesh.clone())),
        );
        *g = Some((d.clone(), l));
        d
    }

    /// Calls `method` with JSON `req`; `done` gets the answer's JSON or the error, once, within
    /// `timeout` (zero: the method's own bound only). Returns at once.
    pub fn call(
        &self,
        method: &str,
        req: &str,
        timeout: Duration,
        done: impl FnOnce(Result<Vec<u8>, Error>) + Send + 'static,
    ) {
        let req = if req.is_empty() { "{}" } else { req };
        self.session().call(method, req.as_bytes(), timeout, done);
    }

    /// Calls `method` and waits for its answer, `timeout` at most. Blocks: never on an I/O
    /// thread (the pages' threads, tests).
    ///
    /// # Errors
    ///
    /// The method's error; [`Error::Timeout`].
    pub fn call_wait(&self, method: &str, req: &str, timeout: Duration) -> Result<Vec<u8>, Error> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.call(method, req, timeout, move |r| {
            let _ = tx.send(r);
        });
        rx.recv_timeout(timeout.saturating_add(Duration::from_secs(1)))
            .unwrap_or(Err(Error::Timeout))
    }

    /// Opens the stream method `method` with JSON `req`: read it as one from
    /// [`Mesh::open_stream`](crate::mesh::Mesh::open_stream).
    ///
    /// # Errors
    ///
    /// The pipe's failure.
    pub fn open_stream(&self, method: &str, req: &str) -> Result<Stream, Error> {
        let req = if req.is_empty() { "{}" } else { req };
        self.session().open_stream(method, req.as_bytes())
    }

    /// The router's methods and stream methods, sorted.
    #[must_use]
    pub fn methods(&self) -> (Vec<String>, Vec<String>) {
        self.0.mesh.upgrade().map_or_else(Default::default, |m| {
            (m.router().methods(), m.router().stream_methods())
        })
    }

    /// Ends the pipe: what runs on it ends as closed.
    pub fn close(&self) {
        if let Some((d, l)) = lock(&self.0.pipe).take() {
            d.close(Error::Closed);
            l.close(Error::Closed);
        }
    }
}
