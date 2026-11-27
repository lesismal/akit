//! The client's state in `data_dir`, fit's files (`bittorrent/state.go`): `torrents.json`, the
//! list of torrents with their sources, directories and whether they are paused, and
//! `torrents/<id>.torrent`, their .torrent files; resume data is akio-bittorrent's, in
//! `resume/<id>.resume`. Each file is written to a temporary file, synced and renamed. Members of
//! an entry akit does not read (a category, own limits, priorities: fit's iterations that akit has
//! not run yet) are kept as they were read, so that fit reads the file back as it wrote it.
//!
//! Everything here reads or writes files: the control thread's, the saver's, or an API call's,
//! never an I/O thread's.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use akit_config::bittorrent::MAX_BT_TORRENTS;
use akit_config::value::{self, Value};

/// fit's `StateVersion`.
pub(crate) const STATE_VERSION: i64 = 1;
/// fit's `MaxStateBytes`: the most `torrents.json` may be.
pub(crate) const MAX_STATE_BYTES: u64 = 64 << 20;
/// The most a .torrent file may be, akio-bittorrent's and fib's metainfo bound.
pub(crate) const MAX_TORRENT_BYTES: u64 = 128 << 20;
/// fit's `MaxMagnetBytes`: the longest magnet link.
pub(crate) const MAX_MAGNET_BYTES: usize = 64 << 10;
/// fit's `maxKeptPeers` and `maxPeerLen`.
const MAX_KEPT_PEERS: usize = 128;
const MAX_PEER_LEN: usize = 256;

pub(crate) const STATE_FILE: &str = "torrents.json";
pub(crate) const TORRENT_DIR: &str = "torrents";
pub(crate) const RESUME_DIR: &str = "resume";

/// A torrent as `torrents.json` lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) magnet: String,
    pub(crate) dir: String,
    pub(crate) paused: bool,
    /// RFC 3339, as fit writes it.
    pub(crate) added: String,
    pub(crate) completed: bool,
    pub(crate) peers: Vec<String>,
    /// The members akit does not read, kept as read.
    pub(crate) other: Vec<(String, Value)>,
}

/// `torrents.json`: its torrents and the members of the document akit does not read
/// (`alt_speed`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Doc {
    pub(crate) torrents: Vec<Entry>,
    pub(crate) other: Vec<(String, Value)>,
}

/// Whether `id` is a torrent's ID: 40 lowercase hex digits.
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 40
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Reads at most `max` bytes of `path`; an error past it.
pub(crate) fn read_bounded(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(max.saturating_add(1)).read_to_end(&mut buf)?;
    if u64::try_from(buf.len()).map_or(true, |n| n > max) {
        return Err(std::io::Error::other(format!("larger than {max} bytes")));
    }
    Ok(buf)
}

