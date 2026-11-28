//! An item a channel plays (fit's `channels/source.go`): the tracks picked of it, its parsed
//! movies, where its samples go on the channel's timeline, and its samples read on the
//! library's file pool.

use std::sync::{Arc, Mutex, PoisonError};

use akio_media::av::{Codec, Packet, Time, Track, Tracks};
use akio_media::mp4::{Movie, Mp4Track, Sample};

use super::list::ListState;
use crate::library::catalog::{KIND_AUDIO, KIND_VIDEO};
use crate::library::{self, Item, Library, Rendition};

/// Why an item cannot be played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceError {
    /// The channel's library does not run.
    NoLibrary,
    /// The library has not been scanned yet.
    NotReady,
    /// The library holds no such item.
    NoItem,
    /// The item has no video or audio akit plays.
    NoMedia,
    /// The video's NAL units are not 4-byte length prefixed.
    NalSize,
    /// A sample is larger than `max_buffer_bytes`.
    Sample,
    /// Nothing to play from the item's offset.
    Nothing,
    /// The library's.
    Library(library::Error),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::NoLibrary => f.write_str("the channel's library does not run"),
            SourceError::NotReady => f.write_str("the library has not been scanned yet"),
            SourceError::NoItem => f.write_str("the library holds no such item"),
            SourceError::NoMedia => f.write_str("the item has no video or audio akit can play"),
            SourceError::NalSize => {
                f.write_str("the video's NAL units are not 4-byte length prefixed")
            }
            SourceError::Sample => f.write_str("a sample is larger than max_buffer_bytes"),
            SourceError::Nothing => f.write_str("nothing to play from its offset"),
            SourceError::Library(e) => write!(f, "{e}"),
        }
    }
}

/// Where an item played came from.
pub(crate) const FROM_LIST: &str = "list";
/// See [`FROM_LIST`].
pub(crate) const FROM_SCHEDULE: &str = "schedule";
/// See [`FROM_LIST`].
pub(crate) const FROM_ADMIN: &str = "admin";

/// An item to play next: its ID, where it came from and where to start it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) item: String,
    pub(crate) from: &'static str,
    /// Into the item.
    pub(crate) offset: Time,
    /// The slot's start, in nanoseconds since 1970, for [`FROM_SCHEDULE`].
    pub(crate) slot: i64,
}

/// A rendition of an item: of its renditions with video, or of its audio alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RendRef {
    pub(crate) audio: bool,
    pub(crate) index: usize,
}

/// A track of an item played: where its samples are, and how far they have been read.
#[derive(Debug, Clone)]
struct SrcTrack {
    rend: RendRef,
    /// The track's index in its file's movie.
    index: usize,
    video: bool,
    /// Where the catalog says it starts and how long it plays.
    start: Time,
    duration: Time,
    movie: Option<Arc<Movie>>,
    /// Ticks added to decode times: the least composition offset, so that DTS <= PTS.
    adjust: i64,
    /// The next sample to read.
    next: u32,
}

impl SrcTrack {
    fn mt(&self) -> Option<&Mp4Track> {
        self.movie.as_ref().and_then(|m| m.tracks.get(self.index))
    }
}

/// What of a source changes as it is placed and read.
#[derive(Debug, Default)]
pub(crate) struct Cursor {
    tracks: Vec<SrcTrack>,
    /// The tracks as the hub's stream has them, once the movies are read.
    pub(crate) avs: Option<Tracks>,
    /// The channel time of the item's time 0: a sample's channel DTS is `off` + its DTS.
    pub(crate) off: Time,
    /// The DTS of its first sample on the channel.
    pub(crate) base: Time,
    /// How much earlier than its first presentation `base` is (B-frames).
    pub(crate) delay: Time,
    /// The channel time its presentation ends at.
    pub(crate) end: Time,
    /// Where its presentation is cut, at the next slot's start; 0 for none.
    pub(crate) cut: Time,
    /// Every sample has been read.
    pub(crate) done: bool,
    /// The cut stopped it.
    pub(crate) cut_hit: bool,
}

/// An item being played. What is in it but [`Cursor`] does not change.
#[derive(Debug)]
pub(crate) struct Source {
    pub(crate) lib: Arc<Library>,
    pub(crate) item: Arc<Item>,
    pub(crate) choice: Choice,
    /// The rendition shown as playing: the video's, or the audio's without video.
    pub(crate) rendition: String,
    /// The channel's list as it was once the item was chosen, for a skip to go on from.
    pub(crate) list_after: ListState,
    renditions: Vec<RendRef>,
    pub(crate) cur: Mutex<Cursor>,
}

