//! The certificate file watcher, as fit's `tlsmgr` watcher tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use akit_tls::{FileWatcher, Report, Sum};
use support::certs::dir;

/// A running config, with the files it loaded, and an apply that loads them again unless told
/// to fail.
struct Watched {
    version: u64,
    files: BTreeMap<String, Sum>,
    applies: usize,
    fail: Option<&'static str>,
    reports: Vec<Report<&'static str>>,
    mtime: SystemTime,
    watcher: FileWatcher,
    now: Instant,
}

impl Watched {
    fn new(paths: &[&Path]) -> Watched {
        Watched {
            version: 0,
            files: paths
                .iter()
                .map(|p| (p.display().to_string(), [0; 32]))
                .collect(),
            applies: 0,
            fail: None,
            reports: Vec::new(),
            mtime: SystemTime::now() - Duration::from_secs(3600),
            watcher: FileWatcher::new(Duration::from_secs(1)),
            now: Instant::now(),
        }
    }

    /// The config applied: it loads the files as they are now.
    fn load(&mut self) {
        self.version += 1;
        for (p, sum) in &mut self.files {
            *sum = akio_crypto::hash::Sha256::digest(&std::fs::read(p).unwrap());
        }
    }

    /// Writes `data` with a modification time of its own, whatever the file system's resolution.
    fn write(&mut self, path: &Path, data: &str) {
        std::fs::write(path, data).unwrap();
        self.mtime += Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(self.mtime)
            .unwrap();
    }

    fn step(&mut self) {
        self.now += Duration::from_secs(1);
        let files = self.files.clone();
        let mut applied = false;
        let fail = self.fail;
        let r = self.watcher.tick(self.now, self.version, &files, || {
            applied = true;
            fail.map_or(Ok(()), Err)
        });
        if applied {
            self.applies += 1;
            if fail.is_none() {
                self.load();
            }
        }
        self.reports.extend(r);
    }

    /// Ticks until a change is past the debounce time.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.step();
        }
    }
}

fn paths(d: &Path) -> (PathBuf, PathBuf) {
    (d.join("a.crt"), d.join("a.key"))
}

#[test]
fn reloads_once_the_files_settle() {
    let d = dir("watch");
    let (cert, key) = paths(&d);
    let mut w = Watched::new(&[&cert, &key]);
    w.write(&cert, "cert 1");
    w.write(&key, "key 1");
    w.load();
    w.settle();
    assert_eq!(w.applies, 0, "applied with nothing changed");

    // A renewal writes the certificate, then the key: one apply, once both stayed as they are.
    w.write(&cert, "cert 2");
    w.step();
    w.write(&key, "key 2");
    w.step();
    assert_eq!(w.applies, 0, "applied before the files stayed as they are");
    w.settle();
    assert_eq!((w.applies, w.version), (1, 2));
    w.settle();
    assert_eq!(w.applies, 1, "applied again with the files loaded");

    // Touched, and changed and changed back: nothing to load.
    w.write(&cert, "cert 2");
    w.settle();
    w.write(&key, "key X");
    w.step();
    w.write(&key, "key 2");
    w.settle();
    assert_eq!(w.applies, 1);

    // A pair that fails is reported once and not tried again until it changes.
    w.fail = Some("does not match");
    w.write(&cert, "cert 3");
    w.settle();
    w.settle();
    assert_eq!(w.applies, 2);
    assert_eq!(w.reports, [Report::Failed("does not match")]);
    w.fail = None;
    w.write(&key, "key 3");
    w.settle();
    assert_eq!((w.applies, w.version), (3, 3));

    // A file that cannot be read, as while it is replaced, is reported once and looked at again
    // when it is back.
    std::fs::remove_file(&key).unwrap();
    w.settle();
    w.settle();
    assert_eq!(w.applies, 3);
    assert_eq!(w.reports.len(), 2);
    assert!(matches!(&w.reports[1], Report::Unreadable(e) if e.contains("a.key")));
    w.write(&key, "key 4");
    w.settle();
    assert_eq!((w.applies, w.reports.len()), (4, 2));
    std::fs::remove_dir_all(&d).unwrap();
}

/// A config applied another way is compared with its files once they have stayed as they are:
/// they may have changed after it read them.
#[test]
fn a_new_config_is_compared_with_its_files() {
    let d = dir("watch-new");
    let (cert, _) = paths(&d);
    let mut w = Watched::new(&[&cert]);
    w.write(&cert, "cert 1");
    w.load();
    w.settle();
    w.write(&cert, "cert 2");
    w.version += 1; // a config applied by SIGHUP, which read "cert 1"
    w.settle();
    assert_eq!(w.applies, 1);

    // A config with no files: nothing is looked at.
    w.version += 1;
    w.files.clear();
    w.settle();
    assert_eq!(w.applies, 1);
    std::fs::remove_dir_all(&d).unwrap();
}