fn string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn boolean(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

const KNOWN: [&str; 8] = [
    "id",
    "name",
    "magnet",
    "dir",
    "paused",
    "added",
    "completed",
    "peers",
];

/// Reads `<dir>/torrents.json`, fit's `loadState`: none when it is not there; an error, which
/// keeps the role from starting rather than have it overwrite the file, when it does not read.
/// Temporary files a crash left are removed.
pub(crate) fn load(dir: &Path) -> Result<Doc, String> {
    for d in [
        dir.to_path_buf(),
        dir.join(TORRENT_DIR),
        dir.join(RESUME_DIR),
    ] {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let n = e.file_name();
                let n = n.to_string_lossy();
                if n.starts_with('.') && n.contains(".tmp-") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    let path = dir.join(STATE_FILE);
    let shown = path.display();
    let data = match read_bounded(&path, MAX_STATE_BYTES) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Doc::default()),
        Err(e) => return Err(format!("the state cannot be read: {shown}: {e}")),
    };
    let text = std::str::from_utf8(&data)
        .map_err(|_| format!("the state cannot be read: {shown}: not UTF-8"))?;
    let v = value::parse(text).map_err(|e| {
        let why = e
            .as_slice()
            .first()
            .map_or_else(String::new, |e| e.msg.clone());
        format!("the state cannot be read: {shown}: {why}")
    })?;
    let Value::Object(members) = v else {
        return Err(format!("the state cannot be read: {shown}: not an object"));
    };
    let mut doc = Doc::default();
    let mut version = None;
    let mut list = None;
    for (k, v) in members {
        match k.as_str() {
            "version" => version = Some(v),
            "torrents" => list = Some(v),
            "saved" => {}
            _ => doc.other.push((k, v)),
        }
    }
    let version = match version {
        Some(Value::Number(n)) => n.parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    if !(1..=STATE_VERSION).contains(&version) {
        return Err(format!(
            "the state cannot be read: {shown} is of version {version}, which this akit does not read"
        ));
    }
    let list = match list {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a,
        Some(_) => {
            return Err(format!(
                "the state cannot be read: {shown}: torrents is not a list"
            ));
        }
    };
    if list.len() > MAX_BT_TORRENTS {
        return Err(format!(
            "the state cannot be read: {shown} lists {} torrents, more than {MAX_BT_TORRENTS}",
            list.len()
        ));
    }
    for (i, e) in list.into_iter().enumerate() {
        let Value::Object(members) = e else {
            return Err(format!(
                "the state cannot be read: {shown}: torrent {i} is not an object"
            ));
        };
        let get = |k: &str| members.iter().find(|(m, _)| m == k).map(|(_, v)| v);
        let id = string(get("id"));
        if !valid_id(&id) {
            return Err(format!(
                "the state cannot be read: {shown}: torrent {i} has the ID {id:?}, not 40 lowercase hex digits"
            ));
        }
        if doc.torrents.iter().any(|t| t.id == id) {
            return Err(format!(
                "the state cannot be read: {shown} lists {id} twice"
            ));
        }
        let dir = string(get("dir"));
        if !Path::new(&dir).is_absolute() {
            return Err(format!(
                "the state cannot be read: {shown}: torrent {id} has the directory {dir:?}, which is not absolute"
            ));
        }
        let peers = match get("peers") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|p| match p {
                    Value::String(s) if !s.is_empty() && s.len() <= MAX_PEER_LEN => Some(s.clone()),
                    _ => None,
                })
                .take(MAX_KEPT_PEERS)
                .collect(),
            _ => Vec::new(),
        };
        doc.torrents.push(Entry {
            name: string(get("name")),
            magnet: string(get("magnet")),
            paused: boolean(get("paused")),
            added: string(get("added")),
            completed: boolean(get("completed")),
            peers,
            other: members
                .iter()
                .filter(|(k, _)| !KNOWN.contains(&k.as_str()))
                .cloned()
                .collect(),
            id,
            dir,
        });
    }
    Ok(doc)
}

/// The document as fit writes `torrents.json`.
pub(crate) fn encode(doc: &Doc, saved: &str) -> String {
    let s = |x: &str| Value::String(x.to_owned());
    let mut top = vec![
        (
            "version".to_owned(),
            Value::Number(STATE_VERSION.to_string()),
        ),
        ("saved".to_owned(), s(saved)),
    ];
    top.extend(doc.other.iter().cloned());
    let list = doc
        .torrents
        .iter()
        .map(|e| {
            let mut m = vec![("id".to_owned(), s(&e.id))];
            if !e.name.is_empty() {
                m.push(("name".to_owned(), s(&e.name)));
            }
            if !e.magnet.is_empty() {
                m.push(("magnet".to_owned(), s(&e.magnet)));
            }
            m.push(("dir".to_owned(), s(&e.dir)));
            if e.paused {
                m.push(("paused".to_owned(), Value::Bool(true)));
            }
            m.push(("added".to_owned(), s(&e.added)));
            if e.completed {
                m.push(("completed".to_owned(), Value::Bool(true)));
            }
            if !e.peers.is_empty() {
                m.push((
                    "peers".to_owned(),
                    Value::Array(e.peers.iter().map(|p| s(p)).collect()),
                ));
            }
            m.extend(e.other.iter().cloned());
            Value::Object(m)
        })
        .collect();
    top.push(("torrents".to_owned(), Value::Array(list)));
    let mut out = Value::Object(top).to_json();
    out.push('\n');
    out
}

/// Writes `data` to `path` through a temporary file beside it, synced and renamed into place.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    let res = f.write_all(data).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = res.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// `<dir>/torrents/<id>.torrent`.
pub(crate) fn torrent_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(TORRENT_DIR).join(format!("{id}.torrent"))
}

/// `<dir>/resume/<id>.resume`.
pub(crate) fn resume_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(RESUME_DIR).join(format!("{id}.resume"))
}
