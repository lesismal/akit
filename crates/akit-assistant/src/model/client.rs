//! The calls themselves (fit's `assistant/model/client.go`), on akio-http's asynchronous client
//! ([`Client::request_with`](akio_http::client::Client::request_with)): a call's answer is
//! handed, piece by piece, to its parse on the connection's I/O thread; its head within the
//! first-byte timeout, its pieces within the idle timeout, the whole within the call's; its
//! reads held while what it hands on waits ([`Call::hold`]). One akio client is kept for each
//! connect timeout and pinned certificate the providers ask for, at most 16, the oldest retired
//! past them.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::{ClientConfig as TlsConfig, RootCerts};
use akio_http::Method;
use akio_http::client::{
    CancelToken, Client as Http, ClientError, ClientOptions, ClientRequest, ResponseBody,
    ResponseHandler, ResponseHead,
};
use akit_config::assistant::ProviderKind;

use super::answer::{Gather, STOPPED};
use super::codec::{Codec, models_url, parse_models};
use super::frame::Framer;
use super::{
    ANTHROPIC_VERSION, Answer, Error, ErrorKind, Event, ModelInfo, Provider, Request, api_message,
    redact,
};

/// Bounds of a client's calls.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// A streamed answer's most bytes, which a call may lower or raise: 8 MiB.
    pub max_stream_bytes: u64,
    /// A line of an answer, and an SSE event's data: 1 MiB.
    pub max_event_bytes: usize,
    /// The tool calls of an answer: 64.
    pub max_tool_calls: usize,
    /// How much of a failed call's body is read for its message: 8 KiB.
    pub max_error_bytes: usize,
    /// A model list's most bytes: 4 MiB.
    pub max_models_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_stream_bytes: 8 << 20,
            max_event_bytes: 1 << 20,
            max_tool_calls: 64,
            max_error_bytes: 8 << 10,
            max_models_bytes: 4 << 20,
        }
    }
}

/// The transports a client keeps: one a connect timeout and pinned certificate.
pub const MAX_TRANSPORTS: usize = 16;
/// Idle connections kept a host, and how long.
const MAX_IDLE_PER_HOST: usize = 4;
const IDLE_CONN_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest answer's head.
const MAX_HEAD_BYTES: usize = 64 << 10;
/// The longest wait for a model list, whatever the provider's timeouts.
const MAX_MODELS_WAIT: Duration = Duration::from_secs(30);

type Key = (Duration, Option<[u8; 32]>);

struct Transports {
    closed: bool,
    map: HashMap<Key, Arc<Http>>,
    order: VecDeque<Key>,
}

/// Calls model APIs over the connections of an engine it is given, which must be running.
/// Cheap to clone.
#[derive(Clone)]
pub struct Client(Arc<ClientInner>);

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

