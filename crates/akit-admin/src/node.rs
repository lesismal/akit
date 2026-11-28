//! Who this akit is, for a manager of many (I131, fit's `admin/node.go`, its I140): `GET /node`
//! and the node ID it answers, made once and kept in the store's section `node`, or in a file
//! beside the config (`akit.node-id` beside `akit.json`), or set by `admin.node_id`; and `GET
//! /schema`, the config's JSON Schema, whose digest `GET /node` gives.
//!
//! The answer is fit's protocol (decision 041): its members, fit's capability names for what
//! akit's API serves, and the store's section; the member fit names after itself is renamed as
//! decision 037 renames it everywhere (`akit_version`), and fit's `fib_version` and `go_version`
//! carry akit's engine (`akio <commit>`) and nothing (decision row 71).

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use akio_crypto::hash::Sha256;
use akio_http::Response;
use akit_config::value::write_string;

use crate::configs::Configs;
use crate::server::{VERSION_HEADER, json};
use crate::store::Store;

/// Where a node ID is kept ([`Node::id_kept`]): `admin.node_id`.
pub const ID_IN_CONFIG: &str = "config";
/// Kept in the store, in its section [`NODE_SECTION`].
pub const ID_IN_STORE: &str = "store";
/// Kept in a file beside the config, its files' path without their extension and
/// [`NODE_ID_SUFFIX`].
pub const ID_IN_FILE: &str = "file";
/// Kept nowhere: made again at each start.
pub const ID_IN_MEMORY: &str = "memory";

/// The store's section that keeps the node ID: fit's.
pub const NODE_SECTION: &str = "node";
/// What the file beside the config that keeps it without a store ends with.
pub const NODE_ID_SUFFIX: &str = ".node-id";

/// The capabilities `GET /node` lists, fit's names (`admin.Cap*`): what a manager may use on this
/// akit, which it checks before it calls the endpoints they name. Each is listed only when akit
/// serves what it names as fit's does.
pub const CAP_NODE: &str = "node";
/// Every config change takes `If-Match`.
pub const CAP_IF_MATCH: &str = "config.if_match";
/// `?dry_run=1` on the config's changes.
pub const CAP_DRY_RUN: &str = "config.dry_run";
/// `POST /config/render`, `/config/parse` and `PUT /config/text`.
pub const CAP_TEXT: &str = "config.text";
/// `GET /config/effective`.
pub const CAP_EFFECTIVE: &str = "config.effective";
/// `GET /versions`, `/versions/{n}` and `POST /rollback/{n}`.
pub const CAP_VERSIONS: &str = "config.versions";
/// `POST /config/export` and `/config/import`.
pub const CAP_TRANSFER: &str = "config.transfer";
/// `POST /reload`.
pub const CAP_RELOAD: &str = "config.reload";
/// `GET /status`.
pub const CAP_STATUS: &str = "status";
/// `GET /config/check` and `POST /config/reconcile` (I138).
pub const CAP_CHECK: &str = "config.check";
/// `GET /status` answers `requests`, when the running config counts them (`metrics`).
pub const CAP_STATUS_REQUESTS: &str = "status.requests";
/// `GET /metrics`.
pub const CAP_METRICS: &str = "metrics";
/// `GET /cache` and `POST /cache/purge`.
pub const CAP_CACHE: &str = "cache";
/// `POST /cache/purge` takes `prefixes`.
pub const CAP_CACHE_PREFIX: &str = "cache.prefix";
/// `GET /logs`, `POST /logs/rotate` and `/logs/cleanup`.
pub const CAP_LOGS: &str = "logs";
/// `GET /logs/recent`: the latest messages of akit's own log, after a number (I139c).
pub const CAP_LOGS_RECENT: &str = "logs.recent";
/// The link tokens, `/tokens`.
pub const CAP_TOKENS: &str = "tokens";
/// `GET /schema`.
pub const CAP_SCHEMA: &str = "schema";
/// `admin.store` keeps the config and the console users.
pub const CAP_STORE: &str = "store";
/// `admin.console` runs: its login answers `{"user", "csrf"}`, and every request but a read
/// sends the token in `X-Akit-Csrf` (fit's `X-Fit-Csrf`, decision 037).
pub const CAP_CONSOLE: &str = "console";

/// Who this akit is, as `GET /node` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The node ID.
    pub id: String,
    /// Where it is kept: [`ID_IN_CONFIG`], [`ID_IN_STORE`], [`ID_IN_FILE`] or [`ID_IN_MEMORY`].
    pub id_kept: &'static str,
    /// The host's name.
    pub hostname: String,
    /// akit's version.
    pub version: String,
    /// The engine's: `akio <commit>`.
    pub engine: String,
    /// When akit started.
    pub started: SystemTime,
}

