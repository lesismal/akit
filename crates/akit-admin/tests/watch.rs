//! The file provider (I29), fit's `provider/file` tests: edits, a file renamed into place, a
//! touch, an edit before watching, debouncing, a broken edit, a file removed and back, a file too
//! large, contents applied another way (SIGHUP), a directory's files added and taken out, and the
//! errors of finding and reading the files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use akit_admin::{
    DEFAULT_WATCH_DEBOUNCE, DEFAULT_WATCH_MAX_BYTES, WatchError, WatchOptions, WatchReport, Watcher,
};
use akit_config::ConfigFile;

/// What a watcher applied and reported.
#[derive(Default)]
struct Recorder {
    applied: Vec<String>,
    errs: Vec<String>,
    /// The `WatchError`s among `errs`.
    unreadable: Vec<WatchError>,
    /// A config apply refuses.
    refuse: String,
}

impl Recorder {
    fn apply(&mut self, files: &[ConfigFile]) -> Result<(), String> {
        let data: Vec<u8> = files.iter().flat_map(|f| f.data.clone()).collect();
        let data = String::from_utf8(data).unwrap();
        if data == self.refuse {
            return Err(format!("refused: {data}"));
        }
        self.applied.push(data);
        Ok(())
    }

    fn report(&mut self, r: WatchReport<String>) {
        match r {
            WatchReport::Unreadable(e) => {
                self.errs.push(e.to_string());
                self.unreadable.push(e);
            }
            WatchReport::Failed(e) => self.errs.push(e),
        }
    }
}

/// Ticks `w` on `r`: one look at the files.
fn tick(w: &mut Watcher, r: &Mutex<Recorder>, now: Instant) {
    let mut rec = r.lock().unwrap();
    if let Some(report) = w.tick(now, |files| rec.apply(files)) {
        rec.report(report);
    }
}

