//! fit's gRPC proxy tests (I50, `handler/proxy/grpc_test.go`), ported: unary calls (an echo, a
//! 200 KB one, the upstream's status and trailers passed on, the call's deadline carried
//! upstream), server, client and bidirectional streaming, backpressure each way, an upstream
//! that refuses connections or goes away midway, and calls cut short by their `grpc-timeout`
//! or the route's `timeout`, each over an `h2c` upstream and, where fit runs it so, an `h2`
//! one with TLS.
//!
//! The gRPC client and server are akio-grpc's, with test messages of their own (`n:text`) where
//! fit's use gRPC's JSON codec; the backpressure tests' ends are written on akio-http's client
//! and server, which tell how much each side has sent. akit serves the calls as `akit run`
//! does for `h2c` (its listener, from a config); for `h2`, whose pool needs the test CA (the
//! `tls_*` fields are I54's), the proxy handler is provisioned on a pool given it and served
//! by akio-http alone. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

#[path = "../../akit-listener/tests/support/mod.rs"]
mod support;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_grpc::{CallOptions, Code, GrpcClient, GrpcServer, Sink, Status};
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akio_http::{BodyEvent, BodyMode, BodyReads, Context, Headers, Request};
use akit_runtime::upstream::{EngineSlot, Pool};
use akit_runtime::{Env, GraphCell, Handler, HandlerParams, MAX_BODY_BYTES, Modules, Registry};

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[akit_proxy::MODULES]).unwrap()
}

/// fit's `config1`: the proxy to `addr` over `protocol`, with the proxy's `params`; bodies of
/// any size, as fit's tests allow them.
fn config(addr: &str, protocol: &str, params: &str) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
  "limits": {{"max_body_bytes": 4294967296}},
  "upstreams": [{{"name": "app", "addresses": ["{addr}"], "protocol": "{protocol}"}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]
}}"#
    )
}

/// akit in front of an upstream, reached over h2c, and its clients' connections, closed before
/// it shuts down.
struct Akit {
    front: Front,
    clients: Mutex<Vec<H2Client>>,
}

/// Through akit's listener (`h2c` upstreams), or its proxy handler alone (`h2`).
enum Front {
    Listener {
        engine: Arc<Engine>,
        _cell: GraphCell,
    },
    Handler {
        engine: Arc<Engine>,
        _modules: Modules,
    },
}

impl Akit {
    /// akit proxying to `up` (`params` added to the proxy's).
    fn start(up: &Upstream, params: &str) -> Akit {
        Akit::to(&up.addr(), up.tls.as_deref(), params)
    }

    /// To `addr`, over h2 with TLS trusting the CA at `ca` when given, else over h2c.
    fn to(addr: &str, ca: Option<&std::path::Path>, params: &str) -> Akit {
        let protocol = if ca.is_some() { "h2" } else { "h2c" };
        let config = akit_config::from_json(&config(addr, protocol, params), None).unwrap();
        let Some(ca) = ca else {
            let slot = EngineSlot::new();
            let graph =
                akit_runtime::compile(&config, &registry(), &Env::new(slot.clone())).unwrap();
            let cell = GraphCell::new(graph);
            let mut o = akit_listener::Options::default();
            o.backend = backend();
            o.shutdown_timeout = Duration::from_secs(1);
            let engine = Arc::new(akit_listener::engine(&cell, &o).unwrap());
            engine.start().unwrap();
            assert!(slot.set(engine.clone()));
            return Akit::new(Front::Listener {
                engine,
                _cell: cell,
            });
        };
        let tls = support::https::client(ca, &["h2"], None);
        let pool = Pool::new(&config.upstreams[0], EngineSlot::with(clients())).with_tls(tls);
        let env = Env::new(EngineSlot::with(clients())).with_upstreams(vec![pool]);
        let mut modules = Modules::new();
        let handler = registry()
            .handler("proxy", &config.routes[0].handler, &env, &mut modules)
            .unwrap();
        Akit::new(Front::Handler {
            engine: serve(akio_http::serve(Alone(handler))),
            _modules: modules,
        })
    }

