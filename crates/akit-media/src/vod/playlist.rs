//! An item's playlists (fit's `handler/vod/playlist.go`): the multivariant playlist from the
//! catalog alone, a media playlist from a file's plan, and a subtitle's playlist.

use std::fmt::Write as _;
use std::time::Duration;

use akio_media::mp4::Plan;

use super::names::{C, KEY_NAME, init_name, media_name, segment_name};
use crate::library::{Item, Rendition};

const AUDIO_GROUP: &str = "audio";
const SUBTITLE_GROUP: &str = "subs";

/// What the segments' clock puts a file's time 0 at, as fit's (MPEG-TS's customary 1.4 s), and
/// that on the 90 kHz clock, for subtitles' `X-TIMESTAMP-MAP`.
pub(crate) const LEAD: Duration = Duration::from_millis(1400);
const LEAD_MPEGTS: u64 = 126_000;

/// Whether a playlist serves video of `codec`.
fn served_video(codec: &str) -> bool {
    matches!(codec, "h264" | "h265" | "av1")
}

/// Whether a playlist serves audio of `codec`.
fn served_audio(codec: &str) -> bool {
    matches!(codec, "aac" | "opus")
}

/// Whether container `c` carries `codec` (fit's `carried`).
pub(crate) fn carried(c: C, codec: &str) -> bool {
    c != C::Ts || codec != "av1"
}

/// The tracks of `r` a playlist carries: its first video track of a codec served, and its first
/// audio track of one (fit's `pickTracks`).
pub(crate) fn pick_tracks(r: &Rendition) -> (Option<usize>, Option<usize>) {
    let (mut video, mut audio) = (None, None);
    for t in &r.tracks {
        if t.samples == 0 {
            continue;
        }
        if video.is_none() && t.kind == "video" && served_video(t.codec) {
            video = Some(t.index);
        } else if audio.is_none() && t.kind == "audio" && served_audio(t.codec) {
            audio = Some(t.index);
        }
    }
    (video, audio)
}

/// The codec of track `i` of `r`.
fn codec_of(r: &Rendition, i: usize) -> &'static str {
    r.tracks
        .iter()
        .find(|t| t.index == i)
        .map_or("unknown", |t| t.codec)
}

/// A rendition, or an audio file of an item of audio alone, a multivariant playlist lists.
#[derive(Debug, Clone, Copy)]
struct Variant<'a> {
    r: &'a Rendition,
    i: usize,
    audio: bool,
    video: Option<usize>,
    sound: Option<usize>,
}

impl Variant<'_> {
    fn codecs(&self) -> Vec<&str> {
        self.r
            .tracks
            .iter()
            .filter(|t| Some(t.index) == self.video || Some(t.index) == self.sound)
            .filter(|t| !t.codec_string.is_empty())
            .map(|t| t.codec_string.as_str())
            .collect()
    }

    fn language(&self) -> &str {
        if self.r.language == "und" {
            ""
        } else {
            &self.r.language
        }
    }
}

/// `r` as a variant of container `c`, `None` when `c` carries none of its tracks.
pub(crate) fn servable(r: &Rendition, c: C) -> Option<(Option<usize>, Option<usize>)> {
    if !r.error.is_empty() {
        return None;
    }
    let (v, a) = pick_tracks(r);
    if let Some(v) = v {
        if !carried(c, codec_of(r, v)) {
            return None;
        }
    }
    (v.is_some() || a.is_some()).then_some((v, a))
}

/// The renditions of `it` that `c` carries as variants, and its audio files as alternates; for
/// an item of audio alone, its audio files as variants and no group.
fn variants(it: &Item, c: C) -> (Vec<Variant<'_>>, Vec<Variant<'_>>) {
    let mut vs = Vec::new();
    for (i, r) in it.renditions.iter().enumerate() {
        if let Some((video @ Some(_), sound)) = servable(r, c) {
            vs.push(Variant {
                r,
                i,
                audio: false,
                video,
                sound,
            });
        }
    }
    let mut alts = Vec::new();
    for (i, r) in it.audio.iter().enumerate() {
        if let Some((video, sound @ Some(_))) = servable(r, c) {
            alts.push(Variant {
                r,
                i,
                audio: true,
                video,
                sound,
            });
        }
    }
    if vs.is_empty() {
        return (alts, Vec::new());
    }
    (vs, alts)
}

/// The EXT-X-VERSION of `c`'s playlists.
fn version(c: C) -> u8 {
    match c {
        C::Ts => 3,
        C::Fmp4 => 7,
    }
}

