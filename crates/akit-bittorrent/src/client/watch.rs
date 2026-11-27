//! `watch_dir` (I153, fit's `bittorrent/watch.go`): a directory looked at every
//! `watch_interval` for .torrent files and .magnet files (a magnet link on the first line that is
//! not blank). Those directly in it are added with no category, those in a subdirectory named
//! after a category in that category. A file is taken once its size and modification time held
//! across two looks, so that one being written is not taken half way; at most `watch_max_files`
//! a look. A file added, or whose torrent is held already, moves into `.done` beside it; one that
//! cannot be added (too large, not a torrent, not a link) into `.failed`, beside a `<name>.error`
//! saying why; one that may be added later (`max_torrents` reached, the state not written) stays
//! to be tried at the next look. Names starting with a dot are left alone. On the manager's
//! thread: it reads and moves files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use super::state::{MAX_MAGNET_BYTES, MAX_TORRENT_BYTES, read_bounded};
use super::{AddRequest, ApiError, Run, Source};

/// The most entries of one directory looked at in a look, whatever `watch_max_files` takes: a
/// directory of a million names costs a bounded look.
const MAX_SCAN: usize = 100_000;

/// What the looks remember.
#[derive(Debug)]
pub(crate) struct Watch {
    /// Each candidate's size and modification time at the look before.
    seen: HashMap<PathBuf, (u64, Option<SystemTime>)>,
    /// When the next look is due.
    pub(crate) next: Instant,
}

impl Default for Watch {
    fn default() -> Watch {
        Watch {
            seen: HashMap::new(),
            next: Instant::now(),
        }
    }
}

/// The candidates of `dir`: .torrent and .magnet files, not hidden, by name.
fn candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .take(MAX_SCAN)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            !n.starts_with('.') && (n.ends_with(".torrent") || n.ends_with(".magnet"))
        })
        .collect();
    out.sort();
    out
}

fn stamp(p: &Path) -> Option<(u64, Option<SystemTime>)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()))
}

/// Moves `p` into `<its dir>/<sub>/`, with `why` beside it in `<name>.error` when given.
fn set_aside(p: &Path, sub: &str, why: Option<&str>) {
    let Some(parent) = p.parent() else { return };
    let Some(name) = p.file_name() else { return };
    let to = parent.join(sub);
    if std::fs::create_dir_all(&to).is_err() {
        return;
    }
    let _ = std::fs::rename(p, to.join(name));
    if let Some(why) = why {
        let _ = std::fs::write(
            to.join(format!("{}.error", name.to_string_lossy())),
            format!("{why}\n"),
        );
    }
}

/// What a file holds: a .torrent file's bytes, or a .magnet file's link.
fn source(p: &Path) -> Result<Source, String> {
    if p.extension().is_some_and(|e| e == "magnet") {
        let b = read_bounded(p, MAX_MAGNET_BYTES as u64).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&b);
        let link = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .ok_or("an empty .magnet file")?;
        Ok(Source::Magnet(link.to_owned()))
    } else {
        read_bounded(p, MAX_TORRENT_BYTES)
            .map(Source::Torrent)
            .map_err(|e| e.to_string())
    }
}

/// One look at `watch_dir`, when one is set. Whether the list changed.
pub(crate) fn look(run: &mut Run) -> bool {
    let dir = run.cfg.watch_dir.trim().to_owned();
    if dir.is_empty() {
        return false;
    }
    let dir = PathBuf::from(dir);
    let mut found: Vec<(PathBuf, String)> = candidates(&dir)
        .into_iter()
        .map(|p| (p, String::new()))
        .collect();
    for c in &run.cfg.categories {
        found.extend(
            candidates(&dir.join(&c.name))
                .into_iter()
                .map(|p| (p, c.name.clone())),
        );
    }
    let mut seen = HashMap::new();
    let mut taken = 0usize;
    let mut changed = false;
    let max = run.cfg.watch_files();
    for (p, category) in found {
        let Some(st) = stamp(&p) else { continue };
        let settled = run.watch.seen.get(&p) == Some(&st);
        seen.insert(p.clone(), st);
        if !settled || taken >= max {
            continue;
        }
        taken = taken.saturating_add(1);
        let req = AddRequest {
            category,
            ..AddRequest::default()
        };
        match source(&p)
            .map_err(ApiError::BadRequest)
            .and_then(|src| run.add(src, req))
        {
            Ok(_) | Err(ApiError::Exists(_)) => {
                changed = true;
                seen.remove(&p);
                set_aside(&p, ".done", None);
            }
            Err(ApiError::BadRequest(why)) => {
                seen.remove(&p);
                set_aside(&p, ".failed", Some(&why));
            }
            // It may be added later: tried again at the next look.
            Err(_) => {}
        }
    }
    run.watch.seen = seen;
    changed
}