    fn new(front: Front) -> Akit {
        Akit {
            front,
            clients: Mutex::new(Vec::new()),
        }
    }

    fn addr(&self) -> SocketAddr {
        let (Front::Listener { engine, .. } | Front::Handler { engine, .. }) = &self.front;
        engine.local_addrs()[0]
    }

    /// A gRPC client of akit's.
    fn client(&self) -> GrpcClient {
        let addr = self.addr();
        let h2 = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
        self.clients.lock().unwrap().push(h2.clone());
        GrpcClient::new(h2, &format!("http://{addr}"))
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.clients
            .lock()
            .unwrap()
            .iter()
            .for_each(H2Client::close);
        match &self.front {
            Front::Listener { engine, .. } => {
                akit_listener::shutdown(engine, Duration::from_secs(1));
            }
            Front::Handler { engine, .. } => engine.shutdown(),
        }
    }
}

/// The proxy handler served alone, with the listener's body limit of fit's tests.
struct Alone(Arc<dyn Handler>);

impl akio_http::Service for Alone {
    fn call(&self, ctx: &mut Context) {
        ctx.set_local(MAX_BODY_BYTES, "4294967296".to_owned());
        self.0.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.0.body_mode(req)
    }
}

fn serve(acceptor: impl akio::Acceptor) -> Arc<Engine> {
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend())
                .threads(2)
                .serve(acceptor),
        )
        .client_threads(1)
        .config({
            // Upstreams that go away go at once (`upstream_gone`).
            let mut c = akio::Config::default();
            c.shutdown_timeout = Duration::from_millis(200);
            c
        })
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

/// An engine for the tests' clients, and the `h2` pools'.
fn clients() -> Arc<Engine> {
    static E: std::sync::OnceLock<Arc<Engine>> = std::sync::OnceLock::new();
    E.get_or_init(|| {
        let e = Engine::builder().client_threads(2).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    })
    .clone()
}

/// A test message: `n` and `text`, as `n:text`.
fn msg(text: &str, n: usize) -> Vec<u8> {
    format!("{n}:{text}").into_bytes()
}

fn parse(m: &[u8]) -> (String, usize) {
    let s = std::str::from_utf8(m).unwrap();
    let (n, text) = s.split_once(':').unwrap();
    (text.to_owned(), n.parse().unwrap())
}

/// A gRPC message of `body`, framed.
fn frame(body: &[u8]) -> Vec<u8> {
    let mut f = vec![0];
    f.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    f.extend_from_slice(body);
    f
}

/// The test service's state: what its calls report, and what holds them.
#[derive(Default)]
struct State {
    /// How far off a `deadline` call's deadline was (`None`: it had none).
    deadline: Mutex<Option<Sender<Option<Duration>>>>,
    /// Told when a `hang` call or a `cut` stream has begun waiting.
    entered: Mutex<Option<Sender<()>>>,
    /// The calls left waiting, never finished.
    held: Mutex<Vec<Sink>>,
    /// The raw calls left waiting (`/raw.Hang`, `/raw.Cut`): their answers or streams.
    parked: Mutex<Vec<Box<dyn std::any::Any + Send>>>,
    /// The messages the backpressure download has written.
    sent: AtomicUsize,
    /// The reads of the backpressure upload, held until the test lets them go.
    gate: Mutex<Option<BodyReads>>,
}

/// The upstream: fit's test service on akio-grpc (`/test.Echo/...`: Unary echoes, Count streams
/// N messages back, Sum adds what it is sent, Chat answers each message as it comes), and on
/// akio-http the backpressure tests' ends (`/bp.Down`, `/bp.Up`) and the calls that ignore their
/// deadline, as fit's `hang` and `cut` do (akio-grpc's server ends a call at its deadline):
/// `/raw.Hang` never answers, `/raw.Cut` sends two messages and no more.
struct Upstream {
    engine: Arc<Engine>,
    state: Arc<State>,
    deadline: Receiver<Option<Duration>>,
    entered: Receiver<()>,
    /// The CA its certificate is of, when it serves TLS.
    tls: Option<std::path::PathBuf>,
}