struct ClientInner {
    engine: Arc<Engine>,
    lim: Limits,
    /// The CAs an https provider's certificate is checked against: the system's, read once on
    /// the control plane; akio's own lazy read without them.
    roots: Option<RootCerts>,
    t: Mutex<Transports>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Client {
    /// A client of `engine`'s connections, its https calls checked against `roots`.
    #[must_use]
    pub fn new(engine: Arc<Engine>, lim: Limits, roots: Option<RootCerts>) -> Client {
        Client(Arc::new(ClientInner {
            engine,
            lim,
            roots,
            t: Mutex::new(Transports {
                closed: false,
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
        }))
    }

    /// The client's bounds.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.0.lim
    }

    /// Closes the client's idle connections; calls after it fail. Calls in flight end as their
    /// callers cancel them, or with the engine.
    pub fn close(&self) {
        let mut t = lock(&self.0.t);
        t.closed = true;
        for h in t.map.values() {
            h.close_idle();
        }
        t.map.clear();
        t.order.clear();
    }

    /// How many transports the client keeps.
    #[must_use]
    pub fn transports(&self) -> usize {
        lock(&self.0.t).map.len()
    }

    /// The transport for `p`: an akio client for its connect timeout and pin.
    fn acquire(&self, p: &Provider) -> Result<Arc<Http>, ErrorKind> {
        let key: Key = (p.connect_timeout, p.pin);
        let mut t = lock(&self.0.t);
        if t.closed {
            return Err(ErrorKind::Closed);
        }
        if let Some(h) = t.map.get(&key) {
            return Ok(Arc::clone(h));
        }
        if t.order.len() >= MAX_TRANSPORTS {
            if let Some(old) = t.order.pop_front() {
                // Retired: its calls in flight keep it until they end.
                if let Some(h) = t.map.remove(&old) {
                    h.close_idle();
                }
            }
        }
        let mut opts = ClientOptions::default();
        opts.max_head_bytes = MAX_HEAD_BYTES;
        opts.max_idle_per_host = MAX_IDLE_PER_HOST;
        opts.idle_timeout = Some(IDLE_CONN_TIMEOUT);
        opts.happy_eyeballs.connect_timeout = p.connect_timeout;
        let mut http = Http::new(Arc::clone(&self.0.engine)).with_options(opts);
        let tls = match (&p.pin, &self.0.roots) {
            (Some(pin), _) => Some(
                TlsConfig::builder()
                    .pin_sha256(vec![*pin])
                    .build()
                    .map_err(|e| ErrorKind::Other(format!("TLS: {e}")))?,
            ),
            (None, Some(r)) => Some(
                TlsConfig::builder()
                    .roots(r.clone())
                    .build()
                    .map_err(|e| ErrorKind::Other(format!("TLS: {e}")))?,
            ),
            (None, None) => None,
        };
        if let Some(cfg) = tls {
            http = http.with_tls(cfg);
        }
        let h = Arc::new(http);
        t.map.insert(key, Arc::clone(&h));
        t.order.push_back(key);
        Ok(h)
    }

    /// Sends `r` to `p` and parses its answer as it streams: `on` hears each piece, text,
    /// thinking or a whole tool call, in order, and `done` hears the whole answer, or why there
    /// is none, once, after the last piece. Both run on akio's I/O threads and must not block;
    /// `done` may run before `stream` returns, for a call refused at once (a bad base URL, a key
    /// that would go in the clear). `max_bytes` bounds the answer's body; the client's
    /// `max_stream_bytes` when 0.
    ///
    /// The call is tried again at once, as the provider's `retries` allow, when its connection
    /// failed or closed before any answer; never after an answer of any status.
    pub fn stream(
        &self,
        p: &Provider,
        r: &Request,
        max_bytes: u64,
        on: impl FnMut(Event) + Send + 'static,
        done: impl FnOnce(Result<Answer, Error>) + Send + 'static,
    ) -> Call {
        let max = if max_bytes == 0 {
            self.0.lim.max_stream_bytes
        } else {
            max_bytes
        };
        if let Err(why) = r.check() {
            done(Err(Error::new(&p.name, ErrorKind::Other(why))));
            return Call::ended(p);
        }
        let (url, body) = Codec::of(p.kind).call(p, r);
        let done: Done = Box::new(move |got| {
            done(got.map(|g| match g {
                Got::Answer(a) => a,
                Got::Body(_) => Answer::default(),
            }));
        });
        self.prepare(
            p,
            Method::Post,
            url,
            body,
            max,
            p.first_byte_timeout,
            p.timeout,
            What::Stream(Some(Box::new(on))),
            done,
        )
        .run()
    }

    /// Asks `p` for its list of models, and calls `done` once with it, or with why there is
    /// none. The wait is bounded by the provider's timeouts and by 30 s.
    pub fn models(
        &self,
        p: &Provider,
        done: impl FnOnce(Result<Vec<ModelInfo>, Error>) + Send + 'static,
    ) -> Call {
        let name = p.name.clone();
        let kind = p.kind;
        let done: Done = Box::new(move |got| {
            done(got.and_then(|g| match g {
                Got::Body(b) => {
                    parse_models(kind, &b).map_err(|m| Error::new(&name, ErrorKind::Other(m)))
                }
                Got::Answer(_) => Ok(Vec::new()),
            }));
        });
        self.prepare(
            p,
            Method::Get,
            models_url(p),
            Vec::new(),
            self.0.lim.max_models_bytes,
            p.first_byte_timeout.min(MAX_MODELS_WAIT),
            p.timeout.min(MAX_MODELS_WAIT),
            What::Whole,
            done,
        )
        .run()
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &self,
        p: &Provider,
        method: Method,
        url: String,
        body: Vec<u8>,
        max: u64,
        first_byte: Duration,
        total: Duration,
        what: What,
        done: Done,
    ) -> Call {
        let (on, stream) = match what {
            What::Stream(on) => (on, true),
            What::Whole => (None, false),
        };
        Call(Arc::new(CallInner {
            client: Some(self.clone()),
            p: p.clone(),
            method,
            url,
            body,
            max,
            first_byte,
            total,
            stream,
            started: Instant::now(),
            on: Mutex::new(on),
            st: Mutex::new(State {
                ended: false,
                cause: None,
                attempts: 0,
                token: None,
                body: None,
                held: false,
                done: Some(done),
                http: None,
                first: None,
            }),
        }))
    }
}

/// What a call's answer is for.
enum What {
    /// Streamed, each piece handed to the function.
    Stream(Option<On>),
    /// Kept whole: a model list.
    Whole,
}

/// What a call got.
enum Got {
    Answer(Answer),
    Body(Vec<u8>),
}

type Done = Box<dyn FnOnce(Result<Got, Error>) + Send>;

/// What hears each piece of a streamed answer.
type On = Box<dyn FnMut(Event) + Send>;

/// A call in flight, which [`cancel`](Call::cancel) abandons. Cheap to clone.
#[derive(Clone)]
pub struct Call(Arc<CallInner>);

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("provider", &self.0.p.name)
            .finish_non_exhaustive()
    }
}

