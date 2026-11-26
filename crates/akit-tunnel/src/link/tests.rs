//! The link and its sealing against fit's vectors, and two links over a wire in memory.

use std::sync::mpsc;

use super::*;
use crate::crypt::Coder;

/// fit's `bodycrypt` and `proto.Prove` at 7daee573, printed by a Go test calling them (decision
/// 038): a body sealed by a dialer under a password and under a token, and a hub's proof.
#[test]
fn fits_vectors() {
    let d = [1u8; 16];
    let l = [2u8; 16];
    let header = [0x84, 4, 0, 0, 0, 0];
    let mut s = Coder::new("pw").unwrap().dialer().session(&d, &l).unwrap();
    let mut body = b"hello".to_vec();
    s.seal(&mut body, &header);
    assert_eq!(
        akit_roles::linktoken::hex(&body),
        "dd597cbd43b17ae26759dc742a756bfa0c13105d96"
    );
    let key: Vec<u8> = (0..32).collect();
    let mut s = Coder::token("0123456789abcdef", &key, "pw")
        .dialer()
        .session(&d, &l)
        .unwrap();
    let mut body = b"hello".to_vec();
    s.seal(&mut body, &header);
    assert_eq!(
        akit_roles::linktoken::hex(&body),
        "66cccef13ac3c9f3c0f4bef8c47ff26d584a350afd"
    );
    let req = crate::msg::RegisterReq {
        info: crate::msg::Info {
            name: "lab".into(),
            kind: "node".into(),
            ..crate::msg::Info::default()
        },
        time: 1_700_000_000,
        nonce: "00112233445566778899aabbccddeeff".into(),
        ..crate::msg::RegisterReq::default()
    };
    let mut rsp = crate::msg::RegisterRsp {
        peer: 1,
        conn: 2,
        index: 0,
        hub: "hub".into(),
        proof: String::new(),
    };
    crate::msg::prove(&req, &mut rsp, "pw");
    assert_eq!(
        rsp.proof,
        "c4ee790d6cf86e4ca93e1b6f698742f4564fd996f871a25332e66e963bb4397c"
    );
    assert!(crate::msg::check_proof(&req, &rsp, "pw").is_ok());
    assert!(crate::msg::check_proof(&req, &rsp, "other").is_err());
    // A listener's session opens what the dialer's sealed.
    let mut a = Coder::new("pw").unwrap().dialer().session(&d, &l).unwrap();
    let mut b = Coder::new("pw").unwrap().session(&l, &d).unwrap();
    let mut body = b"x".to_vec();
    a.seal(&mut body, &header);
    assert_eq!(b.open(&mut body, &header).unwrap(), b"x");
    // Replayed: the next number does not open it.
    let mut again = body.clone();
    assert!(b.open(&mut again, &header).is_err());
    assert!(
        Coder::new("pw").unwrap().session(&d, &d).is_err(),
        "reflected"
    );
}

/// A wire in memory: what one side sends, the other's link reads, on a thread of its own.
struct Mem {
    tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    max: AtomicUsize,
}

impl Wire for Mem {
    fn send(&self, b: &[u8]) -> bool {
        lock_tx(&self.tx)
            .as_ref()
            .is_some_and(|t| t.send(b.to_vec()).is_ok())
    }
    fn close(&self) {
        lock_tx(&self.tx).take();
    }
    fn close_after_send(&self) {
        self.close();
    }
    fn peer(&self) -> String {
        "mem".into()
    }
    fn hold_reads(&self, _hold: bool) {}
    fn queued(&self) -> usize {
        0
    }
    fn on_drain(&self, f: Box<dyn FnOnce() + Send>) {
        f();
    }
    fn set_max_queued(&self, n: usize) {
        self.max.store(n, Ordering::Relaxed);
    }
}

fn lock_tx(
    m: &Mutex<Option<mpsc::Sender<Vec<u8>>>>,
) -> MutexGuard<'_, Option<mpsc::Sender<Vec<u8>>>> {
    m.lock().unwrap()
}

/// Echoes calls of method 9, and records notifies.
struct Echo(Mutex<Vec<(u8, Vec<u8>)>>);

