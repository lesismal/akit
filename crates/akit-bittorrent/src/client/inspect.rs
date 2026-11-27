//! What the Admin API (I157, fit's `bittorrent/inspect.go` and `api.go`) reads of a torrent
//! beyond its status, and the changes it makes besides I152–I154's: its files, its peers, its
//! trackers and web seeds, a recheck, its category, the alternative rates' state, the publish
//! entries, the event stream.
//!
//! Two of fit's need what akio-bittorrent does not give yet (akio's A288): the state of a
//! torrent's announces to each tracker, its peer sources and its web seeds' state, which are
//! listed as its .torrent file and magnet link name them, their state unknown, as fit lists them
//! for a torrent its client does not run; and a recheck in place, which is done by adding the
//! torrent again without its resume data: it checks every piece, but its peers are dialed again
//! and its session's counters start from zero. A file's bytes had are known only where the
//! torrent's are (one file, every piece, or none) until akio's A287 gives the pieces had, and
//! every file is wanted at the normal priority: akio-bittorrent downloads them all.

use std::sync::Arc;

use akio_bittorrent::client::Torrent;
use akio_bittorrent::magnet::Magnet;
use akio_bittorrent::metainfo::MetaInfo;
use akio_http::BodyStream;

use super::events::{Hub, Refused};
use super::publish::EntryStatus;
use super::{ApiError, ClientApi, Status, lock, metrics, start, state};

/// fib's normal priority, which every file has: akio-bittorrent downloads them all.
pub const PRIORITY_NORMAL: u8 = 4;

/// A file of a torrent, fit's `FileStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Its index in the info dictionary's file list.
    pub index: usize,
    /// Its path in the torrent, `/` between its parts.
    pub path: String,
    /// Its size in bytes.
    pub length: u64,
    /// Its bytes had, where known: `None` for a file of a torrent of several, part had, until
    /// akio's A287.
    pub completed: Option<u64>,
    /// Its priority, fib's 0 to 7: [`PRIORITY_NORMAL`].
    pub priority: u8,
}

/// A peer connected to a torrent, fit's `PeerStatus` but its rates and `snubbed`, which
/// akio-bittorrent does not give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerStatus {
    /// Its address.
    pub addr: String,
    /// Its peer ID in hex.
    pub id: String,
    /// The client its extension handshake names.
    pub client: String,
    /// Dialed by us.
    pub outgoing: bool,
    /// Over uTP.
    pub utp: bool,
    /// Encrypted (MSE).
    pub encrypted: bool,
    /// It has every piece.
    pub seed: bool,
    /// Pieces it has.
    pub pieces: usize,
    /// Bytes downloaded from it.
    pub downloaded: u64,
    /// Bytes uploaded to it.
    pub uploaded: u64,
    /// We choke it.
    pub am_choking: bool,
    /// It chokes us.
    pub peer_choking: bool,
}

/// A tracker of a torrent, as its .torrent file or magnet link names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerStatus {
    /// Its URL.
    pub url: String,
    /// Its tier (BEP 12), from 0.
    pub tier: usize,
}

/// A torrent's trackers and web seeds, fit's `Trackers` with their state unknown (akio's A288).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trackers {
    /// Its trackers, by tier.
    pub trackers: Vec<TrackerStatus>,
    /// Its web seeds (http and https).
    pub web_seeds: Vec<String>,
}

/// The alternative rates, fit's `AltSpeedStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltSpeedStatus {
    /// `auto`, `on` or `off`.
    pub mode: String,
    /// The config has `alt_speed`: without it the alternative rates never hold.
    pub configured: bool,
    /// They hold now.
    pub active: bool,
    /// The alternative rates, bytes a second; 0 unlimited.
    pub upload_rate: u64,
    /// Download.
    pub download_rate: u64,
}

