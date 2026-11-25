//! The `compress` middleware, as fit's `middleware/compress`: responses compressed with zstd,
//! brotli or gzip, as the request's `Accept-Encoding` allows, as they are written. A response
//! that streams (a proxied one) stays streamed: akio encodes each write of its body and flushes
//! it, so what the upstream sent reaches the client at once (akio's `Response::set_stream_coding`,
//! ADR-112); a response whose body is whole (a `respond` handler's, a small file's) is compressed
//! whole.
//!
//! The choice is fit's: the encoding the client prefers most by q value among those offered,
//! and among equals the first offered; a response is compressed when its status carries a body
//! and is not 206, it has no `Content-Encoding` (but `identity`) or `Content-Range`, its
//! `Cache-Control` does not say `no-transform`, its type compresses, and its declared length,
//! if any, is at least `min_bytes`. Such a response says so in `Vary`, loses its
//! `Content-Length`, and a strong `ETag` it carries is made weak. A HEAD answer gets the `Vary`
//! and nothing else.
//!
//! At most `max_in_flight` responses are compressed at once: past it they go uncompressed, so
//! that the encoders' memory is bounded. A streamed response holds its place until its stream
//! ends, or its client goes (akio's `Response::keep_until_sent`); a whole one gives it back once
//! its body is compressed.
//!
//! A body akio sends from a file on disk (a static file larger than `max_file_bytes`) is given
//! the coding as a streamed one is: akio reads it in pieces on its file pool and encodes them
//! there, under the connection's backpressure (akio ADR-124), and it holds its place until it is
//! sent.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use akio_http::{BodyMode, Coding, Context, Headers as Fields, Method, Request, Response};
use akit_config::Encoding;
use akit_runtime::{Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned};

/// The window of the streaming encoders, as fit's: `2^18` bytes, so that each stream under way
/// costs a few hundred kilobytes, not megabytes.
const WINDOW_BITS: u32 = 18;

/// The middleware, compiled.
#[derive(Debug)]
pub(crate) struct Compress {
    shared: Arc<Shared>,
}

/// What every response of the middleware's routes reads, and the count of those under way.
#[derive(Debug)]
struct Shared {
    encodings: Vec<Encoding>,
    gzip_level: u32,
    brotli_level: u32,
    zstd_level: u32,
    min_bytes: u64,
    max_in_flight: u64,
    /// Responses being compressed. One counter for the middleware, touched only by the
    /// responses it compresses: the bound is on them all.
    in_flight: AtomicU64,
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Compress(c) = p.params() else {
        return Err(ProvisionError::new(
            "the compress middleware was given another's params",
        ));
    };
    Ok(Provisioned::new(Arc::new(Compress {
        shared: Arc::new(Shared {
            encodings: c.encodings.clone(),
            gzip_level: c.gzip_level,
            brotli_level: c.brotli_level,
            zstd_level: c.zstd_level,
            min_bytes: c.min_bytes,
            max_in_flight: c.max_in_flight,
            in_flight: AtomicU64::new(0),
        }),
    }) as Arc<dyn Middleware>))
}

impl Middleware for Compress {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Compressed {
            shared: Arc::clone(&self.shared),
            next,
        })
    }
}