struct CallInner {
    client: Option<Client>,
    p: Provider,
    method: Method,
    url: String,
    body: Vec<u8>,
    /// The bound on the answer's body.
    max: u64,
    first_byte: Duration,
    total: Duration,
    stream: bool,
    started: Instant,
    on: Mutex<Option<On>>,
    st: Mutex<State>,
}

struct State {
    ended: bool,
    /// Why the call is being abandoned.
    cause: Option<ErrorKind>,
    attempts: u32,
    /// The attempt in flight's.
    token: Option<CancelToken>,
    body: Option<ResponseBody>,
    /// Whether its reads are held.
    held: bool,
    done: Option<Done>,
    http: Option<Arc<Http>>,
    /// The first-byte timer, armed at a successful answer's head until its first piece.
    first: Option<akio::timer::SendTimer>,
}

impl Call {
    /// A call that ended before it began.
    fn ended(p: &Provider) -> Call {
        Call(Arc::new(CallInner {
            client: None,
            p: p.clone(),
            method: Method::Get,
            url: String::new(),
            body: Vec::new(),
            max: 0,
            first_byte: Duration::ZERO,
            total: Duration::ZERO,
            stream: false,
            started: Instant::now(),
            on: Mutex::new(None),
            st: Mutex::new(State {
                ended: true,
                cause: None,
                attempts: 0,
                token: None,
                body: None,
                held: false,
                done: None,
                http: None,
                first: None,
            }),
        }))
    }

    /// Abandons the call: its `done` hears `why` (that it was cancelled, when empty), unless it
    /// has heard already, and its connection is closed. From any thread, any number of times.
    pub fn cancel(&self, why: &str) {
        let why = if why.is_empty() {
            "the call was cancelled"
        } else {
            why
        };
        self.abort(ErrorKind::Cancelled(why.to_owned()));
    }

    /// Abandons the call for `cause`: the attempt in flight is cancelled, and its callbacks end
    /// the call with the cause.
    fn abort(&self, cause: ErrorKind) {
        let token = {
            let mut st = lock(&self.0.st);
            if st.ended || st.cause.is_some() {
                return;
            }
            st.cause = Some(cause.clone());
            st.token.clone()
        };
        match token {
            Some(t) => t.cancel(),
            // Not sent: nothing will call back.
            None => self.finish(Err(cause)),
        }
    }

    /// The answer's first piece came: its timer stops.
    fn first_piece(&self) {
        let t = lock(&self.0.st).first.take();
        if let Some(t) = t {
            let _ = t.cancel();
        }
    }