fn rendition(it: &Item, r: RendRef) -> Option<&Rendition> {
    let list = if r.audio { &it.audio } else { &it.renditions };
    list.get(r.index)
}

/// The rendition of `it` named `name`, or the one with the most pixels, the highest bit rate
/// among equals; `None` when none can be served (fit's `pickRendition`).
pub(crate) fn pick_rendition(it: &Item, name: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, r) in it.renditions.iter().enumerate() {
        if !r.error.is_empty() || !r.moov_known() {
            continue;
        }
        if !name.is_empty() && r.name == name {
            return Some(i);
        }
        let px = |r: &Rendition| u64::from(r.width).saturating_mul(u64::from(r.height));
        let better = match best.and_then(|b| it.renditions.get(b)) {
            None => true,
            Some(b) => px(r) > px(b) || px(r) == px(b) && r.bitrate > b.bitrate,
        };
        if better {
            best = Some(i);
        }
    }
    best
}

/// Whether akit carries a track of `kind` and `codec`: (video, carried).
pub(crate) fn playable(kind: &str, codec: &str) -> (bool, bool) {
    match codec {
        "h264" | "h265" | "av1" => (true, kind == KIND_VIDEO),
        "aac" | "opus" | "mp3" => (false, kind == KIND_AUDIO),
        _ => (false, false),
    }
}

/// Whether a video track's NAL units are prefixed with 4 bytes, as akio-media carries them.
pub(crate) fn nal_size4(t: &Track) -> bool {
    match t.codec {
        Codec::H264 => t.config.get(4).is_some_and(|b| b & 3 == 3),
        Codec::H265 => t.config.get(21).is_some_and(|b| b & 3 == 3),
        _ => true,
    }
}

impl Source {
    /// The tracks of `it` to play (fit's `newSource`): its rendition's first video and first
    /// audio, or, when the rendition has no audio, the item's default audio's.
    pub(crate) fn new(
        lib: Arc<Library>,
        it: Arc<Item>,
        rendition_name: &str,
        choice: Choice,
        list_after: ListState,
    ) -> Result<Source, SourceError> {
        let mut tracks = Vec::new();
        let mut audio: Option<SrcTrack> = None;
        let rend = pick_rendition(&it, rendition_name).map(|index| RendRef {
            audio: false,
            index,
        });
        let track = |r: RendRef, t: &library::Track, video: bool| SrcTrack {
            rend: r,
            index: t.index,
            video,
            start: i64::try_from(t.start.as_nanos()).unwrap_or(i64::MAX),
            duration: i64::try_from(t.duration.as_nanos()).unwrap_or(i64::MAX),
            movie: None,
            adjust: 0,
            next: 0,
        };
        if let Some(r) = rend {
            for t in rendition(&it, r).map_or(&[][..], |r| &r.tracks) {
                match playable(t.kind, t.codec) {
                    (_, false) => {}
                    (true, true) if tracks.is_empty() => tracks.push(track(r, t, true)),
                    (false, true) if audio.is_none() => audio = Some(track(r, t, false)),
                    _ => {}
                }
            }
        }
        if audio.is_none() {
            let mut alt: Option<usize> = None;
            for (i, r) in it.audio.iter().enumerate() {
                if !r.error.is_empty() || !r.moov_known() {
                    continue;
                }
                let better = alt
                    .and_then(|a| it.audio.get(a))
                    .is_none_or(|a| r.default && !a.default);
                if better {
                    alt = Some(i);
                }
            }
            if let Some(i) = alt {
                let r = RendRef {
                    audio: true,
                    index: i,
                };
                audio = rendition(&it, r).and_then(|rd| {
                    rd.tracks
                        .iter()
                        .find(|t| playable(t.kind, t.codec) == (false, true))
                        .map(|t| track(r, t, false))
                });
            }
        }
        tracks.extend(audio);
        let Some(first) = tracks.first() else {
            return Err(SourceError::NoMedia);
        };
        let shown = rend.unwrap_or(first.rend);
        let rendition = rendition(&it, shown)
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let mut renditions: Vec<RendRef> = Vec::new();
        for t in &tracks {
            if !renditions.contains(&t.rend) {
                renditions.push(t.rend);
            }
        }
        Ok(Source {
            lib,
            item: it,
            choice,
            rendition,
            list_after,
            renditions,
            cur: Mutex::new(Cursor {
                tracks,
                ..Cursor::default()
            }),
        })
    }

