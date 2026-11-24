//! Where certificate management keeps what it needs across restarts, and shares it between
//! instances: fit's `tlsmgr.Storage` and `tlsmgr.FileStorage`.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// The largest value [`FileStorage`] stores or loads, as fit's `tlsmgr.MaxValueBytes`.
pub const MAX_VALUE_BYTES: u64 = 4 << 20;

/// How often a holder touches its lock, by default (fit's `DefaultLockRefresh`).
pub const DEFAULT_LOCK_REFRESH: Duration = Duration::from_secs(5);
/// How old a lock gets before it is taken to be left by a holder that stopped
/// (fit's `DefaultLockStale`).
pub const DEFAULT_LOCK_STALE: Duration = Duration::from_secs(30);
/// How often a waiter looks at a held lock again (fit's `DefaultLockPoll`).
pub const DEFAULT_LOCK_POLL: Duration = Duration::from_secs(1);
/// The most locks one [`FileStorage`] holds at once (rule 11); each has a thread touching it.
pub const MAX_HELD_LOCKS: usize = 256;

/// What a [`Storage`] call failed with.
#[derive(Debug)]
#[non_exhaustive]
pub enum StorageError {
    /// The key has no value (fit's `fs.ErrNotExist`).
    NotFound,
    /// A key or lock name that is empty, absolute, has an empty, `.` or `..` part, a backslash
    /// or a NUL.
    BadKey(String),
    /// A value larger than [`MAX_VALUE_BYTES`].
    TooLarge(String),
    /// The deadline passed (waiting for a lock, or before the call began).
    Timeout,
    /// [`Storage::unlock`] of a lock not held here.
    NotHeld(String),
    /// More than [`MAX_HELD_LOCKS`] held at once.
    TooManyLocks,
    /// The store failed.
    Io(io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::NotFound => f.write_str("akit-tls: no such key"),
            StorageError::BadKey(k) => write!(f, "akit-tls: bad storage key {k:?}"),
            StorageError::TooLarge(k) => {
                write!(f, "akit-tls: {k} is larger than {MAX_VALUE_BYTES} bytes")
            }
            StorageError::Timeout => f.write_str("akit-tls: storage deadline passed"),
            StorageError::NotHeld(n) => write!(f, "akit-tls: lock {n:?} is not held"),
            StorageError::TooManyLocks => {
                write!(f, "akit-tls: more than {MAX_HELD_LOCKS} storage locks held")
            }
            StorageError::Io(e) => write!(f, "akit-tls: storage: {e}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StorageError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for StorageError {
    fn from(e: io::Error) -> StorageError {
        if e.kind() == io::ErrorKind::NotFound {
            StorageError::NotFound
        } else {
            StorageError::Io(e)
        }
    }
}

/// Keeps what certificate management needs across restarts (account keys, certificates and
/// their keys) and shares it between instances of akit that use the same one. Keys are paths of
/// `/`-separated parts.
///
/// **Its methods block**, on a disk or a network store: they are called off the I/O threads
/// (rules.md rule 1), on akio's logical pool with `spawn_blocking` or a thread of the control
/// plane, each bounded by a deadline.
pub trait Storage: Send + Sync + fmt::Debug {
    /// The value of `key`, or [`StorageError::NotFound`].
    ///
    /// # Errors
    ///
    /// No value, a bad key, a value too large, the deadline, or the store's failure.
    fn load(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, StorageError>;
    /// Sets the value of `key`, all of it or none.
    ///
    /// # Errors
    ///
    /// A bad key, a value too large, the deadline, or the store's failure.
    fn store(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), StorageError>;
    /// Removes `key`; removing one that is not there is no error.
    ///
    /// # Errors
    ///
    /// A bad key, the deadline, or the store's failure.
    fn delete(&self, key: &str, deadline: Instant) -> Result<(), StorageError>;
    /// The keys under `prefix`, at any depth, sorted.
    ///
    /// # Errors
    ///
    /// A bad prefix, the deadline, or the store's failure.
    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, StorageError>;
    /// Takes the lock `name`, waiting for as long as another holder, in this process or another,
    /// has it and `deadline` allows. A holder that stops without unlocking loses it once it goes
    /// stale.
    ///
    /// # Errors
    ///
    /// A bad name, the deadline, too many locks held, or the store's failure.
    fn lock(&self, name: &str, deadline: Instant) -> Result<(), StorageError>;
    /// Gives back the lock `name`, taken with [`lock`](Self::lock).
    ///
    /// # Errors
    ///
    /// A lock not held here, or the store's failure.
    fn unlock(&self, name: &str) -> Result<(), StorageError>;
}

/// The directory under [`FileStorage::root`] its locks are kept in; not a key `list` returns.
const LOCKS_DIR: &str = "locks";
/// The prefix of a file being written, beside the key's, until it is renamed over it.
const TMP_PREFIX: &str = ".tmp-";

/// A [`Storage`] in a directory, as fit's `tlsmgr.FileStorage`: each key a file under the root,
/// each lock a file under `root/locks`, which its holder touches every
/// [`lock_refresh`](Self::lock_refresh). A lock file left untouched for
/// [`lock_stale`](Self::lock_stale) is taken to be left by a holder that stopped, and is
/// removed. Files and directories are made readable by their owner only.
#[derive(Debug)]
pub struct FileStorage {
    root: PathBuf,
    refresh: Duration,
    stale: Duration,
    poll: Duration,
    /// The locks held, each with what stops the thread touching it (dropping it).
    held: Mutex<HashMap<String, SyncSender<()>>>,
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl FileStorage {
    /// A storage in `root`, made when first written to, with fit's lock timings.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> FileStorage {
        FileStorage {
            root: root.into(),
            refresh: DEFAULT_LOCK_REFRESH,
            stale: DEFAULT_LOCK_STALE,
            poll: DEFAULT_LOCK_POLL,
            held: Mutex::new(HashMap::new()),
        }
    }

    /// Sets how often a holder touches its lock ([`DEFAULT_LOCK_REFRESH`] when zero).
    #[must_use]
    pub fn lock_refresh(mut self, d: Duration) -> FileStorage {
        self.refresh = if d.is_zero() { DEFAULT_LOCK_REFRESH } else { d };
        self
    }

    /// Sets how old an untouched lock gets before it is removed ([`DEFAULT_LOCK_STALE`] when
    /// zero).
    #[must_use]
    pub fn lock_stale(mut self, d: Duration) -> FileStorage {
        self.stale = if d.is_zero() { DEFAULT_LOCK_STALE } else { d };
        self
    }

    /// Sets how often a waiter looks at a held lock again ([`DEFAULT_LOCK_POLL`] when zero).
    #[must_use]
    pub fn lock_poll(mut self, d: Duration) -> FileStorage {
        self.poll = if d.is_zero() { DEFAULT_LOCK_POLL } else { d };
        self
    }

    /// The directory it keeps its files in.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file of `key`.
    fn filename(&self, key: &str) -> Result<PathBuf, StorageError> {
        let bad = || StorageError::BadKey(key.to_owned());
        if key.is_empty() || key.starts_with('/') || key.contains('\\') || key.contains('\0') {
            return Err(bad());
        }
        let mut p = self.root.clone();
        for part in key.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return Err(bad());
            }
            p.push(part);
        }
        Ok(p)
    }

    /// The file of the lock `name`.
    fn lock_file(&self, name: &str) -> Result<PathBuf, StorageError> {
        if name.is_empty() {
            return Err(StorageError::BadKey(name.to_owned()));
        }
        self.filename(&format!("{LOCKS_DIR}/{}.lock", name.replace('/', "_")))
    }

    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<String, SyncSender<()>>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Touches `file` every refresh until the lock is given back (its sender dropped).
    fn keep(&self, name: &str, file: PathBuf) -> Result<(), StorageError> {
        let (tx, rx) = mpsc::sync_channel::<()>(0);
        let refresh = self.refresh;
        std::thread::Builder::new()
            .name("akit-tls-lock".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(refresh) {
                    if let Ok(f) = File::options().write(true).open(&file) {
                        let _ = f.set_modified(SystemTime::now());
                    }
                }
            })?;
        self.held().insert(name.to_owned(), tx);
        Ok(())
    }
}

