//! fit's `provider/internal/watch` tests: `TestFollower`, `TestFollowerInterval`,
//! `TestFollowerWatchEnds` and `TestFollowerReadFailsAfterChange`.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Conn, Events, Follower, Running, Source, Stop};
use crate::testing::{Lines, Parts, short};

#[derive(Default)]
struct State {
    part: String,
    problems: Vec<String>,
    read_err: Option<String>,
    open_err: Option<String>,
    opens: usize,
    reads: usize,
}

/// A source whose watch the test drives.
#[derive(Clone)]
struct Fake {
    st: Arc<Mutex<State>>,
    /// Each watch, once it is in place.
    watching: SyncSender<FakeWatch>,
    /// The watch ends at once, without an error.
    ends: bool,
}

/// A watch the test drives: changes it reports, and an error that ends it.
struct FakeWatch {
    ev: Events,
    fail: SyncSender<String>,
}

impl FakeWatch {
    fn changed(&self) {
        self.ev.changed();
    }
}

impl Fake {
    fn new(part: &str) -> (Fake, Receiver<FakeWatch>) {
        let (tx, rx) = mpsc::sync_channel(16);
        let st = State {
            part: part.to_owned(),
            ..State::default()
        };
        (
            Fake {
                st: Arc::new(Mutex::new(st)),
                watching: tx,
                ends: false,
            },
            rx,
        )
    }

    fn set(&self, part: &str, problems: &[&str]) {
        let mut st = self.st.lock().unwrap();
        part.clone_into(&mut st.part);
        st.problems = problems.iter().map(|p| (*p).to_owned()).collect();
    }

    fn count(&self) -> (usize, usize) {
        let st = self.st.lock().unwrap();
        (st.opens, st.reads)
    }
}

impl Source for Fake {
    type Conn = Fake;

    fn open(&mut self, _stop: &Stop) -> Result<Arc<Fake>, String> {
        let mut st = self.st.lock().unwrap();
        st.opens += 1;
        match &st.open_err {
            Some(e) => Err(e.clone()),
            None => Ok(Arc::new(self.clone())),
        }
    }
}

