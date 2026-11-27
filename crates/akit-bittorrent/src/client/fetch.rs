//! .torrent files by URL (I153, fit's `bittorrent/fetch.go`): on akio's HTTP client, on the
//! listeners' engine, the caller's thread waiting for the answer (never an I/O thread's). The body
//! is bounded by 32 MiB, at most 5 redirects are followed (a redirect to a magnet link adds that
//! link), `url_timeout` bounds the whole download, and at most 4 run at once, one more refused
//! (fit's decision 1 of I198).

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use akio_http::client::{Client, ClientOptions, ClientRequest, RedirectPolicy};

use super::{ApiError, Shared, Source};

/// The most bytes of a .torrent file fetched by URL.
pub(crate) const MAX_URL_BYTES: usize = 32 << 20;
/// The most fetches at once.
pub(crate) const MAX_FETCHES: usize = 4;
/// The most redirects followed.
const MAX_REDIRECTS: usize = 5;

/// Takes one of the fetches' places, given back when dropped.
struct Slot<'a>(&'a Shared);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetches.fetch_sub(1, Ordering::AcqRel);
    }
}

/// `next` resolved against `base`: absolute, or a path on `base`'s host.
fn resolve(base: &str, next: &str) -> String {
    if next.contains("://") || next.starts_with("magnet:") {
        return next.to_owned();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let host = rest.split('/').next().unwrap_or("");
    if next.starts_with('/') {
        format!("{scheme}://{host}{next}")
    } else {
        format!("{scheme}://{host}/{next}")
    }
}

/// What `url` names: the .torrent file it serves, or the magnet link it is or redirects to.
pub(crate) fn fetch(shared: &Shared, url: &str, timeout: Duration) -> Result<Source, ApiError> {
    if url.starts_with("magnet:") {
        return Ok(Source::Magnet(url.to_owned()));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError::BadRequest(format!(
            "{url:?} is not an http, https or magnet URL"
        )));
    }
    if shared.fetches.fetch_add(1, Ordering::AcqRel) >= MAX_FETCHES {
        shared.fetches.fetch_sub(1, Ordering::AcqRel);
        return Err(ApiError::Busy);
    }
    let _slot = Slot(shared);
    let mut o = ClientOptions::default();
    o.max_body_bytes = MAX_URL_BYTES;
    o.max_idle_per_host = 0;
    let client = Client::new(std::sync::Arc::clone(&shared.engine))
        .with_options(o)
        .with_redirects(RedirectPolicy::none());
    let now = Instant::now();
    let deadline = now.checked_add(timeout).unwrap_or(now);
    let mut at = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ApiError::Fetch(format!("{url}: timed out")));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        // On the client's thread: hands the outcome over, never waits (room for one).
        client.request(ClientRequest::get(&at).timeout(left), move |r| {
            let _ = tx.try_send(r);
        });
        let resp = match rx.recv_timeout(left.saturating_add(Duration::from_secs(1))) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(ApiError::Fetch(format!("{at}: {e}"))),
            Err(_) => return Err(ApiError::Fetch(format!("{url}: timed out"))),
        };
        match resp.status {
            200 => return Ok(Source::Torrent(resp.body)),
            301 | 302 | 303 | 307 | 308 => {
                let Some(loc) = resp.headers.get("location") else {
                    return Err(ApiError::Fetch(format!(
                        "{at}: HTTP {} without a Location",
                        resp.status
                    )));
                };
                let next = resolve(&at, loc);
                if next.starts_with("magnet:") {
                    return Ok(Source::Magnet(next));
                }
                at = next;
            }
            s => return Err(ApiError::Fetch(format!("{at}: HTTP {s}"))),
        }
    }
    Err(ApiError::Fetch(format!(
        "{url}: more than {MAX_REDIRECTS} redirects"
    )))
}
