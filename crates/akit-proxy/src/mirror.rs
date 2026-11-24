//! Copies of requests sent to a mirror upstream, their answers dropped (I46): fit's proxy
//! `mirror.copyTo`.
//!
//! A copy goes only of a request whose body arrived whole (a body that streams is the client's,
//! read once), chosen at random by `percent`, and while fewer than [`MAX_MIRRORED`] copies are
//! under way: past that, requests are not copied, so a slow mirror costs memory and connections
//! up to the bound and no more. The request never waits for its copy: the copy has its own
//! head and body, no cancel token of the client's, and its response is read and dropped by
//! the pool's callbacks.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use akio_http::Headers;
use akio_http::client::ClientRequest;
use akit_config::MAX_MIRRORED;
use akit_runtime::upstream::{Download, Error, Exchange, Head, Pool, split};

/// A proxy's `mirror`, compiled.
pub(crate) struct Mirror {
    pool: Pool,
    percent: f64,
    /// The copies under way, at most [`MAX_MIRRORED`].
    in_flight: Arc<AtomicUsize>,
}

impl Mirror {
    pub(crate) fn new(pool: Pool, percent: f64) -> Mirror {
        Mirror {
            pool,
            percent,
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A slot for a copy of a request, if this one is chosen and one is free.
    pub(crate) fn slot(&self) -> Option<Slot> {
        if !split::chance(self.percent) {
            return None;
        }
        if self.in_flight.fetch_add(1, Ordering::AcqRel) >= MAX_MIRRORED {
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Slot(Arc::clone(&self.in_flight)))
    }

    /// Sends `req`, the copy, to the mirror, holding `slot` until its exchange is over.
    pub(crate) fn send(&self, slot: Slot, req: ClientRequest) {
        let backend = self.pool.pick();
        self.pool.stream(backend, req, Copied { _slot: slot });
    }

    /// The copies under way.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// One of the copies under way: given back when dropped.
pub(crate) struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A copy's exchange: its response read and dropped.
struct Copied {
    _slot: Slot,
}

impl Exchange for Copied {
    fn on_head(&mut self, _: Head, _: Download) {}
    fn on_data(&mut self, _: &[u8]) {}
    fn on_end(&mut self, _: &Headers) {}
    fn on_error(&mut self, _: Error) {}
}

/// The copy of the request `out`, about to go upstream with `body`: its method, target and
/// fields (but `Trailer`, since the copy goes whole, framed by its length), without the client's
/// cancel token, its time bounded by the mirror's own `response_timeout`.
pub(crate) fn copy_of(out: &ClientRequest, body: &[u8]) -> ClientRequest {
    let mut c = ClientRequest::new(out.method.clone(), &out.url);
    c.headers = out.headers.clone();
    c.headers.remove("trailer");
    c.body = body.to_vec();
    c
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn slots_are_bounded() {
        let u = akit_config::from_json(
            r#"{"upstreams": [{"name": "m", "addresses": ["127.0.0.1:1"]}]}"#,
            None,
        )
        .unwrap()
        .upstreams
        .remove(0);
        let pool = Pool::new(&u, akit_runtime::upstream::EngineSlot::new());
        let m = Mirror::new(pool.clone(), 100.0);
        let held: Vec<Slot> = (0..MAX_MIRRORED).map(|_| m.slot().unwrap()).collect();
        assert_eq!(m.in_flight(), MAX_MIRRORED);
        assert!(m.slot().is_none());
        assert_eq!(m.in_flight(), MAX_MIRRORED);
        drop(held);
        assert_eq!(m.in_flight(), 0);
        // None chosen at 0 percent.
        let none = Mirror::new(pool, 0.0);
        assert!((0..100).all(|_| none.slot().is_none()));
        assert_eq!(none.in_flight(), 0);
    }

    #[test]
    fn copies() {
        let mut out = ClientRequest::new(akio_http::Method::Post, "/p?q=1")
            .cancel_with(&akio_http::client::CancelToken::new());
        out.headers.append("X-Test", "t");
        out.headers.append("Trailer", "X-Sum");
        out.timeout = Some(std::time::Duration::from_secs(1));
        let c = copy_of(&out, b"payload");
        assert_eq!(c.method, akio_http::Method::Post);
        assert_eq!(c.url, "/p?q=1");
        assert_eq!(c.headers.get("x-test"), Some("t"));
        assert_eq!(c.headers.get("trailer"), None);
        assert_eq!(c.body, b"payload");
        assert_eq!(c.timeout, None);
    }
}
