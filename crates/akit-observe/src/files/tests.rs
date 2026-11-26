//! The sink, as fit's `logs/file_test.go` and `file_link_test.go` test it.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akit_config::Rotation;

use super::{Files, period_start, rotated_suffix};
use crate::zone::Zone;

/// A directory of the test's own, removed when it ends.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!(
            "akit-files-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).display().to_string()
    }
    /// The names in the directory, sorted.
    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A clock the test moves, in milliseconds since the epoch.
#[derive(Clone)]
struct Clock(Arc<AtomicI64>);

impl Clock {
    fn at(ms: i64) -> Clock {
        Clock(Arc::new(AtomicI64::new(ms)))
    }
    fn add(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn set(&self, t: SystemTime) {
        let ms = t.duration_since(UNIX_EPOCH).unwrap().as_millis();
        self.0.store(i64::try_from(ms).unwrap(), Ordering::SeqCst);
    }
    fn files(&self) -> (Files, Arc<Mutex<Vec<String>>>) {
        let c = Arc::clone(&self.0);
        let reported = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::clone(&reported);
        let files = Files::with(
            Arc::new(move || {
                UNIX_EPOCH + Duration::from_millis(u64::try_from(c.load(Ordering::SeqCst)).unwrap())
            }),
            Duration::from_secs(3600),
            Arc::new(move |m: &str| r.lock().unwrap().push(m.to_owned())),
        );
        (files, reported)
    }
}

fn rotation(max_bytes: u64) -> Rotation {
    Rotation {
        max_bytes,
        interval: Duration::ZERO,
        max_age: Duration::ZERO,
        max_files: 0,
        max_total_bytes: 0,
        compress: false,
    }
}

/// 2026-11-05 03:59:59 UTC, in milliseconds.
const T0: i64 = 1_793_851_199_000;

fn rotated(d: &Dir, base: &str) -> Vec<String> {
    d.names()
        .into_iter()
        .filter(|n| n == base || n.starts_with(&format!("{base}.")))
        .collect()
}

fn all_lines(d: &Dir) -> Vec<String> {
    let mut lines = Vec::new();
    for n in d.names() {
        let p = d.0.join(&n);
        let data = fs::read(&p).unwrap();
        let data = if n.ends_with(".gz") {
            akio_deflate::decompress(akio_deflate::Format::Gzip, &data, 1 << 26).unwrap()
        } else {
            data
        };
        let text = String::from_utf8(data).unwrap();
        assert!(text.is_empty() || text.ends_with('\n'), "{n} ends mid-line");
        lines.extend(text.lines().map(str::to_owned));
    }
    lines
}

#[test]
fn rotates_by_size() {
    let d = Dir::new("size");
    let clock = Clock::at(T0);
    let (files, _) = clock.files();
    let f = files
        .open(&d.path("a.log"), Some(rotation(100)), Zone::utc())
        .unwrap();
    for i in 0..30 {
        f.write(format!("line {i:04} xxxxxxxxxxxxxxxxxxxx\n").as_bytes())
            .unwrap();
        clock.add(1);
    }
    // A line larger than max_bytes gets a file of its own.
    let mut big = vec![b'y'; 299];
    big.push(b'\n');
    f.write(&big).unwrap();
    f.sync().unwrap();
    let r = rotated(&d, "a.log");
    assert!(r.len() >= 8, "{r:?}");
    for n in &r {
        let size = fs::metadata(d.0.join(n)).unwrap().len();
        assert!(size <= 100 || size == 300, "{n}: {size}");
        assert!(
            n == "a.log" || rotated_suffix(n.strip_prefix("a.log.").unwrap()),
            "{n}"
        );
    }
    drop(f);
    let lines = all_lines(&d);
    for i in 0..30 {
        let want = format!("line {i:04} xxxxxxxxxxxxxxxxxxxx");
        assert_eq!(lines.iter().filter(|l| **l == want).count(), 1, "{want}");
    }
}

#[test]
fn rotates_at_each_period_in_the_zone() {
    let d = Dir::new("interval");
    // One second before 04:00 UTC, which is midnight at UTC-4.
    let clock = Clock::at(T0);
    let (files, _) = clock.files();
    let mut r = rotation(0);
    r.interval = Duration::from_secs(24 * 3600);
    let minus4 = Zone::fixed("minus4", -4 * 3600);
    let f = files.open(&d.path("a.log"), Some(r), minus4).unwrap();
    f.write(b"before midnight\n").unwrap();
    f.sync().unwrap();
    clock.add(2000);
    f.write(b"after midnight\n").unwrap();
    f.sync().unwrap();
    let names = rotated(&d, "a.log");
    // The rotation's time in UTC names the file.
    assert_eq!(names, ["a.log", "a.log.2026-11-05T04-00-01.000"]);
    assert_eq!(
        fs::read_to_string(d.0.join(&names[1])).unwrap(),
        "before midnight\n"
    );
    // In UTC the day began at 00:00 UTC: a new zone starts a new period, and then the day goes
    // on.
    f.set_zone(Zone::utc());
    clock.add(1000);
    f.write(b"in UTC\n").unwrap();
    clock.add(3600 * 1000);
    f.write(b"same day in UTC\n").unwrap();
    f.sync().unwrap();
    assert_eq!(rotated(&d, "a.log").len(), 3);
}

#[test]
fn period_starts() {
    let at = |ms: i64| UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap());
    let ns = |ms: i64| i128::from(ms) * 1_000_000;
    let hour = Duration::from_secs(3600);
    let day = 24 * hour;
    // 03:59:59 UTC: the day began at midnight UTC; at UTC+4 it is 07:59:59, the day began
    // at 20:00 UTC the day before.
    assert_eq!(
        period_start(at(T0), day, &Zone::utc()),
        ns(T0 - (4 * 3600 - 1) * 1000)
    );
    let plus4 = Zone::fixed("p", 4 * 3600);
    assert_eq!(
        period_start(at(T0), day, &plus4),
        ns(T0 - (8 * 3600 - 1) * 1000)
    );
    assert_eq!(period_start(at(T0), hour, &plus4), ns(T0 - 3_599_000));
    // A quarter of an hour, at UTC-5:30.
    let minus = Zone::fixed("m", -(5 * 3600 + 1800));
    assert_eq!(
        period_start(at(T0), Duration::from_secs(900), &minus),
        ns(T0 - 899_000)
    );
    // Two days group days from 1970-01-01: 2026-11-05 is day 20762, even, so its period began
    // that midnight, and the next day's the same.
    assert_eq!(
        period_start(at(T0), 2 * day, &Zone::utc()),
        ns(T0 - (4 * 3600 - 1) * 1000)
    );
    assert_eq!(
        period_start(at(T0 + 86_400_000), 2 * day, &Zone::utc()),
        ns(T0 - (4 * 3600 - 1) * 1000)
    );
}

