//! The access log (I21), as fit's `observe/accesslog_test.go` tests it: the fields of each
//! format, the levels, the buffer handed over when full and on the interval, a writer slower than
//! the traffic (lines dropped and counted, never waited for), closing, files and many threads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use akit_observe::time::Stamp;
use akit_observe::{AccessLog, FLUSH_BYTES, Format, Options, Record, Zone, append_line};

/// What the writer wrote, shared with the test.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    fn lines(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|&&c| c == b'\n')
            .count()
    }
}

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// fit's `TestLine` request: everything a line can have.
fn full() -> Record<'static> {
    Record {
        listener: "web",
        route: "api",
        client: IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        user: "alice",
        proto: "HTTP/1.1",
        method: "POST",
        host: "example.com",
        uri: "/api/x?q=1&s=%22",
        status: 201,
        bytes_in: 4,
        bytes_out: 512,
        duration: Duration::from_millis(1500),
        upstream: Some(("10.0.0.1:8080", Duration::from_millis(1200))),
        user_agent: "agent \"quoted\"\n\x01",
        referer: "https://ref.example/",
    }
}

/// A request no route took, nobody authenticated and nothing proxied.
fn bare(status: u16) -> Record<'static> {
    Record {
        listener: "web",
        route: "",
        client: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
        user: "",
        proto: "HTTP/1.1",
        method: "GET",
        host: "example.com",
        uri: "/",
        status,
        bytes_in: 0,
        bytes_out: 19,
        duration: Duration::from_micros(412),
        upstream: None,
        user_agent: "",
        referer: "",
    }
}

/// 2026-10-19T07:39:56.123Z.
fn at() -> Stamp {
    Stamp::of(UNIX_EPOCH + Duration::from_millis(1_792_395_596_123))
}

fn line(format: Format, r: &Record<'_>, offset: i32) -> String {
    let mut b = Vec::new();
    append_line(&mut b, format, r, at(), offset);
    String::from_utf8(b).unwrap()
}

#[test]
fn json_fields() {
    assert_eq!(
        line(Format::Json, &full(), 0),
        concat!(
            r#"{"time":"2026-10-19T07:39:56.123Z","listener":"web","route":"api","client":"2001:db8::1","#,
            r#""user":"alice","proto":"HTTP/1.1","method":"POST","host":"example.com","#,
            r#""uri":"/api/x?q=1&s=%22","status":201,"bytes_in":4,"bytes_out":512,"duration":1.500000,"#,
            r#""upstream":"10.0.0.1:8080","upstream_duration":1.200000,"#,
            r#""user_agent":"agent \"quoted\"\n\u0001","referer":"https://ref.example/"}"#,
            "\n"
        )
    );
    // Route, user and upstream left out when there are none; the zone's offset on the time.
    assert_eq!(
        line(Format::Json, &bare(404), 8 * 3600),
        concat!(
            r#"{"time":"2026-10-19T15:39:56.123+08:00","listener":"web","client":"192.0.2.1","#,
            r#""proto":"HTTP/1.1","method":"GET","host":"example.com","uri":"/","status":404,"#,
            r#""bytes_in":0,"bytes_out":19,"duration":0.000412,"user_agent":"","referer":""}"#,
            "\n"
        )
    );
}

#[test]
fn text_and_line_fields() {
    let mut r = full();
    r.status = 503;
    r.user = "a=b";
    let fields = concat!(
        r#"listener=web route=api client=2001:db8::1 user="a=b" proto=HTTP/1.1 method=POST "#,
        r#"host=example.com uri="/api/x?q=1&s=%22" status=503 bytes_in=4 bytes_out=512 "#,
        r#"duration=1.500000 upstream=10.0.0.1:8080 upstream_duration=1.200000 "#,
        r#"user_agent="agent \"quoted\"\n\x01" referer=https://ref.example/"#,
        "\n"
    );
    assert_eq!(
        line(Format::Text, &r, 0),
        format!("time=2026-10-19T07:39:56.123Z level=ERROR msg=access {fields}")
    );
    assert_eq!(
        line(Format::Line, &r, 8 * 3600),
        format!("2026-10-19 15:39:56.123 ERROR access {fields}")
    );
    // Levels by status, padded to five in the line format; an empty value quoted.
    for (status, level) in [
        (200, "INFO "),
        (399, "INFO "),
        (404, "WARN "),
        (500, "ERROR"),
    ] {
        let l = line(Format::Line, &bare(status), 0);
        assert!(
            l.starts_with(&format!(
                "2026-10-19 07:39:56.123 {level} access listener=web client=192.0.2.1 "
            )),
            "{l}"
        );
        assert!(l.ends_with(" user_agent=\"\" referer=\"\"\n"), "{l}");
    }
}

fn options(interval: Duration, bytes: usize) -> Options {
    let mut o = Options::new();
    o.flush_interval = interval;
    o.buffer_bytes = bytes;
    o
}

#[test]
fn level_and_zone_change_in_place() {
    let sink = Sink::default();
    let mut o = options(Duration::from_secs(3600), 0);
    o.min_status = 400;
    let log = AccessLog::new(Box::new(sink.clone()), o).unwrap();
    log.log(&bare(200));
    log.log(&bare(404));
    log.set_min_status(0);
    log.log(&bare(200));
    // A zone without daylight time, as a POSIX rule would give one: here a fixed one from TZif.
    if let Ok(z) = Zone::load("Asia/Shanghai") {
        log.set_zone(z);
        log.log(&bare(201));
    }
    log.close();
    let text = sink.text();
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].contains("\"status\":404") && lines[0].contains("Z\","),
        "{text}"
    );
    assert!(lines[1].contains("\"status\":200"), "{text}");
    if let Some(l) = lines.get(2) {
        assert!(
            l.contains("+08:00\",") && l.contains("\"status\":201"),
            "{text}"
        );
    }
    assert_eq!(log.dropped(), 0);
}

