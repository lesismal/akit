//! Limits (I154, fit's I199): each torrent's own limits over its category's, applied through
//! akio-bittorrent's per-torrent rates and seed limits (zero takes the client's; a negative rate
//! or ratio is none for the torrent); and the alternative rates, `alt_speed`, in place of the
//! client's rates while one of their weekly schedules holds in their time zone, or while the API
//! turns them on (`on`, `off`, or `auto` to follow the schedules), the API's choice kept in
//! `torrents.json` as fit keeps it (`alt_speed`). Without `alt_speed` in the config the rates
//! never switch. The manager looks at the schedules every second.

use akio_bittorrent::client::{RateLimits, SeedLimits};
use akit_config::bittorrent::{BitTorrentClient, BitTorrentTorrentLimits};
use akit_config::value::Value;

use super::{ApiError, ClientApi, Run, Status, lock};

/// The API's choice of the alternative rates.
pub const ALT_ON: &str = "on";
/// The alternative rates off, whatever the schedules say.
pub const ALT_OFF: &str = "off";
/// The schedules decide (saved as `""`).
pub const ALT_AUTO: &str = "auto";

/// The alternative rates' zone and the API's choice.
#[derive(Debug, Default)]
pub(crate) struct Alt {
    zone: Option<akit_observe::Zone>,
    /// `on`, `off` or `""` (auto).
    mode: String,
    /// Whether they hold now.
    pub(crate) active: bool,
}

impl Alt {
    /// Reads `cfg`'s time zone (it reads files: the control thread's) and the choice `other`,
    /// `torrents.json`'s members, keeps.
    pub(crate) fn open(
        &mut self,
        cfg: &BitTorrentClient,
        other: &mut [(String, Value)],
    ) -> Result<(), String> {
        self.zone = match &cfg.alt_speed {
            Some(a) => Some(
                akit_observe::Zone::load(&a.time_zone)
                    .map_err(|e| format!("alt_speed.time_zone: {e}"))?,
            ),
            None => None,
        };
        if let Some((_, Value::String(m))) = other.iter().find(|(k, _)| k == "alt_speed") {
            self.mode = if m == ALT_ON || m == ALT_OFF {
                m.clone()
            } else {
                String::new()
            };
        }
        Ok(())
    }
}

/// Whether one of the schedules of `cfg` holds at `now`, seconds since 1970, in `zone`.
fn scheduled(cfg: &BitTorrentClient, zone: &akit_observe::Zone, now: i64) -> bool {
    let Some(a) = &cfg.alt_speed else {
        return false;
    };
    let local = now.saturating_add(i64::from(zone.offset(now)));
    let day = local.div_euclid(86_400);
    // 1970-01-01 was a Thursday: Sunday is 0.
    let weekday = usize::try_from(day.saturating_add(4).rem_euclid(7)).unwrap_or(0);
    let minute = u32::try_from(local.rem_euclid(86_400) / 60).unwrap_or(0);
    a.schedules.iter().any(|s| s.holds(weekday, minute))
}

fn rate(v: i64) -> Option<u64> {
    u64::try_from(v).ok().filter(|v| *v > 0)
}

impl Run {
    /// Each torrent's limits, its own over its category's.
    pub(crate) fn effective(&self, e: &super::state::Entry) -> BitTorrentTorrentLimits {
        let cat = self
            .cfg
            .category(&e.category)
            .map(|c| c.limits.clone())
            .unwrap_or_default();
        cat.over(&e.limits)
    }

    /// Sets each torrent's rates and seed limits from its limits, the client's seed limits
    /// where they take them.
    pub(crate) fn apply_limits(&mut self) {
        let client = self.seed();
        for r in &self.recs {
            let Some(t) = &r.torrent else { continue };
            let l = self.effective(&r.entry);
            t.set_rate_limits(RateLimits::new(rate(l.upload_rate), rate(l.download_rate)));
            let ratio = if l.seed_ratio > 0.0 {
                Some(l.seed_ratio)
            } else if l.seed_ratio < 0.0 {
                None
            } else {
                client.ratio
            };
            let time = if l.seed_time.is_zero() {
                client.time
            } else {
                Some(l.seed_time)
            };
            t.set_seed_limits(SeedLimits::new(ratio, time));
        }
        self.apply_alt();
    }