    /// Holds (`true`) or resumes (`false`) the reads of the call's answer, so that the API stops
    /// sending once the socket's buffers fill, while what was handed on waits to be taken; the
    /// idle timeout stops while they are held, the call's own does not.
    pub fn hold(&self, hold: bool) {
        let body = {
            let mut st = lock(&self.0.st);
            st.held = hold;
            st.body.clone()
        };
        if let Some(b) = body {
            b.hold_reads(hold);
        }
    }

    /// How many times the call was sent so far.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        lock(&self.0.st).attempts
    }

    /// Whether the call has ended.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        lock(&self.0.st).ended
    }

    fn run(self) -> Call {
        let p = &self.0.p;
        let Some(host) = http_host(&self.0.url) else {
            self.finish(Err(ErrorKind::Other(format!(
                "{:?} is not an http or https URL",
                p.base_url
            ))));
            return self;
        };
        if !p.key.is_empty() && self.0.url.starts_with("http://") && !loopback(&host) {
            self.finish(Err(ErrorKind::KeyInClear));
            return self;
        }
        let http = match self.0.client.as_ref().map(|c| c.acquire(p)) {
            Some(Ok(h)) => h,
            Some(Err(e)) => {
                self.finish(Err(e));
                return self;
            }
            None => {
                self.finish(Err(ErrorKind::Closed));
                return self;
            }
        };
        lock(&self.0.st).http = Some(http);
        self.start();
        self
    }

    /// Sends the call once more.
    fn start(&self) {
        let token = CancelToken::new();
        let http = {
            let mut st = lock(&self.0.st);
            if st.ended {
                return;
            }
            if let Some(c) = st.cause.clone() {
                drop(st);
                return self.finish(Err(c));
            }
            st.attempts = st.attempts.saturating_add(1);
            st.token = Some(token.clone());
            st.body = None;
            st.http.clone()
        };
        let Some(http) = http else { return };
        let left = self.0.total.saturating_sub(self.0.started.elapsed());
        if left.is_zero() {
            return self.finish(Err(ErrorKind::Timeout));
        }
        let p = &self.0.p;
        let mut req = ClientRequest::new(self.0.method.clone(), &self.0.url)
            .timeout(left)
            .response_header_timeout(self.0.first_byte.min(left))
            .response_idle_timeout(p.idle_timeout)
            .cancel_with(&token);
        for (name, value) in headers(p, &self.0.method) {
            req = req.header(&name, &value);
        }
        if self.0.method == Method::Post {
            req = req.body(self.0.body.clone());
        }
        let sink = if self.0.stream {
            let codec = Codec::of(p.kind);
            let me = self.clone();
            let emit: Box<super::answer::Emit> = Box::new(move |e| me.emit(e));
            let lim = self
                .0
                .client
                .as_ref()
                .map(Client::limits)
                .unwrap_or_default();
            Sink::Stream {
                framer: Framer::new(codec.ndjson(), lim.max_event_bytes),
                codec,
                gather: Box::new(Gather::new(Answer::default(), emit, lim.max_tool_calls)),
            }
        } else {
            Sink::Whole(Vec::new())
        };
        http.request_with(
            req,
            Attempt {
                call: self.clone(),
                status: None,
                retry_after: String::new(),
                error_body: Vec::new(),
                bytes: 0,
                body: None,
                sink,
            },
        );
    }

    /// Hands an event on, unless the call has ended: false then, and the parse stops.
    fn emit(&self, e: Event) -> bool {
        {
            let st = lock(&self.0.st);
            if st.ended || st.cause.is_some() {
                return false;
            }
        }
        let mut on = lock(&self.0.on);
        if let Some(f) = on.as_mut() {
            f(e);
        }
        drop(on);
        let st = lock(&self.0.st);
        !st.ended && st.cause.is_none()
    }

    /// The cause the call is being abandoned for, if it is.
    fn cause(&self) -> Option<ErrorKind> {
        lock(&self.0.st).cause.clone()
    }

    /// Ends the call once.
    fn finish(&self, got: Result<Got, ErrorKind>) {
        let (done, attempts, first) = {
            let mut st = lock(&self.0.st);
            if st.ended {
                return;
            }
            st.ended = true;
            st.body = None;
            st.token = None;
            st.http = None;
            (st.done.take(), st.attempts, st.first.take())
        };
        if let Some(t) = first {
            let _ = t.cancel();
        }
        // No more events: the caller's function goes with the call.
        lock(&self.0.on).take();
        let Some(done) = done else { return };
        done(match got {
            Ok(Got::Answer(mut a)) => {
                a.attempts = attempts;
                Ok(Got::Answer(a))
            }
            Ok(g) => Ok(g),
            Err(k) => Err(self.describe(k)),
        });
    }

    /// The error naming the provider, the key taken out of it.
    fn describe(&self, k: ErrorKind) -> Error {
        let key = &self.0.p.key;
        let k = match k {
            ErrorKind::Api {
                status,
                message,
                retry_after,
            } => ErrorKind::Api {
                status,
                message: redact(&message, key),
                retry_after,
            },
            ErrorKind::Other(m) => ErrorKind::Other(redact(&m, key)),
            other => other,
        };
        Error::new(&self.0.p.name, k)
    }
}

