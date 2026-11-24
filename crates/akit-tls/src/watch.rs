//! Certificate files watched, and the running config applied again when they change: fit's
//! `tlsmgr.FileWatcher`, behind `run -watch-certs`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::certs::Sum;

/// How often the files are looked at, by default (fit's `DefaultWatchInterval`).
pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// How long the files must stay as they are before a change is acted on, by default (fit's
/// `DefaultWatchDebounce`).
pub const DEFAULT_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);

/// What a look at a file tells without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
    ok: bool,
}

/// Has the running config applied again when the certificate and key files it loaded change, as
/// a tool that renews certificates outside akit changes them, so that the listeners serve the
/// new ones without a reload by hand: fit's `tlsmgr.FileWatcher`.
///
/// It polls, as fit's does (decision 006): the caller calls [`tick`](Self::tick) every
/// interval, on the control plane, never on an I/O thread (it reads files). Each tick looks at
/// each file's size and modification time and, once they have stayed as they are for the
/// debounce time, reads the files and compares them with what the running config loaded.
/// Applying the config again loads them as any reload does, so a certificate and key that do not
/// match, as when one is written and the other not yet, fail and leave the running config, and
/// its certificates, as they are; the same files are not tried again until they change.
#[derive(Debug)]
pub struct FileWatcher {
    debounce: Duration,
    version: Option<u64>,
    want: BTreeMap<String, Sum>,
    seen: Vec<Stamp>,
    since: Option<Instant>,
    /// `seen` changed, or the config did, and that is not handled yet.
    dirty: bool,
    /// The sum of the files' contents that applying failed on last.
    failed: Option<Sum>,
    /// A file could not be read, and that was reported.
    unreadable: bool,
}

/// What a [`FileWatcher::tick`] has to say.
#[derive(Debug, PartialEq, Eq)]
pub enum Report<E> {
    /// A file could not be read (being replaced, perhaps): said once, until it reads again.
    Unreadable(String),
    /// Applying the config again failed; the running config stays.
    Failed(E),
}

impl FileWatcher {
    /// A watcher acting on a change once the files have stayed as they are for `debounce`
    /// ([`DEFAULT_WATCH_DEBOUNCE`] when zero).
    #[must_use]
    pub fn new(debounce: Duration) -> FileWatcher {
        FileWatcher {
            debounce: if debounce.is_zero() {
                DEFAULT_WATCH_DEBOUNCE
            } else {
                debounce
            },
            version: None,
            want: BTreeMap::new(),
            seen: Vec::new(),
            since: None,
            dirty: false,
            failed: None,
            unreadable: false,
        }
    }

    /// Looks at the files once. `version` and `files` are the running config's: a number that
    /// changes with each config applied, and the files it loaded with their SHA-256. When the
    /// files changed from those and have stayed as they are for the debounce time, `apply` applies
    /// the running config again. What went wrong is returned, to be reported.
    pub fn tick<E>(
        &mut self,
        now: Instant,
        version: u64,
        files: &BTreeMap<String, Sum>,
        apply: impl FnOnce() -> Result<(), E>,
    ) -> Option<Report<E>> {
        if self.version != Some(version) {
            // A config applied since the last look, by this watcher or another way. Its files
            // are compared with what it loaded once they have stayed as they are, since they may
            // have changed after it read them.
            self.version = Some(version);
            self.want.clone_from(files);
            self.seen = self.look();
            self.since = Some(now);
            self.dirty = !self.want.is_empty();
            return None;
        }
        if self.want.is_empty() {
            return None;
        }
        let seen = self.look();
        if seen != self.seen {
            self.seen = seen;
            self.since = Some(now);
            self.dirty = true;
            return None;
        }
        let settled = self
            .since
            .is_some_and(|s| now.saturating_duration_since(s) >= self.debounce);
        if !self.dirty || !settled {
            return None;
        }
        self.dirty = false;
        let mut all = akio_crypto::hash::Sha256::new();
        let mut changed = false;
        for (path, want) in &self.want {
            let data = match crate::read(path) {
                Ok(d) => d,
                Err(e) => {
                    // Being replaced, perhaps; its next change is looked at afresh.
                    if self.unreadable {
                        return None;
                    }
                    self.unreadable = true;
                    return Some(Report::Unreadable(e));
                }
            };
            let sum = akio_crypto::hash::Sha256::digest(&data);
            changed |= sum != *want;
            all.update(path.as_bytes());
            all.update(&sum);
        }
        self.unreadable = false;
        if !changed {
            return None; // touched, or changed and changed back
        }
        let sum = all.finalize();
        if self.failed == Some(sum) {
            return None; // failed as they are; reported then
        }
        match apply() {
            Ok(()) => {
                self.failed = None;
                None
            }
            Err(e) => {
                self.failed = Some(sum);
                Some(Report::Failed(e))
            }
        }
    }

    /// The stamps of the files, in path order.
    fn look(&self) -> Vec<Stamp> {
        self.want
            .keys()
            .map(|p| match std::fs::metadata(p) {
                Ok(m) => Stamp {
                    size: m.len(),
                    modified: m.modified().ok(),
                    ok: true,
                },
                Err(_) => Stamp {
                    size: 0,
                    modified: None,
                    ok: false,
                },
            })
            .collect()
    }
}
