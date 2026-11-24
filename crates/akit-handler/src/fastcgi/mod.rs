//! `fastcgi`: requests answered by a FastCGI application, as PHP-FPM is (I75), as fit's
//! `handler/fastcgi`. The request's CGI parameters ([`params`]) and its body go to the
//! application, and the response it writes comes back, both streamed and neither held whole
//! ([`exchange`]).
//!
//! The client is akit's own, over connections akio's engine dials ([`pool`]), so nothing waits
//! on a thread: one request at a time on each connection, idle ones kept for the next, their
//! number bounded, and a request that finds none free waits in a bounded queue. Every exchange
//! has its timeouts: connect, the response's header, the response going idle, and the whole. A
//! request with no body and a method safe to repeat that took an idle connection the application
//! closed unanswered is sent again, once, on a new one.
//!
//! A script whose path has a segment that starts with a dot, as `/.git/hooks/x.php` or `/.env`,
//! is not run: it is answered 404, and `try_files` passes over it, but for `/.well-known/` under
//! the root, unless `dotfiles` is `allow`. A script whose path has a `..` segment, a NUL or a
//! backslash is answered 400, with `try_files` or without. Its path info is the application's to
//! read, dots and all. `try_files` looks at the disk on akio's logical pool, never on an I/O
//! thread.
//!
//! Where akit differs from fit's handler, and the akio API requests that close the gaps (A147, a
//! Unix-domain socket dialed; A148, the address a request came to), `docs/progress.md` says
//! (I75).

mod exchange;
mod params;
mod pool;
mod record;

use std::borrow::Cow;
use std::sync::Arc;

use akio_http::{BodyEvent, BodyMode, Context, Method, Request};
use akit_config::{FastCgi, Scan};
use akit_runtime::{
    CANONICAL_PATH, Handler, HandlerParams, MAX_BODY_BYTES, Provision, ProvisionError, Provisioned,
    TLS,
};

use exchange::{Exchange, Failure, Parts, error_response};
use params::Params;
use pool::{Limits, Pool};

/// The largest request body taken whole: a larger one, or one of no declared length, streams to
/// the application as it arrives (fit's listener `StreamThreshold`, as the proxy's).
const STREAM_THRESHOLD: u64 = 64 << 10;

/// What a handler's exchanges share: its config and its connections.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) cfg: FastCgi,
    pub(crate) pool: Arc<Pool>,
}

/// The handler.
struct FastCgiHandler {
    h: Arc<Shared>,
}

impl Handler for FastCgiHandler {}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::FastCgi(cfg) = p.params() else {
        return Err(ProvisionError::new(
            "the fastcgi handler's params are not fastcgi's",
        ));
    };
    let pool = Pool::new(
        Limits {
            address: cfg.address.clone(),
            connect_timeout: cfg.connect_timeout,
            idle_timeout: cfg.idle_timeout,
            max_conns: cfg.max_conns,
            max_idle: cfg.max_idle_conns,
            max_pending: cfg.max_pending,
        },
        p.env().engine().clone(),
    );
    let closing = pool.clone();
    Ok(Provisioned::new(Arc::new(FastCgiHandler {
        h: Arc::new(Shared {
            cfg: cfg.clone(),
            pool,
        }),
    }) as Arc<dyn Handler>)
    // Run once the last request of the config is done: the idle connections closed.
    .on_cleanup(move || closing.close()))
}

/// The body length `req` declares (`Content-Length`), if any.
fn declared_length(req: &Request) -> Option<u64> {
    req.headers
        .get("content-length")
        .and_then(|v| v.trim().parse().ok())
}

/// `target` in origin form: an absolute-form one without its scheme and authority.
fn origin_form(target: &str) -> Cow<'_, str> {
    if target.starts_with('/') {
        return Cow::Borrowed(target);
    }
    let Some((_, rest)) = target.split_once("://") else {
        return Cow::Borrowed(target);
    };
    match rest.find(['/', '?']).and_then(|i| rest.get(i..)) {
        Some(path) if path.starts_with('/') => Cow::Borrowed(path),
        Some(query) => Cow::Owned(format!("/{query}")),
        None => Cow::Borrowed("/"),
    }
}

/// `s` with its `%XX` escapes decoded; `None` for a malformed escape or what is not UTF-8.
fn unescape(s: &str) -> Option<Cow<'_, str>> {
    if !s.contains('%') {
        return Some(Cow::Borrowed(s));
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%' {
            let hex = |j: usize| b.get(j).and_then(|h| (*h as char).to_digit(16));
            let (hi, lo) = (hex(i.saturating_add(1))?, hex(i.saturating_add(2))?);
            out.push(u8::try_from(hi.saturating_mul(16).saturating_add(lo)).ok()?);
            i = i.saturating_add(3);
        } else {
            out.push(c);
            i = i.saturating_add(1);
        }
    }
    String::from_utf8(out).ok().map(Cow::Owned)
}

