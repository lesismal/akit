//! The client's manager (I153, fit's `bittorrent/manage.go`): a thread of the role's own that
//! looks every second, as fit's manager does, at what the client's events noted (completions,
//! seed limits, a magnet link's metadata), at the queue and the disk, and at `watch_dir` when its
//! interval came, and saves the state when something changed. The queue and the disk are also
//! looked at in each API call that changes the list, so that an add, a pause or a removal takes
//! effect at once.
//!
//! The queue goes by the list's order: past `max_active_downloads` (or `max_active_seeds`) a
//! torrent is held paused, `queued`, and the first held starts when one finishes, is paused or
//! removed; a torrent fetching a magnet link's metadata or checking its data counts as
//! downloading. A download starts only when its directory's disk has what it still has to
//! download and `min_free_bytes` more free (those started in the same look reserving theirs, by
//! directory); one downloading is held, `disk`, once less than `min_free_bytes` is left, and
//! starts again by itself once the space is there. Seeds are never held for the disk. Which
//! torrents are held is not saved but worked out again at start (fit's decisions 7 and 9 of
//! I198).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use akio_bittorrent::client::{AddOptions, FileStorage, Torrent, TorrentStatus};

use super::{Run, SaverMsg, Shared, lock};

/// How often the manager looks: fit's second.
const LOOK: Duration = Duration::from_secs(1);

/// Why the role holds a torrent paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Hold {
    /// It is not held.
    #[default]
    None,
    /// Past `max_active_downloads` or `max_active_seeds`.
    Queue,
    /// Short of disk space.
    Disk,
}

impl Hold {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Hold::None => "",
            Hold::Queue => "queued",
            Hold::Disk => "disk",
        }
    }
}

/// The space free for a process that is not root on the file system of `dir`, or of its nearest
/// ancestor that is there; `None` where it cannot be told. May block: the manager's thread or an
/// API call's.
fn free_space(dir: &Path) -> Option<u64> {
    let mut p = Some(dir);
    while let Some(d) = p {
        if d.exists() {
            return akio_sys::fs::space(d).ok().map(|s| s.available);
        }
        p = d.parent();
    }
    None
}

fn paused(t: &Torrent) -> bool {
    t.stats().status == TorrentStatus::Paused
}

impl Run {
    /// Holds and starts the torrents as the queue and the disk say. Whether a torrent's hold
    /// changed.
    pub(crate) fn schedule(&mut self) -> bool {
        let l = self.cfg.limits_or_default();
        let (max_dl, max_seed, min_free) =
            (l.max_active_downloads, l.max_active_seeds, l.free_bytes());
        let (mut dl, mut seeds) = (0usize, 0usize);
        // What the downloads started in this look will still write, by directory.
        let mut reserved: Vec<(PathBuf, u64)> = Vec::new();
        let mut changed = false;
        for r in &mut self.recs {
            let Some(t) = r.torrent.clone() else { continue };
            if r.entry.paused {
                if !paused(&t) {
                    t.pause();
                }
                changed |= r.held != Hold::None;
                r.held = Hold::None;
                continue;
            }
            let st = t.stats();
            let running = st.status != TorrentStatus::Paused;
            let complete = st.pieces_total > 0 && st.pieces_have == st.pieces_total;
            let mut hold = Hold::None;
            let mut msg = String::new();
            if complete {
                if max_seed > 0 && seeds >= max_seed {
                    hold = Hold::Queue;
                } else {
                    seeds = seeds.saturating_add(1);
                }
            } else if max_dl > 0 && dl >= max_dl {
                hold = Hold::Queue;
            } else {
                let dir = PathBuf::from(&r.entry.dir);
                if let Some(free) = free_space(&dir) {
                    if running {
                        if free < min_free {
                            hold = Hold::Disk;
                            msg = format!(
                                "the disk of {} has {free} bytes free, less than min_free_bytes, \
                                 {min_free}",
                                dir.display()
                            );
                        }
                    } else {
                        let res: u64 = reserved
                            .iter()
                            .filter(|(d, _)| *d == dir)
                            .map(|(_, n)| *n)
                            .sum();
                        let need = st.bytes_left.saturating_add(min_free).saturating_add(res);
                        if free < need {
                            hold = Hold::Disk;
                            msg = format!(
                                "the disk of {} has {free} bytes free, short of the {} still to \
                                 download and min_free_bytes, {min_free}",
                                dir.display(),
                                st.bytes_left
                            );
                        } else {
                            reserved.push((dir, st.bytes_left));
                        }
                    }
                }
                if hold == Hold::None {
                    dl = dl.saturating_add(1);
                }
            }
            if hold == Hold::None && !running {
                t.resume();
            } else if hold != Hold::None && running {
                t.pause();
            }
            changed |= r.held != hold;
            r.held = hold;
            r.hold_msg = msg;
        }
        changed
    }