impl Node {
    /// A node whose ID is kept nowhere, made now, on this host.
    #[must_use]
    pub fn new() -> Node {
        Node {
            id: new_node_id(),
            id_kept: ID_IN_MEMORY,
            hostname: akit_config::roles::host_name(),
            version: String::new(),
            engine: String::new(),
            started: SystemTime::now(),
        }
    }

    /// `n`, with what it leaves empty taken from `self`: fit's `UseNode`.
    #[must_use]
    pub fn merged(&self, n: Node) -> Node {
        let pick = |a: String, b: &str| if a.is_empty() { b.to_owned() } else { a };
        let (id, id_kept) = if n.id.is_empty() {
            (self.id.clone(), self.id_kept)
        } else {
            (n.id, n.id_kept)
        };
        Node {
            id,
            id_kept,
            hostname: pick(n.hostname, &self.hostname),
            version: pick(n.version, &self.version),
            engine: pick(n.engine, &self.engine),
            started: n.started,
        }
    }
}

impl Default for Node {
    fn default() -> Node {
        Node::new()
    }
}

/// What `GET /node` reads of the running akit each time: the roles running and whether the
/// running config counts requests. Without it, none and no.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeLive {
    /// The IDs of the roles running, as `tunnel.hub`.
    pub roles: Vec<String>,
    /// Whether `GET /status` answers `requests`.
    pub counts_requests: bool,
}

/// How the Admin API reads [`NodeLive`]: on its I/O thread, so it must not block.
pub type NodeView = Arc<dyn Fn() -> NodeLive + Send + Sync>;

/// A node ID: a random UUID, version 4, as fit's `newNodeID`.
#[must_use]
pub fn new_node_id() -> String {
    let mut b = [0u8; 16];
    let _ = akio_crypto::rand::fill_random(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = akit_roles::linktoken::hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        h.get(..8).unwrap_or(""),
        h.get(8..12).unwrap_or(""),
        h.get(12..16).unwrap_or(""),
        h.get(16..20).unwrap_or(""),
        h.get(20..).unwrap_or("")
    )
}

/// This akit's node ID and where it is kept, fit's `NodeID`: in `store` when there is one, else
/// in the file beside the config whose files' path without their extension is `prefix`; made,
/// and kept, the first time. An ID in the file is taken into a store that has none yet, so that
/// an akit given a store keeps its ID. With no prefix and no store, or when keeping it fails, the
/// ID is kept in memory only, with why when something failed. Reads and writes files: call it
/// before serving, on the control thread.
pub fn node_id(
    store: Option<&mut Store>,
    prefix: Option<&str>,
) -> (String, &'static str, Option<String>) {
    if let Some(st) = store {
        if let Some(sec) = st.section(NODE_SECTION) {
            let id = std::str::from_utf8(sec)
                .ok()
                .and_then(|t| akit_config::value::parse(t).ok())
                .and_then(|v| match v.get("id") {
                    Some(akit_config::value::Value::String(s)) => Some(s.clone()),
                    _ => None,
                })
                .filter(|s| akit_config::cluster::valid_node_id(s));
            return match id {
                Some(id) => (id, ID_IN_STORE, None),
                None => (
                    new_node_id(),
                    ID_IN_MEMORY,
                    Some(format!(
                        "the store's section {NODE_SECTION:?} holds no node ID"
                    )),
                ),
            };
        }
        let id = read_node_id(prefix)
            .ok()
            .flatten()
            .unwrap_or_else(new_node_id);
        let mut data = String::from("{\"id\":");
        write_string(&mut data, &id);
        data.push('}');
        return match st.set_section(NODE_SECTION, data.as_bytes()) {
            Ok(()) => (id, ID_IN_STORE, None),
            Err(e) => (id, ID_IN_MEMORY, Some(e.to_string())),
        };
    }
    let Some(prefix) = prefix.filter(|p| !p.is_empty()) else {
        return (new_node_id(), ID_IN_MEMORY, None);
    };
    match read_node_id(Some(prefix)) {
        Err(e) => (new_node_id(), ID_IN_MEMORY, Some(e)),
        Ok(Some(id)) => (id, ID_IN_FILE, None),
        Ok(None) => {
            let id = new_node_id();
            match write_node_id(&format!("{prefix}{NODE_ID_SUFFIX}"), &id) {
                Ok(()) => (id, ID_IN_FILE, None),
                Err(e) => (id, ID_IN_MEMORY, Some(e.to_string())),
            }
        }
    }
}

/// The ID in the file beside the config, `None` when there is no file.
fn read_node_id(prefix: Option<&str>) -> Result<Option<String>, String> {
    let Some(prefix) = prefix.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let path = format!("{prefix}{NODE_ID_SUFFIX}");
    match fs::read_to_string(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{path}: {e}")),
        Ok(data) => {
            let id = data.trim();
            if akit_config::cluster::valid_node_id(id) {
                Ok(Some(id.to_owned()))
            } else {
                Err(format!(
                    "{path} holds no node ID: 1 to 64 letters, digits, '.', '_' and '-'"
                ))
            }
        }
    }
}

