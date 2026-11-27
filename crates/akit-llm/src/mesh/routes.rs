//! fit's `mesh.Routes` (I106b): the model calls a role relays, counted per route (from the peer
//! they came from, to the peer they went to, and the model). A call's bytes are read from its
//! stream as they cross, so a long answer shows as it streams, not when it ends. At most
//! [`MAX_ROUTES`] routes are counted apart; the calls of any route past them are counted
//! together, as [`ROUTE_OTHER`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::json::Obj;
use crate::wire::Stream;

/// The routes a role counts apart.
pub const MAX_ROUTES: usize = 1024;

/// The `from`, `to` and `model` of the calls counted together past [`MAX_ROUTES`].
pub const ROUTE_OTHER: &str = "other";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The counts of one route (fit's `model.RouteStats`). `from` is the peer the calls came from,
/// empty for a proxy's own agents; `to` the peer they went to, empty for a node's upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteStats {
    /// The peer the calls came from.
    pub from: String,
    /// The peer they went to.
    pub to: String,
    /// The model.
    pub model: String,
    /// The calls.
    pub calls: i64,
    /// Those that failed.
    pub errors: i64,
    /// Those running.
    pub inflight: i64,
    /// The requests' body bytes.
    pub req_bytes: i64,
    /// The responses' body bytes.
    pub resp_bytes: i64,
    /// When the last call began, in seconds since 1970.
    pub last_at: i64,
}

impl RouteStats {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str_omit("from", &self.from)
            .str_omit("to", &self.to)
            .str("model", &self.model)
            .int("calls", self.calls)
            .int("errors", self.errors)
            .int("inflight", self.inflight)
            .int("req_bytes", self.req_bytes)
            .int("resp_bytes", self.resp_bytes)
            .int_omit("last_at", self.last_at)
            .end()
    }
}

#[derive(Default)]
struct Live {
    st: Option<Stream>,
    inbound: bool,
    req: i64,
}

impl Live {
    /// The call's request and response bytes so far.
    fn bytes(&self) -> (i64, i64) {
        let mut req = self.req;
        let mut resp = 0;
        if let Some(st) = &self.st {
            let (i, o) = st.bytes();
            let (i, o) = (
                i64::try_from(i).unwrap_or(i64::MAX),
                i64::try_from(o).unwrap_or(i64::MAX),
            );
            if self.inbound {
                req = req.saturating_add(i);
                resp = o;
            } else {
                req = req.saturating_add(o);
                resp = i;
            }
        }
        (req, resp)
    }
}

struct Entry {
    stats: RouteStats,
    live: HashMap<u64, Live>,
}

#[derive(Default)]
struct St {
    m: HashMap<(String, String, String), Entry>,
    next: u64,
}

/// The routes of one role.
#[derive(Clone, Default)]
pub struct Routes(Arc<Mutex<St>>);

impl std::fmt::Debug for Routes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Routes").finish_non_exhaustive()
    }
}

/// One call on a route, from [`Routes::begin`] to [`RouteCall::end`]; dropped without an end,
/// it ends as a failure.
#[derive(Debug)]
pub struct RouteCall {
    r: Routes,
    key: (String, String, String),
    id: u64,
}

impl Routes {
    /// No routes yet.
    #[must_use]
    pub fn new() -> Routes {
        Routes::default()
    }

    /// Counts a call from peer `from` to peer `to`, of `model`.
    #[must_use]
    pub fn begin(&self, from: &str, to: &str, model: &str) -> RouteCall {
        let mut g = lock(&self.0);
        let mut k = (from.to_owned(), to.to_owned(), model.to_owned());
        if !g.m.contains_key(&k) && g.m.len() >= MAX_ROUTES {
            k = (
                ROUTE_OTHER.to_owned(),
                ROUTE_OTHER.to_owned(),
                ROUTE_OTHER.to_owned(),
            );
        }
        g.next = g.next.wrapping_add(1);
        let id = g.next;
        let e = g.m.entry(k.clone()).or_insert_with(|| Entry {
            stats: RouteStats {
                from: k.0.clone(),
                to: k.1.clone(),
                model: k.2.clone(),
                ..RouteStats::default()
            },
            live: HashMap::new(),
        });
        e.stats.calls = e.stats.calls.saturating_add(1);
        e.stats.last_at = akit_roles::linktoken::unix_now();
        e.live.insert(id, Live::default());
        RouteCall {
            r: self.clone(),
            key: k,
            id,
        }
    }