/// The headers of a call to `p`: its own, then the key's and the body's, which its own cannot
/// be (the config refuses them).
fn headers(p: &Provider, method: &Method) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = p.headers.clone();
    let key = p.key.expose();
    match p.kind {
        ProviderKind::Anthropic => {
            if !key.is_empty() {
                h.push(("x-api-key".into(), key.to_owned()));
            }
            h.push(("anthropic-version".into(), ANTHROPIC_VERSION.into()));
        }
        ProviderKind::Gemini => {
            if !key.is_empty() {
                h.push(("x-goog-api-key".into(), key.to_owned()));
            }
        }
        ProviderKind::AzureOpenAi => {
            if !key.is_empty() {
                h.push(("api-key".into(), key.to_owned()));
            }
        }
        _ => {
            if !key.is_empty() {
                h.push(("Authorization".into(), format!("Bearer {key}")));
            }
        }
    }
    if *method == Method::Post {
        h.push(("Content-Type".into(), "application/json".into()));
        let accept = if p.kind == ProviderKind::Ollama {
            "application/x-ndjson"
        } else {
            "text/event-stream"
        };
        h.push(("Accept".into(), accept.into()));
    } else {
        h.push(("Accept".into(), "application/json".into()));
    }
    h
}

/// The host of an http or https URL, without its port.
fn http_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') || authority.contains(' ') {
        return None;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_owned())
}

/// Whether `host` is `localhost` or a loopback address.
pub(crate) fn loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// What takes an attempt's answer.
enum Sink {
    Stream {
        framer: Framer,
        codec: Codec,
        gather: Box<Gather>,
    },
    Whole(Vec<u8>),
}

/// One attempt of a call, as its answer arrives.
struct Attempt {
    call: Call,
    /// The answer's status, once its head came.
    status: Option<u16>,
    retry_after: String,
    error_body: Vec<u8>,
    bytes: u64,
    body: Option<ResponseBody>,
    sink: Sink,
}

impl Attempt {
    fn ok(&self) -> bool {
        self.status.is_some_and(|s| (200..300).contains(&s))
    }

    /// Ends the call with `e`, the rest of the answer not read.
    fn stop(&mut self, e: ErrorKind) {
        let e = match e {
            ErrorKind::Other(m) if m == STOPPED => self
                .call
                .cause()
                .unwrap_or(ErrorKind::Cancelled("the call was cancelled".into())),
            e => e,
        };
        self.call.finish(Err(e));
        if let Some(b) = &self.body {
            b.cancel();
        }
    }

    fn api_error(&self) -> ErrorKind {
        ErrorKind::Api {
            status: self.status.unwrap_or(0),
            message: api_message(&self.error_body),
            retry_after: self.retry_after.clone(),
        }
    }
}