/// A handler behind the middleware.
struct Compressed {
    shared: Arc<Shared>,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Compressed {
    fn call(&self, ctx: &mut Context) {
        let shared = Arc::clone(&self.shared);
        ctx.add_response_filter(move |resp: &mut Response, req: &Request| {
            shared.filter(resp, req);
        });
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Compressed {}

/// A response's place among those compressed at once, given back when dropped.
struct Place(Arc<Shared>);

impl Drop for Place {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Shared {
    /// Compresses `resp`, the answer to `req`, when it may be and the client takes it.
    fn filter(self: &Arc<Self>, resp: &mut Response, req: &Request) {
        if !self.eligible(resp) {
            return;
        }
        add_vary(&mut resp.headers);
        if req.method == Method::Head {
            return;
        }
        let Some(encoding) = negotiate(&req.headers, &self.encodings) else {
            return;
        };
        if self.in_flight.fetch_add(1, Ordering::AcqRel) >= self.max_in_flight {
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        let place = Place(Arc::clone(self));
        if resp.is_streaming() || resp.file.is_some() {
            // A body from disk is read and encoded on akio's file pool (akio ADR-124).
            resp.set_stream_coding(Some(self.coding(encoding)));
            resp.keep_until_sent(place);
        } else {
            resp.body = self.compress(encoding, &resp.body);
            drop(place);
        }
        resp.headers.set("Content-Encoding", encoding.name());
        resp.headers.remove("Content-Length");
        if let Some(tag) = resp.headers.get("etag") {
            if !tag.starts_with("W/") {
                let weak = format!("W/{tag}");
                resp.headers.set("ETag", weak);
            }
        }
    }

    /// Whether `resp` may be compressed, as fit's `eligible` says.
    fn eligible(&self, resp: &Response) -> bool {
        let status = resp.status;
        if status < 200 || matches!(status, 204 | 206 | 304) {
            return false;
        }
        let h = &resp.headers;
        if h.get("content-encoding")
            .is_some_and(|e| !e.trim().eq_ignore_ascii_case("identity") && !e.is_empty())
        {
            return false;
        }
        if h.get("content-range").is_some() {
            return false;
        }
        if h.get_all("cache-control")
            .any(|v| contains_ignoring_case(v, "no-transform"))
        {
            return false;
        }
        // The declared length: the field's, else that of a body known whole.
        let declared = match h.get("content-length") {
            Some(v) => v.trim().parse::<u64>().ok(),
            None if resp.is_streaming() => None,
            None => Some(resp.body_len()),
        };
        if declared.is_some_and(|n| n < self.min_bytes) {
            return false;
        }
        h.get("content-type").is_some_and(compressible)
    }

    /// The coding a streamed body of `encoding` is written with: fit's levels and window.
    fn coding(&self, encoding: Encoding) -> Coding {
        match encoding {
            Encoding::Gzip => Coding::gzip(self.gzip_level),
            Encoding::Br => Coding::br(self.brotli_level, WINDOW_BITS),
            Encoding::Zstd => Coding::zstd(self.zstd_level, WINDOW_BITS),
        }
    }

    /// `body` compressed whole with `encoding`.
    fn compress(&self, encoding: Encoding, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity((body.len() / 4).saturating_add(64));
        match encoding {
            Encoding::Gzip => {
                out = akio_deflate::compress(
                    akio_deflate::Format::Gzip,
                    akio_deflate::Level::new(self.gzip_level),
                    body,
                );
            }
            Encoding::Br => {
                akio_brotli::Compressor::new(akio_brotli::Level::new(self.brotli_level))
                    .window_bits(WINDOW_BITS)
                    .compress(body, &mut out, akio_brotli::Flush::Finish);
            }
            Encoding::Zstd => {
                akio_zstd::Compressor::new(akio_zstd::Level::new(self.zstd_level))
                    .window_log(WINDOW_BITS)
                    .compress(body, &mut out, akio_zstd::Flush::Finish);
            }
        }
        out
    }
}

/// Whether `haystack` holds `needle` (lower case), ignoring ASCII case.
fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Adds `Accept-Encoding` to `h`'s `Vary`, unless it is there or `Vary` is `*`.
fn add_vary(h: &mut Fields) {
    let covered = h.get_all("vary").any(|v| {
        v.split(',').any(|name| {
            let name = name.trim();
            name == "*" || name.eq_ignore_ascii_case("accept-encoding")
        })
    });
    if !covered {
        h.append("Vary", "Accept-Encoding");
    }
}

/// The encoding of `offered` the request's `Accept-Encoding` fields prefer most, the first of
/// `offered` among those they prefer as much, or `None` when they accept none of them, as fit's
/// `negotiate`: names compare ignoring case, `x-gzip` is gzip, `*` stands for the encodings not
/// named, and a q value that does not parse counts as 1.
fn negotiate(h: &Fields, offered: &[Encoding]) -> Option<Encoding> {
    // q values of br, zstd and gzip as named, and of `*`.
    let mut q = [None::<f64>; 3];
    let mut any = None::<f64>;
    let slot = |e: Encoding| match e {
        Encoding::Br => 0,
        Encoding::Zstd => 1,
        Encoding::Gzip => 2,
    };
    for field in h.get_all("accept-encoding") {
        for member in field.split(',') {
            let mut parts = member.split(';');
            let name = parts.next().unwrap_or("").trim();
            let mut weight = 1.0;
            for param in parts {
                if let Some((k, v)) = param.split_once('=') {
                    if k.trim().eq_ignore_ascii_case("q") {
                        if let Ok(v) = v.trim().parse::<f64>() {
                            weight = v;
                        }
                    }
                }
            }
            let named = if name == "*" {
                any = Some(weight);
                continue;
            } else if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
                Encoding::Gzip
            } else if name.eq_ignore_ascii_case("br") {
                Encoding::Br
            } else if name.eq_ignore_ascii_case("zstd") {
                Encoding::Zstd
            } else {
                continue;
            };
            if let Some(s) = q.get_mut(slot(named)) {
                *s = Some(weight);
            }
        }
    }
    let mut best: Option<(Encoding, f64)> = None;
    for &e in offered {
        let w = q.get(slot(e)).copied().flatten().or(any).unwrap_or(-1.0);
        if w > best.map_or(0.0, |(_, b)| b) {
            best = Some((e, w));
        }
    }
    best.map(|(e, _)| e)
}

/// Whether a body of `content_type` compresses, as fib's `compress.Compressible` says: text,
/// JSON, JavaScript, XML, SVG, WebAssembly, uncompressed fonts and icons; not images, video or
/// archives, which are compressed already.
fn compressible(content_type: &str) -> bool {
    let t = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .as_bytes();
    let starts = |p: &[u8]| t.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
    let ends = |p: &[u8]| {
        t.len()
            .checked_sub(p.len())
            .and_then(|i| t.get(i..))
            .is_some_and(|h| h.eq_ignore_ascii_case(p))
    };
    starts(b"text/")
        || ends(b"+json")
        || ends(b"+xml")
        || COMPRESSIBLE
            .iter()
            .any(|m| t.eq_ignore_ascii_case(m.as_bytes()))
}

/// fib's other compressible types, besides `text/*`, `*+json` and `*+xml`.
const COMPRESSIBLE: &[&str] = &[
    "application/json",
    "application/javascript",
    "application/x-javascript",
    "application/ecmascript",
    "application/xml",
    "application/wasm",
    "application/x-ndjson",
    "application/vnd.ms-fontobject",
    "application/x-font-ttf",
    "font/ttf",
    "font/otf",
    "image/svg+xml",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/bmp",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(v: &str) -> Fields {
        let mut h = Fields::new();
        h.append("Accept-Encoding", v);
        h
    }

    // fit's TestNegotiate.
    #[test]
    fn negotiate_as_fit() {
        use Encoding::{Br, Gzip, Zstd};
        let offered = [Zstd, Br, Gzip];
        for (a, want) in [
            ("", None),
            ("gzip", Some(Gzip)),
            ("GZIP", Some(Gzip)),
            ("deflate", None),
            ("br;q=0.9, gzip;q=0.8", Some(Br)),
            ("br; q=0.1, *", Some(Zstd)),
            ("zstd;q=0, br;q=0, *;q=0.3", Some(Gzip)),
            ("gzip;q=abc", Some(Gzip)),
            ("x-gzip", Some(Gzip)),
            ("identity", None),
        ] {
            assert_eq!(negotiate(&accept(a), &offered), want, "{a:?}");
        }
        let mut two = accept("zstd");
        two.append("Accept-Encoding", "gzip");
        assert_eq!(
            negotiate(&two, &[Gzip, Zstd]),
            Some(Gzip),
            "the server's order"
        );
        assert_eq!(negotiate(&Fields::new(), &offered), None);
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            encodings: vec![Encoding::Zstd, Encoding::Gzip, Encoding::Br],
            gzip_level: 1,
            brotli_level: 1,
            zstd_level: 1,
            min_bytes: 1400,
            max_in_flight: 1024,
            in_flight: AtomicU64::new(0),
        })
    }

    // fit's TestEligibleAndVary.
    #[test]
    fn eligible_and_vary() {
        let s = shared();
        let text = |k: &str, v: &str| {
            let mut r = Response::with_body(200, "text/plain", vec![b'x'; 2000]);
            r.headers.set(k, v);
            r
        };
        assert!(!s.eligible(&text("Content-Range", "bytes 0-1/2")));
        assert!(!s.eligible(&text("Cache-Control", "public, No-Transform")));
        assert!(s.eligible(&text("X", "y")));
        assert!(s.eligible(&text("Content-Encoding", "identity")));
        assert!(!s.eligible(&text("Content-Encoding", "gzip")));
        assert!(!s.eligible(&text("Content-Length", "1399")));
        assert!(!s.eligible(&text("Content-Type", "image/png")));
        assert!(!s.eligible(&text("Content-Type", "")));
        let mut small = text("X", "y");
        small.body.truncate(10);
        assert!(!s.eligible(&small), "a whole body is its declared length");
        for status in [101, 204, 206, 304] {
            let mut r = text("X", "y");
            r.status = status;
            assert!(!s.eligible(&r), "{status}");
        }
        for vary in ["*", "Origin, accept-encoding"] {
            let mut h = Fields::new();
            h.append("Vary", vary);
            add_vary(&mut h);
            assert_eq!(h.get_all("vary").count(), 1, "{vary}");
        }
        let mut h = Fields::new();
        h.append("Vary", "Origin");
        add_vary(&mut h);
        assert_eq!(h.get_all("vary").count(), 2);
    }

    #[test]
    fn compressible_types() {
        for t in [
            "text/html; charset=utf-8",
            "Application/JSON",
            "application/ld+json",
            "image/svg+xml",
            "font/otf",
        ] {
            assert!(compressible(t), "{t}");
        }
        for t in [
            "image/png",
            "application/zip",
            "video/mp4",
            "application/octet-stream",
        ] {
            assert!(!compressible(t), "{t}");
        }
    }

    #[test]
    fn whole_bodies_decode() {
        let s = shared();
        let body = "a record, a record, a record\n".repeat(200);
        for e in [Encoding::Gzip, Encoding::Br, Encoding::Zstd] {
            let out = s.compress(e, body.as_bytes());
            assert!(out.len() < body.len() / 10, "{e:?}");
            let back = match e {
                Encoding::Gzip => {
                    akio_deflate::decompress(akio_deflate::Format::Gzip, &out, 1 << 20).ok()
                }
                Encoding::Br => akio_brotli::decompress(&out, 1 << 20).ok(),
                Encoding::Zstd => akio_zstd::decompress(&out, 1 << 20).ok(),
            };
            assert_eq!(back.as_deref(), Some(body.as_bytes()), "{e:?}");
        }
    }
}
