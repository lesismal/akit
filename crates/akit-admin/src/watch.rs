//! The file provider (I29): a config's files watched, and applied each time they change, fit's
//! `provider/file` behind `run -watch`.
//!
//! The watcher polls the files' sizes and modification times rather than asking the operating
//! system for events, as fit decided (fit's decision 006): it needs no dependency, works the same
//! everywhere, and sees a file an editor replaces by renaming another over it as well as one
//! written in place. A change is applied once the files have stopped changing for the debounce
//! time, and only when their contents differ from what was applied last, so that an editor's
//! several writes make one reload and a touch none. A config may be read from several files and
//! directories; their files are found afresh at each look, so that a file added to a directory,
//! or taken out, is a change too.
//!
//! The watcher reads files, so it runs on the control plane, never on an I/O thread: `akit run`
//! ticks it on its control thread every `-watch-interval`, and applies what it reads through the
//! same [`Configs::update`](crate::Configs::update) as SIGHUP and the Admin API.

use std::fmt;
use std::io::Read;
use std::time::{Duration, Instant, SystemTime};

use akit_config::ConfigFile;

/// How often the files are looked at, by default (fit's `file.DefaultInterval`).
pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// How long the files must stay as they are before a change is applied, by default (fit's
/// `file.DefaultDebounce`).
pub const DEFAULT_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// The most the files are read, all together, by default: a config past it is reported, not
/// applied (fit's `file.DefaultMaxBytes`, akit's [`akit_config::MAX_CONFIG_BYTES`]).
pub const DEFAULT_WATCH_MAX_BYTES: usize = akit_config::MAX_CONFIG_BYTES;

/// The files the watched paths stand for, as directories stand for the config files in them.
pub type Expand = Box<dyn Fn(&[String]) -> Result<Vec<String>, String> + Send>;

/// How the files are watched.
#[non_exhaustive]
pub struct WatchOptions {
    /// How long the files must stay as they are before a change is applied;
    /// [`DEFAULT_WATCH_DEBOUNCE`] when zero.
    pub debounce: Duration,
    /// The most read, all the files together; [`DEFAULT_WATCH_MAX_BYTES`] when zero.
    pub max_bytes: usize,
    /// The files the watched paths stand for; the paths as they are when `None`.
    pub expand: Option<Expand>,
}

impl Default for WatchOptions {
    fn default() -> WatchOptions {
        WatchOptions {
            debounce: DEFAULT_WATCH_DEBOUNCE,
            max_bytes: DEFAULT_WATCH_MAX_BYTES,
            expand: None,
        }
    }
}

impl fmt::Debug for WatchOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchOptions")
            .field("debounce", &self.debounce)
            .field("max_bytes", &self.max_bytes)
            .field("expand", &self.expand.is_some())
            .finish()
    }
}

/// Why the files could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchError {
    /// The files the paths stand for could not be found (a directory gone, say).
    Find(String),
    /// A file could not be read.
    Read {
        /// The file.
        path: String,
        /// What went wrong, as the system says it.
        kind: std::io::ErrorKind,
        /// The system's message.
        msg: String,
    },
    /// The files hold more than the watcher reads.
    TooLarge {
        /// The file past the limit.
        path: String,
        /// The limit, for all the files together.
        max: usize,
    },
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchError::Find(msg) => f.write_str(msg),
            WatchError::Read { path, msg, .. } => write!(f, "{path}: {msg}"),
            WatchError::TooLarge { path, max } => write!(
                f,
                "{path}: the config's files are larger than the most the watcher reads ({max} bytes)"
            ),
        }
    }
}

impl std::error::Error for WatchError {}

/// What a [`Watcher::tick`] has to say.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchReport<E> {
    /// The files could not be read: said once, until they read again.
    Unreadable(WatchError),
    /// Applying them failed; the running config stays. Said once: the same contents are not
    /// tried again, the next edit is.
    Failed(E),
}

/// What a look at a file tells without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    path: String,
    size: u64,
    modified: Option<SystemTime>,
}

/// A SHA-256 of files' names and contents.
type Sum = [u8; 32];

/// Applies a config's files each time they change: fit's `file.Watcher`.
///
/// The caller calls [`tick`](Self::tick) every interval, on the control plane (it reads files),
/// and [`applied`](Self::applied) when the files are applied another way, as SIGHUP applies them.
pub struct Watcher {
    paths: Vec<String>,
    opts: WatchOptions,
    /// The contents applied last, or found at the start.
    last: Sum,
    /// The files as last looked at; `None` when they could not be found.
    seen: Option<Vec<Stamp>>,
    /// When `seen` last changed.
    since: Instant,
    /// `seen` changed and the change is not handled yet.
    dirty: bool,
    /// The files could not be read, and that was reported.
    gone: bool,
}