    /// Every route's counts, the calls running included as far as they got, sorted by `from`,
    /// `to` and `model`.
    #[must_use]
    pub fn snapshot(&self) -> Vec<RouteStats> {
        let g = lock(&self.0);
        let mut out: Vec<RouteStats> =
            g.m.values()
                .map(|e| {
                    let mut s = e.stats.clone();
                    for c in e.live.values() {
                        let (req, resp) = c.bytes();
                        s.req_bytes = s.req_bytes.saturating_add(req);
                        s.resp_bytes = s.resp_bytes.saturating_add(resp);
                        s.inflight = s.inflight.saturating_add(1);
                    }
                    s
                })
                .collect();
        drop(g);
        out.sort_by(|a, b| (&a.from, &a.to, &a.model).cmp(&(&b.from, &b.to, &b.model)));
        out
    }
}

impl RouteCall {
    fn with(&self, f: impl FnOnce(&mut Live)) {
        let mut g = lock(&self.r.0);
        if let Some(c) =
            g.m.get_mut(&self.key)
                .and_then(|e| e.live.get_mut(&self.id))
        {
            f(c);
        }
    }

    /// Counts the call's bytes from `st`: on a stream the role took (`inbound`), what it
    /// receives is the request and what it sends the response; on one it opened, the other way
    /// round.
    pub fn attach(&self, st: &Stream, inbound: bool) {
        // A call that ended is no longer kept: what it is given then is not counted.
        self.with(|c| {
            c.st = Some(st.clone());
            c.inbound = inbound;
        });
    }

    /// Counts `n` bytes of the request carried outside the stream's data, as a body inline in
    /// its opening request.
    pub fn add_req(&self, n: usize) {
        self.with(|c| c.req = c.req.saturating_add(i64::try_from(n).unwrap_or(i64::MAX)));
    }

    /// Ends the call, a failure when `failed`. Only its first end counts.
    pub fn end(&self, failed: bool) {
        let mut g = lock(&self.r.0);
        let Some(e) = g.m.get_mut(&self.key) else {
            return;
        };
        let Some(c) = e.live.remove(&self.id) else {
            return;
        };
        let (req, resp) = c.bytes();
        e.stats.req_bytes = e.stats.req_bytes.saturating_add(req);
        e.stats.resp_bytes = e.stats.resp_bytes.saturating_add(resp);
        if failed {
            e.stats.errors = e.stats.errors.saturating_add(1);
        }
    }
}

impl Drop for RouteCall {
    fn drop(&mut self) {
        self.end(true);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    /// fit's `TestRoutes`: calls counted per route, ended once, failures apart.
    #[test]
    fn counts_per_route() {
        let r = Routes::new();
        let a = r.begin("p1", "n1", "m1");
        a.add_req(10);
        let b = r.begin("p1", "n1", "m1");
        let c = r.begin("", "n2", "m2");
        let s = r.snapshot();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].to, "n2");
        assert_eq!(s[1].inflight, 2);
        assert_eq!(s[1].req_bytes, 10);
        a.end(false);
        a.end(true); // only the first counts
        b.end(true);
        drop(c); // dropped: a failure
        let s = r.snapshot();
        assert_eq!(
            (s[1].calls, s[1].errors, s[1].inflight, s[1].req_bytes),
            (2, 1, 0, 10)
        );
        assert_eq!((s[0].calls, s[0].errors), (1, 1));
        assert!(s[0].last_at > 0);
        assert_eq!(
            s[0].to_json(),
            format!(
                r#"{{"to":"n2","model":"m2","calls":1,"errors":1,"inflight":0,"req_bytes":0,"resp_bytes":0,"last_at":{}}}"#,
                s[0].last_at
            )
        );
    }

    /// fit's `TestRoutesBound`: past MAX_ROUTES, the calls of new routes are counted together.
    #[test]
    fn bounded() {
        let r = Routes::new();
        for i in 0..MAX_ROUTES {
            r.begin("p", "n", &format!("m{i}")).end(false);
        }
        r.begin("p", "n", "m0").end(false);
        r.begin("p", "n", "new").end(true);
        r.begin("p", "n", "newer").end(false);
        let s = r.snapshot();
        assert_eq!(s.len(), MAX_ROUTES + 1);
        let o = s.iter().find(|x| x.from == ROUTE_OTHER).unwrap();
        assert_eq!((o.calls, o.errors, o.model.as_str()), (2, 1, ROUTE_OTHER));
        assert_eq!(s.iter().find(|x| x.model == "m0").unwrap().calls, 2);
    }
}