impl Router for Echo {
    fn call(&self, _l: &Arc<Link>, method: u8, body: &[u8], r: Responder) {
        if method == 9 {
            r.ok(body);
        } else {
            r.err("no method");
        }
    }
    fn notify(&self, _l: &Arc<Link>, method: u8, body: &[u8]) {
        self.0.lock().unwrap().push((method, body.to_vec()));
    }
    fn closed(&self, _l: &Arc<Link>, _err: &str) {}
}

/// Two links joined in memory, with these coders.
fn pair(a: Option<Coder>, b: Option<Coder>) -> (Arc<Link>, Arc<Link>, Arc<Echo>) {
    let (to_b, from_a) = mpsc::channel::<Vec<u8>>();
    let (to_a, from_b) = mpsc::channel::<Vec<u8>>();
    let echo = Arc::new(Echo(Mutex::new(Vec::new())));
    let la = Link::new(
        Arc::new(Mem {
            tx: Mutex::new(Some(to_b)),
            max: AtomicUsize::new(0),
        }),
        Arc::clone(&echo) as Arc<dyn Router>,
        a,
    );
    let lb = Link::new(
        Arc::new(Mem {
            tx: Mutex::new(Some(to_a)),
            max: AtomicUsize::new(0),
        }),
        Arc::clone(&echo) as Arc<dyn Router>,
        b,
    );
    let (la2, lb2) = (Arc::clone(&la), Arc::clone(&lb));
    std::thread::spawn(move || {
        while let Ok(b) = from_a.recv() {
            lb2.on_data(&b);
        }
    });
    std::thread::spawn(move || {
        while let Ok(b) = from_b.recv() {
            la2.on_data(&b);
        }
    });
    (la, lb, echo)
}

fn call(l: &Arc<Link>, method: u8, body: &[u8]) -> Result<Vec<u8>, CallError> {
    let (tx, rx) = mpsc::channel();
    l.call(
        method,
        body,
        Duration::from_secs(5),
        None,
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    );
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

#[test]
fn calls_and_notifies_plain_and_sealed() {
    for sealed in [false, true] {
        let (a, b) = if sealed {
            let c = Coder::new("pw").unwrap();
            (Some(c.dialer()), Some(c))
        } else {
            (None, None)
        };
        let (la, _lb, echo) = pair(a, b);
        assert_eq!(call(&la, 9, b"ping").unwrap(), b"ping");
        assert_eq!(call(&la, 9, &vec![7u8; 1 << 20]).unwrap().len(), 1 << 20);
        assert_eq!(
            call(&la, 8, b"x"),
            Err(CallError::Remote("no method".into()))
        );
        assert!(la.send_tunnel(33, 5, b"data"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while echo.0.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let got = echo.0.lock().unwrap().clone();
        assert_eq!(got, [(33, [&5u64.to_be_bytes()[..], b"data"].concat())]);
    }
}

#[test]
fn mismatched_sides_fail() {
    // A sealed side and a plain one: the sealed one closes at the plain frame.
    let c = Coder::new("pw").unwrap();
    let (la, _lb, _) = pair(None, Some(c));
    assert!(call(&la, 9, b"x").is_err());
    // Two passwords: the first sealed frame does not open.
    let (la, _lb, _) = pair(
        Some(Coder::new("pw").unwrap().dialer()),
        Some(Coder::new("other").unwrap()),
    );
    assert!(call(&la, 9, b"x").is_err());
}

#[test]
fn frames_out_of_bounds_close_the_link() {
    let (la, lb, _) = pair(None, None);
    lb.set_max_frame(64);
    // A call past the listener's limit closes it: the call fails.
    assert!(call(&la, 9, &[0u8; 100]).is_err());
    assert!(lb.is_closed() || la.is_closed() || call(&la, 9, b"x").is_err());
}

/// A link bounds its wire's queue at fit's `LinkQueue` (A234): its tunnels' windows pace it, and a
/// wire never fails a send they allow.
#[test]
fn a_links_wire_is_bounded_at_the_link_queue() {
    let w = Arc::new(Mem {
        tx: Mutex::new(None),
        max: AtomicUsize::new(0),
    });
    let echo = Arc::new(Echo(Mutex::new(Vec::new())));
    let _l = Link::new(Arc::clone(&w) as Arc<dyn Wire>, echo, None);
    assert_eq!(w.max.load(Ordering::Relaxed), crate::msg::LINK_QUEUE);
    assert_eq!(crate::msg::LINK_QUEUE, 64 * (4 << 20) + (4 << 20));
}