    pub(crate) fn cur(&self) -> std::sync::MutexGuard<'_, Cursor> {
        self.cur.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads the parsed movies of the source's renditions, through the library, and calls
    /// `done` once: at once, on the caller's thread, for movies the library keeps, or from its
    /// pool (fit's `load`).
    pub(crate) fn load(
        self: &Arc<Self>,
        done: impl FnOnce(Result<(), SourceError>) + Send + 'static,
    ) {
        type Done = Box<dyn FnOnce(Result<(), SourceError>) + Send>;
        struct Wait {
            left: usize,
            first: Option<SourceError>,
            done: Option<Done>,
        }
        let wait = Arc::new(Mutex::new(Wait {
            left: self.renditions.len(),
            first: None,
            done: Some(Box::new(done)),
        }));
        for r in self.renditions.clone() {
            let Some(rd) = rendition(&self.item, r) else {
                continue;
            };
            let me = Arc::clone(self);
            let w = Arc::clone(&wait);
            self.lib.movie(rd, move |res| {
                match res {
                    Ok(m) => {
                        for t in me.cur().tracks.iter_mut().filter(|t| t.rend == r) {
                            t.movie = Some(Arc::clone(&m));
                        }
                    }
                    Err(e) => {
                        let mut g = w.lock().unwrap_or_else(PoisonError::into_inner);
                        g.first.get_or_insert(SourceError::Library(e));
                    }
                }
                let (last, done, first) = {
                    let mut g = w.lock().unwrap_or_else(PoisonError::into_inner);
                    g.left = g.left.saturating_sub(1);
                    let last = g.left == 0;
                    (
                        last,
                        if last { g.done.take() } else { None },
                        g.first.clone(),
                    )
                };
                if let (true, Some(done)) = (last, done) {
                    let r = match first {
                        Some(e) => Err(e),
                        None => me.tracks_ready(),
                    };
                    done(r);
                }
            });
        }
    }

    /// The tracks as the hub's stream has them, once every movie is read; their NAL units
    /// checked.
    fn tracks_ready(&self) -> Result<(), SourceError> {
        let mut cur = self.cur();
        let mut avs = Vec::new();
        for t in &cur.tracks {
            let (Some(m), Some(mt)) = (&t.movie, t.mt()) else {
                return Err(SourceError::NoMedia);
            };
            if !nal_size4(&mt.track) {
                return Err(SourceError::NalSize);
            }
            avs.push(m.av_tracks().swap_remove(t.index));
        }
        cur.avs = Some(Tracks::from(avs));
        Ok(())
    }
}

fn dts(t: &SrcTrack, mt: &Mp4Track, s: &Sample) -> Time {
    mt.time(
        i64::try_from(s.dts)
            .unwrap_or(i64::MAX)
            .saturating_add(t.adjust),
    )
}

fn pts_ticks(s: &Sample) -> i64 {
    i64::try_from(s.dts)
        .unwrap_or(i64::MAX)
        .saturating_add(i64::from(s.cts))
}

