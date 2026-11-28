//! The `vod` handler (I142, fit's `handler/vod`): a media library's items as video on demand
//! over HLS, cut from the files as they are stored, without transcoding, and the files
//! themselves with ranges.
//!
//! An item's URLs are its ID's path after the handler's prefix, as
//! `/vod/films/films/bbb/index.m3u8`: a multivariant playlist of its renditions, with its audio
//! files and subtitles as groups; a media playlist of each, of fMP4 segments or MPEG-TS for older
//! players; the segments, cut at keyframes by akio-media's `mp4::Plan` near `segment_duration`;
//! the subtitles as WebVTT; and the files themselves, served by akio-http's `Files` with ranges
//! and conditional requests. [`names`] lists them.
//!
//! Nothing here blocks an I/O thread: a plan is made from the parsed movie the library reads on
//! its file pool, a segment's samples are read and muxed there too, and a request that waits for
//! either is suspended and answered from the pool, or 503 past `timeout`. What is built is kept
//! in a cache bounded by `max_cache_bytes`, shared by the handlers of the same params across
//! reloads; it is built once for every request that waits for it. A file found changed or gone
//! when it is read is answered 404 and what the cache keeps of it dropped; a new catalog drops
//! what it no longer lists.
//!
//! The segments' clock puts a file's time 0 at 1.4 s, as fit's, in both containers, so that one
//! `X-TIMESTAMP-MAP` places the subtitles.

mod cache;
pub(crate) mod names;
pub(crate) mod playlist;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use akio_http::files::Files;
use akio_http::{Context, Method, Response, Service, Suspended};
use akio_media::av::{Time, Track};
use akio_media::mp4::{Movie, Plan};
use akio_media::{fmp4, mpegts};

use akit_config::medialib::Container;
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

use crate::http::{Access, HandlerAuth, redact};
use crate::library::{self, Error, Item, Library, Rendition, Subtitle};
use cache::{Begin, Cache, FileKey, Key, What};
use names::{C, Kind, Name, parse_name};

pub use cache::CacheStats;

/// The gap of bytes between two samples a segment's read takes rather than reading twice.
const READ_GAP: u64 = 64 << 10;

/// What is kept: a blob to answer with, or a file's plan.
#[derive(Clone)]
enum Val {
    Blob(Arc<Blob>),
    Plan(Arc<Plan>),
}

/// An answer built: its bytes and header fields.
#[derive(Debug)]
struct Blob {
    data: Vec<u8>,
    ctype: &'static str,
    etag: String,
    modified: SystemTime,
}

