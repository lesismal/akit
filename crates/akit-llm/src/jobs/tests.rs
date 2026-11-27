//! fit's `node_test.go` tests of the jobs, the output buffer and the commands, ported.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use super::*;
use crate::json;
use crate::wire::session::pipe;
use crate::wire::{Event, Handler, Options, Session};

fn sh(script: &str) -> Command {
    Command {
        argv: vec!["sh".into(), "-c".into(), script.into()],
        ..Command::default()
    }
}

fn jobs() -> Jobs {
    Jobs::new(pool("test-kill", 2, 16))
}

struct Serve(Jobs);

impl Handler for Serve {
    fn stream(&self, _s: &Session, st: Stream) {
        let v = json::parse(st.payload()).unwrap();
        self.0.follow(
            json::get_int(&v, "id"),
            usize::try_from(json::get_int(&v, "offset")).unwrap(),
            st,
        );
    }
}

struct Nothing;
impl Handler for Nothing {}

/// Follows job `id` to its end: the output of its frames, and its end.
fn follow(js: &Jobs, id: i64) -> (String, Result<String, Error>) {
    let (d, _l) = pipe(
        Options::default(),
        Arc::new(Nothing),
        Arc::new(Serve(js.clone())),
    );
    let st = d
        .open_stream("/node/job/follow", format!(r#"{{"id":{id}}}"#).as_bytes())
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let s2 = st.clone();
    st.on_data(move |ev| {
        let _ = tx.send(ev);
    });
    let mut out = String::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            Event::Data(b) => {
                s2.release(b.len());
                out.push_str(&json::get_str(&json::parse(&b).unwrap(), "output"));
            }
            Event::End(r) => {
                d.close(Error::Closed);
                return (
                    out,
                    r.map(|()| String::from_utf8(s2.result().unwrap()).unwrap()),
                );
            }
        }
    }
}

fn field(j: &str, k: &str) -> String {
    let v = json::parse(j.as_bytes()).unwrap();
    match v.get(k) {
        Some(akit_config::value::Value::String(s)) => s.clone(),
        Some(v) => json::write(v),
        None => String::new(),
    }
}

/// fit's `TestCommandSudoAndString`.
#[test]
fn command_sudo_and_display() {
    crate::stand_ins::setup();
    let c = Command {
        argv: vec!["sh".into(), "-c".into(), "set -e\necho hi\n".into()],
        env: vec!["A=1".into()],
        root: true,
        ..Command::default()
    };
    assert_eq!(c.display(), "sh -c 'set -e ...'");
    let c2 = Command {
        argv: vec!["ollama".into(), "pull".into(), "a b".into()],
        ..Command::default()
    };
    assert_eq!(c2.display(), "ollama pull 'a b'");
    assert_eq!(c.clone().sudo(false).argv, c.argv);
    let plain = Command {
        argv: vec!["x".into()],
        ..Command::default()
    };
    assert_eq!(plain.clone().sudo(true).argv, plain.argv);
    crate::exec::init_euid();
    if !crate::exec::is_root() {
        assert_eq!(
            c.clone().sudo(true).argv[..3],
            ["sudo", "-n", "--preserve-env=A"]
        );
    }
}

/// fit's `TestOutBuf`.
#[test]
fn out_buf() {
    crate::stand_ins::setup();
    let o = OutBuf::with_limit(100);
    let woke = Arc::new(AtomicUsize::new(0));
    let w2 = Arc::clone(&woke);
    o.on_change(o.generation(), move || {
        w2.fetch_add(1, Ordering::SeqCst);
    });
    o.write("a".repeat(60).as_bytes());
    assert_eq!(woke.load(Ordering::SeqCst), 1, "not woken by a write");
    o.write("b".repeat(60).as_bytes());
    let (s, off, total) = o.from(0);
    assert_eq!(total, 120);
    assert!(s.len() <= 100 && off + s.len() == 120 && s.ends_with(&"b".repeat(60)));
    let (s, off, _) = o.from(110);
    assert_eq!((s.as_str(), off), ("b".repeat(10).as_str(), 110));
    let (s, off, _) = o.from(500);
    assert_eq!(off + s.len(), 120);
    for _ in 0..1000 {
        o.write(b"0123456789");
    }
    let (s, _, total) = o.from(0);
    assert!(s.len() <= 100 && total == 10120 && o.len() == 10120);
    // Never from the middle of a character.
    let u = OutBuf::with_limit(10);
    u.write("中文中文中文".as_bytes());
    let (s, _, _) = u.from(0);
    assert!(s.starts_with('中') || s.starts_with('文'), "{s:?}");
    assert_eq!(u.from(u.len() - 1).0, "文");
    // A change that came already wakes at once.
    let e = u.generation();
    u.notify();
    let (tx, rx) = mpsc::channel();
    u.on_change(e, move || tx.send(()).unwrap());
    rx.try_recv().unwrap();
}