/// Writes `id` to `path`, through a file beside it renamed over it, 0644 as fit's.
fn write_node_id(path: &str, id: &str) -> io::Result<()> {
    let p = Path::new(path);
    let dir = p
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut suffix = [0u8; 8];
    let _ = akio_crypto::rand::fill_random(&mut suffix);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!("{name}.{}", akit_roles::linktoken::hex(&suffix)));
    let r = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(id.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
        }
        fs::rename(&tmp, p)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// The schema `GET /schema` answers: the console's, fit's schema renamed (I91).
#[must_use]
pub fn schema() -> &'static [u8] {
    crate::console::pages::SCHEMA
}

/// `sha256:` and the hex SHA-256 of [`schema`]: two akits with the same digest take the same
/// configs.
#[must_use]
pub fn schema_digest() -> &'static str {
    static DIGEST: OnceLock<String> = OnceLock::new();
    DIGEST.get_or_init(|| {
        format!(
            "sha256:{}",
            akit_roles::linktoken::hex(&Sha256::digest(schema()))
        )
    })
}

fn time(b: &mut String, t: SystemTime) {
    let mut v = Vec::new();
    akit_observe::time::rfc3339(&mut v, akit_observe::time::Stamp::of(t), 0);
    write_string(b, &String::from_utf8_lossy(&v));
}

/// The capabilities this akit's API serves, sorted.
fn capabilities(configs: &Configs, live: &NodeLive, metrics: bool) -> Vec<&'static str> {
    let mut caps = vec![
        CAP_NODE,
        CAP_IF_MATCH,
        CAP_DRY_RUN,
        CAP_TEXT,
        CAP_EFFECTIVE,
        CAP_VERSIONS,
        CAP_TRANSFER,
        CAP_RELOAD,
        CAP_STATUS,
        CAP_CHECK,
        CAP_CACHE,
        CAP_CACHE_PREFIX,
        CAP_LOGS,
        CAP_LOGS_RECENT,
        CAP_TOKENS,
        CAP_SCHEMA,
        crate::managed::CAP_MANAGED,
    ];
    if metrics {
        caps.push(CAP_METRICS);
    }
    if configs.sealer().is_some() {
        caps.push(CAP_STORE);
    }
    if live.counts_requests {
        caps.push(CAP_STATUS_REQUESTS);
    }
    if configs.console_running() {
        caps.push(CAP_CONSOLE);
    }
    caps.sort_unstable();
    caps
}

/// `GET /node`'s answer: who this akit is, what config runs and what the API offers, with the
/// running config's ETag and version in its headers.
pub(crate) fn answer(configs: &Configs, live: Option<&NodeView>, metrics: bool) -> Response {
    let n = configs.node();
    let v = configs.current();
    let live = live.map(|f| f()).unwrap_or_default();
    let mut b = String::from("{\"id\":");
    write_string(&mut b, &n.id);
    b.push_str(",\"id_kept\":");
    write_string(&mut b, n.id_kept);
    b.push_str(",\"hostname\":");
    write_string(&mut b, &n.hostname);
    b.push_str(",\"akit_version\":");
    write_string(&mut b, &n.version);
    b.push_str(",\"fib_version\":");
    write_string(&mut b, &n.engine);
    b.push_str(",\"go_version\":\"\",\"os\":");
    write_string(&mut b, std::env::consts::OS);
    b.push_str(",\"arch\":");
    write_string(&mut b, std::env::consts::ARCH);
    b.push_str(",\"started\":");
    time(&mut b, n.started);
    let uptime = SystemTime::now()
        .duration_since(n.started)
        .map_or(0, |d| d.as_secs());
    b.push_str(&format!(
        ",\"uptime_seconds\":{uptime},\"config_version\":{},\"config_etag\":",
        v.number
    ));
    write_string(&mut b, &v.etag);
    b.push_str(",\"config_applied\":");
    time(&mut b, v.applied);
    b.push_str(",\"config_source\":");
    write_string(&mut b, &v.source);
    b.push_str(",\"roles\":[");
    let mut roles = live.roles.clone();
    roles.sort_unstable();
    for (i, r) in roles.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, r);
    }
    b.push_str(&format!(
        "],\"schema_version\":{},\"schema_digest\":",
        akit_config::SCHEMA_VERSION
    ));
    write_string(&mut b, schema_digest());
    b.push_str(",\"capabilities\":[");
    for (i, c) in capabilities(configs, &live, metrics).iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, c);
    }
    b.push_str("]}\n");
    json(200, b)
        .header("ETag", v.etag.clone())
        .header(VERSION_HEADER, v.number.to_string())
}

