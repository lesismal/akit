//! fit's `session_test.go`, `flow_test.go`, `edge_test.go` and `async_test.go`, ported: two
//! sessions joined in memory.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::model::CODE_BAD_REQUEST;

const WAIT: Duration = Duration::from_secs(10);

type ReqFn = Box<dyn Fn(&Session, Request, Reply) + Send + Sync>;
type StreamFn = Box<dyn Fn(&Session, Stream) + Send + Sync>;
type PushFn = Box<dyn Fn(&Session, Request) + Send + Sync>;

#[derive(Default)]
pub(crate) struct H {
    pub req: Option<ReqFn>,
    pub stream: Option<StreamFn>,
    pub push: Option<PushFn>,
    pub drain: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Handler for H {
    fn request(&self, s: &Session, req: Request, reply: Reply) {
        match &self.req {
            Some(f) => f(s, req, reply),
            None => reply.answer(Err(not_supported("requests"))),
        }
    }
    fn stream(&self, s: &Session, st: Stream) {
        match &self.stream {
            Some(f) => f(s, st),
            None => st.end(Err(not_supported("streams"))),
        }
    }
    fn push(&self, s: &Session, msg: Request) {
        if let Some(f) = &self.push {
            f(s, msg);
        }
    }
    fn drain(&self, _s: &Session) {
        if let Some(f) = &self.drain {
            f();
        }
    }
}

fn opts() -> Options {
    Options::default()
}

fn pair(l: H) -> (Session, Session) {
    pipe(opts(), Arc::new(H::default()), Arc::new(l))
}

fn call(s: &Session, method: &str, payload: &[u8], timeout: Duration) -> Result<Vec<u8>, Error> {
    let (tx, rx) = mpsc::channel();
    s.call(method, payload, timeout, move |r| {
        let _ = tx.send(r);
    });
    rx.recv_timeout(WAIT).expect("the call settles")
}

type EndFn = Box<dyn FnOnce(&Stream) + Send>;

/// Sends `chunks` on `st` as credit allows, then runs `end`.
pub(crate) fn send_all(
    st: Stream,
    chunks: VecDeque<Vec<u8>>,
    end: impl FnOnce(&Stream) + Send + 'static,
) {
    struct S {
        st: Stream,
        q: Mutex<(VecDeque<Vec<u8>>, Option<EndFn>)>,
    }
    fn go(s: &Arc<S>) {
        loop {
            let mut g = s.q.lock().unwrap();
            let Some(c) = g.0.pop_front() else {
                let end = g.1.take();
                drop(g);
                if let Some(e) = end {
                    e(&s.st);
                }
                return;
            };
            drop(g);
            match s.st.try_send(&c) {
                Ok(true) => {}
                Ok(false) => {
                    s.q.lock().unwrap().0.push_front(c);
                    let t = Arc::clone(s);
                    if !s.st.on_credit(move || go(&t)) {
                        continue;
                    }
                    return;
                }
                Err(_) => return,
            }
        }
    }
    go(&Arc::new(S {
        st,
        q: Mutex::new((chunks, Some(Box::new(end)))),
    }));
}

/// Reads `st` whole, giving back what it reads.
pub(crate) fn read_all(st: &Stream) -> Receiver<(Vec<u8>, Result<(), Error>)> {
    let (tx, rx) = mpsc::channel();
    let buf = Mutex::new(Vec::new());
    let s = st.clone();
    let tx = Mutex::new(Some(tx));
    st.on_data(move |ev| match ev {
        Event::Data(d) => {
            buf.lock().unwrap().extend_from_slice(&d);
            s.release(d.len());
        }
        Event::End(r) => {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send((std::mem::take(&mut *buf.lock().unwrap()), r));
            }
        }
    });
    rx
}

pub(crate) fn chunks(n: usize, size: usize) -> VecDeque<Vec<u8>> {
    (0..n).map(|i| vec![(i % 251) as u8; size]).collect()
}