/// The messages of the backpressure tests, and their size.
const BP_MESSAGES: usize = 1000;
const BP_SIZE: usize = 32 << 10;

struct UpService {
    grpc: GrpcServer,
    state: Arc<State>,
}

impl akio_http::Service for UpService {
    fn call(&self, ctx: &mut Context) {
        match ctx.req().path() {
            "/bp.Down" => down(ctx, &self.state),
            "/bp.Up" => up(ctx, &self.state),
            "/raw.Hang" | "/raw.Cut" => park(ctx, &self.state),
            _ => self.grpc.call(ctx),
        }
    }

    fn body_mode(&self, _: &Request) -> BodyMode {
        BodyMode::Stream
    }
}

/// The backpressure download: [`BP_MESSAGES`] messages of [`BP_SIZE`], each written once less
/// than 64 KiB waits to be sent, so that what it has written is what the proxy has taken.
fn down(ctx: &mut Context, state: &Arc<State>) {
    let state = state.clone();
    ctx.on_body(move |ctx, ev| {
        if let BodyEvent::End(_) = ev {
            let Some(d) = ctx.detach() else { return };
            let mut h = Headers::new();
            h.append("content-type", "application/grpc");
            let Some(stream) = d.stream_response(200, h) else {
                return;
            };
            let state = state.clone();
            thread::spawn(move || {
                let pad = "p".repeat(BP_SIZE);
                for i in 0..BP_MESSAGES {
                    while stream.queued_bytes() > 64 << 10 {
                        thread::sleep(Duration::from_millis(1));
                    }
                    if stream.write(&frame(&msg(&pad, i))).is_err() {
                        return;
                    }
                    state.sent.fetch_add(1, Ordering::SeqCst);
                }
                let mut t = Headers::new();
                t.append("grpc-status", "0");
                stream.finish_with_trailers(t);
            });
        }
    });
}

/// `/raw.Hang` and `/raw.Cut`, once their request is in: left waiting, and the test told.
fn park(ctx: &mut Context, state: &Arc<State>) {
    let state = state.clone();
    ctx.on_body(move |ctx, ev| {
        if let BodyEvent::End(_) = ev {
            let cut = ctx.req().path() == "/raw.Cut";
            let Some(d) = ctx.detach() else { return };
            let parked: Box<dyn std::any::Any + Send> = if cut {
                let mut h = Headers::new();
                h.append("content-type", "application/grpc");
                let Some(stream) = d.stream_response(200, h) else {
                    return;
                };
                for i in 0..2 {
                    let _ = stream.write(&frame(&msg("cut", i)));
                }
                Box::new(stream)
            } else {
                Box::new(d)
            };
            state.parked.lock().unwrap().push(parked);
            if let Some(tx) = state.entered.lock().unwrap().as_ref() {
                let _ = tx.send(());
            }
        }
    });
}

/// The backpressure upload: its reads held until the test lets them go, then the messages
/// counted, and answered with their count.
fn up(ctx: &mut Context, state: &Arc<State>) {
    if let Some(reads) = ctx.body_reads() {
        reads.hold(true);
        *state.gate.lock().unwrap() = Some(reads);
    }
    let (mut buf, mut count) = (Vec::new(), 0usize);
    ctx.on_body(move |ctx, ev| match ev {
        BodyEvent::Chunk(b) => {
            buf.extend_from_slice(b);
            while buf.len() >= 5 {
                let len = u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize;
                if buf.len() < 5 + len {
                    break;
                }
                buf.drain(..5 + len);
                count += 1;
            }
        }
        BodyEvent::End(_) => {
            let Some(d) = ctx.detach() else { return };
            let mut h = Headers::new();
            h.append("content-type", "application/grpc");
            if let Some(stream) = d.stream_response(200, h) {
                let _ = stream.write(&frame(&msg("sum", count)));
                let mut t = Headers::new();
                t.append("grpc-status", "0");
                stream.finish_with_trailers(t);
            }
        }
    });
}