/// `GET /schema`'s answer.
pub(crate) fn schema_answer() -> Response {
    Response::with_body(200, "application/schema+json", schema().to_vec())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn new_node_ids() {
        // fit's TestNewNodeID.
        let (a, b) = (new_node_id(), new_node_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
        assert!(b"89ab".contains(&a.as_bytes()[19]));
        assert!(akit_config::cluster::valid_node_id(&a));
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "akit-node-{name}-{}-{}",
            std::process::id(),
            new_node_id()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    const CHEAP: crate::store::Params = crate::store::Params {
        time: 1,
        memory_kib: 64,
        threads: 1,
    };

    fn new_store(path: &Path) -> Store {
        let mut st = Store::create_with(path, "secret", CHEAP).unwrap();
        st.write(b"{}".to_vec()).unwrap();
        st
    }

    /// fit's TestNodeIDKept: made once and read back the same, from the file beside the config
    /// without a store and from the store with one; a store takes the file's ID; with nowhere to
    /// keep it, or a file that holds no ID, it lasts in memory.
    #[test]
    fn node_id_kept() {
        let dir = tmp("kept");
        let prefix = dir.join("akit").to_string_lossy().into_owned();
        let (id, kept, err) = node_id(None, Some(&prefix));
        assert_eq!((kept, err), (ID_IN_FILE, None));
        assert!(akit_config::cluster::valid_node_id(&id));
        let file = fs::read_to_string(format!("{prefix}{NODE_ID_SUFFIX}")).unwrap();
        assert_eq!(file.trim(), id);
        assert_eq!(node_id(None, Some(&prefix)), (id.clone(), ID_IN_FILE, None));

        // Given a store: the file's ID moves into it, and stays there.
        let path = dir.join("akit.store");
        let mut st = new_store(&path);
        assert_eq!(
            node_id(Some(&mut st), Some(&prefix)),
            (id.clone(), ID_IN_STORE, None)
        );
        let (mut st2, _) = Store::open(&path, "secret").unwrap();
        assert_eq!(
            node_id(Some(&mut st2), None),
            (id.clone(), ID_IN_STORE, None)
        );

        // A store of its own, no file: made in it, and read back.
        let mut st3 = new_store(&dir.join("other.store"));
        let (other, kept3, err) = node_id(Some(&mut st3), None);
        assert_eq!((kept3, err), (ID_IN_STORE, None));
        assert_ne!(other, id);
        assert_eq!(node_id(Some(&mut st3), None).0, other);

        // A section that holds no ID, a store with no config yet, no place, a file that holds
        // no ID, and a directory that cannot be written.
        st3.set_section(NODE_SECTION, br#"{"id": "no spaces allowed"}"#)
            .unwrap();
        let (_, kept, err) = node_id(Some(&mut st3), None);
        assert!(kept == ID_IN_MEMORY && err.is_some());
        let mut empty = Store::create_with(dir.join("e.store"), "secret", CHEAP).unwrap();
        let (id_e, kept, err) = node_id(Some(&mut empty), None);
        assert!(kept == ID_IN_MEMORY && err.is_some() && !id_e.is_empty());
        let (id_n, kept, err) = node_id(None, None);
        assert!(kept == ID_IN_MEMORY && err.is_none() && !id_n.is_empty());
        let bad = dir.join("bad").to_string_lossy().into_owned();
        fs::write(format!("{bad}{NODE_ID_SUFFIX}"), "not an id\n").unwrap();
        let (_, kept, err) = node_id(None, Some(&bad));
        assert!(kept == ID_IN_MEMORY && err.is_some());
        assert_eq!(
            fs::read_to_string(format!("{bad}{NODE_ID_SUFFIX}")).unwrap(),
            "not an id\n"
        );
        let missing = dir
            .join("missing")
            .join("akit")
            .to_string_lossy()
            .into_owned();
        let (_, kept, err) = node_id(None, Some(&missing));
        assert!(kept == ID_IN_MEMORY && err.is_some());
        let unreadable = dir.join("dir").to_string_lossy().into_owned();
        fs::create_dir(format!("{unreadable}{NODE_ID_SUFFIX}")).unwrap();
        let (_, kept, err) = node_id(None, Some(&unreadable));
        assert!(kept == ID_IN_MEMORY && err.is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn merged_keeps_what_is_left_out() {
        // fit's TestUseNodeKeeps.
        let before = Node::new();
        let n = before.merged(Node {
            id: String::new(),
            id_kept: ID_IN_MEMORY,
            hostname: "h1".into(),
            version: String::new(),
            engine: "akio 0".into(),
            started: before.started,
        });
        assert_eq!(n.id, before.id);
        assert_eq!(n.id_kept, before.id_kept);
        assert_eq!(n.hostname, "h1");
        assert_eq!(n.engine, "akio 0");
    }
}
