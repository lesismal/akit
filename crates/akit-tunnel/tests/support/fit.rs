//! fit's binary run by a test (akit-tunnel's, akit-llm's and akit-webrtc's, by `#[path]`): in a
//! directory of its own, stopped when dropped, the lines it logs read as they come. Its listeners
//! bind port 0, as akit's do, and the test reads where from what fit logs ([`Fit::addr`]): a port
//! chosen by the test and named in fit's config could be taken by another process before fit
//! binds it.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// How long fit has to log what [`Fit::addr`] waits for.
const STARTUP: Duration = Duration::from_secs(15);

/// fit's binary running a config.
pub struct Fit {
    child: Child,
    dir: PathBuf,
    lines: Receiver<String>,
    /// The lines read so far.
    seen: Mutex<Vec<String>>,
}

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Runs `bin run -c` with `doc` written in `dir` (made here, removed with the [`Fit`]); `cmd`
/// may set more of the command (its environment).
pub fn start(bin: &str, dir: PathBuf, doc: &str, cmd: impl FnOnce(&mut Command)) -> Fit {
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fit.json");
    std::fs::write(&path, doc).unwrap();
    let mut c = Command::new(bin);
    c.args(["run", "-c"])
        .arg(&path)
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd(&mut c);
    let mut child = c.spawn().unwrap();
    let (tx, lines) = channel();
    let tag = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out: Box<dyn std::io::Read + Send> = Box::new(child.stdout.take().unwrap());
    let err: Box<dyn std::io::Read + Send> = Box::new(child.stderr.take().unwrap());
    for r in [out, err] {
        let (tx, tag) = (tx.clone(), tag.clone());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(r).lines() {
                let Ok(line) = line else { break };
                eprintln!("{tag}: {line}");
                let _ = tx.send(line);
            }
        });
    }
    Fit {
        child,
        dir,
        lines,
        seen: Mutex::new(Vec::new()),
    }
}

impl Fit {
    /// The address in the first line fit logs that holds `has`, the text after `before` in it up
    /// to a '/', a ',' or a space (`127.0.0.1:1234` from `on ws://127.0.0.1:1234/`), waiting for that
    /// line up to [`STARTUP`].
    pub fn addr_in(&self, has: &str, before: &str) -> String {
        let pick = |line: &str| {
            if !line.contains(has) {
                return None;
            }
            let (_, rest) = line.split_once(before)?;
            rest.split(|c: char| c == '/' || c == ',' || c.is_whitespace())
                .next()
                .map(str::to_owned)
        };
        let mut seen = self.seen.lock().unwrap();
        if let Some(a) = seen.iter().find_map(|l| pick(l)) {
            return a;
        }
        let deadline = Instant::now() + STARTUP;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("fit logged no {has:?}: {e}"));
            let a = pick(&line);
            seen.push(line);
            if let Some(a) = a {
                return a;
            }
        }
    }

    /// [`addr_in`](Self::addr_in) of the line holding `before`.
    pub fn addr(&self, before: &str) -> String {
        self.addr_in(before, before)
    }

    /// The port of [`addr_in`](Self::addr_in).
    pub fn port_in(&self, has: &str, before: &str) -> u16 {
        let a = self.addr_in(has, before);
        a.rsplit_once(':')
            .and_then(|(_, p)| p.parse().ok())
            .unwrap_or_else(|| panic!("no port in {a:?}"))
    }

    /// The port of [`addr`](Self::addr).
    pub fn port(&self, before: &str) -> u16 {
        self.port_in(before, before)
    }

    /// What fit logged so far, its lines joined.
    pub fn log(&self) -> String {
        let mut seen = self.seen.lock().unwrap();
        while let Ok(line) = self.lines.try_recv() {
            seen.push(line);
        }
        seen.join("\n")
    }
}