/// `name` with `query` after it when there is one.
fn uri(name: &str, query: &str) -> String {
    if query.is_empty() {
        name.to_owned()
    } else {
        format!("{name}?{query}")
    }
}

fn yes_no(v: bool) -> &'static str {
    if v { "YES" } else { "NO" }
}

/// `s` as a quoted-string attribute may hold it.
fn quoted(s: &str) -> String {
    s.replace('"', "'").replace(['\r', '\n'], " ")
}

fn name_or(def: &str, lang: &str) -> String {
    if lang.is_empty() || lang == "und" {
        def.to_owned()
    } else {
        lang.to_owned()
    }
}

#[allow(clippy::too_many_arguments)]
fn media_tag(
    b: &mut String,
    typ: &str,
    group: &str,
    name: &str,
    lang: &str,
    def: bool,
    forced: bool,
    uri: Option<&str>,
) {
    let _ = write!(
        b,
        "#EXT-X-MEDIA:TYPE={typ},GROUP-ID=\"{group}\",NAME=\"{}\"",
        quoted(name)
    );
    if !lang.is_empty() && lang != "und" {
        let _ = write!(b, ",LANGUAGE=\"{}\"", quoted(lang));
    }
    let _ = write!(b, ",DEFAULT={},AUTOSELECT=YES", yes_no(def));
    if typ == "SUBTITLES" {
        let _ = write!(b, ",FORCED={}", yes_no(forced));
    }
    if let Some(u) = uri {
        let _ = write!(b, ",URI=\"{u}\"");
    }
    b.push('\n');
}

/// The multivariant playlist of `it` in container `c`, its URIs with `query`; `None` when `c`
/// carries nothing of it (fit's `renderMaster`).
pub(crate) fn render_master(it: &Item, c: C, query: &str) -> Option<String> {
    let (vs, alts) = variants(it, c);
    if vs.is_empty() {
        return None;
    }
    let mut b = String::with_capacity(512);
    let _ = writeln!(b, "#EXTM3U\n#EXT-X-VERSION:{}", version(c));
    let mut names: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut unique = |n: String| {
        let k = names.entry(n.clone()).or_insert(0);
        *k = k.saturating_add(1);
        if *k > 1 { format!("{n} ({k})") } else { n }
    };
    let group = !alts.is_empty();
    let mut alt_codecs: Vec<&str> = Vec::new();
    let mut alt_rate = 0u64;
    if group {
        let main = vs.iter().all(|v| v.sound.is_some());
        let mut def = None;
        if main {
            let lang = vs.first().map_or("", Variant::language);
            let n = unique(name_or("main", lang));
            media_tag(&mut b, "AUDIO", AUDIO_GROUP, &n, lang, true, false, None);
        } else {
            def = alts
                .iter()
                .find(|a| a.r.default)
                .or_else(|| alts.first())
                .map(|a| a.i);
        }
        for a in &alts {
            let u = uri(&media_name(true, a.i, c), query);
            let lang = a.language();
            let n = if a.r.name.is_empty() {
                name_or(&format!("audio {}", a.i), lang)
            } else {
                a.r.name.clone()
            };
            let n = unique(n);
            media_tag(
                &mut b,
                "AUDIO",
                AUDIO_GROUP,
                &n,
                lang,
                def == Some(a.i),
                false,
                Some(&u),
            );
            for cs in a.codecs() {
                if !alt_codecs.contains(&cs) {
                    alt_codecs.push(cs);
                }
            }
            alt_rate = alt_rate.max(a.r.bitrate);
        }
    }
    let mut subs = false;
    for (k, s) in it.subtitles.iter().enumerate() {
        if !s.error.is_empty() {
            continue;
        }
        subs = true;
        let u = uri(&format!("s{k}.m3u8"), query);
        let n = if s.name.is_empty() {
            name_or(&format!("subtitles {k}"), &s.language)
        } else {
            s.name.clone()
        };
        let n = unique(n);
        media_tag(
            &mut b,
            "SUBTITLES",
            SUBTITLE_GROUP,
            &n,
            &s.language,
            s.default,
            s.forced,
            Some(&u),
        );
    }
    for v in &vs {
        let mut rate = v.r.bitrate.max(1);
        if group {
            rate = rate.saturating_add(alt_rate);
        }
        let _ = write!(
            b,
            "#EXT-X-STREAM-INF:BANDWIDTH={rate},AVERAGE-BANDWIDTH={rate}"
        );
        let mut cs = v.codecs();
        if group {
            for a in &alt_codecs {
                if !cs.contains(a) {
                    cs.push(a);
                }
            }
        }
        if !cs.is_empty() {
            let _ = write!(b, ",CODECS=\"{}\"", cs.join(","));
        }
        if v.video.is_some() && v.r.width > 0 && v.r.height > 0 {
            let _ = write!(b, ",RESOLUTION={}x{}", v.r.width, v.r.height);
        }
        if group {
            let _ = write!(b, ",AUDIO=\"{AUDIO_GROUP}\"");
        }
        if subs {
            let _ = write!(b, ",SUBTITLES=\"{SUBTITLE_GROUP}\"");
        }
        b.push('\n');
        b.push_str(&uri(&media_name(v.audio, v.i, c), query));
        b.push('\n');
    }
    Some(b)
}