#[test]
fn flush_interval() {
    let sink = Sink::default();
    let log = AccessLog::new(
        Box::new(sink.clone()),
        options(Duration::from_millis(20), 0),
    )
    .unwrap();
    log.log(&bare(200));
    let deadline = Instant::now() + Duration::from_secs(5);
    while sink.lines() < 1 {
        assert!(
            Instant::now() < deadline,
            "the interval passed and nothing was written"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    log.close();
}

#[test]
fn flush_when_full() {
    let sink = Sink::default();
    let log = AccessLog::new(
        Box::new(sink.clone()),
        options(Duration::from_secs(3600), 0),
    )
    .unwrap();
    let n = FLUSH_BYTES / line(Format::Json, &bare(200), 0).len() + 1;
    for _ in 0..n {
        log.log(&bare(200));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while sink.lines() < n - 1 {
        assert!(
            Instant::now() < deadline,
            "a full buffer was not written: {} of {n} lines",
            sink.lines()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    log.close();
    assert_eq!(sink.lines(), n);
}

/// An output that blocks every write until the test releases it.
struct Blocking {
    entered: Sender<()>,
    release: Arc<Mutex<Receiver<()>>>,
    out: Sink,
}

impl Write for Blocking {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let _ = self.entered.send(());
        // Released once for good: the sender dropped.
        let _ = self.release.lock().unwrap().recv();
        self.out.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer slower than the traffic: with the output stuck, logging goes on at full speed, and
/// the lines past the buffer are dropped, counted and reported; none is half written.
#[test]
fn slow_output_does_not_block() {
    let (entered_tx, entered) = channel();
    let (release_tx, release_rx) = channel::<()>();
    let out = Sink::default();
    let blocking = Blocking {
        entered: entered_tx,
        release: Arc::new(Mutex::new(release_rx)),
        out: out.clone(),
    };
    let reported = Arc::new(Mutex::new(0u64));
    let mut o = options(Duration::from_millis(10), 16 << 10);
    let r = Arc::clone(&reported);
    o.on_dropped = Box::new(move |n| *r.lock().unwrap() += n);
    o.shards = 4;
    let log = AccessLog::new(Box::new(blocking), o).unwrap();
    log.log(&bare(200));
    // The writer is stuck in its first write.
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = Instant::now();
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let log = log.clone();
            std::thread::spawn(move || {
                for _ in 0..2500 {
                    log.log(&bare(200));
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let took = start.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "10000 lines took {took:?} with the output blocked"
    );
    let dropped = log.dropped();
    assert!(dropped > 0, "a full buffer dropped nothing");
    // What waits is bounded by buffer_bytes (and a line per shard).
    drop(release_tx);
    log.close();
    let written = out.lines() as u64;
    assert_eq!(
        written + dropped,
        10_001,
        "{written} written, {dropped} dropped"
    );
    assert_eq!(
        *reported.lock().unwrap(),
        dropped,
        "the drops were not all reported"
    );
    for l in out.text().lines() {
        assert!(
            l.starts_with("{\"time\":\"") && l.ends_with("\"referer\":\"\"}"),
            "{l}"
        );
    }
    while entered.try_recv().is_ok() {}
}

/// An output that fails: its lines count as dropped.
struct Failing;

impl Write for Failing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn failing_output_counts_drops() {
    let log = AccessLog::new(Box::new(Failing), options(Duration::from_secs(3600), 0)).unwrap();
    for _ in 0..3 {
        log.log(&bare(200));
    }
    log.close();
    assert_eq!(log.dropped(), 3);
}

/// Many threads at once: every line whole, each thread's lines in its order.
#[test]
fn threads_keep_their_order() {
    let sink = Sink::default();
    let log = AccessLog::new(
        Box::new(sink.clone()),
        options(Duration::from_millis(5), 64 << 20),
    )
    .unwrap();
    let threads: Vec<_> = (0..8u16)
        .map(|t| {
            let log = log.clone();
            std::thread::spawn(move || {
                for i in 0..2000u32 {
                    let uri = format!("/{t}/{i}");
                    let mut r = bare(200);
                    r.uri = &uri;
                    log.log(&r);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    log.close();
    assert_eq!(log.dropped(), 0);
    let mut next = [0u32; 8];
    let text = sink.text();
    for l in text.lines() {
        let uri = l
            .split("\"uri\":\"/")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let (t, i) = uri.split_once('/').unwrap();
        let (t, i): (usize, u32) = (t.parse().unwrap(), i.parse().unwrap());
        assert_eq!(i, next[t], "thread {t}'s lines out of order");
        next[t] += 1;
    }
    assert_eq!(next, [2000; 8]);
}

#[test]
fn file_output() {
    let dir = std::env::temp_dir().join(format!("akit-observe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("access.log");
    std::fs::write(&path, "before\n").unwrap();
    let out = akit_observe::open_output(path.to_str().unwrap()).unwrap();
    let log = AccessLog::new(out, options(Duration::from_secs(3600), 0)).unwrap();
    log.log(&bare(200));
    log.close();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(text.starts_with("before\n{\"time\":"), "appended: {text}");
    assert_eq!(text.lines().count(), 2);
    assert!(akit_observe::open_output("/nonexistent-dir-akit/x.log").is_err());
}