fn grpc_server(state: &Arc<State>) -> GrpcServer {
    let (s1, s2) = (state.clone(), state.clone());
    GrpcServer::new()
        .unary("/test.Echo/Unary", move |md, req, sink| {
            let (text, n) = parse(&req);
            match text.as_str() {
                "fail" => return sink.finish(Status::new(Code::NotFound, "no such thing")),
                "trailer" => sink.set_trailer("t", "trailer"),
                "deadline" => {
                    let left = md
                        .deadline
                        .map(|at| at.saturating_duration_since(Instant::now()));
                    if let Some(tx) = s1.deadline.lock().unwrap().as_ref() {
                        let _ = tx.send(left);
                    }
                }
                "hang" => {
                    s1.held.lock().unwrap().push(sink);
                    if let Some(tx) = s1.entered.lock().unwrap().as_ref() {
                        let _ = tx.send(());
                    }
                    return;
                }
                _ => {}
            }
            sink.send(&msg(&text, n));
            sink.finish(Status::ok());
        })
        .server_stream("/test.Echo/Count", move |_, req, sink| {
            let (text, n) = parse(&req);
            for i in 0..n {
                if text == "cut" && i == 2 {
                    s2.held.lock().unwrap().push(sink);
                    if let Some(tx) = s2.entered.lock().unwrap().as_ref() {
                        let _ = tx.send(());
                    }
                    return;
                }
                sink.send(&msg(&text, i));
            }
            sink.finish(Status::ok());
        })
        .client_stream("/test.Echo/Sum", |_, reqs, sink| {
            let total: usize = reqs.iter().map(|m| parse(m).1).sum();
            sink.send(&msg("sum", total * 1000 + reqs.len()));
            sink.finish(Status::ok());
        })
        .bidi("/test.Echo/Chat", |_, sink| {
            Box::new(move |m| match m {
                Some(m) => {
                    let (text, n) = parse(&m);
                    sink.send(&msg(&text.to_uppercase(), n));
                }
                None => sink.finish(Status::ok()),
            })
        })
}

