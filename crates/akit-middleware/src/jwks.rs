//! A JWK set fetched from a URL for the `jwt` middleware, as fit's `jwks`: once when the engine
//! is there, every `jwks_refresh_interval`, and when a token names a `kid` the set does not have,
//! as a key rotation brings, at most every [`MIN_REFETCH`].
//!
//! - The set is fetched with akio's asynchronous client on akit's engine (rule 2), as the
//!   upstream pools' requests are: the request is bounded ([`MAX_JWKS_BYTES`], a 200 answer
//!   only, [`JWKS_TIMEOUT`]) and its answer parsed in the client's callback, on the I/O thread it
//!   arrives on; nothing waits for it. The refresh and the delayed fetches are jobs on akio's
//!   timer thread ([`akio::timer::after_send`]), which only start a fetch.
//! - A fetch that fails keeps the keys there were, and says why ([`Jwks::last_error`]).
//! - Requests whose token names a `kid` the set does not have wait for the next fetch, at most
//!   [`MAX_KEY_WAITERS`] at once; each waits suspended (akio's `Context::suspend`, ADR-109),
//!   holding no thread, and is verified again with what the fetch brought, on the thread it
//!   ends on. A request checked against a set that a fetch has replaced since is verified again
//!   at once, as fit's `wait` does.
//! - When the config is released, fetching stops: a fetch under way is cancelled and the
//!   requests still waiting are verified with the keys there are.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use akio::timer::after_send;
use akio_http::client::{
    CancelToken, Client, ClientError, ClientOptions, ClientRequest, ClientResponse, RedirectPolicy,
};
use akit_runtime::upstream::EngineSlot;

use crate::jws::{Key, KeySet, parse_jwks};

/// The largest JWK set read, as fit's `maxJWKSBytes`.
pub(crate) const MAX_JWKS_BYTES: usize = 1 << 20;

/// How long a fetch may take, as fit's `jwksTimeout`.
pub(crate) const JWKS_TIMEOUT: Duration = Duration::from_secs(10);

/// The most requests that wait for a fetch at once, as fit's `maxKeyWaiters`.
pub(crate) const MAX_KEY_WAITERS: usize = 1024;

/// The least time between two fetches a token with an unknown `kid` asks for, so that such
/// tokens cannot have the set fetched on every request: fit's `minRefetch`.
pub(crate) const MIN_REFETCH: Duration = Duration::from_secs(10);

/// [`MIN_REFETCH`] for the sets made from now on, in nanoseconds: shorter in tests.
static MIN_REFETCH_NANOS: AtomicU64 =
    AtomicU64::new(MIN_REFETCH.as_secs().saturating_mul(1_000_000_000));