#[test]
fn request_response() {
    let (d, _l) = pair(H {
        req: Some(Box::new(|_, req, reply| match req.method.as_str() {
            "/echo" => reply.answer(Ok(req.payload)),
            "/later" => {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(50));
                    reply.answer(Ok(b"later".to_vec()));
                });
            }
            "/drop" => drop(reply),
            _ => reply.answer(Err(Error::code(CODE_BAD_REQUEST, "bad"))),
        })),
        ..H::default()
    });
    assert_eq!(call(&d, "/echo", b"hi", WAIT).unwrap(), b"hi");
    assert_eq!(call(&d, "/later", b"", WAIT).unwrap(), b"later");
    assert_eq!(
        call(&d, "/other", b"", WAIT),
        Err(Error::code(CODE_BAD_REQUEST, "bad"))
    );
    assert_eq!(call(&d, "/drop", b"", WAIT).unwrap_err().code_msg().0, 5000);
    let big = vec![7u8; 3 << 20];
    assert_eq!(call(&d, "/echo", &big, WAIT).unwrap(), big);
    assert_eq!(d.inflight(), 0);
    let (d2, _l2) = pair(H::default());
    assert_eq!(
        call(&d2, "/x", b"", WAIT),
        Err(Error::code(3000, "requests not supported"))
    );
}

