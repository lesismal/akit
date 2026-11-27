//! The commands a node runs (fit's `llm/node/exec.go`): always a fixed argv built in code, never
//! a shell, but for `sh -c` with a script that is a constant of this program, its parameters
//! never in its text. A command runs in a process group of its own; its output, standard output
//! and error together in the order written, goes to an [`OutBuf`]; killing it kills the group.
//!
//! Running a command blocks: it is done on a pool, never on an I/O thread.

use std::io::Read;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::model::MAX_JOB_OUTPUT;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the node runs as root (its effective user, akio-sys's `process::euid`).
#[must_use]
pub fn is_root() -> bool {
    akio_sys::process::euid() == 0
}

/// A fixed command built in code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Command {
    /// Run as it is, the first element looked up on `PATH`.
    pub argv: Vec<String>,
    /// Extra `KEY=VALUE` pairs.
    pub env: Vec<String>,
    /// The working directory, `""` for the node's.
    pub dir: String,
    /// It needs root: [`Command::sudo`] prefixes it with `sudo -n`.
    pub root: bool,
}

impl Command {
    /// The command prefixed with `sudo -n` (keeping its environment) when it needs root,
    /// `enabled` is set and the node does not run as root.
    #[must_use]
    pub fn sudo(mut self, enabled: bool) -> Command {
        if !self.root || !enabled || is_root() {
            return self;
        }
        let mut argv = vec!["sudo".to_owned(), "-n".to_owned()];
        if !self.env.is_empty() {
            let keys: Vec<&str> = self
                .env
                .iter()
                .map(|kv| kv.split_once('=').map_or(kv.as_str(), |(k, _)| k))
                .collect();
            argv.push(format!("--preserve-env={}", keys.join(",")));
        }
        argv.append(&mut self.argv);
        self.argv = argv;
        self
    }

    /// The command for logs and a job's output (a script shown by its first line).
    #[must_use]
    pub fn display(&self) -> String {
        let mut parts = Vec::with_capacity(self.argv.len());
        for (i, a) in self.argv.iter().enumerate() {
            let after_c = i
                .checked_sub(1)
                .and_then(|j| self.argv.get(j))
                .is_some_and(|p| p == "-c");
            if after_c && a.contains('\n') {
                let first = a.trim().split('\n').next().unwrap_or("");
                parts.push(format!("'{first} ...'"));
                continue;
            }
            if a.is_empty()
                || a.chars().any(|c| {
                    matches!(
                        c,
                        ' ' | '\t'
                            | '\n'
                            | '\''
                            | '"'
                            | '\\'
                            | '$'
                            | '`'
                            | ';'
                            | '&'
                            | '|'
                            | '<'
                            | '>'
                            | '('
                            | ')'
                            | '{'
                            | '}'
                            | '*'
                            | '?'
                    )
                })
            {
                parts.push(format!("'{}'", a.replace('\'', r"'\''")));
            } else {
                parts.push(a.clone());
            }
        }
        parts.join(" ")
    }

    /// Runs the command in a process group of its own, its output written to `out`, killed
    /// with its group by `kill` or after `timeout`. Blocks until it ends and its output is read.
    /// Returns the exit code (-1 when it did not exit) and why it failed.
    pub fn run(&self, out: &OutBuf, kill: &Killer, timeout: Duration) -> (i64, Result<(), String>) {
        let Some(name) = self.argv.first().filter(|n| !n.is_empty()) else {
            return (-1, Err("empty command".to_owned()));
        };
        let Some(path) = look_path(name) else {
            return (
                -1,
                Err(format!(
                    "exec: {name:?}: executable file not found in $PATH"
                )),
            );
        };
        let (mut rd, wr) = match std::os::unix::net::UnixStream::pair() {
            Ok(p) => p,
            Err(e) => return (-1, Err(e.to_string())),
        };
        let wr2 = match wr.try_clone() {
            Ok(w) => w,
            Err(e) => return (-1, Err(e.to_string())),
        };
        let mut cmd = std::process::Command::new(path);
        cmd.args(self.argv.iter().skip(1))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(std::os::fd::OwnedFd::from(wr)))
            .stderr(std::process::Stdio::from(std::os::fd::OwnedFd::from(wr2)));
        for kv in &self.env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        if !self.dir.is_empty() {
            cmd.current_dir(&self.dir);
        }
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return (-1, Err(e.to_string())),
        };
        // The parent's ends of the socket pair went with `cmd`'s Stdio when it spawned.
        drop(cmd);
        kill.started(child.id());
        let timer = (!timeout.is_zero())
            .then(|| {
                let k = kill.clone();
                akio::timer::after_send(timeout, move || k.kill(Ended::Timeout)).ok()
            })
            .flatten();
        let mut buf = vec![0u8; 32 << 10];
        loop {
            match rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.write(buf.get(..n).unwrap_or(&[])),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let status = child.wait();
        kill.exited();
        if let Some(t) = timer {
            t.cancel();
        }
        if let Some(why) = kill.why() {
            return (-1, Err(why.to_string()));
        }
        match status {
            Ok(s) if s.success() => (0, Ok(())),
            Ok(s) => match s.code() {
                Some(c) => (i64::from(c), Err(format!("exit status {c}"))),
                None => (-1, Err(s.to_string())),
            },
            Err(e) => (-1, Err(e.to_string())),
        }
    }
}

