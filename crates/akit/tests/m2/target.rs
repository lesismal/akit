//! akit served for M2's exit tests: as `akit run` (its config swapped through SIGHUP and the
//! Admin API, as an operator does), or in process, as `akit run` builds it (its config swapped
//! through `Listeners::reload`, the path both take), on the backend `AKIO_TEST_BACKEND` names.
#![allow(
    dead_code,
    unreachable_pub,
    missing_debug_implementations,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use akio::{Backend, UringConfig};
use akio_sys::process::{SIGHUP, SIGTERM, kill};
use akit_listener::{Listeners, Reloaded};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Previous, Registry, compile};

/// How long `akit run` has to print what a test waits for.
const PRINT: Duration = Duration::from_secs(20);

fn lines<R: Read + Send + 'static>(r: R) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// `akit run` on a config file, killed if the test fails before it stops.
pub struct Proc {
    child: Child,
    out: mpsc::Receiver<String>,
    err: mpsc::Receiver<String>,
    path: PathBuf,
    /// The Admin API's address, when the config has one.
    pub api: Option<SocketAddr>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Proc {
    /// Writes `json` to `path` and runs `akit run` on it, its drain timeout `drain` (a Go
    /// duration), until it prints a line starting with `ready`.
    pub fn start(
        path: &Path,
        json: &str,
        api: Option<SocketAddr>,
        drain: &str,
        ready: &str,
    ) -> Proc {
        std::fs::write(path, json).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(["run", "-shutdown-timeout", drain, "-c"])
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = lines(child.stdout.take().unwrap());
        let err = lines(child.stderr.take().unwrap());
        let p = Proc {
            child,
            out,
            err,
            path: path.to_owned(),
            api,
        };
        p.until(ready);
        p
    }

    /// stdout's lines up to and including the first starting with `last`; what stderr said
    /// meanwhile when it never comes.
    pub fn until(&self, last: &str) -> Vec<String> {
        let mut v = Vec::new();
        loop {
            match self.out.recv_timeout(PRINT) {
                Ok(line) => {
                    let done = line.starts_with(last);
                    v.push(line);
                    if done {
                        return v;
                    }
                }
                Err(e) => {
                    let errs: Vec<String> = self.err.try_iter().collect();
                    panic!("waiting for {last:?} after {v:?}: {e}; stderr: {errs:?}");
                }
            }
        }
    }

    /// stderr's lines so far.
    pub fn stderr(&self) -> Vec<String> {
        self.err.try_iter().collect()
    }

    /// `json` written to the config file and applied by SIGHUP.
    pub fn sighup(&self, json: &str) {
        std::fs::write(&self.path, json).unwrap();
        kill(self.child.id(), SIGHUP).unwrap();
        self.until("akit: reloaded");
    }

    /// `json` loaded through the Admin API's `POST /load`.
    pub fn load(&self, json: &str) {
        let (status, body) = self.call("POST", "/load", Some(json));
        assert_eq!(status, 200, "POST /load: {body}");
        self.until("akit: loaded");
    }

    /// `json` set at `pointer` through the Admin API's `PATCH /config/{pointer}`.
    pub fn patch(&self, pointer: &str, json: &str) {
        let (status, body) = self.call("PATCH", &format!("/config{pointer}"), Some(json));
        assert_eq!(status, 200, "PATCH {pointer}: {body}");
        self.until("akit: loaded");
    }

    /// A request to the Admin API: the status and the body.
    pub fn call(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let api = self.api.expect("a config with an Admin API");
        let mut s = TcpStream::connect(api).unwrap();
        s.set_read_timeout(Some(PRINT)).unwrap();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {api}\r\nConnection: close\r\n");
        if let Some(b) = body {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                b.len()
            ));
        } else {
            req.push_str("\r\n");
        }
        s.write_all(req.as_bytes()).unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, body.to_owned())
    }

    /// The scrape at the Admin API's `GET /metrics`.
    pub fn metrics(&self) -> String {
        let (status, body) = self.call("GET", "/metrics", None);
        assert_eq!(status, 200, "{body}");
        body
    }

    /// The process's open file descriptors: from `/proc` on Linux, `lsof` elsewhere.
    pub fn fds(&self) -> Option<usize> {
        match std::fs::read_dir(format!("/proc/{}/fd", self.child.id())) {
            Ok(d) => Some(d.count()),
            Err(_) => self.lsof().map(|v| v.len()),
        }
    }

    /// `lsof`'s descriptors of the process, each its type and name.
    fn lsof(&self) -> Option<Vec<(String, String)>> {
        let out = Command::new("lsof")
            .args(["-n", "-P", "-F", "ftn", "-p"])
            .arg(self.child.id().to_string())
            .output()
            .ok()?;
        let mut v: Vec<(String, String)> = Vec::new();
        let mut numbered = false;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            match line.split_at_checked(1) {
                Some(("f", fd)) => {
                    numbered = fd.chars().all(|c| c.is_ascii_digit());
                    if numbered {
                        v.push((String::new(), String::new()));
                    }
                }
                Some(("t", t)) if numbered => {
                    if let Some(last) = v.last_mut() {
                        last.0 = t.to_owned();
                    }
                }
                Some(("n", n)) if numbered => {
                    if let Some(last) = v.last_mut() {
                        last.1 = n.to_owned();
                    }
                }
                _ => {}
            }
        }
        (!v.is_empty()).then_some(v)
    }

    /// What the process's descriptors are, counted by kind (through `lsof` off Linux): TCP sockets by state and
    /// local and remote port, the rest by what they link to.
    pub fn fd_kinds(&self) -> std::collections::BTreeMap<String, usize> {
        let mut tcp = std::collections::HashMap::new();
        for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
            for line in std::fs::read_to_string(f)
                .unwrap_or_default()
                .lines()
                .skip(1)
            {
                let w: Vec<&str> = line.split_whitespace().collect();
                if w.len() > 9 {
                    let port = |a: &str| {
                        a.rsplit(':')
                            .next()
                            .and_then(|p| u16::from_str_radix(p, 16).ok())
                            .unwrap_or(0)
                    };
                    let kind = format!("tcp {} :{} -> :{}", w[3], port(w[1]), port(w[2]));
                    tcp.insert(w[9].to_owned(), kind);
                }
            }
        }
        let mut kinds = std::collections::BTreeMap::new();
        let dir = format!("/proc/{}/fd", self.child.id());
        if std::fs::metadata(&dir).is_err() {
            for (t, n) in self.lsof().unwrap_or_default() {
                // A connection by its remote port, a listener by its own.
                let kind = match n.split_once("->") {
                    Some((_, to)) => format!("{t} -> :{}", to.rsplit(':').next().unwrap_or("")),
                    None if t.starts_with("IPv") => format!("{t} {n}"),
                    None => t,
                };
                *kinds.entry(kind).or_insert(0) += 1;
            }
            return kinds;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let link = std::fs::read_link(e.path())
                .map(|l| l.display().to_string())
                .unwrap_or_default();
            let kind = match link
                .strip_prefix("socket:[")
                .and_then(|l| l.strip_suffix(']'))
            {
                Some(inode) => tcp.get(inode).cloned().unwrap_or_else(|| "socket".into()),
                None if link.starts_with('/') => "file".into(),
                None => link.split(':').next().unwrap_or("").to_owned(),
            };
            *kinds.entry(kind).or_insert(0) += 1;
        }
        kinds
    }

    /// SIGTERM: a graceful stop, which must exit 0.
    pub fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        self.until("akit: stopped");
        let status = self.child.wait().unwrap();
        assert_eq!(status.code(), Some(0), "stderr: {:?}", self.stderr());
    }
}