/// The trackers and web seeds `mi` and `magnet` name, fit's `listed`.
fn listed(mi: Option<&MetaInfo>, magnet: &str) -> Trackers {
    let mut out = Trackers::default();
    if let Some(mi) = mi {
        let tiers: Vec<Vec<String>> = if mi.announce_list.is_empty() {
            mi.announce.iter().map(|a| vec![a.clone()]).collect()
        } else {
            mi.announce_list.clone()
        };
        for (tier, urls) in tiers.into_iter().enumerate() {
            for url in urls {
                out.trackers.push(TrackerStatus { url, tier });
            }
        }
        out.web_seeds.extend(mi.url_list.iter().cloned());
    }
    if !magnet.is_empty() {
        if let Ok(m) = Magnet::parse(magnet) {
            for u in m.trackers {
                if !out.trackers.iter().any(|t| t.url == u) {
                    let tier = out.trackers.len();
                    out.trackers.push(TrackerStatus { url: u, tier });
                }
            }
            for u in m.web_seeds {
                if !out.web_seeds.contains(&u) {
                    out.web_seeds.push(u);
                }
            }
        }
    }
    out.web_seeds
        .retain(|u| u.starts_with("http://") || u.starts_with("https://"));
    out
}

impl ClientApi {
    /// Whether the client runs.
    #[must_use]
    pub fn running(&self) -> bool {
        lock(&self.0.run).is_some()
    }