/// The script of the first of `try_files` whose script is a file under the root and that
/// [`FastCgi::check`] lets run, and its path info; or the status to answer: 400 for a candidate
/// with a `..` segment, a backslash or a NUL, which only the request's path can bring; 404 when
/// none is found. Dotfiles under `deny` are passed over. It looks at the disk: it runs on akio's
/// logical pool.
fn try_files(cfg: &FastCgi, req_path: &str) -> Result<(String, String), u16> {
    for t in &cfg.try_files {
        let mut candidate = params::clean(&format!("/{}", t.replace("{path}", req_path)));
        if t.ends_with('/') || (req_path.ends_with('/') && t.ends_with("{path}")) {
            candidate.push('/');
        }
        let (script, info) = cfg.split(&candidate);
        if akit_config::scan(&script) == Scan::Unsafe {
            return Err(400);
        }
        if cfg.check(&script) != 0 {
            continue;
        }
        let file = params::join(&cfg.root, &params::clean(&script));
        if std::fs::metadata(&file).is_ok_and(|m| m.is_file()) {
            return Ok((script.into_owned(), info.to_owned()));
        }
    }
    Err(404)
}

/// The client's streamed body, as it arrives.
struct Sink {
    x: Arc<Exchange>,
    /// The body ended: dropped after that, the sink is done, not cut off.
    ended: bool,
}

impl Sink {
    fn event(&mut self, ev: BodyEvent<'_>) {
        match ev {
            BodyEvent::Chunk(data) => self.x.body_chunk(data),
            BodyEvent::End(_) => {
                self.ended = true;
                self.x.body_end();
            }
        }
    }
}

impl Drop for Sink {
    /// Dropped before the body ended, the sink is the client gone mid-upload (akio drops a
    /// request's body sink with its connection or stream).
    fn drop(&mut self) {
        if !self.ended {
            self.x.fail(Failure::ClientGone);
        }
    }
}

impl akio_http::Service for FastCgiHandler {
    fn call(&self, ctx: &mut Context) {
        let h = &self.h;
        let req = ctx.req();
        let escaped = ctx.local(CANONICAL_PATH).unwrap_or_else(|| req.path());
        let Some(path) = unescape(escaped) else {
            ctx.respond(error_response(400));
            return;
        };
        let path = path.into_owned();
        // Refused before anything else when there is no try_files to look at the disk with.
        let script = if h.cfg.try_files.is_empty() {
            let (script, info) = h.cfg.split(&path);
            match h.cfg.check(&script) {
                0 => Some((script.into_owned(), info.to_owned())),
                status => {
                    ctx.respond(error_response(status));
                    return;
                }
            }
        } else {
            None
        };
        let length = declared_length(req);
        let reads = ctx.body_reads();
        let req = ctx.req();
        let body = reads.is_none().then(|| req.body.clone());
        let length = match (&body, length) {
            (Some(b), None) if !b.is_empty() => Some(b.len() as u64),
            (_, l) => l,
        };
        let target = origin_form(&req.target);
        let fields = akit_runtime::request_headers(ctx);
        let params = Params::new(
            &params::Request {
                method: &req.method,
                target: &target,
                path: &path,
                version: req.version,
                headers: &fields,
                peer: req.peer,
                tls: ctx.local(TLS).is_some(),
                length,
            },
            &h.cfg.root,
            &h.cfg.env,
        );
        let replayable = match req.method {
            Method::Head => true,
            Method::Get | Method::Options | Method::Trace => {
                reads.is_none() && length.unwrap_or(0) == 0
            }
            _ => false,
        };
        let limit = ctx
            .local(MAX_BODY_BYTES)
            .and_then(|v| v.parse().ok())
            .unwrap_or(akit_config::DEFAULT_MAX_BODY_BYTES);
        let version = req.version;
        let streamed = reads.is_some();
        // The pool try_files looks at the disk on, taken before the request is detached.
        let exec = if script.is_none() {
            match ctx.executor() {
                Some(e) => Some(e),
                None => {
                    ctx.respond(error_response(503));
                    return;
                }
            }
        } else {
            None
        };
        let x = Exchange::new(Parts {
            h: h.clone(),
            params,
            reads,
            replayable,
            version,
            limit,
            body,
        });
        // The client gone, whenever: the exchange ends, its connection closed. Weak: akio keeps
        // the callback with the answer, which the exchange holds.
        let weak = Arc::downgrade(&x);
        ctx.on_gone(move || {
            if let Some(x) = weak.upgrade() {
                x.fail(Failure::ClientGone);
            }
        });
        if streamed {
            let mut sink = Sink {
                x: x.clone(),
                ended: false,
            };
            ctx.on_body(move |_, ev| sink.event(ev));
        }
        let Some(answer) = ctx.detach() else { return };
        if !x.begin(answer) {
            return;
        }
        match (script, exec) {
            (Some((script, info)), _) => x.start(script, info),
            (None, Some(exec)) => {
                let job = x.clone();
                let looked = exec.execute(move || match try_files(&job.h.cfg, &path) {
                    Ok((script, info)) => job.start(script, info),
                    Err(status) => job.fail(Failure::Status(status)),
                });
                if looked.is_err() {
                    x.fail(Failure::Status(503));
                }
            }
            (None, None) => x.fail(Failure::Status(503)),
        }
    }

    /// A body of more than [`STREAM_THRESHOLD`], or of no declared length, streams.
    fn body_mode(&self, req: &Request) -> BodyMode {
        match declared_length(req) {
            Some(n) if n <= STREAM_THRESHOLD => BodyMode::Buffered,
            _ => BodyMode::Stream,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    #[test]
    fn unescapes() {
        assert_eq!(super::unescape("/a%20b").unwrap(), "/a b");
        assert_eq!(super::unescape("/a").unwrap(), "/a");
        assert!(super::unescape("/a%2").is_none());
        assert!(super::unescape("/a%zz").is_none());
        assert!(super::unescape("/%ff").is_none());
        assert_eq!(super::unescape("/%2e%2E").unwrap(), "/..");
    }
}