/// The listeners' options: the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`),
/// a drain timeout of `drain`.
pub fn options(drain: Duration) -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = drain;
    o
}

/// akit in process, built as `akit run` builds it: the graph with its pools, then the engine,
/// then the slot the pools dial through filled; akit's own modules.
pub struct Inproc {
    slot: EngineSlot,
    pub cell: GraphCell,
    pub engine: Arc<akio::Engine>,
    listeners: Option<Listeners>,
}

impl Inproc {
    /// `json` served, the graphs a reload replaces drained for `drain`.
    pub fn start(json: &str, drain: Duration) -> Inproc {
        let slot = EngineSlot::new();
        let cell = GraphCell::new(graph(json, &slot, None));
        let engine = Arc::new(akit_listener::engine(&cell, &options(drain)).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let listeners = Listeners::new(engine.clone(), cell.clone(), &options(drain));
        Inproc {
            slot,
            cell,
            engine,
            listeners: Some(listeners),
        }
    }

    /// `json` compiled and swapped in, as SIGHUP and the Admin API do: the running graph's
    /// pools shared for the upstreams that stay as they are, and the graphs it replaced retired
    /// once no request uses them, as `akit run`'s control thread does after each swap.
    pub fn reload(&mut self, json: &str) -> Reloaded {
        let g = graph(json, &self.slot, Some(self.cell.with(Previous::of)));
        let r = self.listeners.as_mut().unwrap().reload(g).unwrap();
        self.cell.collect();
        r
    }

    pub fn served(&self) -> Vec<akit_listener::Served> {
        self.listeners.as_ref().unwrap().served()
    }
}

impl Drop for Inproc {
    fn drop(&mut self) {
        drop(self.listeners.take());
        let stopped = akit_listener::shutdown(&self.engine, Duration::from_secs(5));
        if !std::thread::panicking() {
            assert!(stopped, "the engine did not stop");
        }
    }
}

fn graph(json: &str, slot: &EngineSlot, previous: Option<Previous>) -> Graph {
    let config = akit_config::from_json(json, None).unwrap();
    let registry: Registry<HandlerParams> = Registry::new(akit::STANDARD).unwrap();
    let env = Env::new(slot.clone()).with_previous(previous);
    compile(&config, &registry, &env).unwrap()
}