/// Why a command was killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Its job was canceled.
    Canceled,
    /// It ran past its timeout.
    Timeout,
}

impl std::fmt::Display for Ended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Ended::Canceled => "context canceled",
            Ended::Timeout => "context deadline exceeded",
        })
    }
}

#[derive(Default)]
struct KillSt {
    pid: Option<u32>,
    why: Option<Ended>,
}

/// Kills a running command's process group, from any thread; never blocks: the group (the
/// command started as its leader, the group's ID its own) gets SIGKILL from akio-sys's
/// `process::kill_group`.
#[derive(Clone, Default)]
pub struct Killer {
    st: Arc<Mutex<KillSt>>,
}

impl std::fmt::Debug for Killer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Killer").finish_non_exhaustive()
    }
}

impl Killer {
    /// A killer of the command to come.
    #[must_use]
    pub fn new() -> Killer {
        Killer::default()
    }

    fn started(&self, pid: u32) {
        let why = {
            let mut g = lock(&self.st);
            g.pid = Some(pid);
            g.why
        };
        if let Some(w) = why {
            // Killed before it started.
            self.kill(w);
        }
    }

    fn exited(&self) {
        lock(&self.st).pid = None;
    }

    /// Why it was killed, if it was.
    #[must_use]
    pub fn why(&self) -> Option<Ended> {
        lock(&self.st).why
    }

    /// Kills the command and its group for `why`; the first reason stays.
    pub fn kill(&self, why: Ended) {
        let mut g = lock(&self.st);
        g.why.get_or_insert(why);
        // Under the lock: until the leader is reaped (then `exited`), its ID, its group's, is
        // not reused. The leader signalled alone too, should it have left its group.
        if let Some(pid) = g.pid {
            let _ = akio_sys::process::kill(pid, akio_sys::process::SIGKILL);
            let _ = akio_sys::process::kill_group(pid, akio_sys::process::SIGKILL);
        }
    }
}

thread_local! {
    static TOOLS: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

static GLOBAL_TOOLS: std::sync::RwLock<Option<std::path::PathBuf>> = std::sync::RwLock::new(None);

/// Tools are looked up in `dir` alone instead of `PATH` on this thread (`None`: `PATH` again):
/// tests' stand-ins for the vendors' tools, as fit's tests set `PATH`, which a Rust test may
/// not change for its process.
#[doc(hidden)]
pub fn tools_on_thread(dir: Option<std::path::PathBuf>) {
    TOOLS.with(|t| *t.borrow_mut() = dir);
}

/// As [`tools_on_thread`], for every thread of the process (an integration test's).
#[doc(hidden)]
pub fn tools_in(dir: Option<std::path::PathBuf>) {
    *GLOBAL_TOOLS.write().unwrap_or_else(PoisonError::into_inner) = dir;
}

/// `name` found as Go's `exec.LookPath` finds it: as it is when it has a slash, else the first
/// executable file of that name in a directory of `PATH`.
#[must_use]
pub fn look_path(name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &std::path::Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        return exe(&p).then_some(p);
    }
    let dir = TOOLS.with(|t| t.borrow().clone()).or_else(|| {
        GLOBAL_TOOLS
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    });
    if let Some(d) = dir {
        let p = d.join(name);
        return exe(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| {
            if d.as_os_str().is_empty() {
                std::path::PathBuf::from(".").join(name)
            } else {
                d.join(name)
            }
        })
        .find(|p| exe(p))
}

/// Runs a short command and returns its output (standard output alone, or with its errors
/// when `combined`), within `timeout`; blocks. Its error says why it failed, the output kept.
///
/// # Errors
///
/// Not found, not started, killed at `timeout`, or a status other than 0.
pub fn output(
    argv: &[&str],
    timeout: Duration,
    combined: bool,
) -> Result<String, (String, String)> {
    let name = argv.first().copied().unwrap_or("");
    let c = Command {
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        ..Command::default()
    };
    if combined {
        let out = OutBuf::new();
        let (_, r) = c.run(&out, &Killer::new(), timeout);
        let (text, _, _) = out.from(0);
        return r
            .map(|()| text.clone())
            .map_err(|e| (text, format!("{name}: {e}")));
    }
    // Standard output alone, its errors dropped.
    let Some(path) = look_path(name) else {
        return Err((
            String::new(),
            format!("exec: {name:?}: executable file not found in $PATH"),
        ));
    };
    let mut cmd = std::process::Command::new(path);
    cmd.args(argv.iter().skip(1))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| (String::new(), e.to_string()))?;
    let k = Killer::new();
    k.started(child.id());
    let k2 = k.clone();
    let timer = akio::timer::after_send(timeout, move || k2.kill(Ended::Timeout)).ok();
    let mut text = Vec::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_end(&mut text);
    }
    let status = child.wait();
    k.exited();
    if let Some(t) = timer {
        t.cancel();
    }
    let text = String::from_utf8_lossy(&text).into_owned();
    match (k.why(), status) {
        (Some(w), _) => Err((text, format!("{name}: {w}"))),
        (None, Ok(s)) if s.success() => Ok(text),
        (None, Ok(s)) => Err((text, format!("{name}: {s}"))),
        (None, Err(e)) => Err((text, format!("{name}: {e}"))),
    }
}