impl ResponseHandler for Attempt {
    fn on_head(&mut self, head: ResponseHead, body: ResponseBody) {
        self.status = Some(head.status);
        if let Some(ms) = head.headers.get("Retry-After-Ms") {
            self.retry_after = format!("{ms} ms");
        } else if let Some(s) = head.headers.get("Retry-After") {
            self.retry_after = if s.bytes().all(|b| b.is_ascii_digit()) && !s.is_empty() {
                format!("{s} s")
            } else {
                s.to_owned()
            };
        }
        let gone = {
            let mut st = lock(&self.call.0.st);
            if st.held {
                body.hold_reads(true);
            }
            st.body = Some(body.clone());
            st.ended || st.cause.is_some()
        };
        self.body = Some(body);
        if gone {
            if let Some(b) = &self.body {
                b.cancel();
            }
            return;
        }
        // A successful answer's first piece is waited for within what is left of the first-byte
        // timeout (akio's head timeout bounded the wait for the head): a timeout, not a retry.
        if self.ok() {
            let left = self
                .call
                .0
                .first_byte
                .saturating_sub(self.call.0.started.elapsed());
            let me = self.call.clone();
            if let Ok(t) = akio::timer::after_send(left, move || me.abort(ErrorKind::FirstByte)) {
                lock(&self.call.0.st).first = Some(t);
            }
        }
    }

    fn on_data(&mut self, chunk: &[u8]) {
        if !self.ok() {
            let max = self
                .call
                .0
                .client
                .as_ref()
                .map(|c| c.limits().max_error_bytes)
                .unwrap_or(8 << 10);
            let room = max.saturating_sub(self.error_body.len());
            self.error_body
                .extend_from_slice(chunk.get(..room.min(chunk.len())).unwrap_or(&[]));
            if self.error_body.len() >= max {
                // Enough to say why: the rest is not read.
                let e = self.api_error();
                self.stop(e);
            }
            return;
        }
        if chunk.is_empty() {
            return;
        }
        if self.bytes == 0 {
            self.call.first_piece();
        }
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if self.bytes > self.call.0.max {
            return self.stop(ErrorKind::TooLarge);
        }
        let r = match &mut self.sink {
            Sink::Stream {
                framer,
                codec,
                gather,
            } => framer.write(chunk, &mut |name, data| codec.parse(gather, name, data)),
            Sink::Whole(b) => {
                b.extend_from_slice(chunk);
                Ok(())
            }
        };
        if let Err(e) = r {
            self.stop(e);
        }
    }

    fn on_end(&mut self, _trailers: &akio_http::Headers) {
        if !self.ok() {
            let e = self.api_error();
            return self.call.finish(Err(e));
        }
        let got = match std::mem::replace(&mut self.sink, Sink::Whole(Vec::new())) {
            Sink::Stream {
                mut framer,
                mut codec,
                mut gather,
            } => framer
                .end(&mut |name, data| codec.parse(&mut gather, name, data))
                .and_then(|()| codec.finish(&mut gather))
                .map(|()| Got::Answer(gather.res)),
            Sink::Whole(b) => Ok(Got::Body(b)),
        };
        match got {
            Ok(g) => self.call.finish(Ok(g)),
            Err(e) => self.stop(e),
        }
    }

    fn on_error(&mut self, e: ClientError) {
        if let Some(c) = self.call.cause() {
            return self.call.finish(Err(c));
        }
        // Before any answer, a connection that failed or closed: tried again at once while the
        // retries last (never after a wait, decision row 100).
        let transport = matches!(
            e,
            ClientError::Io(_) | ClientError::Closed | ClientError::Refused
        );
        if self.status.is_none() && transport && self.call.attempts() <= self.call.0.p.retries {
            return self.call.start();
        }
        let kind = match e {
            ClientError::HeaderTimeout => ErrorKind::FirstByte,
            ClientError::IdleTimeout if self.bytes == 0 && self.ok() => ErrorKind::FirstByte,
            ClientError::IdleTimeout => ErrorKind::Idle,
            ClientError::Timeout => ErrorKind::Timeout,
            ClientError::Cancelled => ErrorKind::Cancelled("the call was cancelled".into()),
            e if self.status.is_some() && !self.ok() => {
                let _ = e;
                self.api_error()
            }
            e if self.status.is_some() => ErrorKind::Other(format!("the answer broke off: {e}")),
            e => ErrorKind::Other(e.to_string()),
        };
        self.call.finish(Err(kind));
    }
}