fn check(deadline: Instant) -> Result<(), StorageError> {
    if Instant::now() >= deadline {
        return Err(StorageError::Timeout);
    }
    Ok(())
}

fn mkdir_all(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

fn create_new(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)
}

/// Writes `path` whole or not at all: to a file beside it, synced, then renamed over it.
fn write_file(path: &Path, value: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    mkdir_all(dir)?;
    let tmp = dir.join(format!(
        "{TMP_PREFIX}{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = create_new(&tmp)?;
        f.write_all(value)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

impl Storage for FileStorage {
    fn load(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, StorageError> {
        let name = self.filename(key)?;
        check(deadline)?;
        let f = File::open(name)?;
        let mut data = Vec::new();
        f.take(MAX_VALUE_BYTES.saturating_add(1))
            .read_to_end(&mut data)?;
        if u64::try_from(data.len()).unwrap_or(u64::MAX) > MAX_VALUE_BYTES {
            return Err(StorageError::TooLarge(key.to_owned()));
        }
        Ok(data)
    }

    fn store(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), StorageError> {
        let name = self.filename(key)?;
        if u64::try_from(value.len()).unwrap_or(u64::MAX) > MAX_VALUE_BYTES {
            return Err(StorageError::TooLarge(key.to_owned()));
        }
        check(deadline)?;
        write_file(&name, value).map_err(StorageError::Io)
    }

    fn delete(&self, key: &str, deadline: Instant) -> Result<(), StorageError> {
        let name = self.filename(key)?;
        check(deadline)?;
        match fs::remove_file(name) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(StorageError::Io(e)),
            _ => Ok(()),
        }
    }

    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, StorageError> {
        let prefix = prefix.trim_matches('/');
        let dir = if prefix.is_empty() {
            self.root.clone()
        } else {
            self.filename(prefix)?
        };
        let mut keys = Vec::new();
        let base = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let mut stack = vec![(dir, base)];
        let mut first = true;
        while let Some((dir, key)) = stack.pop() {
            check(deadline)?;
            let entries = match fs::read_dir(&dir) {
                Err(e) if first && e.kind() == io::ErrorKind::NotFound => break,
                r => r.map_err(StorageError::Io)?,
            };
            first = false;
            for entry in entries {
                let entry = entry.map_err(StorageError::Io)?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue; // not a key's: keys are UTF-8
                };
                let k = format!("{key}{name}");
                if entry.file_type().map_err(StorageError::Io)?.is_dir() {
                    if k != LOCKS_DIR {
                        stack.push((entry.path(), format!("{k}/")));
                    }
                } else if !name.starts_with(TMP_PREFIX) {
                    keys.push(k);
                }
            }
        }
        keys.sort();
        Ok(keys)
    }

    fn lock(&self, name: &str, deadline: Instant) -> Result<(), StorageError> {
        let file = self.lock_file(name)?;
        if self.held().len() >= MAX_HELD_LOCKS {
            return Err(StorageError::TooManyLocks);
        }
        if let Some(dir) = file.parent() {
            mkdir_all(dir)?;
        }
        loop {
            match create_new(&file) {
                Ok(mut f) => {
                    let written = writeln!(f, "{}", std::process::id());
                    drop(f);
                    if let Err(e) = written
                        .map_err(StorageError::Io)
                        .and_then(|()| self.keep(name, file.clone()))
                    {
                        let _ = fs::remove_file(&file);
                        return Err(e);
                    }
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(StorageError::Io(e)),
            }
            // Held. A lock nobody has touched for `stale` is removed, and taken on the next
            // try; any other is waited for.
            let age = fs::metadata(&file)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok());
            if age.is_some_and(|a| a > self.stale) {
                let _ = fs::remove_file(&file);
                continue;
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(StorageError::Timeout);
            }
            // Off the I/O threads (see `Storage`): a wait here blocks only its caller.
            std::thread::sleep(self.poll.min(deadline.saturating_duration_since(now)));
        }
    }

    fn unlock(&self, name: &str) -> Result<(), StorageError> {
        let file = self.lock_file(name)?;
        let Some(stop) = self.held().remove(name) else {
            return Err(StorageError::NotHeld(name.to_owned()));
        };
        drop(stop);
        match fs::remove_file(file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(StorageError::Io(e)),
            _ => Ok(()),
        }
    }
}