/// A watcher ticked every 10ms on a thread of its own, as `akit run` ticks it every
/// `-watch-interval` on its control thread.
struct Running {
    w: Arc<Mutex<Watcher>>,
    r: Arc<Mutex<Recorder>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn start(w: Watcher) -> Running {
        let w = Arc::new(Mutex::new(w));
        let r = Arc::new(Mutex::new(Recorder::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (w, r, stop) = (w.clone(), r.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                    tick(&mut w.lock().unwrap(), &r, Instant::now());
                }
            })
        };
        Running {
            w,
            r,
            stop,
            thread: Some(thread),
        }
    }

    fn state(&self) -> (Vec<String>, Vec<String>) {
        let r = self.r.lock().unwrap();
        (r.applied.clone(), r.errs.clone())
    }

    /// Waits until `cond` holds of what was applied and reported.
    fn wait_for(&self, what: &str, cond: impl Fn(&[String], &[String]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (applied, errs) = self.state();
            if cond(&applied, &errs) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what}: applied {applied:?}, errors {errs:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A fresh directory under the target's test directory.
fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("watch-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// Writes `data` to `path`, with a modification time of its own, since a file system may keep
/// only seconds.
fn write(path: &Path, data: &str) {
    std::fs::write(path, data).unwrap();
    let len = u64::try_from(data.len()).unwrap();
    stamp(path, SystemTime::now() + Duration::from_secs(len));
}

fn stamp(path: &Path, t: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn file(path: &Path, data: &str) -> ConfigFile {
    ConfigFile {
        path: s(path),
        data: data.as_bytes().to_vec(),
    }
}

fn fast() -> WatchOptions {
    let mut o = WatchOptions::default();
    o.debounce = Duration::from_millis(50);
    o
}

/// A watcher of `fit.json` in `name`'s directory, holding `initial`, running.
fn start(name: &str, initial: &str, opts: WatchOptions) -> (PathBuf, Running) {
    let path = dir(name).join("fit.json");
    write(&path, initial);
    let w = Watcher::new(&[s(&path)], &[file(&path, initial)], opts);
    (path, Running::start(w))
}

#[test]
fn applies_on_change() {
    let (path, r) = start("change", "v1", fast());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(r.state().0, Vec::<String>::new(), "applied with no change");
    write(&path, "v22");
    r.wait_for("an edit", |a, _| a == ["v22"]);

    // A file an editor renames into place.
    let tmp = path.with_extension("tmp");
    write(&tmp, "v333");
    std::fs::rename(&tmp, &path).unwrap();
    r.wait_for("a rename", |a, _| a == ["v22", "v333"]);

    // Touched, its contents the same: nothing applied.
    stamp(&path, SystemTime::now() + Duration::from_secs(3600));
    std::thread::sleep(Duration::from_millis(150));
    let (applied, errs) = r.state();
    assert_eq!((applied.len(), errs.len()), (2, 0), "after a touch");
}

/// An edit made after the running config was read and before the watcher started, as `akit
/// run`'s listeners are up before its watcher, is applied, though the file shows no change once
/// watched.
#[test]
fn an_edit_before_watching_is_applied() {
    let path = dir("before").join("fit.json");
    write(&path, "v2"); // the same size as "v1", and not touched again
    let w = Watcher::new(&[s(&path)], &[file(&path, "v1")], fast());
    let r = Running::start(w);
    r.wait_for("the edit", |a, _| a == ["v2"]);
}

/// A file written several times in a row is applied once, as it ends up.
#[test]
fn writes_in_a_row_apply_once() {
    let mut opts = WatchOptions::default();
    opts.debounce = Duration::from_millis(150);
    let (path, r) = start("debounce", "v1", opts);
    for v in ["a2", "a33", "a444", "a5555"] {
        write(&path, v);
        std::thread::sleep(Duration::from_millis(30));
    }
    r.wait_for("the last write", |a, _| a.len() == 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.state().0, ["a5555"], "the last write once");
}

/// An edit that does not apply is reported once and the watcher goes on; the next edit applies.
#[test]
fn a_broken_edit_is_reported_once() {
    let (path, r) = start("broken", "v1", fast());
    r.r.lock().unwrap().refuse = "broken!".into();
    write(&path, "broken!");
    r.wait_for("the broken edit", |_, e| e.len() == 1);
    std::thread::sleep(Duration::from_millis(150));
    let (applied, errs) = r.state();
    assert!(applied.is_empty(), "{applied:?}");
    assert_eq!(errs, ["refused: broken!"]);
    write(&path, "fixed");
    r.wait_for("the fix", |a, _| a == ["fixed"]);
}

/// A file that disappears is reported once; when it comes back, it is applied.
#[test]
fn a_file_gone_and_back() {
    let (path, r) = start("gone", "v1", fast());
    std::fs::remove_file(&path).unwrap();
    r.wait_for("the file gone", |_, e| e.len() == 1);
    std::thread::sleep(Duration::from_millis(150));
    {
        let rec = r.r.lock().unwrap();
        assert_eq!(rec.errs.len(), 1, "{:?}", rec.errs);
        assert!(
            matches!(&rec.unreadable[..], [WatchError::Read { kind: std::io::ErrorKind::NotFound, path: p, .. }] if *p == s(&path)),
            "{:?}",
            rec.unreadable
        );
    }
    write(&path, "back");
    r.wait_for("the file back", |a, _| a == ["back"]);
}

#[test]
fn a_file_too_large() {
    let mut opts = fast();
    opts.max_bytes = 8;
    let (path, r) = start("large", "v1", opts);
    write(&path, "123456789");
    r.wait_for("a file too large", |_, e| e.len() == 1);
    assert!(matches!(
        &r.r.lock().unwrap().unreadable[..],
        [WatchError::TooLarge { max: 8, .. }]
    ));
    write(&path, "1234");
    r.wait_for("a file small enough", |a, _| a == ["1234"]);
}

/// Contents applied another way (SIGHUP) are not applied again.
#[test]
fn contents_applied_another_way_are_not_applied_again() {
    let (path, r) = start("applied", "v1", fast());
    {
        // Under the watcher's lock, as SIGHUP is handled on the thread that ticks it.
        let mut w = r.w.lock().unwrap();
        write(&path, "by-sighup");
        w.applied(&[file(&path, "by-sighup")]);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.state().0, Vec::<String>::new(), "applied again");
}

/// The files a directory stands for are found at each look, so that a file added, changed or
/// taken out is applied.
#[test]
fn a_directory_s_files_added_and_taken_out() {
    let d = dir("directory");
    let a = d.join("a.conf");
    write(&a, "A");
    let mut opts = fast();
    opts.expand = Some(Box::new(|paths: &[String]| {
        let mut files = Vec::new();
        for p in paths {
            let mut es: Vec<_> = std::fs::read_dir(p)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| s(&e.path())).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            es.sort();
            files.extend(es);
        }
        Ok(files)
    }));
    let w = Watcher::new(&[s(&d)], &[file(&a, "A")], opts);
    let r = Running::start(w);
    write(&d.join("b.conf"), "B");
    r.wait_for("a file added", |a, _| a == ["AB"]);
    std::fs::remove_file(&a).unwrap();
    r.wait_for("a file taken out", |a, _| a == ["AB", "B"]);
}

/// The config's own directory reading, `akit_config::config_files`, as `akit run -watch` expands
/// its paths: a file added in a subdirectory is applied, one with another extension is not read.
#[test]
fn config_files_expand_a_directory() {
    let d = dir("config-files");
    std::fs::create_dir_all(d.join("routes")).unwrap();
    let a = d.join("a.json");
    write(&a, "A");
    let mut opts = fast();
    opts.expand = Some(Box::new(|paths: &[String]| {
        akit_config::config_files(paths)
            .map(|fs| fs.iter().map(|f| s(f)).collect())
            .map_err(|e| e.to_string())
    }));
    let w = Watcher::new(&[s(&d)], &[file(&a, "A")], opts);
    let r = Running::start(w);
    write(&d.join("routes").join("b.yaml"), "B");
    write(&d.join("notes.txt"), "not read");
    r.wait_for("a file added", |a, _| a == ["AB"]);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(r.state().0, ["AB"]);
}

#[test]
fn defaults() {
    let o = WatchOptions::default();
    assert_eq!(o.debounce, DEFAULT_WATCH_DEBOUNCE);
    assert_eq!(o.max_bytes, DEFAULT_WATCH_MAX_BYTES);
    assert_eq!(DEFAULT_WATCH_MAX_BYTES, 64 << 20);
    // Zero is the default.
    let mut o = WatchOptions::default();
    o.debounce = Duration::ZERO;
    o.max_bytes = 0;
    let mut w = Watcher::new(&["x".into()], &[], o);
    let now = Instant::now();
    // Not yet settled at the default debounce...
    assert!(w.tick(now, |_| Ok::<(), ()>(())).is_none());
    assert!(
        w.tick(now + Duration::from_millis(100), |_| Ok::<(), ()>(()))
            .is_none()
    );
    // ...and settled after it.
    assert!(matches!(
        w.tick(now + DEFAULT_WATCH_DEBOUNCE, |_| Ok::<(), ()>(())),
        Some(WatchReport::Unreadable(WatchError::Read { .. }))
    ));
}

/// Files that cannot be found are reported, once.
#[test]
fn files_that_cannot_be_found_are_reported_once() {
    let mut opts = WatchOptions::default();
    opts.expand = Some(Box::new(|_: &[String]| Err("no such dir".to_owned())));
    let mut w = Watcher::new(&["x".into()], &[], opts);
    let r = Mutex::new(Recorder::default());
    let now = Instant::now() + Duration::from_secs(3600);
    tick(&mut w, &r, now);
    tick(&mut w, &r, now + Duration::from_secs(3600));
    tick(&mut w, &r, now + Duration::from_secs(7200));
    let rec = r.lock().unwrap();
    assert!(rec.applied.is_empty());
    assert_eq!(rec.errs, ["no such dir"]);
}

/// A path that cannot be read as a file, a directory, is reported.
#[test]
fn a_directory_read_as_a_file_is_reported() {
    let d = dir("read-directory");
    let mut w = Watcher::new(&[s(&d)], &[], WatchOptions::default());
    let r = Mutex::new(Recorder::default());
    let now = Instant::now() + Duration::from_secs(3600);
    tick(&mut w, &r, now); // sees the directory
    tick(&mut w, &r, now + Duration::from_secs(3600));
    let rec = r.lock().unwrap();
    assert!(rec.applied.is_empty());
    assert_eq!(rec.errs.len(), 1, "{:?}", rec.errs);
}