/// What the handlers of the same params share: their cache, kept across reloads.
struct Store {
    key: StoreKey,
    refs: Mutex<usize>,
    cache: Cache<Val>,
    /// Tells the store's segments apart in their ETags: those cut by one `segment_duration`
    /// differ from another's.
    tag: String,
    /// akio-http's `Files` of each root, which serves the files themselves.
    files: Mutex<HashMap<PathBuf, Arc<Files>>>,
    /// The catalog last seen, by address, so that a new one drops what it no longer lists.
    seen: Mutex<usize>,
    /// The requests its handlers took, and the bytes of the bodies they answered (fit's
    /// `requests` and `sent`), for the Admin API's `/media/stats`.
    requests: AtomicU64,
    sent: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StoreKey {
    engine: usize,
    library: String,
    seg_dur: Duration,
    seg_bytes: u64,
    cache_bytes: u64,
    cache_control: String,
    cors: String,
    /// `encrypt` (I147): the store's segments are encrypted.
    encrypt: bool,
}

type Stores = Mutex<HashMap<StoreKey, Arc<Store>>>;

fn stores() -> &'static Stores {
    static S: std::sync::LazyLock<Stores> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    &S
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn take_store(k: StoreKey) -> Arc<Store> {
    let mut m = lock(stores());
    let st = Arc::clone(m.entry(k.clone()).or_insert_with(|| {
        let tag = library::catalog::hex(
            akio_crypto::hash::Sha256::digest(
                format!(
                    "1 {} {}{}",
                    k.seg_dur.as_nanos(),
                    k.seg_bytes,
                    if k.encrypt { " aes-128" } else { "" }
                )
                .as_bytes(),
            )
            .get(..4)
            .unwrap_or_default(),
        );
        Arc::new(Store {
            cache: Cache::new(k.cache_bytes),
            key: k,
            refs: Mutex::new(0),
            tag,
            files: Mutex::new(HashMap::new()),
            seen: Mutex::new(0),
            requests: AtomicU64::new(0),
            sent: AtomicU64::new(0),
        })
    }));
    let mut refs = lock(&st.refs);
    *refs = refs.saturating_add(1);
    drop(refs);
    st
}

fn give_store(st: &Store) {
    let mut m = lock(stores());
    let mut refs = lock(&st.refs);
    *refs = refs.saturating_sub(1);
    if *refs == 0 {
        m.remove(&st.key);
    }
}

/// What the `vod` handlers served, one object a library, sorted by its name (fit's `vod.Stats`,
/// as `GET /media/stats` writes it): the requests, the bytes of the bodies answered, and what
/// their caches hold and found.
#[must_use]
pub fn stats_value() -> akit_config::value::Value {
    use akit_config::value::Value;
    let m = lock(stores());
    let mut libs: Vec<String> = m.values().map(|st| st.key.library.clone()).collect();
    libs.sort();
    libs.dedup();
    let num = |v: u64| Value::Number(v.to_string());
    let out = libs
        .iter()
        .map(|lib| {
            let (mut req, mut sent) = (0u64, 0u64);
            let mut c = CacheStats::default();
            for st in m.values().filter(|st| st.key.library == *lib) {
                req = req.saturating_add(st.requests.load(Relaxed));
                sent = sent.saturating_add(st.sent.load(Relaxed));
                let s = st.cache.stats();
                c.entries = c.entries.saturating_add(s.entries);
                c.bytes = c.bytes.saturating_add(s.bytes);
                c.max = c.max.saturating_add(s.max);
                c.hits = c.hits.saturating_add(s.hits);
                c.misses = c.misses.saturating_add(s.misses);
            }
            let o = |m: Vec<(&str, Value)>| {
                Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
            };
            o(vec![
                ("library", Value::String(lib.clone())),
                ("requests", num(req)),
                ("bytes", num(sent)),
                (
                    "cache",
                    o(vec![
                        ("entries", num(c.entries as u64)),
                        ("bytes", num(c.bytes)),
                        ("max", num(c.max)),
                        ("hits", num(c.hits)),
                        ("misses", num(c.misses)),
                    ]),
                ),
            ])
        })
        .collect();
    Value::Array(out)
}

/// What the caches of the `vod` handlers serving `library` hold, summed.
#[must_use]
pub fn cache_stats(library: &str) -> CacheStats {
    let m = lock(stores());
    let mut out = CacheStats::default();
    for st in m.values().filter(|st| st.key.library == library) {
        let s = st.cache.stats();
        out.entries = out.entries.saturating_add(s.entries);
        out.bytes = out.bytes.saturating_add(s.bytes);
        out.max = out.max.saturating_add(s.max);
        out.hits = out.hits.saturating_add(s.hits);
        out.misses = out.misses.saturating_add(s.misses);
    }
    out
}

/// The handler.
struct Vod {
    library: String,
    prefix: String,
    container: C,
    cors: String,
    timeout: Duration,
    encrypt: bool,
    auth: HandlerAuth,
    found: Arc<OnceLock<(usize, Arc<Store>)>>,
}

/// The handler module, for the registry.
pub(crate) fn new_vod(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Vod(v) = p.params() else {
        return Err(ProvisionError::new(
            "the vod handler's params are not vod's",
        ));
    };
    let auth = HandlerAuth::new(p.env().engine(), &v.auth, v.policy.as_ref());
    let found = Arc::new(OnceLock::new());
    let f = Arc::clone(&found);
    let key = StoreKey {
        engine: 0,
        library: v.library.clone(),
        seg_dur: v.segment(),
        seg_bytes: v.segment_bytes(),
        cache_bytes: v.cache_bytes(),
        cache_control: v.cache_control.clone(),
        cors: v.cors.clone(),
        encrypt: v.encrypt,
    };
    p.env().engine().when_set(move |engine| {
        let engine = library::key(engine);
        let st = take_store(StoreKey { engine, ..key });
        let _ = f.set((engine, st));
    });
    let cleanup = Arc::clone(&found);
    Ok(Provisioned::new(Arc::new(Vod {
        library: v.library.clone(),
        prefix: v.prefix_or(),
        container: match v.container {
            Container::Fmp4 => C::Fmp4,
            Container::Ts => C::Ts,
        },
        cors: v.cors.clone(),
        timeout: v.wait(),
        encrypt: v.encrypt,
        auth,
        found,
    }) as Arc<dyn Handler>)
    .on_cleanup(move || {
        if let Some((_, st)) = cleanup.get() {
            give_store(st);
        }
    }))
}

impl Handler for Vod {}

/// A request suspended while what it needs is built: the work and its deadline race to answer
/// it, the first wins.
#[derive(Clone)]
struct Waiter(Arc<Mutex<Option<(Suspended, crate::deadline::Deadline)>>>);

impl Waiter {
    /// Suspends `ctx` until the waiter answers it, or 503 after `timeout`.
    fn new(ctx: &mut Context, timeout: Duration, cors: &str) -> Option<Waiter> {
        let s = ctx.suspend()?;
        let slot: Arc<Mutex<Option<(Suspended, crate::deadline::Deadline)>>> =
            Arc::new(Mutex::new(None));
        let late = Arc::clone(&slot);
        let cors = cors.to_owned();
        let d = crate::deadline::after(timeout, move || {
            if let Some((s, _)) = lock(&late).take() {
                s.resume(|ctx| {
                    text(
                        ctx,
                        503,
                        "timed out waiting for the media library",
                        &cors,
                        &[("Retry-After", "1")],
                    );
                });
            }
        });
        *lock(&slot) = Some((s, d));
        Some(Waiter(slot))
    }