impl fmt::Debug for Watcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watcher")
            .field("paths", &self.paths)
            .field("opts", &self.opts)
            .field("dirty", &self.dirty)
            .field("gone", &self.gone)
            .finish_non_exhaustive()
    }
}

impl Watcher {
    /// A watcher of the files `paths` stand for, which the running config was read from as
    /// `current`.
    #[must_use]
    pub fn new(paths: &[String], current: &[ConfigFile], mut opts: WatchOptions) -> Watcher {
        if opts.debounce.is_zero() {
            opts.debounce = DEFAULT_WATCH_DEBOUNCE;
        }
        if opts.max_bytes == 0 {
            opts.max_bytes = DEFAULT_WATCH_MAX_BYTES;
        }
        // `seen` starts unknown rather than as the files look now: they may have changed since
        // `current` was read from them, and a stamp taken now would hide that change for good,
        // as a same-sized edit within the file system's timestamp granularity leaves nothing
        // newer to see. So the watcher starts as if the files had just changed, and the contents
        // decide, which for unchanged files costs one read and no apply.
        Watcher {
            paths: paths.to_vec(),
            opts,
            last: sum(current),
            seen: None,
            since: Instant::now(),
            dirty: true,
            gone: false,
        }
    }

    /// Records that `files`, what the watched files hold, were applied another way, as SIGHUP
    /// applies them, so that the watcher does not apply them again.
    pub fn applied(&mut self, files: &[ConfigFile]) {
        self.last = sum(files);
    }

    /// Looks at the files once and, when they changed and have stayed as they are for the
    /// debounce time, reads them and, when what they hold differs from what was applied last,
    /// has `apply` apply them. What went wrong is returned, to be reported, once.
    pub fn tick<E>(
        &mut self,
        now: Instant,
        apply: impl FnOnce(&[ConfigFile]) -> Result<(), E>,
    ) -> Option<WatchReport<E>> {
        let seen = self.look();
        if seen != self.seen {
            self.seen = seen;
            self.since = now;
            self.dirty = true;
            return None;
        }
        if !self.dirty || now.saturating_duration_since(self.since) < self.opts.debounce {
            return None;
        }
        self.dirty = false;
        let files = match self.read() {
            Ok(f) => f,
            Err(e) => {
                if self.gone {
                    return None;
                }
                self.gone = true;
                return Some(WatchReport::Unreadable(e));
            }
        };
        self.gone = false;
        let h = sum(&files);
        // Recorded whether or not it applies: a broken config is reported once, not at every
        // tick, and the next edit is tried afresh.
        let same = h == self.last;
        self.last = h;
        if same {
            return None; // touched, or changed and changed back
        }
        apply(&files).err().map(WatchReport::Failed)
    }

    /// The files the paths stand for.
    fn files(&self) -> Result<Vec<String>, String> {
        match &self.opts.expand {
            Some(expand) => expand(&self.paths),
            None => Ok(self.paths.clone()),
        }
    }

    /// What a look at the files tells without reading them: which they are, their sizes and
    /// modification times; `None` when they cannot be found.
    fn look(&self) -> Option<Vec<Stamp>> {
        let files = self.files().ok()?;
        files
            .into_iter()
            .map(|path| {
                let m = std::fs::metadata(&path).ok()?;
                Some(Stamp {
                    size: m.len(),
                    modified: m.modified().ok(),
                    path,
                })
            })
            .collect()
    }

    /// Reads the files, refusing more than `max_bytes` in all.
    fn read(&self) -> Result<Vec<ConfigFile>, WatchError> {
        let paths = self.files().map_err(WatchError::Find)?;
        let mut files = Vec::with_capacity(paths.len());
        let mut left = self.opts.max_bytes;
        for path in paths {
            let data = match read_file(&path, left) {
                Ok(Some(d)) => d,
                Ok(None) => {
                    return Err(WatchError::TooLarge {
                        path,
                        max: self.opts.max_bytes,
                    });
                }
                Err(e) => {
                    return Err(WatchError::Read {
                        path,
                        kind: e.kind(),
                        msg: e.to_string(),
                    });
                }
            };
            left = left.saturating_sub(data.len());
            files.push(ConfigFile { path, data });
        }
        Ok(files)
    }
}

/// The file at `path`, `None` when it holds more than `limit` bytes.
fn read_file(path: &str, limit: usize) -> std::io::Result<Option<Vec<u8>>> {
    let f = std::fs::File::open(path)?;
    let mut data = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    f.take(cap).read_to_end(&mut data)?;
    Ok((data.len() <= limit).then_some(data))
}

/// What `files` hold, their names and contents, hashed.
fn sum(files: &[ConfigFile]) -> Sum {
    let mut h = akio_crypto::hash::Sha256::new();
    for f in files {
        h.update(format!("{}:", f.path.len()).as_bytes());
        h.update(f.path.as_bytes());
        h.update(format!("{}:", f.data.len()).as_bytes());
        h.update(&f.data);
    }
    h.finalize()
}