#[test]
fn retention_by_age_count_and_total() {
    let d = Dir::new("retention");
    let clock = Clock::at(T0);
    let (files, _) = clock.files();
    // Files the sink did not make stay, whatever their names look like.
    fs::write(d.0.join("a.log.old"), "x").unwrap();
    fs::write(d.0.join("b.log.2026-01-01T00-00-00.000"), "x").unwrap();
    let mut r = rotation(10);
    r.max_files = 3;
    let f = files.open(&d.path("a.log"), Some(r), Zone::utc()).unwrap();
    for i in 0..6 {
        f.write(format!("line {i:02} xx\n").as_bytes()).unwrap();
        f.sync().unwrap();
        clock.add(1000);
    }
    f.write(b"last\n").unwrap();
    f.sync().unwrap();
    assert_eq!(rotated(&d, "a.log").len(), 1 + 1 + 3, "{:?}", d.names());
    assert!(
        d.names()
            .contains(&"b.log.2026-01-01T00-00-00.000".to_owned())
    );
    // By total: two of 10 bytes fit in 25.
    let mut r = rotation(10);
    r.max_total_bytes = 25;
    f.set_rotation(Some(r));
    let removed = f.cleanup_now(true).unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(rotated(&d, "a.log").len(), 5, "a dry run removes nothing");
    assert_eq!(f.cleanup_now(false).unwrap(), removed);
    assert_eq!(rotated(&d, "a.log").len(), 4);
    // By age: every rotated file was last written over a second ago; the open file is never
    // removed.
    let mut r = rotation(10);
    r.max_age = Duration::from_secs(1);
    f.set_rotation(Some(r));
    clock.set(SystemTime::now() + Duration::from_secs(60));
    f.cleanup_now(false).unwrap();
    assert_eq!(rotated(&d, "a.log"), ["a.log", "a.log.old"]);
}