impl Upstream {
    /// The test service over `proto`: `h2c`, or `h2` with TLS.
    fn start(proto: &str) -> Upstream {
        let state = Arc::new(State::default());
        let (dtx, deadline) = channel();
        let (etx, entered) = channel();
        *state.deadline.lock().unwrap() = Some(dtx);
        *state.entered.lock().unwrap() = Some(etx);
        let service = UpService {
            grpc: grpc_server(&state),
            state: state.clone(),
        };
        let (engine, tls) = if proto == "h2" {
            static N: AtomicUsize = AtomicUsize::new(0);
            let d = support::certs::dir(&format!(
                "akit-proxy-grpc-{}",
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let ca = support::certs::Ca::new("akit test CA");
            let ca_file = ca.write(&d, "ca.pem");
            let (cert, key) = ca.leaf(&d, "leaf", &["localhost", "127.0.0.1"]);
            let scfg = akio::tls::ServerConfig::builder(
                akio::tls::Certificate::from_pem(&std::fs::read(cert).unwrap()).unwrap(),
                akio::tls::PrivateKey::from_pem(&std::fs::read(key).unwrap()).unwrap(),
            )
            .alpn(["h2"])
            .build()
            .unwrap();
            let engine = serve(akio::tls::Tls::server(scfg, akio_http::serve(service)));
            (engine, Some(ca_file))
        } else {
            (serve(akio_http::serve(service)), None)
        };
        Upstream {
            engine,
            state,
            deadline,
            entered,
            tls,
        }
    }

    fn addr(&self) -> String {
        self.engine.local_addrs()[0].to_string()
    }

    fn deadline(&self) -> Option<Duration> {
        self.deadline.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn entered(&self) {
        self.entered.recv_timeout(Duration::from_secs(10)).unwrap();
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.state.held.lock().unwrap().clear();
        self.state.parked.lock().unwrap().clear();
        if let Some(reads) = self.state.gate.lock().unwrap().take() {
            reads.hold(false);
        }
        self.engine.shutdown();
    }
}

fn each_proto(f: impl Fn(&str)) {
    for proto in ["h2c", "h2"] {
        eprintln!("-- {proto}");
        f(proto);
    }
}

fn opts(timeout: Duration) -> CallOptions {
    let mut o = CallOptions::default();
    o.timeout = Some(timeout);
    o
}

/// A call's messages, status and trailers, by `bidi`, the requests sent as they are given.
struct Call {
    sink: akio_grpc::ClientSink,
    messages: Receiver<Vec<u8>>,
    done: Receiver<(Status, Headers)>,
}

impl Call {
    fn open(c: &GrpcClient, path: &str, timeout: Option<Duration>) -> Call {
        let (mtx, messages) = channel();
        let (dtx, done) = channel();
        let mut o = CallOptions::default();
        o.timeout = timeout;
        let sink = c.bidi(
            path,
            o,
            move |m| {
                let _ = mtx.send(m);
            },
            move |s, t| {
                let _ = dtx.send((s, t));
            },
        );
        Call {
            sink,
            messages,
            done,
        }
    }

    fn recv(&self) -> (String, usize) {
        parse(&self.messages.recv_timeout(Duration::from_secs(20)).unwrap())
    }

    fn done(&self) -> (Status, Headers) {
        self.done.recv_timeout(Duration::from_secs(20)).unwrap()
    }
}

/// fit's `TestGRPCUnary`: an echo, a 200 KB one, the upstream's status and trailers passed on,
/// and the call's deadline carried upstream.
#[test]
fn unary() {
    each_proto(|proto| {
        let up = Upstream::start(proto);
        let akit = Akit::start(&up, "");
        let c = akit.client();
        let o = || opts(Duration::from_secs(10));
        let out = c.unary("/test.Echo/Unary", &msg("hello", 7), o()).unwrap();
        assert_eq!(parse(&out), ("hello".to_owned(), 7));
        let big = "x".repeat(200 << 10);
        let out = c.unary("/test.Echo/Unary", &msg(&big, 0), o()).unwrap();
        assert_eq!(parse(&out).0, big);
        let e = c
            .unary("/test.Echo/Unary", &msg("fail", 0), o())
            .unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            (Code::NotFound, "no such thing")
        );

        let call = Call::open(&c, "/test.Echo/Unary", Some(Duration::from_secs(10)));
        call.sink.send(&msg("trailer", 0));
        call.sink.close_send();
        assert_eq!(call.recv().0, "trailer");
        let (status, trailers) = call.done();
        assert_eq!(status.code, Code::Ok);
        assert_eq!(trailers.get("t"), Some("trailer"));

        c.unary(
            "/test.Echo/Unary",
            &msg("deadline", 0),
            opts(Duration::from_secs(5)),
        )
        .unwrap();
        let d = up.deadline().expect("a deadline");
        assert!(
            d > Duration::from_secs(4) && d <= Duration::from_secs(5),
            "the upstream saw a deadline {d:?} away, want about 5s"
        );
    });
}

/// fit's `TestGRPCServerStreaming`: more messages back than akit holds whole.
#[test]
fn server_streaming() {
    each_proto(|proto| {
        let up = Upstream::start(proto);
        let akit = Akit::start(&up, "");
        let pad = "p".repeat(100);
        let (msgs, status) = akit.client().collect(
            "/test.Echo/Count",
            vec![msg(&pad, 2000)],
            opts(Duration::from_secs(20)),
        );
        assert_eq!(status.code, Code::Ok, "{status:?}");
        assert_eq!(msgs.len(), 2000);
        for (i, m) in msgs.iter().enumerate() {
            assert_eq!(parse(m), (pad.clone(), i));
        }
    });
}

/// fit's `TestGRPCClientStreaming`: more messages up than akit holds whole, before the one
/// answer.
#[test]
fn client_streaming() {
    each_proto(|proto| {
        let up = Upstream::start(proto);
        let akit = Akit::start(&up, "");
        let pad = "p".repeat(1000);
        let n = 500;
        let reqs = (0..n).map(|i| msg(&pad, i)).collect();
        let (msgs, status) =
            akit.client()
                .collect("/test.Echo/Sum", reqs, opts(Duration::from_secs(20)));
        assert_eq!(status.code, Code::Ok, "{status:?}");
        let want: usize = (0..n).sum();
        assert_eq!(parse(&msgs[0]).1, want * 1000 + n);
    });
}

/// fit's `TestGRPCBidiStreaming`: each message answered before the next is sent, which only
/// works if neither side's messages wait at akit for more to come.
#[test]
fn bidi_streaming() {
    each_proto(|proto| {
        let up = Upstream::start(proto);
        let akit = Akit::start(&up, "");
        let c = akit.client();
        let call = Call::open(&c, "/test.Echo/Chat", None);
        for i in 0..50 {
            let text = if i % 10 == 9 {
                "big".repeat(100 << 10)
            } else {
                "ping".to_owned()
            };
            assert!(call.sink.send(&msg(&text, i)));
            let (got, n) = call.recv();
            assert_eq!(n, i);
            assert_eq!(got, text.to_uppercase(), "answer {i}");
        }
        call.sink.close_send();
        let (status, _) = call.done();
        assert_eq!(status.code, Code::Ok, "{status:?}");
        assert!(call.messages.try_recv().is_err());
    });
}

/// A raw gRPC call over h2c to `addr`: `path` with `body` (framed messages), the stream's reads
/// held while `hold` is, written as fast as its queue lets it; the response's messages and
/// fields (its trailers among them) arrive on the receivers.
struct Raw {
    up: Arc<akio_http::h2client::H2Upload>,
    data: Receiver<Vec<u8>>,
    end: Receiver<Headers>,
    client: H2Client,
}

impl Drop for Raw {
    fn drop(&mut self) {
        self.client.close();
    }
}

impl Raw {
    fn open(addr: SocketAddr, path: &str, fields: &[(&str, &str)]) -> Raw {
        let client = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
        let mut req = ClientRequest::post(&format!("http://{addr}{path}"), Vec::new());
        req.headers.append("content-type", "application/grpc");
        req.headers.append("te", "trailers");
        for (n, v) in fields {
            req.headers.append(n, v);
        }
        let (dtx, data) = channel();
        let (etx, end) = channel();
        let mut fields = Headers::new();
        let up = client.open(req, move |e| match e {
            H2Event::Head { status, headers } => {
                fields.append(":status", status.to_string());
                for (n, v) in headers.iter() {
                    fields.append(n, v);
                }
            }
            H2Event::Data(d) => {
                let _ = dtx.send(d.to_vec());
            }
            H2Event::Trailers(t) => {
                for (n, v) in t.iter() {
                    fields.append(n, v);
                }
            }
            H2Event::End => {
                let _ = etx.send(std::mem::take(&mut fields));
            }
            H2Event::Error(e) => {
                fields.append("x-error", e.to_string());
                let _ = etx.send(std::mem::take(&mut fields));
            }
        });
        Raw {
            up: Arc::new(up),
            data,
            end,
            client,
        }
    }

    /// A unary call, by HTTP/2 rather than a gRPC client, so that the client keeps no deadline
    /// of its own (fit's `rawCall`): the response's fields, trailers included.
    fn call(addr: SocketAddr, path: &str, text: &str, fields: &[(&str, &str)]) -> Headers {
        let r = Raw::open(addr, path, fields);
        assert!(r.up.write(&frame(&msg(text, 0))));
        r.up.finish();
        r.end.recv_timeout(Duration::from_secs(20)).unwrap()
    }
}

/// fit's `TestGRPCBackpressure`: each side of a stream outpaces the other. A client that reads
/// nothing holds back the upstream's stream to it, and an upstream that reads nothing holds back
/// the client's; akit holds neither stream whole, and every message arrives once the slow side
/// reads. 32 MB each way.
#[test]
fn backpressure() {
    each_proto(|proto| {
        let up = Upstream::start(proto);
        let akit = Akit::start(&up, "");
        let n = BP_MESSAGES;

        // Down: the client's reads held.
        let down = Raw::open(akit.addr(), "/bp.Down", &[]);
        down.up.stream().hold_reads(true);
        assert!(down.up.write(&frame(&msg("go", n))));
        down.up.finish();
        thread::sleep(Duration::from_secs(1));
        let sent = up.state.sent.load(Ordering::SeqCst);
        assert!(
            sent < n / 2,
            "the upstream sent {sent} of {n} messages to a client that read none"
        );
        down.up.stream().hold_reads(false);
        let (mut buf, mut got) = (Vec::new(), 0);
        while got < n {
            buf.extend(down.data.recv_timeout(Duration::from_secs(20)).unwrap());
            while buf.len() >= 5 {
                let len = u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize;
                if buf.len() < 5 + len {
                    break;
                }
                assert_eq!(parse(&buf[5..5 + len]).1, got);
                buf.drain(..5 + len);
                got += 1;
            }
        }
        let end = down.end.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(end.get("grpc-status"), Some("0"), "{end:?}");

        // Up: the upstream's reads held; the client writes as fast as its queue lets it.
        let call = Raw::open(akit.addr(), "/bp.Up", &[]);
        let written = Arc::new(AtomicUsize::new(0));
        let (body, count) = (call.up.clone(), written.clone());
        let writer = thread::spawn(move || {
            let pad = "p".repeat(BP_SIZE);
            for i in 0..n {
                while body.queued_bytes() > 64 << 10 {
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(body.write(&frame(&msg(&pad, i))));
                count.fetch_add(1, Ordering::SeqCst);
            }
            body.finish();
        });
        thread::sleep(Duration::from_secs(1));
        let k = written.load(Ordering::SeqCst);
        assert!(
            k < n / 2,
            "the client sent {k} of {n} messages to an upstream that read none"
        );
        up.state.gate.lock().unwrap().take().unwrap().hold(false);
        writer.join().unwrap();
        let first = call.data.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(parse(&first[5..]), ("sum".to_owned(), n));
        let end = call.end.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(end.get("grpc-status"), Some("0"), "{end:?}");
    });
}

/// fit's `TestGRPCNoUpstream`: a call whose upstream refuses connections is answered
/// `UNAVAILABLE`.
#[test]
fn no_upstream() {
    let refused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = refused.local_addr().unwrap().to_string();
    drop(refused);
    let akit = Akit::to(&addr, None, "");
    let e = akit
        .client()
        .unary(
            "/test.Echo/Unary",
            &msg("hello", 0),
            opts(Duration::from_secs(10)),
        )
        .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (Code::Unavailable, "akit: the upstream is unavailable")
    );
}

/// fit's `TestGRPCUpstreamGone`: a server stream whose upstream went away midway ends with
/// `UNAVAILABLE`, after the messages that came.
#[test]
fn upstream_gone() {
    each_proto(|proto| {
        let up = Upstream::start(proto);
        let akit = Akit::start(&up, "");
        let c = akit.client();
        let call = Call::open(&c, "/test.Echo/Count", Some(Duration::from_secs(20)));
        call.sink.send(&msg("cut", 5));
        call.sink.close_send();
        for i in 0..2 {
            assert_eq!(call.recv(), ("cut".to_owned(), i));
        }
        up.entered();
        up.engine.shutdown();
        let (status, _) = call.done();
        assert_eq!(
            (status.code, status.message.as_str()),
            (Code::Unavailable, "akit: the upstream is unavailable")
        );
    });
}

/// fit's `TestGRPCTimeouts`: calls cut short at akit by their `grpc-timeout`, and by the
/// route's `timeout`, which the upstream is told; either is answered `DEADLINE_EXCEEDED`, and
/// neither counts against the upstream.
#[test]
fn timeouts() {
    // By the call's grpc-timeout.
    let up = Upstream::start("h2c");
    let akit = Akit::start(&up, "");
    let start = Instant::now();
    let h = Raw::call(akit.addr(), "/raw.Hang", "", &[("grpc-timeout", "300m")]);
    up.entered();
    let took = start.elapsed();
    assert_eq!(h.get("grpc-status"), Some("4"), "{h:?}");
    assert_eq!(h.get("grpc-message"), Some("akit: the call took too long"));
    assert_eq!(h.get(":status"), Some("200"));
    assert!(took < Duration::from_secs(3), "after {took:?}");
    drop(akit);
    drop(up);

    // By the route's timeout.
    let up = Upstream::start("h2c");
    let akit = Akit::start(&up, r#", "timeout": "2s""#);
    let h = Raw::call(akit.addr(), "/test.Echo/Unary", "deadline", &[]);
    assert_eq!(h.get("grpc-status"), Some("0"), "{h:?}");
    let d = up.deadline().expect("a deadline");
    assert!(
        d > Duration::from_secs(1) && d <= Duration::from_secs(2),
        "the upstream saw a deadline {d:?} away, want about 2s"
    );
    let h = Raw::call(
        akit.addr(),
        "/test.Echo/Unary",
        "deadline",
        &[("grpc-timeout", "5S")],
    );
    let d = up.deadline().expect("a deadline");
    assert_eq!(h.get("grpc-status"), Some("0"), "{h:?}");
    assert!(
        d > Duration::from_secs(1) && d <= Duration::from_secs(2),
        "with a longer grpc-timeout, the upstream saw a deadline {d:?} away, want about 2s"
    );
    let h = Raw::call(akit.addr(), "/raw.Hang", "", &[]);
    up.entered();
    assert_eq!(h.get("grpc-status"), Some("4"), "{h:?}");
    assert_eq!(h.get("grpc-message"), Some("akit: the call took too long"));
    // The upstream is still in rotation.
    let h = Raw::call(akit.addr(), "/test.Echo/Unary", "ok", &[]);
    assert_eq!(h.get("grpc-status"), Some("0"), "after the timeout: {h:?}");
}

/// A call cut short mid-response by its `grpc-timeout` ends with `DEADLINE_EXCEEDED` in its
/// trailers, after the messages that came.
#[test]
fn deadline_mid_response() {
    let up = Upstream::start("h2c");
    let akit = Akit::start(&up, "");
    let r = Raw::open(akit.addr(), "/raw.Cut", &[("grpc-timeout", "500m")]);
    assert!(r.up.write(&frame(&msg("cut", 5))));
    r.up.finish();
    up.entered();
    let h = r.end.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(h.get(":status"), Some("200"));
    assert_eq!(h.get("grpc-status"), Some("4"), "{h:?}");
    assert_eq!(h.get("grpc-message"), Some("akit: the call took too long"));
    let body: Vec<u8> = r.data.try_iter().flatten().collect();
    assert_eq!(body.len(), 2 * frame(&msg("cut", 0)).len());
}

/// What goes upstream: `TE: trailers`, and the route's timeout as the call's `grpc-timeout` when
/// it is the shorter; the upstream's trailers-only answer is passed on as one.
#[test]
fn fields_upstream() {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let up = serve(akio_http::serve(move |ctx: &mut Context| {
        let h = &ctx.req().headers;
        let seen = (
            h.get("te").map(str::to_owned),
            h.get("grpc-timeout").map(str::to_owned),
        );
        let _ = tx.lock().unwrap().send(seen);
        let mut r = akio_http::Response::new(200);
        r.headers.append("content-type", "application/grpc");
        r.headers.append("grpc-status", "7");
        r.headers.append("grpc-message", "no");
        ctx.respond(r);
    }));
    let akit = Akit::to(
        &up.local_addrs()[0].to_string(),
        None,
        r#", "timeout": "3s""#,
    );
    let h = Raw::call(
        akit.addr(),
        "/test.Echo/Unary",
        "x",
        &[("grpc-timeout", "10S")],
    );
    let (te, timeout) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(te.as_deref(), Some("trailers"));
    assert_eq!(timeout.as_deref(), Some("3000000u"));
    assert_eq!(h.get(":status"), Some("200"));
    assert_eq!(h.get("grpc-status"), Some("7"), "{h:?}");
    assert_eq!(h.get("grpc-message"), Some("no"));
    // The client's own, when it is the shorter.
    Raw::call(
        akit.addr(),
        "/test.Echo/Unary",
        "x",
        &[("grpc-timeout", "1S")],
    );
    let (_, timeout) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(timeout.as_deref(), Some("1S"));
    up.shutdown();
}