    /// Answers the request with `f`, unless it was answered already.
    fn answer(&self, f: impl FnOnce(&mut Context)) {
        let Some((s, d)) = lock(&self.0).take() else {
            return;
        };
        crate::deadline::cancel(d);
        s.resume(f);
    }
}

/// Answers with a line of text and `extra` header fields.
fn text(ctx: &mut Context, status: u16, msg: &str, cors: &str, extra: &[(&str, &str)]) {
    redact(ctx);
    let mut r = Response::with_body(
        status,
        "text/plain; charset=utf-8",
        format!("{msg}\n").into_bytes(),
    );
    if !cors.is_empty() {
        r = r.header("Access-Control-Allow-Origin", cors);
    }
    for (k, v) in extra {
        r = r.header(k, v);
    }
    ctx.respond(r);
}

/// The HTTP date of `t`, as `Last-Modified` writes it.
// The civil-date arithmetic stays in range: every term is at most the days since 1970.
#[allow(clippy::arithmetic_side_effects)]
fn http_date(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days.saturating_add(719_468);
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(mo <= 2);
    const WD: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let wd = WD
        .get(usize::try_from(days % 7).unwrap_or(0))
        .unwrap_or(&"Thu");
    let mon = MO
        .get(usize::try_from(mo.saturating_sub(1)).unwrap_or(0))
        .unwrap_or(&"Jan");
    format!("{wd}, {d:02} {mon} {y} {h:02}:{m:02}:{sec:02} GMT")
}

/// The entity tag of what is made of `f`: its time and size, and `what`.
fn etag(f: &library::File, what: &str) -> String {
    let nanos = f
        .modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    if what.is_empty() {
        format!("\"{nanos:x}-{:x}\"", f.size)
    } else {
        format!("\"{nanos:x}-{:x}-{what}\"", f.size)
    }
}

/// The error of work that failed: 404 for a file not as the catalog has it, whose cache entries
/// are dropped, or one not served; 503 for a busy pool; 500 otherwise.
fn fail(ctx: &mut Context, st: &Store, f: Option<&library::File>, e: &Error) {
    let cors = &st.key.cors;
    match e {
        Error::Changed => {
            if let Some(f) = f {
                st.cache.drop_file(&FileKey::of(f));
            }
            text(
                ctx,
                404,
                "the file changed since the library was scanned",
                cors,
                &[],
            );
        }
        Error::Removed | Error::NotServed | Error::Outside => {
            text(ctx, 404, "the file is not served", cors, &[]);
        }
        Error::Busy | Error::TooLarge => text(
            ctx,
            503,
            "the media library is busy",
            cors,
            &[("Retry-After", "1")],
        ),
        Error::Malformed(m) | Error::Io(m) => text(ctx, 500, m, cors, &[]),
    }
}

impl Store {
    /// Sends `b`, or 304 to a request whose `If-None-Match` names it, with `own`'s header.
    fn send(&self, ctx: &mut Context, b: &Blob, own: &Own) {
        let matches = ctx
            .header("If-None-Match")
            .is_some_and(|v| v.split(',').any(|t| t.trim() == b.etag || t.trim() == "*"));
        let mut r = if matches {
            Response::new(304)
        } else {
            Response::with_body(200, b.ctype, b.data.clone())
        };
        if !b.etag.is_empty() {
            r = r.header("ETag", &b.etag);
        }
        r = r.header("Last-Modified", http_date(b.modified));
        if !own.cookie.is_empty() {
            r = r.header("Set-Cookie", &own.cookie);
        }
        if own.private {
            r = r.header("Cache-Control", "private, no-store");
        } else if !self.key.cache_control.is_empty() {
            r = r.header("Cache-Control", &self.key.cache_control);
        }
        if !self.key.cors.is_empty() {
            r = r.header("Access-Control-Allow-Origin", &self.key.cors);
        }
        self.sent
            .fetch_add(u64::try_from(r.body.len()).unwrap_or(0), Relaxed);
        ctx.respond(r);
    }

    /// Drops what the cache keeps of files `cat` does not list as they were, on `l`'s pool,
    /// when `cat` is not the catalog last seen.
    fn watch(self: &Arc<Self>, l: &Library, cat: &Arc<library::Catalog>) {
        let at = Arc::as_ptr(cat).addr();
        {
            let mut seen = lock(&self.seen);
            if *seen == at {
                return;
            }
            *seen = at;
        }
        let me = Arc::clone(self);
        let cat = Arc::clone(cat);
        l.submit(
            Box::new(move || {
                let keep: HashSet<FileKey> = cat
                    .items
                    .iter()
                    .flat_map(|it| it.files())
                    .map(FileKey::of)
                    .collect();
                me.cache.keep(&keep);
            }),
            None,
        );
    }