#[test]
fn call_timeout_reaches_the_handler() {
    let (tx, rx) = mpsc::channel::<Reply>();
    let tx = Mutex::new(tx);
    let (d, l) = pair(H {
        req: Some(Box::new(move |_, _, reply| {
            let _ = tx.lock().unwrap().send(reply);
        })),
        ..H::default()
    });
    assert_eq!(
        call(&d, "/slow", b"", Duration::from_millis(100)),
        Err(Error::Timeout)
    );
    let reply = rx.recv_timeout(WAIT).unwrap();
    for _ in 0..200 {
        if reply.canceled() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(reply.canceled(), "the cancel reached the handler");
    reply.answer(Ok(b"late".to_vec())); // dropped
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(l.inflight(), 0);
    assert_eq!(d.inflight(), 0);
}

pub(crate) fn streamer() -> H {
    H {
        stream: Some(Box::new(|_, st| match st.method() {
            "/down" => {
                let n: usize = String::from_utf8_lossy(st.payload()).parse().unwrap();
                send_all(st.clone(), chunks(n, 16 << 10), |st| {
                    st.end(Ok(b"done".to_vec()));
                });
            }
            "/echo" => {
                let s = st.clone();
                let total = Arc::new(AtomicUsize::new(0));
                st.on_data(move |ev| match ev {
                    Event::Data(d) => {
                        total.fetch_add(d.len(), Ordering::Relaxed);
                        let _ = s.try_send(&d);
                        s.release(d.len());
                    }
                    Event::End(_) => {
                        s.end(Ok(total.load(Ordering::Relaxed).to_string().into_bytes()));
                    }
                });
            }
            "/fail" => st.end(Err(Error::code(4003, "no node"))),
            "/hold" => {} // never ends: the idle timeout or a cancel does
            _ => st.end(Err(not_supported("streams"))),
        })),
        ..H::default()
    }
}

#[test]
fn streams_flow_under_credit() {
    let (d, _l) = pair(streamer());
    // 4 MiB through a 256 KiB window: the credit turns many times.
    let st = d.open_stream("/down", b"256").unwrap();
    let (data, r) = read_all(&st).recv_timeout(WAIT).unwrap();
    assert_eq!(r, Ok(()));
    assert_eq!(data.len(), 256 * (16 << 10));
    assert_eq!(st.result().unwrap(), b"done");
    assert_eq!(st.bytes().0, 256 * (16 << 10));
    // Errors keep their code.
    let st = d.open_stream("/fail", b"").unwrap();
    let (_, r) = read_all(&st).recv_timeout(WAIT).unwrap();
    assert_eq!(r, Err(Error::code(4003, "no node")));
    let st = d.open_stream("/nope", b"").unwrap();
    let (_, r) = read_all(&st).recv_timeout(WAIT).unwrap();
    assert_eq!(r.unwrap_err().code_msg().0, 3000);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(d.inflight(), 0);
}

#[test]
fn concurrent_streams_and_half_close() {
    let (d, _l) = pair(streamer());
    let mut waits = Vec::new();
    for _ in 0..8 {
        let st = d.open_stream("/echo", b"").unwrap();
        let rx = read_all(&st);
        send_all(st, chunks(64, 8 << 10), move |s2| {
            let _ = s2.close_send();
        });
        waits.push(rx);
    }
    for rx in waits {
        let (data, r) = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(r, Ok(()));
        assert_eq!(data.len(), 64 * (8 << 10));
    }
}

#[test]
fn a_slow_stream_holds_itself_only() {
    let (d, _l) = pair(streamer());
    // A reader that never gives back: its sender stops at the window.
    let slow = d.open_stream("/down", b"100").unwrap();
    let got = Arc::new(AtomicUsize::new(0));
    let g = Arc::clone(&got);
    slow.on_data(move |ev| {
        if let Event::Data(d) = ev {
            g.fetch_add(d.len(), Ordering::Relaxed);
        }
    });
    let fast = d.open_stream("/down", b"64").unwrap();
    let (data, r) = read_all(&fast).recv_timeout(WAIT).unwrap();
    assert_eq!(r, Ok(()));
    assert_eq!(data.len(), 64 * (16 << 10));
    std::thread::sleep(Duration::from_millis(100));
    let n = got.load(Ordering::Relaxed);
    assert!(
        n <= DEFAULT_WINDOW + (16 << 10),
        "{n} bytes past the window"
    );
    assert!(n >= DEFAULT_WINDOW / 2, "{n}");
    slow.release(n);
    slow.cancel();
}

#[test]
fn cancel_idle_and_close() {
    let (tx, rx) = mpsc::channel::<Option<Error>>();
    let tx = Mutex::new(tx);
    let mut o = opts();
    o.stream_idle = Duration::from_millis(300);
    let (d, _l) = pipe(
        o,
        Arc::new(H::default()),
        Arc::new(H {
            stream: Some(Box::new(move |_, st| {
                let tx = Mutex::new(tx.lock().unwrap().clone());
                st.on_finish(move |e| {
                    let _ = tx.lock().unwrap().send(e.cloned());
                });
            })),
            ..H::default()
        }),
    );
    // The opener cancels: the responder hears it.
    let st = d.open_stream("/hold", b"").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    st.cancel();
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), Some(Error::Canceled));
    // Idle: both ends end.
    let st = d.open_stream("/hold", b"").unwrap();
    let r = read_all(&st);
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), Some(Error::StreamIdle));
    assert!(r.recv_timeout(WAIT).unwrap().1.is_err());
    // A keep-alive holds it open past the idle timeout.
    let st = d.open_stream("/hold", b"").unwrap();
    let stop = st.keep_alive(Duration::from_millis(80));
    std::thread::sleep(Duration::from_millis(700));
    assert!(!st.is_finished());
    stop.store(true, Ordering::Release);
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), Some(Error::StreamIdle));
    // Close fails what is pending.
    let st = d.open_stream("/hold", b"").unwrap();
    let r = read_all(&st);
    let (ctx, crx) = mpsc::channel();
    d.call("/x", b"", Duration::ZERO, move |r| {
        let _ = ctx.send(r);
    });
    d.close(Error::Closed);
    assert_eq!(r.recv_timeout(WAIT).unwrap().1, Err(Error::Closed));
    assert!(crx.recv_timeout(WAIT).unwrap().is_err());
    assert!(d.open_stream("/hold", b"").is_err());
    assert_eq!(d.push("/p", b""), Err(Error::Closed));
}

#[test]
fn pushes_in_order_and_pings() {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let tx = Mutex::new(tx);
    let (d, _l) = pair(H {
        push: Some(Box::new(move |_, m| {
            assert_eq!(m.method, "/p");
            let _ = tx.lock().unwrap().send(m.payload);
        })),
        ..H::default()
    });
    for i in 0..500u32 {
        d.push("/p", &i.to_be_bytes()).unwrap();
    }
    for i in 0..500u32 {
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), i.to_be_bytes());
    }
    d.ping().unwrap();
    for _ in 0..200 {
        if d.rtt().samples > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(d.rtt().samples, 1);
    assert_eq!(d.unanswered(), Duration::ZERO);
    let (rxb, txb) = d.traffic();
    assert!(rxb > 0 && txb > 0);
}