/// `d` in seconds with three decimals, as EXTINF has it.
fn seconds(d: Duration) -> String {
    let ms = d.as_millis();
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

fn plan_duration(p: &Plan, i: usize) -> Duration {
    Duration::from_nanos(u64::try_from(p.duration(i)).unwrap_or(0))
}

/// The media playlist of `p`, in container `c`, of the rendition or audio file `(audio, i)`, its
/// URIs with `query`; with `encrypted`, its segments under the item's key (fit's `renderMedia`).
pub(crate) fn render_media(
    p: &Plan,
    audio: bool,
    i: usize,
    c: C,
    query: &str,
    encrypted: bool,
) -> String {
    let mut b = String::with_capacity(256usize.saturating_add(p.len().saturating_mul(40)));
    let _ = write!(
        b,
        "#EXTM3U\n#EXT-X-VERSION:{}\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n",
        version(c),
        p.target_duration()
    );
    if c == C::Fmp4 {
        let _ = writeln!(b, "#EXT-X-MAP:URI=\"{}\"", uri(&init_name(audio, i), query));
    }
    if encrypted {
        let _ = writeln!(
            b,
            "#EXT-X-KEY:METHOD=AES-128,URI=\"{}\"",
            uri(KEY_NAME, query)
        );
    }
    for n in 0..p.len() {
        let _ = write!(
            b,
            "#EXTINF:{},\n{}\n",
            seconds(plan_duration(p, n)),
            uri(&segment_name(audio, i, n, c), query)
        );
    }
    b.push_str("#EXT-X-ENDLIST\n");
    b
}

/// The playlist of subtitle `k` of an item of duration `d`: the whole WebVTT file as one segment.
pub(crate) fn render_subtitles(k: usize, d: Duration, query: &str) -> String {
    let d = d.max(Duration::from_millis(1));
    format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:{},\n{}\n#EXT-X-ENDLIST\n",
        d.as_secs_f64().ceil(),
        seconds(d),
        uri(&format!("s{k}.vtt"), query)
    )
}

/// `vtt` with an `X-TIMESTAMP-MAP` header after its first line, unless it has one, so that HLS
/// players show its cues at the times of the segments' clock.
pub(crate) fn with_timestamp_map(vtt: &[u8]) -> Vec<u8> {
    let head_end = vtt
        .windows(2)
        .position(|w| w == b"\n\n")
        .unwrap_or(vtt.len());
    let head = vtt.get(..head_end).unwrap_or(vtt);
    if head.windows(15).any(|w| w == b"X-TIMESTAMP-MAP") {
        return vtt.to_vec();
    }
    let map = format!("X-TIMESTAMP-MAP=MPEGTS:{LEAD_MPEGTS},LOCAL:00:00:00.000");
    let i = vtt.iter().position(|b| *b == b'\n').unwrap_or(vtt.len());
    let (first, rest) = vtt.split_at(i);
    let mut out = Vec::with_capacity(vtt.len().saturating_add(map.len()).saturating_add(2));
    out.extend_from_slice(first);
    out.push(b'\n');
    out.extend_from_slice(map.as_bytes());
    if rest.is_empty() {
        out.push(b'\n');
    } else {
        out.extend_from_slice(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_maps() {
        assert_eq!(
            with_timestamp_map(b"WEBVTT\n\n00:01.000 --> 00:02.000\nhi\n"),
            b"WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:126000,LOCAL:00:00:00.000\n\n00:01.000 --> 00:02.000\nhi\n"
        );
        assert_eq!(
            with_timestamp_map(b"WEBVTT"),
            b"WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:126000,LOCAL:00:00:00.000\n"
        );
        let has = b"WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\nx\n";
        assert_eq!(with_timestamp_map(has), has);
        assert_eq!(seconds(Duration::from_millis(4004)), "4.004");
        assert_eq!(
            render_subtitles(1, Duration::from_millis(1500), "token=a"),
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:1.500,\ns1.vtt?token=a\n#EXT-X-ENDLIST\n"
        );
    }
}