type Waker = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct OutSt {
    b: Vec<u8>,
    base: usize,
    limit: usize,
    epoch: u64,
    waiters: Vec<Waker>,
}

/// A job's output (fit's `outBuf`): the last [`MAX_JOB_OUTPUT`] bytes kept, offsets absolute
/// (the bytes written since the start), so that a follower's stays good when the oldest part is
/// dropped. Followers are woken at the next write or [`OutBuf::notify`].
#[derive(Clone, Default)]
pub struct OutBuf(Arc<Mutex<OutSt>>);

impl std::fmt::Debug for OutBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutBuf").finish_non_exhaustive()
    }
}

impl OutBuf {
    /// An empty output.
    #[must_use]
    pub fn new() -> OutBuf {
        OutBuf::default()
    }

    /// An empty output keeping `limit` bytes (0: [`MAX_JOB_OUTPUT`]).
    #[must_use]
    pub fn with_limit(limit: usize) -> OutBuf {
        let o = OutBuf::default();
        lock(&o.0).limit = limit;
        o
    }

    fn wake(g: &mut OutSt) -> Vec<Waker> {
        g.epoch = g.epoch.wrapping_add(1);
        std::mem::take(&mut g.waiters)
    }

    /// Appends `p`, dropping the oldest part past the limit (a quarter more than needed, so that
    /// a chatty job does not copy the buffer on every write).
    pub fn write(&self, p: &[u8]) {
        if p.is_empty() {
            return;
        }
        let ws = {
            let mut g = lock(&self.0);
            let limit = if g.limit == 0 {
                MAX_JOB_OUTPUT
            } else {
                g.limit
            };
            g.b.extend_from_slice(p);
            let over = g.b.len().saturating_sub(limit);
            if over > 0 {
                let drop = over.saturating_add(limit / 4).min(g.b.len());
                g.b.drain(..drop);
                g.base = g.base.saturating_add(drop);
            }
            Self::wake(&mut g)
        };
        for w in ws {
            w();
        }
    }

    /// Wakes the followers without writing (the job's state changed).
    pub fn notify(&self) {
        let ws = Self::wake(&mut lock(&self.0));
        for w in ws {
            w();
        }
    }

    /// The output's generation: it changes at every write and [`OutBuf::notify`].
    #[must_use]
    pub fn generation(&self) -> u64 {
        lock(&self.0).epoch
    }

    /// Runs `f` once the generation is no longer `epoch`: at once if it changed already.
    pub fn on_change(&self, epoch: u64, f: impl FnOnce() + Send + 'static) {
        let mut g = lock(&self.0);
        if g.epoch != epoch {
            drop(g);
            f();
            return;
        }
        g.waiters.push(Box::new(f));
    }

    /// The output from absolute `offset` (moved to the oldest byte kept, and back to a
    /// character's start), the offset used, and the length written.
    #[must_use]
    pub fn from(&self, offset: usize) -> (String, usize, usize) {
        let g = lock(&self.0);
        let end = g.base.saturating_add(g.b.len());
        let offset = if offset < g.base || offset > end {
            g.base
        } else {
            offset
        };
        let start = |i: usize| g.b.get(i).is_some_and(|b| (*b & 0xC0) != 0x80);
        let mut i = offset.saturating_sub(g.base);
        while i > 0 && i < g.b.len() && !start(i) {
            i = i.saturating_sub(1);
        }
        // The first byte kept may be inside a character: skip it.
        if i == 0 && g.base > 0 {
            while i < g.b.len() && !start(i) {
                i = i.saturating_add(1);
            }
        }
        let text = String::from_utf8_lossy(g.b.get(i..).unwrap_or(&[])).into_owned();
        (text, g.base.saturating_add(i), end)
    }

    /// The length written.
    #[must_use]
    pub fn len(&self) -> usize {
        let g = lock(&self.0);
        g.base.saturating_add(g.b.len())
    }

    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