#[test]
fn compresses_rotated_files() {
    let d = Dir::new("gzip");
    let clock = Clock::at(T0);
    let (files, reported) = clock.files();
    let mut r = rotation(64);
    r.compress = true;
    let f = files.open(&d.path("a.log"), Some(r), Zone::utc()).unwrap();
    for i in 0..20 {
        f.write(format!("line {i:02} {}\n", "z".repeat(20)).as_bytes())
            .unwrap();
        clock.add(1);
    }
    f.sync().unwrap();
    drop(f);
    // The compression thread finishes on its own time.
    for _ in 0..200 {
        if !d
            .names()
            .iter()
            .any(|n| n.starts_with("a.log.") && !n.ends_with(".gz"))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let names = d.names();
    assert!(
        names.iter().filter(|n| n.ends_with(".gz")).count() >= 5,
        "{names:?}"
    );
    assert!(
        names.iter().all(|n| n == "a.log" || n.ends_with(".gz")),
        "{names:?}"
    );
    let lines = all_lines(&d);
    assert_eq!(lines.len(), 20, "{lines:?}");
    assert!(reported.lock().unwrap().is_empty(), "{reported:?}");
}

#[test]
fn one_sink_a_path() {
    let d = Dir::new("shared");
    let files = Files::new();
    let a = files
        .open(&d.path("a.log"), Some(rotation(100)), Zone::utc())
        .unwrap();
    let b = files
        .open(
            &format!("{}//a.log", d.0.display()),
            Some(rotation(100)),
            Zone::utc(),
        )
        .unwrap();
    assert!(
        files
            .open(&d.path("a.log"), Some(rotation(200)), Zone::utc())
            .unwrap_err()
            .to_string()
            .contains("other rotation settings")
    );
    a.write(b"one\n").unwrap();
    b.write(b"two\n").unwrap();
    drop(a);
    b.sync().unwrap();
    assert_eq!(files.list().len(), 1);
    drop(b);
    assert!(files.list().is_empty());
    assert_eq!(fs::read_to_string(d.0.join("a.log")).unwrap(), "one\ntwo\n");
}

#[test]
fn links_and_odd_names_are_refused() {
    let d = Dir::new("links");
    let files = Files::new();
    let target = d.0.join("target");
    fs::write(&target, "keep\n").unwrap();
    std::os::unix::fs::symlink(&target, d.0.join("a.log")).unwrap();
    let e = files.open(&d.path("a.log"), None, Zone::utc()).unwrap_err();
    assert!(e.to_string().contains("not a regular file"), "{e}");
    fs::hard_link(&target, d.0.join("b.log")).unwrap();
    let e = files.open(&d.path("b.log"), None, Zone::utc()).unwrap_err();
    assert!(e.to_string().contains("hard links"), "{e}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
    // A link at the name a rotation would take is skipped for the next name.
    let clock = Clock::at(T0);
    let (files, _) = clock.files();
    let f = files
        .open(&d.path("c.log"), Some(rotation(4)), Zone::utc())
        .unwrap();
    std::os::unix::fs::symlink(&target, d.0.join("c.log.2026-11-05T03-59-59.000")).unwrap();
    f.write(b"one\n").unwrap();
    f.write(b"two\n").unwrap();
    f.sync().unwrap();
    assert!(
        d.names()
            .contains(&"c.log.2026-11-05T03-59-59.000-1".to_owned())
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
}

#[test]
fn a_full_queue_refuses_and_counts() {
    let d = Dir::new("full");
    let files = Files::new();
    let f = files.open(&d.path("a.log"), None, Zone::utc()).unwrap();
    let block = vec![b'x'; super::QUEUE_BYTES / 4];
    let mut refused = 0;
    for _ in 0..64 {
        if f.write(&block).is_err() {
            refused += 1;
        }
    }
    f.sync().unwrap();
    assert_eq!(f.dropped(), refused);
    let size = fs::metadata(d.0.join("a.log")).unwrap().len();
    assert_eq!(size, (64 - refused) * block.len() as u64);
}

#[test]
fn names() {
    for (s, ok) in [
        ("2026-11-05T04-00-00.000", true),
        ("2026-11-05T04-00-00.000-12", true),
        ("2026-11-05T04-00-00.000.gz", true),
        ("2026-11-05T04-00-00.000-1.gz", true),
        ("2026-11-05T04-00-00.000-", false),
        ("2026-11-05T04-00-00.00", false),
        ("2026-11-05 04-00-00.000", false),
        ("old", false),
        ("2026-11-05T04-00-00.000.gz.tmp", false),
    ] {
        assert_eq!(rotated_suffix(s), ok, "{s}");
    }
    assert_eq!(
        super::utc_stamp(UNIX_EPOCH + Duration::from_millis(u64::try_from(T0 + 7).unwrap())),
        "2026-11-05T03-59-59.007"
    );
    let _ = Path::new(".");
}