impl Cursor {
    /// Puts the source on the channel's timeline, started at `offset` into the item: its
    /// presentation from `pres` on, its first packet's DTS at `dts` or later, and each track's
    /// first DTS at its `after` or later, so that decode times never go back (fit's `place`).
    /// False when nothing is left to play from there.
    pub(crate) fn place(&mut self, offset: Time, pres: Time, dts_at: Time, after: &[Time]) -> bool {
        for t in &mut self.tracks {
            let mc = t.mt().map_or(0, |mt| mt.index.min_cts());
            t.adjust = if mc < 0 { i64::from(mc) } else { 0 };
            t.next = 0;
        }
        let mut start = offset.max(0);
        for t in &mut self.tracks {
            if !t.video || offset <= 0 {
                continue;
            }
            let Some(mt) = t.movie.as_ref().and_then(|m| m.tracks.get(t.index)) else {
                return false;
            };
            let Some(k) = mt.keyframe_at(offset) else {
                return false;
            };
            t.next = k;
            if let Some(s) = mt.index.sample(k) {
                start = mt.pts(&s);
            }
        }
        for t in &mut self.tracks {
            if t.video {
                continue;
            }
            let Some(mt) = t.movie.as_ref().and_then(|m| m.tracks.get(t.index)) else {
                continue;
            };
            let ticks = u64::try_from(mt.ticks(start).max(0)).unwrap_or(0);
            let mut i = mt.index.first_at_or_after_dts(ticks).saturating_sub(1);
            while let Some(s) = mt.index.sample(i) {
                let end = pts_ticks(&s).saturating_add(i64::from(s.duration));
                if mt.time(end) > start {
                    break;
                }
                i = i.saturating_add(1);
            }
            t.next = i;
        }
        let mut found = false;
        let (mut first, mut end) = (0, 0);
        for t in &self.tracks {
            let Some(mt) = t.mt() else { continue };
            let Some(s) = mt.index.sample(t.next) else {
                continue;
            };
            let d = dts(t, mt, &s).saturating_sub(start);
            let end_ticks = i64::try_from(mt.index.end_dts())
                .unwrap_or(i64::MAX)
                .saturating_add(t.adjust);
            let mut e = mt.time(end_ticks).saturating_sub(start);
            if t.duration > 0 {
                e = t.start.saturating_add(t.duration).saturating_sub(start);
            }
            if !found || d < first {
                first = d;
            }
            if !found || e > end {
                end = e;
            }
            found = true;
        }
        if !found {
            return false;
        }
        // Where the presentation starts on the channel.
        let mut p = pres.max(dts_at.saturating_sub(first));
        for (i, t) in self.tracks.iter().enumerate() {
            let (Some(a), Some(mt)) = (after.get(i), t.mt()) else {
                continue;
            };
            if let Some(s) = mt.index.sample(t.next) {
                p = p.max(a.saturating_sub(dts(t, mt, &s).saturating_sub(start)));
            }
        }
        self.off = p.saturating_sub(start);
        self.base = p.saturating_add(first);
        self.delay = first.saturating_neg();
        self.end = p.saturating_add(end);
        self.cut = 0;
        self.done = false;
        self.cut_hit = false;
        true
    }

    /// The track whose next sample comes first, the sample and its channel DTS; `None` when
    /// every track has been read, or has reached the cut (fit's `head`).
    fn head(&mut self) -> Option<(usize, Sample, Time)> {
        let mut best: Option<(usize, Sample, Time)> = None;
        let mut hit = false;
        for (i, t) in self.tracks.iter().enumerate() {
            let Some(mt) = t.mt() else { continue };
            let Some(s) = mt.index.sample(t.next) else {
                continue;
            };
            let at = self.off.saturating_add(dts(t, mt, &s));
            if self.cut > 0 {
                // Video by its DTS, decoded the delay before it is shown; the rest by their
                // presentation.
                let past = if t.video {
                    at.saturating_add(self.delay) >= self.cut
                } else {
                    self.off.saturating_add(mt.time(pts_ticks(&s))) >= self.cut
                };
                if past {
                    hit = true;
                    continue;
                }
            }
            if best.as_ref().is_none_or(|b| at < b.2) {
                best = Some((i, s, at));
            }
        }
        if hit {
            self.cut_hit = true;
        }
        best
    }

    /// Whether every track has been read (for the tests).
    #[cfg(test)]
    pub(crate) fn at_end(&mut self) -> bool {
        for t in &mut self.tracks {
            if let Some(mt) = t.mt() {
                t.next = mt.index.len();
            }
        }
        self.head().is_none()
    }

    /// The index of the first sample a track reads next (for the tests).
    #[cfg(test)]
    pub(crate) fn next_of(&self, i: usize) -> Option<(u32, Time)> {
        let t = self.tracks.get(i)?;
        let mt = t.mt()?;
        let s = mt.index.sample(t.next)?;
        Some((
            t.next,
            mt.time(pts_ticks(&s).saturating_add(i64::from(s.duration))),
        ))
    }

    /// The tracks' renditions, video first (for the tests).
    #[cfg(test)]
    pub(crate) fn rends(&self) -> Vec<RendRef> {
        self.tracks.iter().map(|t| t.rend).collect()
    }
}

/// A sample chosen to read.
struct Pick {
    track: usize,
    s: Sample,
    pos: usize,
}