    /// Torrent `id`'s files, padding left out.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent.
    pub fn files(&self, id: &str) -> Result<Vec<FileStatus>, ApiError> {
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            let r = run.recs.get(i).ok_or(ApiError::NotFound)?;
            let st = run.status(r);
            let Some(mi) =
                r.mi.clone()
                    .or_else(|| r.torrent.as_ref().and_then(Torrent::metainfo))
            else {
                return Ok(Vec::new());
            };
            let wanted: Vec<_> = mi
                .info
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| !f.padding)
                .collect();
            let one = wanted.len() == 1;
            Ok(wanted
                .into_iter()
                .map(|(index, f)| FileStatus {
                    index,
                    path: if f.path.is_empty() {
                        mi.info.name.clone()
                    } else {
                        f.path.join("/")
                    },
                    length: f.length,
                    completed: if st.complete {
                        Some(f.length)
                    } else if st.pieces_have == 0 {
                        Some(0)
                    } else if one {
                        Some(st.completed.min(f.length))
                    } else {
                        None
                    },
                    priority: PRIORITY_NORMAL,
                })
                .collect())
        })
    }

    /// The peers connected to torrent `id`, in the order akio-bittorrent gives them; none while
    /// it does not run.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent.
    pub fn peers(&self, id: &str) -> Result<Vec<PeerStatus>, ApiError> {
        let t = self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            Ok(run.recs.get(i).and_then(|r| r.torrent.clone()))
        })?;
        let Some(t) = t else {
            return Ok(Vec::new());
        };
        let total = t.stats().pieces_total;
        Ok(t.peers()
            .into_iter()
            .map(|p| PeerStatus {
                addr: p.addr.to_string(),
                id: super::hex(&p.id),
                client: p.client.unwrap_or_default(),
                outgoing: p.outgoing,
                utp: p.utp,
                encrypted: p.encrypted,
                seed: total > 0 && p.pieces >= total,
                pieces: p.pieces,
                downloaded: p.downloaded,
                uploaded: p.uploaded,
                am_choking: p.am_choking,
                peer_choking: p.peer_choking,
            })
            .collect())
    }

    /// Torrent `id`'s trackers and web seeds as its .torrent file and magnet link name them,
    /// their state unknown until akio's A288.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent.
    pub fn trackers(&self, id: &str) -> Result<Trackers, ApiError> {
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            let r = run.recs.get(i).ok_or(ApiError::NotFound)?;
            let mi =
                r.mi.clone()
                    .or_else(|| r.torrent.as_ref().and_then(Torrent::metainfo));
            Ok(listed(mi.as_ref(), &r.entry.magnet))
        })
    }

    /// Checks torrent `id`'s data again, every piece: until akio's A288 gives a recheck in
    /// place, the torrent is added again without its resume data, which checks it all; it dials
    /// its peers again, and keeps its place, limits and pause. A magnet link whose info has not
    /// come has nothing to check.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent, its info not known, the state not written.
    pub fn recheck(&self, id: &str) -> Result<Status, ApiError> {
        let meters = &self.0.meters;
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            let seed = run.seed();
            let data_dir = run.data_dir.clone();
            let client = run.client.clone();
            let r = run.recs.get_mut(i).ok_or(ApiError::NotFound)?;
            let mi =
                r.mi.clone()
                    .or_else(|| r.torrent.as_ref().and_then(Torrent::metainfo))
                    .ok_or_else(|| {
                        ApiError::BadRequest("its info is not known yet: nothing to check".into())
                    })?;
            metrics::account_one(r, &mut lock(meters));
            if let Some(t) = r.torrent.take() {
                t.pause();
                client.remove(&t.info_hash());
            }
            let _ = std::fs::remove_file(state::resume_path(&data_dir, &r.entry.id));
            r.mi = Some(mi);
            r.err.clear();
            r.entry.completed = false;
            start(&client, r, seed);
            run.apply_limits();
            run.schedule();
            run.save().map_err(ApiError::Io)?;
            run.recs
                .get(i)
                .map(|r| run.status(r))
                .ok_or(ApiError::NotFound)
        })
    }

    /// Puts torrent `id` in category `name` (`""` for none): its limits follow it; its files
    /// stay where they are, as fit's.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent or category, the state not written.
    pub fn set_category(&self, id: &str, name: &str) -> Result<Status, ApiError> {
        self.with(|run| {
            if !name.is_empty() && run.cfg.category(name).is_none() {
                return Err(ApiError::BadRequest(format!(
                    "no category is named {name:?}"
                )));
            }
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            if let Some(r) = run.recs.get_mut(i) {
                name.clone_into(&mut r.entry.category);
            }
            run.apply_limits();
            run.save().map_err(ApiError::Io)?;
            run.recs
                .get(i)
                .map(|r| run.status(r))
                .ok_or(ApiError::NotFound)
        })
    }

    /// The alternative rates: the API's choice, whether the config has them, whether they hold.
    ///
    /// # Errors
    ///
    /// Not running.
    pub fn alt_speed_status(&self) -> Result<AltSpeedStatus, ApiError> {
        let (mode, active) = self.alt_speed()?;
        self.with(|run| {
            let a = run.cfg.alt_speed.as_ref();
            Ok(AltSpeedStatus {
                mode: mode.clone(),
                configured: a.is_some(),
                active,
                upload_rate: a.map_or(0, |a| a.upload_rate),
                download_rate: a.map_or(0, |a| a.download_rate),
            })
        })
    }

    /// The publish entries' states (I156).
    #[must_use]
    pub fn publishing(&self) -> Vec<EntryStatus> {
        lock(&self.0.pubs).iter().map(|s| lock(s).clone()).collect()
    }

    /// Subscribes the stream `start` begins to the role's events after `after` (0 for none):
    /// see [`super::events`].
    ///
    /// # Errors
    ///
    /// Not running, [`ApiError::TooManySubscribers`], or the connection gone
    /// ([`ApiError::Io`]).
    pub fn subscribe(
        &self,
        after: u64,
        start: impl FnOnce() -> Option<BodyStream>,
    ) -> Result<(), ApiError> {
        // The hub's own lock, not the role's, which an API call writing the state may hold:
        // this is called on the Admin API's I/O thread.
        self.hub().subscribe(after, start).map_err(|e| match e {
            Refused::Closed => ApiError::NotRunning,
            Refused::Full => ApiError::TooManySubscribers,
            Refused::Gone => ApiError::Io("the connection went away".into()),
        })
    }

    /// Whether the role's events are still published: until the role stops.
    #[must_use]
    pub fn events_open(&self) -> bool {
        self.0.events.open()
    }

    pub(crate) fn hub(&self) -> Arc<Hub> {
        Arc::clone(&self.0.events)
    }
}