#[test]
fn drain_and_max_inbound() {
    let drained = Arc::new(AtomicUsize::new(0));
    let dr = Arc::clone(&drained);
    let held: Arc<Mutex<Vec<Reply>>> = Arc::default();
    let h2 = Arc::clone(&held);
    let mut o = opts();
    o.max_inbound = 2;
    let (d, l) = pipe(
        o,
        Arc::new(H::default()),
        Arc::new(H {
            req: Some(Box::new(move |_, _, r| h2.lock().unwrap().push(r))),
            drain: Some(Box::new(move || {
                dr.fetch_add(1, Ordering::Relaxed);
            })),
            ..H::default()
        }),
    );
    let (tx, rx) = mpsc::channel();
    for _ in 0..2 {
        let tx = tx.clone();
        d.call("/x", b"", Duration::ZERO, move |r| {
            let _ = tx.send(r);
        });
    }
    // The third is refused at once.
    assert_eq!(
        call(&d, "/x", b"", WAIT).unwrap_err().code_msg().0,
        CODE_RATE_LIMITED
    );
    for r in held.lock().unwrap().drain(..) {
        r.answer(Ok(Vec::new()));
    }
    assert!(rx.recv_timeout(WAIT).unwrap().is_ok());
    assert!(rx.recv_timeout(WAIT).unwrap().is_ok());
    d.drain();
    d.drain();
    for _ in 0..200 {
        if drained.load(Ordering::Relaxed) > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(drained.load(Ordering::Relaxed), 1);
    assert!(d.draining() && l.draining() && l.peer_draining() && !d.peer_draining());
}

#[test]
fn hold_streams_and_on_credit() {
    let (d, _l) = pair(streamer());
    let st = d.open_stream("/echo", b"").unwrap();
    d.hold_streams(true);
    assert!(!st.credit());
    assert_eq!(st.try_send(b"x"), Ok(false));
    let (tx, rx) = mpsc::channel();
    assert!(st.on_credit(move || {
        let _ = tx.send(());
    }));
    d.hold_streams(false);
    rx.recv_timeout(WAIT).unwrap();
    assert_eq!(st.try_send(b"x"), Ok(true));
    st.cancel();
    assert!(!st.on_credit(|| {}));
    assert!(st.try_send(b"x").is_err());
}

/// A session fed by hand: what it sends is captured.
fn raw(dialer: bool, h: H) -> (Session, Arc<Mutex<Vec<Vec<u8>>>>) {
    let out: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let o = Arc::clone(&out);
    let s = Session::new(Options { dialer, ..opts() }, move |b| {
        o.lock().unwrap().push(b);
        Ok(())
    });
    s.set_handler(Arc::new(h));
    (s, out)
}

fn f(kind: Type, flags: u8, id: u32, method: &str, payload: &[u8]) -> Vec<u8> {
    frame::marshal(kind, flags, id, method, payload, 0, 0).unwrap()
}

#[test]
fn peer_protocol_errors() {
    // A listener takes odd IDs from its dialer.
    let (s, _) = raw(false, H::default());
    assert!(matches!(
        s.receive(&f(Type::Request, 0, 2, "/x", b"")),
        Err(Error::Protocol(_))
    ));
    assert!(s.is_closed());
    let (s, _) = raw(false, streamer());
    s.receive(&f(Type::StreamOpen, 0, 1, "/hold", b"")).unwrap();
    assert!(
        s.receive(&f(Type::StreamOpen, 0, 1, "/hold", b"")).is_err(),
        "in use"
    );
    let (s, _) = raw(false, streamer());
    s.receive(&f(Type::StreamOpen, 0, 1, "/hold", b"")).unwrap();
    assert!(
        s.receive(&f(Type::Credit, 0, 1, "", b"abc")).is_err(),
        "bad credit"
    );
    assert!(
        raw(false, H::default())
            .0
            .receive(&[99, 0, 0, 0, 0, 0])
            .is_err()
    );
    // Frames of ended streams are dropped.
    let (s, _) = raw(false, H::default());
    s.receive(&f(Type::StreamData, 0, 5, "", b"late")).unwrap();
    s.receive(&f(Type::Response, 0, 5, "", b"late")).unwrap();
}

#[test]
fn flow_control_violation() {
    let (s, out) = raw(false, streamer());
    s.receive(&f(Type::StreamOpen, 0, 1, "/hold", b"")).unwrap();
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..17 {
        s.receive(&f(Type::StreamData, 0, 1, "", &chunk)).unwrap();
    }
    // Past the window and a frame: ended with an error for the peer.
    let last = out.lock().unwrap().last().cloned().unwrap();
    let fr = frame::unmarshal(&last, 0).unwrap();
    assert_eq!(fr.kind, Type::StreamEnd);
    assert_eq!(fr.flags & FLAG_ERROR, FLAG_ERROR);
    assert!(String::from_utf8_lossy(&fr.payload).contains("window"));
    assert_eq!(s.inflight(), 0);
}

#[test]
fn ids_wrap_and_skip() {
    let (s, _) = raw(true, H::default());
    s.0.st.lock().unwrap().next_id = u32::MAX;
    let a = s.open_stream("/a", b"").unwrap();
    assert_eq!(a.id(), u32::MAX);
    let b = s.open_stream("/b", b"").unwrap();
    assert_eq!(b.id(), 1);
}

#[test]
fn pump_holds_the_writer_to_the_reader() {
    // The listener relays "/relay" streams into "/sink" streams it opens back on the dialer,
    // whose reader is slow: the writer behind the relay is held, and everything arrives.
    let (stx, srx) = mpsc::channel::<Stream>();
    let stx: Mutex<Sender<Stream>> = Mutex::new(stx);
    let dialer = H {
        stream: Some(Box::new(move |_, st| {
            let _ = stx.lock().unwrap().send(st);
        })),
        ..H::default()
    };
    let listener = H {
        stream: Some(Box::new(|s, st| {
            let out = s.open_stream("/sink", b"").unwrap();
            let src = st.clone();
            let o2 = out.clone();
            pump(&st, &out, None, move |r, _| {
                assert_eq!(r, Ok(()));
                let _ = o2.close_send();
                src.end(Ok(Vec::new()));
            });
        })),
        ..H::default()
    };
    let (d, _l) = pipe(opts(), Arc::new(dialer), Arc::new(listener));
    let relay = d.open_stream("/relay", b"").unwrap();
    let relay_end = read_all(&relay);
    let sent = Arc::new(AtomicUsize::new(0));
    let s3 = Arc::clone(&sent);
    send_all(relay.clone(), chunks(128, 16 << 10), move |st| {
        s3.store(1, Ordering::Release);
        let _ = st.close_send();
    });
    let sink = srx.recv_timeout(WAIT).unwrap();
    // Nobody reads the sink yet: the writer is held at two windows and a frame or two.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sent.load(Ordering::Acquire), 0, "the writer is held");
    let total = Arc::new(AtomicUsize::new(0));
    let t2 = Arc::clone(&total);
    let (etx, erx) = mpsc::channel();
    let sk = sink.clone();
    sink.on_data(move |ev| match ev {
        Event::Data(b) => {
            t2.fetch_add(b.len(), Ordering::Relaxed);
            sk.release(b.len());
        }
        Event::End(r) => {
            let _ = etx.send(r);
        }
    });
    assert_eq!(erx.recv_timeout(WAIT).unwrap(), Ok(()));
    sink.end(Ok(Vec::new()));
    assert_eq!(total.load(Ordering::Relaxed), 128 * (16 << 10));
    assert_eq!(relay_end.recv_timeout(WAIT).unwrap().1, Ok(()));
}