/// fit's `TestJobsBound`: at most MAX_RUNNING jobs run; one more is refused.
#[test]
fn jobs_bound() {
    crate::stand_ins::setup();
    let js = jobs();
    let mut ids = Vec::new();
    for _ in 0..MAX_RUNNING {
        ids.push(
            js.start(
                KIND_ACTION,
                "block",
                Duration::from_secs(60),
                run_command(sh("sleep 30")),
            )
            .unwrap()
            .id,
        );
    }
    assert!(
        js.start(
            KIND_ACTION,
            "one more",
            Duration::from_secs(60),
            run_command(sh("true"))
        )
        .is_err()
    );
    js.cancel_all();
    for id in ids {
        let (_, end) = follow(&js, id);
        assert_eq!(field(&end.unwrap(), "status"), JOB_CANCELED);
    }
}

/// fit's `TestJobsLifecycle`: output followed, exit codes, cancel kills the whole process group,
/// the timeout fails the job.
#[test]
fn jobs_lifecycle() {
    crate::stand_ins::setup();
    let js = jobs();
    let changes = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&changes);
    js.on_change(move || {
        c2.fetch_add(1, Ordering::SeqCst);
    });
    let j = js
        .start(
            KIND_ACTION,
            "echo",
            Duration::from_secs(60),
            run_command(sh("echo one; sleep 0.1; echo two; exit 3")),
        )
        .unwrap();
    assert_eq!((j.status.as_str(), j.id), (JOB_RUNNING, 1));
    assert_eq!(js.running(), 1);
    let (out, end) = follow(&js, j.id);
    let end = end.unwrap();
    assert_eq!(field(&end, "status"), JOB_FAILED);
    assert_eq!(field(&end, "exit_code"), "3");
    assert_eq!(field(&end, "error"), "exit status 3");
    assert!(
        out.starts_with("$ sh -c") && out.contains("one\n") && out.contains("two\n"),
        "{out:?}"
    );
    let g = js.get(j.id, 0).unwrap();
    assert_eq!(g.output, out);
    assert_eq!(g.output_len, i64::try_from(out.len()).unwrap());
    let ok = js
        .start(
            KIND_ACTION,
            "ok",
            Duration::from_secs(60),
            run_command(sh("true")),
        )
        .unwrap();
    let (_, end) = follow(&js, ok.id);
    let end = end.unwrap();
    assert_eq!(
        (
            field(&end, "status").as_str(),
            field(&end, "exit_code").as_str()
        ),
        (JOB_DONE, "0")
    );
    assert!(!end.contains("\"output\""), "{end}");

    // Cancel kills the whole process group, the sleep child too.
    let long = js
        .start(
            KIND_ACTION,
            "sleep",
            Duration::from_secs(60),
            run_command(sh("sleep 30 & wait")),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(js.cancel(long.id));
    let start = Instant::now();
    let (_, end) = follow(&js, long.id);
    let end = end.unwrap();
    assert_eq!(field(&end, "status"), JOB_CANCELED);
    assert_eq!(field(&end, "error"), "canceled");
    assert_eq!(field(&end, "exit_code"), "-1");
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(!js.cancel(long.id), "an ended job canceled");

    let to = js
        .start(
            KIND_ACTION,
            "timeout",
            Duration::from_millis(50),
            run_command(sh("sleep 30")),
        )
        .unwrap();
    let (_, end) = follow(&js, to.id);
    let end = end.unwrap();
    assert_eq!(
        (
            field(&end, "status").as_str(),
            field(&end, "error").as_str()
        ),
        (JOB_FAILED, "timeout")
    );

    let missing = js
        .start(
            KIND_ACTION,
            "missing",
            Duration::from_secs(60),
            run_command(Command {
                argv: vec!["akit-no-such-tool".into()],
                ..Command::default()
            }),
        )
        .unwrap();
    let (_, end) = follow(&js, missing.id);
    assert!(
        field(&end.unwrap(), "error").contains("executable file not found"),
        "a tool not found"
    );

    let l = js.list();
    assert_eq!(l.len(), 5);
    assert_eq!(l[0].id, 5);
    assert!(l.iter().all(|j| j.output.is_empty()));
    assert_eq!(js.running(), 0);
    let (_, end) = follow(&js, 99);
    assert!(matches!(end, Err(Error::Remote { code: 3000, .. })));
    assert!(changes.load(Ordering::SeqCst) > 0, "on_change never ran");
}

/// fit's `TestJobsPrune`.
#[test]
fn jobs_prune() {
    crate::stand_ins::setup();
    let js = jobs();
    for _ in 0..MAX_JOBS + 20 {
        let j = js
            .start(
                KIND_ACTION,
                "noop",
                Duration::from_secs(60),
                Box::new(|_, _| (0, Ok(()))),
            )
            .unwrap();
        let _ = follow(&js, j.id);
    }
    assert!(js.list().len() <= MAX_JOBS);
    assert!(js.get(1, 0).is_none(), "the oldest job kept");
}