/// Sets the least time between two fetches of the JWK sets made from now on (tests only).
pub(crate) fn set_min_refetch(d: Duration) {
    MIN_REFETCH_NANOS.store(
        u64::try_from(d.as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

/// A request waiting for a fetch: run with the keys there are once it is done.
type Waiter = Box<dyn FnOnce(Option<Arc<KeySet>>) + Send>;

/// A JWK set fetched from a URL. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Jwks {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Jwks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jwks")
            .field("url", &self.inner.url)
            .field("refresh", &self.inner.refresh)
            .finish_non_exhaustive()
    }
}

struct Inner {
    url: String,
    refresh: Duration,
    min_refetch: Duration,
    /// The config's own keys, which every set has too.
    static_keys: Vec<Arc<Key>>,
    /// The current set's generation; 0 before the first fetch brought one.
    generation: AtomicU64,
    /// Made once the engine is there.
    client: OnceLock<Client>,
    /// Cancels the fetch under way when the config is released.
    cancel: CancelToken,
    st: Mutex<State>,
}

#[derive(Default)]
struct State {
    keys: Option<Arc<KeySet>>,
    waiters: Vec<Waiter>,
    /// A fetch is under way.
    fetching: bool,
    /// A fetch the waiters asked for is due on the timer thread.
    refetch_due: bool,
    /// When the last fetch began.
    fetched: Option<Instant>,
    /// Fetches done: a refresh scheduled after one is dropped when another was done since.
    round: u64,
    /// The last fetch's failure, or `None`.
    error: Option<String>,
    closed: bool,
}

impl Jwks {
    /// A set fetched from `url` every `refresh`, with `static_keys` in it too, through the engine
    /// `slot` holds once it is filled (never, in `akit validate`).
    pub(crate) fn new(
        url: &str,
        refresh: Duration,
        static_keys: Vec<Arc<Key>>,
        slot: &EngineSlot,
    ) -> Jwks {
        let inner = Arc::new(Inner {
            url: url.to_owned(),
            refresh,
            min_refetch: Duration::from_nanos(MIN_REFETCH_NANOS.load(Ordering::Relaxed)),
            static_keys,
            generation: AtomicU64::new(0),
            client: OnceLock::new(),
            cancel: CancelToken::new(),
            st: Mutex::new(State::default()),
        });
        let weak = Arc::downgrade(&inner);
        slot.when_set(move |engine| {
            let Some(inner) = weak.upgrade() else { return };
            let mut opts = ClientOptions::default();
            opts.max_body_bytes = MAX_JWKS_BYTES;
            opts.max_idle_per_host = 1;
            opts.max_idle = 1;
            let client = Client::new(Arc::clone(engine))
                .with_options(opts)
                .with_redirects(RedirectPolicy::limit(10))
                .with_timeout(JWKS_TIMEOUT);
            let _ = inner.client.set(client);
            Inner::fetch(&inner);
        });
        Jwks { inner }
    }

    /// The current set's generation; 0 before a fetch brought one. Read on every request: one
    /// atomic load.
    pub(crate) fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    /// The current set, if a fetch brought one.
    pub(crate) fn current(&self) -> Option<Arc<KeySet>> {
        self.inner.lock().keys.clone()
    }

    /// Why the last fetch failed, or `None` when it did not.
    #[cfg(test)]
    pub(crate) fn last_error(&self) -> Option<String> {
        self.inner.lock().error.clone()
    }

    /// Queues `then` to run once the next fetch is done, and asks for one, as fit's `wait`.
    /// `seen` is the generation of the set the request was checked against: when a fetch has
    /// brought another since, `then` runs at once, on this thread. `then` comes back when
    /// [`MAX_KEY_WAITERS`] wait already or the set is closed.
    pub(crate) fn wait<F>(&self, seen: u64, then: F) -> Result<(), F>
    where
        F: FnOnce(Option<Arc<KeySet>>) + Send + 'static,
    {
        let inner = &self.inner;
        let mut st = inner.lock();
        if st.closed {
            return Err(then);
        }
        // A fetch stores its keys under the lock it takes the waiters under, so a request that
        // finds `seen` current here is taken by the next.
        let current = st.keys.as_ref().map_or(0, |k| k.generation);
        if current != seen {
            let keys = st.keys.clone();
            drop(st);
            then(keys);
            return Ok(());
        }
        if st.waiters.len() >= MAX_KEY_WAITERS {
            return Err(then);
        }
        st.waiters.push(Box::new(then));
        if st.fetching || st.refetch_due || inner.client.get().is_none() {
            // The fetch under way, or the first, answers it.
            return Ok(());
        }
        let delay = st.fetched.map_or(Duration::ZERO, |t| {
            inner.min_refetch.saturating_sub(t.elapsed())
        });
        st.refetch_due = true;
        drop(st);
        let weak = Arc::downgrade(inner);
        let job = move || {
            if let Some(inner) = weak.upgrade() {
                inner.lock().refetch_due = false;
                Inner::fetch(&inner);
            }
        };
        if delay.is_zero() || after_send(delay, job).is_err() {
            // No timer: fetched now rather than never.
            inner.lock().refetch_due = false;
            Inner::fetch(inner);
        }
        Ok(())
    }

    /// Stops fetching: a fetch under way is cancelled, and the requests still waiting are
    /// verified with the keys there are. Never blocks: it runs when the config is released.
    pub(crate) fn close(&self) {
        let (waiters, keys) = {
            let mut st = self.inner.lock();
            st.closed = true;
            (std::mem::take(&mut st.waiters), st.keys.clone())
        };
        self.inner.cancel.cancel();
        for w in waiters {
            w(keys.clone());
        }
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a fetch, unless one is under way, the set is closed or there is no engine yet.
    fn fetch(inner: &Arc<Inner>) {
        let Some(client) = inner.client.get() else {
            return;
        };
        {
            let mut st = inner.lock();
            if st.closed || st.fetching {
                return;
            }
            st.fetching = true;
            st.fetched = Some(Instant::now());
        }
        let weak = Arc::downgrade(inner);
        let req = ClientRequest::get(&inner.url)
            .header("user-agent", "akit")
            .cancel_with(&inner.cancel);
        // The callback runs once, on an I/O thread (or this one, for a URL the client refuses).
        client.request(req, move |r| {
            if let Some(inner) = weak.upgrade() {
                Inner::fetched(&inner, r);
            }
        });
    }

    /// A fetch is done: its keys are stored (or its failure), a refresh is scheduled, and the
    /// requests waiting are verified with what it brought.
    fn fetched(inner: &Arc<Inner>, r: Result<ClientResponse, ClientError>) {
        let parsed = match r {
            Ok(resp) if resp.status == 200 => parse_jwks(&resp.body),
            Ok(resp) => Err(format!("status {}", resp.status)),
            Err(e) => Err(e.to_string()),
        };
        let (waiters, keys, round, closed) = {
            let mut st = inner.lock();
            st.fetching = false;
            match parsed {
                Ok(fetched) => {
                    let set = Arc::new(KeySet::new([&inner.static_keys[..], &fetched[..]]));
                    inner.generation.store(set.generation, Ordering::Release);
                    st.keys = Some(set);
                    st.error = None;
                }
                Err(e) => st.error = Some(format!("jwt: fetching {}: {e}", inner.url)),
            }
            st.round = st.round.wrapping_add(1);
            (
                std::mem::take(&mut st.waiters),
                st.keys.clone(),
                st.round,
                st.closed,
            )
        };
        if !closed {
            let weak: Weak<Inner> = Arc::downgrade(inner);
            // Past the timers' bound the set is no longer refreshed, but still fetched when a
            // token names a kid it does not have.
            let _ = after_send(inner.refresh, move || {
                if let Some(inner) = weak.upgrade() {
                    let due = {
                        let st = inner.lock();
                        st.round == round && !st.closed
                    };
                    if due {
                        Inner::fetch(&inner);
                    }
                }
            });
        }
        for w in waiters {
            w(keys.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn unfetched() -> Jwks {
        Jwks::new(
            "http://127.0.0.1:1/keys",
            MIN_REFETCH,
            Vec::new(),
            &EngineSlot::new(),
        )
    }

    // fit's TestJWKSWaitStale: a request checked against a set a fetch has replaced since runs at
    // once, rather than waiting for the fetch after.
    #[test]
    fn waiting_against_a_stale_set() {
        let j = unfetched();
        let ran = Arc::new(AtomicUsize::new(0));
        let run = || {
            let ran = Arc::clone(&ran);
            move |_: Option<Arc<KeySet>>| {
                ran.fetch_add(1, Ordering::SeqCst);
            }
        };
        let count = || ran.load(Ordering::SeqCst);
        let waiting = |j: &Jwks| j.inner.lock().waiters.len();

        // Before the first fetch: the request waits for it.
        assert!(j.wait(0, run()).is_ok());
        assert_eq!((count(), waiting(&j)), (0, 1));
        let first = Arc::new(KeySet::new([]));
        j.inner.lock().keys = Some(Arc::clone(&first));
        assert!(j.wait(first.generation, run()).is_ok());
        assert_eq!((count(), waiting(&j)), (0, 2));

        // Against a set replaced since: at once.
        assert!(j.wait(0, run()).is_ok());
        assert_eq!((count(), waiting(&j)), (1, 2));
        j.inner.lock().keys = Some(Arc::new(KeySet::new([])));
        assert!(j.wait(first.generation, run()).is_ok());
        assert_eq!((count(), waiting(&j)), (2, 2));

        // Closed: those waiting run, and no more wait.
        j.close();
        assert_eq!((count(), waiting(&j)), (4, 0));
        assert!(j.wait(0, run()).is_err());
    }

    // Part of fit's TestJWKSClosedWaiting: past MAX_KEY_WAITERS, a request is not queued.
    #[test]
    fn waiters_bounded() {
        let j = unfetched();
        for _ in 0..MAX_KEY_WAITERS {
            assert!(j.wait(0, |_| {}).is_ok());
        }
        assert!(j.wait(0, |_| {}).is_err());
        j.close();
    }

    // Part of fit's TestJWKSRefresh: a fetch that fails keeps the keys there were, says why, and
    // still answers the requests waiting.
    #[test]
    fn failed_fetches_keep_the_keys() {
        let j = unfetched();
        let set = Arc::new(KeySet::new([]));
        j.inner.lock().keys = Some(Arc::clone(&set));
        let got = Arc::new(Mutex::new(None));
        let g = Arc::clone(&got);
        assert!(
            j.wait(set.generation, move |k| *g.lock().unwrap() =
                k.map(|k| k.generation))
                .is_ok()
        );
        Inner::fetched(&j.inner, Err(ClientError::TooLarge("body")));
        assert_eq!(*got.lock().unwrap(), Some(set.generation));
        assert_eq!(j.current().map(|k| k.generation), Some(set.generation));
        let e = j.last_error().unwrap();
        assert!(
            e.contains("fetching http://127.0.0.1:1/keys") && e.contains("too large"),
            "{e}"
        );
        assert!(!j.inner.lock().fetching);
        j.close();
    }

    #[test]
    fn min_refetch_set() {
        let before = MIN_REFETCH_NANOS.load(Ordering::Relaxed);
        assert_eq!(Duration::from_nanos(before), MIN_REFETCH);
        assert_eq!(unfetched().inner.min_refetch, MIN_REFETCH);
    }
}