impl Source {
    /// Reads the source's next samples, on the library's pool: up to `span` of media, and up
    /// to `budget` bytes but for the first, which may not pass `limit` (fit's `read`). The
    /// packets of the hub's stream, in decode order, and their bytes; marks the source done
    /// once every sample has been read.
    pub(crate) fn read(
        &self,
        span: Time,
        budget: u64,
        limit: u64,
    ) -> Result<(Vec<Packet>, u64), SourceError> {
        let mut cur = self.cur();
        let mut picks: Vec<Pick> = Vec::new();
        let mut total: u64 = 0;
        let mut first: Option<Time> = None;
        loop {
            let Some((i, s, at)) = cur.head() else {
                cur.done = true;
                break;
            };
            let f = *first.get_or_insert(at);
            let size = u64::from(s.size);
            if !picks.is_empty()
                && (at.saturating_sub(f) >= span || total.saturating_add(size) > budget)
            {
                break;
            }
            if size > limit {
                return Err(SourceError::Sample);
            }
            picks.push(Pick {
                track: i,
                s,
                pos: 0,
            });
            total = total.saturating_add(size);
            if let Some(t) = cur.tracks.get_mut(i) {
                t.next = t.next.saturating_add(1);
            }
        }
        if picks.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let slab = self.read_samples(&cur, &mut picks, total)?;
        let mut out = Vec::with_capacity(picks.len());
        for p in &picks {
            let Some(t) = cur.tracks.get(p.track) else {
                continue;
            };
            let Some(mt) = t.mt() else { continue };
            let end = p.pos.saturating_add(p.s.size as usize);
            let data = slab.get(p.pos..end).map(<[u8]>::to_vec).unwrap_or_default();
            let d = cur.off.saturating_add(dts(t, mt, &p.s));
            let pt = cur
                .off
                .saturating_add(mt.time(pts_ticks(&p.s).saturating_add(t.adjust)));
            out.push(Packet::new(p.track, d, pt, p.s.keyframe || !t.video, data));
        }
        Ok((out, total))
    }

    /// Reads the samples picked into one slab, each file's in the order they lie in it, a run
    /// of samples that follow one another read at once, and sets each pick's place in the slab.
    fn read_samples(
        &self,
        cur: &Cursor,
        picks: &mut [Pick],
        total: u64,
    ) -> Result<Vec<u8>, SourceError> {
        let mut slab = vec![0u8; usize::try_from(total).map_err(|_| SourceError::Sample)?];
        // The picks by file, then by offset: (file, offset, size, pick).
        let mut order: Vec<(RendRef, u64, u32, usize)> = picks
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let r = cur.tracks.get(p.track)?.rend;
                Some((r, p.s.offset, p.s.size, i))
            })
            .collect();
        order.sort_by_key(|&(r, off, _, _)| ((r.audio, r.index), off));
        let mut pos = 0usize;
        let mut rest = order.as_slice();
        while let Some(&(r, _, _, _)) = rest.first() {
            let n_file = rest.iter().take_while(|e| e.0 == r).count();
            let (file_picks, after) = rest.split_at(n_file);
            rest = after;
            let Some(file) = rendition(&self.item, r).map(|rd| &rd.file) else {
                return Err(SourceError::NoMedia);
            };
            let h = self.lib.open_file(file).map_err(SourceError::Library)?;
            let mut run = file_picks;
            while let Some(&(_, start, _, _)) = run.first() {
                // A run of samples that follow one another in the file.
                let mut end = start;
                let mut n_run = 0usize;
                for &(_, off, size, i) in run {
                    if off != end {
                        break;
                    }
                    if let Some(p) = picks.get_mut(i) {
                        p.pos = pos.saturating_add(
                            usize::try_from(end.saturating_sub(start)).unwrap_or(0),
                        );
                    }
                    end = end.saturating_add(u64::from(size));
                    n_run = n_run.saturating_add(1);
                }
                run = run.get(n_run..).unwrap_or_default();
                let n = usize::try_from(end.saturating_sub(start)).unwrap_or(0);
                let Some(buf) = slab.get_mut(pos..pos.saturating_add(n)) else {
                    return Err(SourceError::Sample);
                };
                read_full(&h, buf, start).map_err(|e| {
                    SourceError::Library(library::Error::Io(format!("reading {}: {e}", file.path)))
                })?;
                pos = pos.saturating_add(n);
            }
        }
        Ok(slab)
    }
}

/// Fills `buf` from `h` at `off`.
fn read_full(h: &library::files::Handle, buf: &mut [u8], off: u64) -> std::io::Result<()> {
    let mut done = 0usize;
    while done < buf.len() {
        let Some(rest) = buf.get_mut(done..) else {
            break;
        };
        let n = h.read_at(rest, off.saturating_add(done as u64))?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        done = done.saturating_add(n);
    }
    Ok(())
}