impl Conn for Fake {
    fn watch(&self, stop: &Stop, ev: &Events) -> Result<(), String> {
        ev.ready();
        if self.ends {
            return Ok(());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let _ = self.watching.try_send(FakeWatch {
            ev: ev.clone(),
            fail: tx.clone(),
        });
        stop.on_cancel(move || {
            let _ = tx.try_send("stopped".to_owned());
        });
        Err(rx.recv().unwrap_or_default())
    }

    fn read(&self, _stop: &Stop) -> Result<(Vec<u8>, Vec<String>), String> {
        let mut st = self.st.lock().unwrap();
        st.reads += 1;
        match &st.read_err {
            Some(e) => Err(e.clone()),
            None => Ok((st.part.clone().into_bytes(), st.problems.clone())),
        }
    }
}

fn follower(src: Fake, interval: Duration, lines: &Lines) -> Follower<Fake> {
    let mut f = Follower::new("p", interval, lines.log(), src);
    f.timings = short();
    f
}

fn next(parts: &Parts) -> String {
    String::from_utf8(parts.next(|_| true)).unwrap()
}

#[test]
fn follower_follows() {
    let (src, watching) = Fake::new("one");
    src.set("one", &["bad key"]);
    let lines = Lines::default();
    let (parts, update) = Parts::new("p");
    let mut r = Running::start(
        follower(src.clone(), Duration::from_secs(3600), &lines),
        update,
    )
    .unwrap();

    assert_eq!(next(&parts), "one");
    let w = watching.recv_timeout(Duration::from_secs(10)).unwrap();
    // Several changes make one read, and a part.
    src.set("two", &["bad key"]);
    let (_, before) = src.count();
    for _ in 0..5 {
        w.changed();
    }
    assert_eq!(next(&parts), "two");
    let (_, after) = src.count();
    assert_eq!(
        after,
        before + 1,
        "five changes made {} reads",
        after - before
    );
    // A change that changes nothing sends nothing.
    w.changed();
    assert!(
        parts.rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "an unchanged read sent a part"
    );

    // The watch fails: the follower reports it, watches again and sends the part at once,
    // unchanged as it is.
    w.fail.send("lost".to_owned()).unwrap();
    assert_eq!(next(&parts), "two");
    let w = watching.recv_timeout(Duration::from_secs(10)).unwrap();

    // The source fails to open: tried again until it opens.
    src.st.lock().unwrap().open_err = Some("refused".to_owned());
    w.fail.send("lost again".to_owned()).unwrap();
    lines.wait("provider p: refused; trying again");
    {
        let mut st = src.st.lock().unwrap();
        st.open_err = None;
        "three".clone_into(&mut st.part);
    }
    assert_eq!(next(&parts), "three");

    r.stop();
    let all = lines.all();
    assert_eq!(
        all.matches("provider p: bad key").count(),
        1,
        "a lasting problem is reported once:\n{all}"
    );
    for want in [
        "provider p: watch: lost; trying again",
        "provider p: refused; trying again",
    ] {
        assert!(all.contains(want), "logs lack {want:?}:\n{all}");
    }
    // Stopped: no part after.
    let ups = parts.ups.clone();
    drop(ups);
}

/// The part is sent every interval whether or not it changed, and a failed read starts the
/// watch over.
#[test]
fn follower_interval() {
    let (src, _watching) = Fake::new("p");
    let lines = Lines::default();
    let (parts, update) = Parts::new("p");
    let _r = Running::start(
        follower(src.clone(), Duration::from_millis(20), &lines),
        update,
    )
    .unwrap();
    for _ in 0..4 {
        assert_eq!(next(&parts), "p");
    }
    src.st.lock().unwrap().read_err = Some("read failed".to_owned());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while src.count().0 < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "the watch was not started over:\n{}",
            lines.all()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    lines.wait("provider p: read failed; trying again");
}

#[test]
fn follower_watch_ends() {
    let (mut src, _watching) = Fake::new("p");
    src.ends = true;
    let lines = Lines::default();
    let (_parts, update) = Parts::new("p");
    let _r = Running::start(follower(src, Duration::from_secs(3600), &lines), update).unwrap();
    lines.wait("provider p: watch: the watch ended; trying again");
}

/// A read after a change that fails starts the watch over.
#[test]
fn follower_read_fails_after_change() {
    let (src, watching) = Fake::new("p");
    let lines = Lines::default();
    let (parts, update) = Parts::new("p");
    let _r = Running::start(
        follower(src.clone(), Duration::from_secs(3600), &lines),
        update,
    )
    .unwrap();
    assert_eq!(next(&parts), "p");
    let w = watching.recv_timeout(Duration::from_secs(10)).unwrap();
    src.st.lock().unwrap().read_err = Some("read failed".to_owned());
    w.changed();
    lines.wait("provider p: read failed; trying again");
}

/// Stopping returns at once, the watch's thread joined, though the source never fails.
#[test]
fn stop_returns() {
    let (src, watching) = Fake::new("p");
    let lines = Lines::default();
    let (parts, update) = Parts::new("p");
    let mut r = Running::start(follower(src, Duration::from_secs(3600), &lines), update).unwrap();
    assert_eq!(next(&parts), "p");
    let _w = watching.recv_timeout(Duration::from_secs(10)).unwrap();
    let t = std::time::Instant::now();
    r.stop();
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn stops() {
    let s = Stop::new();
    let c = s.child();
    assert!(s.sleep(Duration::from_millis(1)));
    let hooked = Arc::new(Mutex::new(false));
    {
        let h = Arc::clone(&hooked);
        c.on_cancel(move || *h.lock().unwrap() = true);
    }
    s.cancel();
    assert!(c.is_stopped() && *hooked.lock().unwrap());
    assert!(!c.sleep(Duration::from_secs(10)));
    assert!(s.child().is_stopped());
    assert!(s.token().is_cancelled());
}