    /// A magnet link's torrent whose metadata came, added again into its own directory: the
    /// stopgap for akio's A286, whose client puts a magnet link's files where the client was
    /// made to. What it wrote there meanwhile is removed. Whether the list changed.
    pub(crate) fn place_magnets(&mut self) -> bool {
        let mut changed = false;
        let seed = self.seed();
        let client_dir = self.client_dir.clone();
        let dirs: Vec<(String, String)> = self
            .recs
            .iter()
            .map(|r| (r.entry.dir.clone(), r.entry.name.clone()))
            .collect();
        for r in &mut self.recs {
            if r.placed {
                continue;
            }
            let Some(t) = r.torrent.clone() else { continue };
            let Some(mi) = t.metainfo() else { continue };
            t.pause();
            self.client.remove(&t.info_hash());
            // What it may have written below the client's directory, unless another torrent
            // there has its name.
            let shared = dirs
                .iter()
                .filter(|(d, n)| Path::new(d) == client_dir && *n == mi.info.name)
                .count();
            if shared == 0 {
                super::delete_data(&client_dir, &mi);
            }
            let mut o = AddOptions::default();
            o.paused = true;
            o.seed_limits = Some(seed);
            o.peers = r
                .entry
                .peers
                .iter()
                .filter_map(|p| p.parse().ok())
                .collect();
            o.storage = Some(Arc::new(FileStorage::new(&r.entry.dir, &mi.info)));
            match self.client.add_torrent(mi.clone(), o) {
                Ok(t) => {
                    r.torrent = Some(t);
                    r.err.clear();
                }
                Err(e) => r.err = e.to_string(),
            }
            r.entry.name.clone_from(&mi.info.name);
            // Its pieces are downloaded again where they belong.
            r.entry.completed = false;
            r.mi = Some(mi);
            r.placed = true;
            changed = true;
        }
        changed
    }
}

impl Shared {
    /// The manager's look: what the events noted, the magnets placed, the queue and the disk,
    /// `watch_dir` when its interval came (or `force`), and the state saved when it changed.
    pub(crate) fn look(&self, force: bool) {
        let notes = std::mem::take(&mut *lock(&self.notes));
        let all = self.overflow.swap(false, Ordering::Relaxed);
        let mut g = lock(&self.run);
        let Some(run) = g.as_mut() else { return };
        let mut changed = run.take_notes(notes, all || force);
        changed |= run.place_magnets();
        run.schedule();
        let now = Instant::now();
        if force || now >= run.watch.next {
            run.watch.next = now.checked_add(run.cfg.watch()).unwrap_or(now);
            changed |= super::watch::look(run);
        }
        if changed {
            let _ = run.save();
        }
    }
}

/// The manager's thread: a look every second, and when an event wakes it, until the role
/// stops.
pub(crate) fn manager(shared: &Arc<Shared>, rx: &Receiver<SaverMsg>) {
    loop {
        match rx.recv_timeout(LOOK) {
            Ok(SaverMsg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(SaverMsg::Wake) | Err(RecvTimeoutError::Timeout) => shared.look(false),
        }
    }
}