    /// `akio-http`'s `Files` of `root`.
    fn files(&self, root: &std::path::Path) -> Arc<Files> {
        let mut m = lock(&self.files);
        Arc::clone(m.entry(root.to_path_buf()).or_insert_with(|| {
            let f = Files::new(root.to_path_buf());
            Arc::new(if self.key.cache_control.is_empty() {
                f
            } else {
                f.cache_control(&self.key.cache_control)
            })
        }))
    }
}

impl Service for Vod {
    fn call(&self, ctx: &mut Context) {
        // Whatever the answer, the access log holds no token: `text` and the guard write it
        // REDACTED.
        if !matches!(ctx.method(), Method::Get | Method::Head) {
            return text(
                ctx,
                405,
                "only GET and HEAD",
                &self.cors,
                &[("Allow", "GET, HEAD")],
            );
        }
        let path = ctx.path().to_owned();
        let Some((id, name)) = path
            .strip_prefix(self.prefix.as_str())
            .and_then(|rest| rest.rsplit_once('/'))
            .filter(|(id, _)| !id.is_empty())
        else {
            return text(
                ctx,
                404,
                "no item in the path: <prefix><id>/index.m3u8",
                &self.cors,
                &[],
            );
        };
        let Some(nm) = parse_name(name).filter(|_| library::catalog::valid_id(id)) else {
            return text(ctx, 404, "not a name an item has", &self.cors, &[]);
        };
        let Some((key, st)) = self.found.get().cloned() else {
            return text(
                ctx,
                503,
                "the media library is not loaded",
                &self.cors,
                &[("Retry-After", "1")],
            );
        };
        st.requests.fetch_add(1, Relaxed);
        let Some(l) = library::lookup(key, &self.library) else {
            return text(
                ctx,
                503,
                "the media library is not loaded",
                &self.cors,
                &[("Retry-After", "1")],
            );
        };
        let cat = l.catalog();
        if cat.scanned.is_none() {
            return text(
                ctx,
                503,
                "the media library's first scan has not ended",
                &self.cors,
                &[("Retry-After", "1")],
            );
        }
        st.watch(&l, &cat);
        let Some(it) = cat.item(id).filter(|it| it.error.is_empty()).cloned() else {
            return text(ctx, 404, "no such item", &self.cors, &[]);
        };
        drop(cat);
        let stream = format!("vod/{}/{id}", self.library);
        // The item's resource, as a signed URL is signed for: the path up to and with the '/'
        // after its ID.
        let resource = format!("{}{id}/", self.prefix);
        let req = Req {
            st,
            l,
            it,
            nm,
            container: self.container,
            timeout: self.timeout,
            cors: self.cors.clone(),
            encrypt: self.encrypt,
        };
        self.auth.guard_resource(
            ctx,
            &stream,
            &resource,
            &self.cors,
            move |ctx, a: Access| {
                req.serve(ctx, &a);
            },
        );
    }
}

/// A request let in, with what it names.
struct Req {
    st: Arc<Store>,
    l: Arc<Library>,
    it: Arc<Item>,
    nm: Name,
    container: C,
    timeout: Duration,
    cors: String,
    encrypt: bool,
}

/// What a response's header says of the viewer: a playlist that names a token or a grant, or
/// sets a grant's cookie, is the viewer's own, which no shared cache may keep (fit's `personal`).
#[derive(Debug, Clone, Default)]
struct Own {
    private: bool,
    /// A `Set-Cookie` field's value, or empty.
    cookie: String,
}

/// The query a playlist of an item `length` long hands the URIs it names, and its header: the
/// request's token, when it came in the query; with a signed URL, the grant it hands on.
fn pass_on(a: &Access, length: Duration) -> (String, Own) {
    let (query, cookie) = a.pass_on(length);
    let private = a.by_url() || !query.is_empty();
    (query, Own { private, cookie })
}

/// The rendition or audio file `(audio, i)` of `it` that may be played in `c`.
fn rendition(it: &Item, audio: bool, i: usize) -> Option<&Rendition> {
    let list = if audio { &it.audio } else { &it.renditions };
    list.get(i).filter(|r| r.error.is_empty() && r.moov_known())
}

/// The file's extension, in lower case.
fn ext_of(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

impl Req {
    fn serve(self, ctx: &mut Context, a: &Access) {
        match self.nm.kind {
            Kind::Master => {
                let (query, own) = pass_on(a, self.it.duration);
                let c = self.nm.c.unwrap_or(self.container);
                match playlist::render_master(&self.it, c, &query) {
                    Some(b) => {
                        let blob = Blob {
                            data: b.into_bytes(),
                            ctype: PLAYLIST,
                            etag: String::new(),
                            modified: self.it.modified,
                        };
                        self.st.send(ctx, &blob, &own);
                    }
                    None => text(
                        ctx,
                        404,
                        &format!("no rendition of the item is carried in {}", c.as_str()),
                        &self.cors,
                        &[],
                    ),
                }
            }
            Kind::Media | Kind::Init | Kind::Segment => {
                let c = self.nm.c.unwrap_or(C::Fmp4);
                let Some(r) = rendition(&self.it, self.nm.audio, self.nm.i).cloned() else {
                    return text(ctx, 404, "no such rendition", &self.cors, &[]);
                };
                if playlist::servable(&r, c).is_none() {
                    return text(
                        ctx,
                        404,
                        &format!("the file has no track {} carries", c.as_str()),
                        &self.cors,
                        &[],
                    );
                }
                let (query, own) = if self.nm.kind == Kind::Media {
                    pass_on(a, r.duration)
                } else {
                    (String::new(), Own::default())
                };
                self.serve_built(ctx, r, c, query, own);
            }
            Kind::Subtitles => {
                let Some(s) = self.subtitle() else {
                    return text(ctx, 404, "no such subtitle", &self.cors, &[]);
                };
                let (query, own) = pass_on(a, self.it.duration);
                let blob = Blob {
                    data: playlist::render_subtitles(self.nm.i, self.it.duration, &query)
                        .into_bytes(),
                    ctype: PLAYLIST,
                    etag: String::new(),
                    modified: s.file.modified,
                };
                self.st.send(ctx, &blob, &own);
            }
            Kind::Vtt => {
                let Some(s) = self.subtitle().cloned() else {
                    return text(ctx, 404, "no such subtitle", &self.cors, &[]);
                };
                self.serve_vtt(ctx, s);
            }
            Kind::Key => self.serve_key(ctx),
            Kind::File if self.encrypt => text(
                ctx,
                404,
                "the files of an encrypted item are not served",
                &self.cors,
                &[],
            ),
            Kind::File => {
                let list = if self.nm.audio {
                    &self.it.audio
                } else {
                    &self.it.renditions
                };
                let Some(r) = list
                    .get(self.nm.i)
                    .filter(|r| r.error.is_empty() && ext_of(&r.file.path) == self.nm.ext)
                else {
                    return text(ctx, 404, "no such file", &self.cors, &[]);
                };
                self.serve_file(ctx, &r.file.clone());
            }
            Kind::Poster => {
                let Some(p) = self
                    .it
                    .poster
                    .as_ref()
                    .filter(|p| ext_of(&p.path) == self.nm.ext)
                else {
                    return text(ctx, 404, "no such poster", &self.cors, &[]);
                };
                self.serve_file(ctx, &p.clone());
            }
        }
    }

    /// Serves the AES-128 key of the item, to a request `auth` let in for it, which no cache
    /// may keep (fit's `serveKey`).
    fn serve_key(&self, ctx: &mut Context) {
        if !self.encrypt {
            return text(ctx, 404, "the item is not encrypted", &self.cors, &[]);
        }
        let Some(keys) = self.l.keys() else {
            return text(
                ctx,
                503,
                "the media library's keys are not loaded",
                &self.cors,
                &[("Retry-After", "1")],
            );
        };
        let key = keys.key(library::keys::KEY_ITEM, &self.it.id, 0);
        let mut r = Response::with_body(200, "application/octet-stream", key.to_vec())
            .header("Cache-Control", "private, no-store");
        if !self.cors.is_empty() {
            r = r.header("Access-Control-Allow-Origin", &self.cors);
        }
        ctx.respond(r);
    }

    fn subtitle(&self) -> Option<&Subtitle> {
        self.it
            .subtitles
            .get(self.nm.i)
            .filter(|s| s.error.is_empty())
    }

    /// Serves the file `f` as it is, with ranges, through akio-http's `Files` of its root.
    fn serve_file(&self, ctx: &mut Context, f: &library::File) {
        let Some(root) = f.abs.to_str().and_then(|a| a.strip_suffix(f.path.as_str())) else {
            return text(ctx, 404, "no such file", &self.cors, &[]);
        };
        let files = self.st.files(std::path::Path::new(root));
        let st = Arc::clone(&self.st);
        ctx.add_response_filter(move |resp, _| {
            let n = resp.file.as_ref().map_or(resp.body.len() as u64, |f| f.len);
            st.sent.fetch_add(n, Relaxed);
        });
        if !self.cors.is_empty() {
            let cors = self.cors.clone();
            ctx.add_response_filter(move |resp, _| {
                resp.headers.set("Access-Control-Allow-Origin", &cors);
            });
        }
        files.serve(ctx, &format!("/{}", f.path));
    }

    /// Serves subtitle `s` as WebVTT, from the cache or read on the library's pool.
    fn serve_vtt(self, ctx: &mut Context, s: Subtitle) {
        let k = Key {
            file: FileKey::of(&s.file),
            what: What::Vtt,
        };
        if let Some(Val::Blob(b)) = self.st.cache.peek(&k) {
            return self.st.send(ctx, &b, &Own::default());
        }
        let Some(w) = Waiter::new(ctx, self.timeout, &self.cors) else {
            return text(ctx, 503, "the media library is busy", &self.cors, &[]);
        };
        let st = Arc::clone(&self.st);
        let file = s.file.clone();
        let reply = waiter_reply(w, Arc::clone(&st), Some(file.clone()), Own::default());
        match self.st.cache.begin(&k, reply) {
            Begin::Kept(v, reply) => reply(Ok(v)),
            Begin::Refused(reply) => reply(Err(Error::Busy)),
            Begin::Waiting => {}
            Begin::Build(b) => {
                let st2 = Arc::clone(&st);
                self.l.subtitle(&s, move |r| {
                    let r = r.map(|data| {
                        Val::Blob(Arc::new(Blob {
                            data: playlist::with_timestamp_map(&data),
                            ctype: "text/vtt; charset=utf-8",
                            etag: etag(&file, "vtt"),
                            modified: file.modified,
                        }))
                    });
                    let bytes = val_bytes(r.as_ref().ok());
                    st2.cache.done(&k, &b, r, bytes);
                });
            }
        }
    }

    /// Serves the media playlist, the initialization section or a segment of `r` in `c`.
    fn serve_built(self, ctx: &mut Context, r: Rendition, c: C, query: String, own: Own) {
        let file = FileKey::of(&r.file);
        let kind = self.nm.kind;
        let (audio, i, n) = (self.nm.audio, self.nm.i, self.nm.n);
        // A playlist with a query is the request's own, made from the plan; one without is the
        // one the cache keeps.
        let what = match kind {
            Kind::Media if query.is_empty() => Some(What::List(c)),
            Kind::Media => None,
            Kind::Init => Some(What::Init),
            _ => self.segment_what(c, n),
        };
        if let Some(what) = what {
            let k = Key {
                file: file.clone(),
                what,
            };
            if let Some(Val::Blob(b)) = self.st.cache.peek(&k) {
                return self.st.send(ctx, &b, &own);
            }
        }
        if kind == Kind::Media {
            if let Some(Val::Plan(p)) = self.st.cache.peek(&Key {
                file: file.clone(),
                what: What::Plan,
            }) {
                let b = self.media_blob(&p, &r, c, &query);
                return self.st.send(ctx, &b, &own);
            }
        }
        let Some(w) = Waiter::new(ctx, self.timeout, &self.cors) else {
            return text(ctx, 503, "the media library is busy", &self.cors, &[]);
        };
        let me = Arc::new(self);
        let l = Arc::clone(&me.l);
        let rr = r.clone();
        l.movie(&r, move |m| {
            let movie = match m {
                Ok(m) => m,
                Err(e) => {
                    let st = Arc::clone(&me.st);
                    let f = rr.file.clone();
                    return w.answer(move |ctx| fail(ctx, &st, Some(&f), &e));
                }
            };
            let (r2, m2) = (rr.clone(), Arc::clone(&movie));
            Arc::clone(&me).with_plan(&r2, &m2, w, move |me, w, plan| match kind {
                Kind::Media => {
                    let b = me.media_blob(&plan, &rr, c, &query);
                    let st = Arc::clone(&me.st);
                    w.answer(move |ctx| st.send(ctx, &b, &own));
                }
                Kind::Init => me.init(&rr, &movie, w),
                _ => me.segment(&rr, &movie, &plan, audio, i, n, c, w),
            });
        });
    }

    /// The media playlist of `r` from `p`, kept when it carries no query.
    fn media_blob(&self, p: &Plan, r: &Rendition, c: C, query: &str) -> Blob {
        let data = playlist::render_media(p, self.nm.audio, self.nm.i, c, query, self.encrypt)
            .into_bytes();
        let b = Blob {
            data,
            ctype: PLAYLIST,
            etag: String::new(),
            modified: r.file.modified,
        };
        if query.is_empty() {
            let k = Key {
                file: FileKey::of(&r.file),
                what: What::List(c),
            };
            let bytes = b.data.len() as u64;
            let kept = Arc::new(Blob {
                data: b.data.clone(),
                ctype: b.ctype,
                etag: String::new(),
                modified: b.modified,
            });
            self.st.cache.put(&k, Val::Blob(kept), bytes);
        }
        b
    }

    /// Gets `r`'s plan, from the cache or made on the library's pool, then runs `next` with it;
    /// an error answers `w`.
    fn with_plan(
        self: Arc<Self>,
        r: &Rendition,
        movie: &Arc<Movie>,
        w: Waiter,
        next: impl FnOnce(Arc<Self>, Waiter, Arc<Plan>) + Send + 'static,
    ) {
        let k = Key {
            file: FileKey::of(&r.file),
            what: What::Plan,
        };
        let me = Arc::clone(&self);
        let f = r.file.clone();
        let w2 = w.clone();
        let reply: cache::Waiter<Val> = Box::new(move |v| match v {
            Ok(Val::Plan(p)) => next(me, w2, p),
            Ok(Val::Blob(_)) => {
                let st = Arc::clone(&me.st);
                w2.answer(move |ctx| fail(ctx, &st, Some(&f), &Error::NotServed));
            }
            Err(e) => {
                let st = Arc::clone(&me.st);
                w2.answer(move |ctx| fail(ctx, &st, Some(&f), &e));
            }
        });
        match self.st.cache.begin(&k, reply) {
            Begin::Kept(v, reply) => reply(Ok(v)),
            Begin::Refused(reply) => reply(Err(Error::Busy)),
            Begin::Waiting => {}
            Begin::Build(b) => {
                let st = Arc::clone(&self.st);
                let movie = Arc::clone(movie);
                let seg = self.st.key.seg_dur;
                let job_st = Arc::clone(&st);
                let job_k = k.clone();
                let ok = self.l.submit(
                    Box::new(move || {
                        let target = Time::try_from(seg.as_nanos()).unwrap_or(Time::MAX);
                        let r = Plan::with_limit(
                            &movie,
                            target,
                            akio_media::mp4::Limits::default().max_segments,
                        )
                        .map(|p| Val::Plan(Arc::new(p)))
                        .map_err(|e| Error::Malformed(e.to_string()));
                        let bytes = r.as_ref().ok().map_or(0, |_| {
                            (movie_segments(&movie, seg) as u64)
                                .saturating_mul(16)
                                .saturating_add(256)
                        });
                        job_st.cache.done(&job_k, &Build(b.0), r, bytes);
                    }),
                    Some(Box::new({
                        let st = Arc::clone(&st);
                        let k = k.clone();
                        let id = b.0;
                        move || st.cache.done(&k, &Build(id), Err(Error::Busy), 0)
                    })),
                );
                if !ok {
                    st.cache.done(&k, &b, Err(Error::Busy), 0);
                }
            }
        }
        drop(w);
    }

    /// The initialization section of `r`, built at once and kept.
    fn init(&self, r: &Rendition, movie: &Movie, w: Waiter) {
        let tracks = selected(r, movie);
        let mux = fmp4::Muxer::new(&tracks);
        let video = tracks.iter().any(|t| t.codec.is_video());
        let b = Arc::new(Blob {
            data: mux.init_segment().to_vec(),
            ctype: if video { "video/mp4" } else { "audio/mp4" },
            etag: etag(&r.file, &format!("init-{}", self.st.tag)),
            modified: r.file.modified,
        });
        let k = Key {
            file: FileKey::of(&r.file),
            what: What::Init,
        };
        self.st
            .cache
            .put(&k, Val::Blob(Arc::clone(&b)), b.data.len() as u64);
        let st = Arc::clone(&self.st);
        w.answer(move |ctx| st.send(ctx, &b, &Own::default()));
    }

    /// What the cache keeps segment `n` in `c` as: encrypted under the library's keys with
    /// `encrypt`, `None` while they are not loaded.
    fn segment_what(&self, c: C, n: usize) -> Option<What> {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        if !self.encrypt {
            return Some(What::Segment(c, n));
        }
        let keys = self.l.keys()?;
        let id = keys.id().as_bytes();
        let mut kid = [0u8; 8];
        for (d, s) in kid.iter_mut().zip(id) {
            *d = *s;
        }
        Some(What::Sealed(c, n, u64::from_be_bytes(kid)))
    }

    /// Segment `n` of `r` in `c`, from the cache or built on the library's pool.
    #[allow(clippy::too_many_arguments)]
    fn segment(
        &self,
        r: &Rendition,
        movie: &Arc<Movie>,
        plan: &Arc<Plan>,
        audio: bool,
        i: usize,
        n: usize,
        c: C,
        w: Waiter,
    ) {
        let _ = (audio, i);
        if n >= plan.len() {
            let cors = self.cors.clone();
            return w.answer(move |ctx| text(ctx, 404, "no such segment", &cors, &[]));
        }
        let seal = if self.encrypt {
            match self.l.keys() {
                Some(keys) => Some((keys, self.it.id.clone())),
                None => {
                    let cors = self.cors.clone();
                    return w.answer(move |ctx| {
                        text(
                            ctx,
                            503,
                            "the media library's keys are not loaded",
                            &cors,
                            &[("Retry-After", "1")],
                        );
                    });
                }
            }
        } else {
            None
        };
        let Some(what) = self.segment_what(c, n) else {
            let cors = self.cors.clone();
            return w.answer(move |ctx| {
                text(
                    ctx,
                    503,
                    "the media library's keys are not loaded",
                    &cors,
                    &[("Retry-After", "1")],
                );
            });
        };
        let k = Key {
            file: FileKey::of(&r.file),
            what,
        };
        let reply = waiter_reply(
            w,
            Arc::clone(&self.st),
            Some(r.file.clone()),
            Own::default(),
        );
        let b = match self.st.cache.begin(&k, reply) {
            Begin::Kept(v, reply) => return reply(Ok(v)),
            Begin::Refused(reply) => return reply(Err(Error::Busy)),
            Begin::Waiting => return,
            Begin::Build(b) => b,
        };
        let (st, l, r, movie, plan) = (
            Arc::clone(&self.st),
            Arc::clone(&self.l),
            r.clone(),
            Arc::clone(movie),
            Arc::clone(plan),
        );
        let (k2, st2, id) = (k.clone(), Arc::clone(&st), b.0);
        let (k3, st3) = (k.clone(), Arc::clone(&st));
        let ok = self.l.submit(
            Box::new(move || {
                let res = build_segment(&l, &st, &r, &movie, &plan, n, c, seal.as_ref());
                let bytes = val_bytes(res.as_ref().ok());
                st.cache.done(&k, &Build(id), res, bytes);
            }),
            Some(Box::new(move || {
                st2.cache.done(&k2, &Build(id), Err(Error::Busy), 0);
            })),
        );
        if !ok {
            st3.cache.done(&k3, &Build(id), Err(Error::Busy), 0);
        }
    }
}

use cache::Build;

const PLAYLIST: &str = "application/vnd.apple.mpegurl";

/// How many segments of `seg` a movie makes, about.
fn movie_segments(m: &Movie, seg: Duration) -> usize {
    let d = Duration::from_nanos(u64::try_from(m.duration_time()).unwrap_or(0));
    let s = seg.as_millis().max(1);
    usize::try_from(d.as_millis().checked_div(s).unwrap_or(0))
        .unwrap_or(usize::MAX)
        .saturating_add(16)
}

fn val_bytes(v: Option<&Val>) -> u64 {
    match v {
        Some(Val::Blob(b)) => b.data.capacity() as u64,
        Some(Val::Plan(_)) => 4096,
        None => 0,
    }
}

/// A cache waiter that answers `w` with the blob, or the error.
fn waiter_reply(
    w: Waiter,
    st: Arc<Store>,
    f: Option<library::File>,
    own: Own,
) -> cache::Waiter<Val> {
    Box::new(move |v| match v {
        Ok(Val::Blob(b)) => w.answer(move |ctx| st.send(ctx, &b, &own)),
        Ok(Val::Plan(_)) => w.answer(move |ctx| fail(ctx, &st, f.as_ref(), &Error::NotServed)),
        Err(e) => w.answer(move |ctx| fail(ctx, &st, f.as_ref(), &e)),
    })
}

/// The tracks of `movie` a playlist of `r` carries, the others left unknown so that a muxer
/// leaves them out.
fn selected(r: &Rendition, movie: &Movie) -> Vec<Track> {
    let (v, a) = playlist::pick_tracks(r);
    movie
        .av_tracks()
        .into_iter()
        .enumerate()
        .map(|(j, t)| {
            if Some(j) == v || Some(j) == a {
                t
            } else {
                Track::default()
            }
        })
        .collect()
}

/// Reads and muxes segment `n` of `plan` on the pool. It blocks.
#[allow(clippy::too_many_arguments)]
fn build_segment(
    l: &Library,
    st: &Store,
    r: &Rendition,
    movie: &Movie,
    plan: &Plan,
    n: usize,
    c: C,
    seal: Option<&(Arc<library::keys::Keys>, String)>,
) -> Result<Val, Error> {
    let tracks = selected(r, movie);
    let mut seg = plan.segment(movie, n);
    for (j, range) in seg.samples.iter_mut().enumerate() {
        if tracks
            .get(j)
            .is_none_or(|t| t.codec == akio_media::av::Codec::Unknown)
        {
            *range = range.end..range.end;
        }
    }
    let ranges = seg.byte_ranges(movie, READ_GAP);
    let total: u64 = ranges.iter().map(|x| x.end.saturating_sub(x.start)).sum();
    if total > st.key.seg_bytes {
        return Err(Error::Malformed(format!(
            "segment {n} holds {total} bytes, past max_segment_bytes, {}",
            st.key.seg_bytes
        )));
    }
    let h = l.open_file(&r.file)?;
    let mut parts: Vec<(u64, Vec<u8>)> = Vec::with_capacity(ranges.len());
    for x in &ranges {
        let len = usize::try_from(x.end.saturating_sub(x.start)).map_err(|_| Error::TooLarge)?;
        let b = h
            .read_exact(x.start, len)
            .map_err(|e| Error::Io(e.to_string()))?;
        parts.push((x.start, b));
    }
    let given: Vec<(u64, &[u8])> = parts.iter().map(|(o, b)| (*o, b.as_slice())).collect();
    let mut packets = seg
        .packets(movie, &given)
        .map_err(|e| Error::Malformed(e.to_string()))?;
    // The segments' clock puts the file's time 0 at LEAD, as fit's.
    let shift = lead_shift(plan);
    for p in &mut packets {
        p.dts = p.dts.saturating_add(shift);
        p.pts = p.pts.saturating_add(shift);
    }
    let mut out = Vec::with_capacity(usize::try_from(total).unwrap_or(0).saturating_add(4096));
    let ctype = match c {
        C::Fmp4 => {
            let mut mux = fmp4::Muxer::new(&tracks);
            mux.set_next_sequence(u32::try_from(n.saturating_add(1)).unwrap_or(u32::MAX));
            for p in &packets {
                if mux.carries(p.track) && !mux.write_packet(p) {
                    return Err(Error::Malformed("the segment passes fMP4's bounds".into()));
                }
            }
            mux.flush(Some(seg.end.saturating_add(shift)), &mut out);
            if tracks.iter().any(|t| t.codec.is_video()) {
                "video/mp4"
            } else {
                "audio/mp4"
            }
        }
        C::Ts => {
            let mut mux = mpegts::Muxer::new(&tracks);
            mux.write_tables(&mut out);
            for p in &packets {
                if mux.carries(p.track) {
                    mux.write_packet(p, &mut out);
                }
            }
            "video/mp2t"
        }
    };
    // I147: encrypted under the item's key, the IV the segment's number (fit's `build`).
    let (out, sealed) = match seal {
        Some((keys, id)) => (
            library::keys::encrypt(&keys.key(library::keys::KEY_ITEM, id, 0), n as u64, &out),
            format!("-{}", keys.id()),
        ),
        None => (out, String::new()),
    };
    Ok(Val::Blob(Arc::new(Blob {
        data: out,
        ctype,
        etag: etag(&r.file, &format!("{}{n}-{}{sealed}", c.as_str(), st.tag)),
        modified: r.file.modified,
    })))
}

/// What the packets' times gain so that the file's time 0 is at [`playlist::LEAD`]: the plan
/// already moved them by its delay.
fn lead_shift(plan: &Plan) -> Time {
    let lead = Time::try_from(playlist::LEAD.as_nanos()).unwrap_or(0);
    lead.saturating_sub(plan.delay()).max(0)
}

impl Rendition {
    /// Whether the probe found its `moov`.
    pub(crate) fn moov_known(&self) -> bool {
        self.moov.is_some()
    }
}