    /// Switches the client's rates to the alternative ones while they hold, and back.
    pub(crate) fn apply_alt(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let active = match (self.alt.mode.as_str(), &self.cfg.alt_speed, &self.alt.zone) {
            (_, None, _) => false,
            (ALT_ON, _, _) => true,
            (ALT_OFF, _, _) => false,
            (_, Some(_), Some(z)) => scheduled(&self.cfg, z, now),
            _ => false,
        };
        self.alt.active = active;
        let rates = match (&self.cfg.alt_speed, active) {
            (Some(a), true) => {
                let r = |v: u64| (v > 0).then_some(v);
                RateLimits::new(r(a.upload_rate), r(a.download_rate))
            }
            _ => super::rates(&self.cfg),
        };
        if self.client.rate_limits() != rates {
            self.client.set_rate_limits(rates);
        }
    }

    fn set_alt_mode(&mut self, mode: &str) {
        self.alt.mode = mode.to_owned();
        self.other.retain(|(k, _)| k != "alt_speed");
        if !mode.is_empty() {
            self.other
                .push(("alt_speed".to_owned(), Value::String(mode.to_owned())));
        }
    }
}

impl ClientApi {
    /// Sets torrent `id`'s own limits, over its category's; saved at once.
    ///
    /// # Errors
    ///
    /// Not running, no such torrent, the state not written.
    pub fn set_torrent_limits(
        &self,
        id: &str,
        limits: BitTorrentTorrentLimits,
    ) -> Result<Status, ApiError> {
        self.with(|run| {
            let i = run.find(id).ok_or(ApiError::NotFound)?;
            if let Some(r) = run.recs.get_mut(i) {
                r.entry.limits = limits;
            }
            run.apply_limits();
            run.save().map_err(ApiError::Io)?;
            run.recs
                .get(i)
                .map(|r| run.status(r))
                .ok_or(ApiError::NotFound)
        })
    }

    /// The alternative rates: the API's choice (`on`, `off` or `auto`) and whether they hold.
    ///
    /// # Errors
    ///
    /// Not running.
    pub fn alt_speed(&self) -> Result<(String, bool), ApiError> {
        self.with(|run| {
            let m = if run.alt.mode.is_empty() {
                ALT_AUTO
            } else {
                &run.alt.mode
            };
            Ok((m.to_owned(), run.alt.active))
        })
    }

    /// Sets the alternative rates `on`, `off` or `auto`; saved in `torrents.json`.
    ///
    /// # Errors
    ///
    /// Not running, a mode none of those, the state not written.
    pub fn set_alt_speed(&self, mode: &str) -> Result<(String, bool), ApiError> {
        let m = match mode {
            ALT_ON | ALT_OFF => mode,
            ALT_AUTO => "",
            _ => {
                return Err(ApiError::BadRequest(format!(
                    "{mode:?} is not on, off or auto"
                )));
            }
        };
        self.with(|run| {
            run.set_alt_mode(m);
            run.apply_alt();
            run.save().map_err(ApiError::Io)
        })?;
        self.alt_speed()
    }

    /// The client's rates now (upload, download), the alternative ones while they hold.
    #[must_use]
    pub fn client_rates(&self) -> Option<(Option<u64>, Option<u64>)> {
        lock(&self.0.run)
            .as_ref()
            .map(|r| r.client.rate_limits())
            .map(|r| (r.upload, r.download))
    }

    /// Torrent `id`'s own rates in akio-bittorrent's client (upload, download).
    #[must_use]
    pub fn torrent_rates(&self, id: &str) -> Option<(Option<u64>, Option<u64>)> {
        let g = lock(&self.0.run);
        let run = g.as_ref()?;
        let r = run.recs.get(run.find(id)?)?;
        let l = r.torrent.as_ref()?.rate_limits();
        Some((l.upload, l.download))
    }
}
