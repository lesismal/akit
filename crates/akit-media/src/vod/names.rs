//! The names of an item's directory (fit's `handler/vod/names.go`). An item's URLs are its ID's
//! path after the handler's prefix, and one of these names after it:
//!
//! ```text
//! index.m3u8             the multivariant playlist, of the default container
//! index.fmp4.m3u8        the same, of fragmented MP4 segments
//! index.ts.m3u8          the same, of MPEG-TS segments
//! v0.fmp4.m3u8           the media playlist of rendition 0 (a for audio)
//! v0.init.mp4            its fMP4 initialization section
//! v0.12.m4s, v0.12.ts    its segment 12
//! s0.m3u8, s0.vtt        subtitle 0's playlist, and the subtitle as WebVTT
//! v0.mp4, a0.m4a         the file itself, its own extension, with Range
//! poster.jpg             the poster, its own extension
//! aes.key                the AES-128 key of an encrypted item's segments (I147)
//! ```

/// What a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Master,
    Media,
    Init,
    Segment,
    Subtitles,
    Vtt,
    File,
    Poster,
    Key,
}

/// A segment format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum C {
    Fmp4,
    Ts,
}

impl C {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            C::Fmp4 => "fmp4",
            C::Ts => "ts",
        }
    }
}

/// The name of an encrypted item's key.
pub(crate) const KEY_NAME: &str = "aes.key";

/// The numbers of a name stay below this.
const MAX_INDEX: usize = 1 << 24;

/// A name of an item's directory, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Name {
    pub(crate) kind: Kind,
    /// Of a playlist or segment; `None` for the default.
    pub(crate) c: Option<C>,
    /// `a0...`, one of the item's audio files; `v0...` a rendition.
    pub(crate) audio: bool,
    /// The rendition, audio file or subtitle.
    pub(crate) i: usize,
    /// The segment.
    pub(crate) n: usize,
    /// Of a file or the poster, as the request writes it.
    pub(crate) ext: String,
}

impl Name {
    fn new(kind: Kind) -> Name {
        Name {
            kind,
            c: None,
            audio: false,
            i: 0,
            n: 0,
            ext: String::new(),
        }
    }
}

/// The extensions of the media files a library holds.
fn media_ext(e: &str) -> bool {
    matches!(e, "mp4" | "m4v" | "m4a" | "mov")
}

fn container_of(s: &str) -> Option<C> {
    match s {
        "fmp4" => Some(C::Fmp4),
        "ts" => Some(C::Ts),
        _ => None,
    }
}

/// A decimal number below [`MAX_INDEX`] without leading zeros, so that a name has one spelling.
fn number(s: &str) -> Option<usize> {
    if s.is_empty() || s.len() > 8 || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<usize>().ok().filter(|n| *n < MAX_INDEX)
}

/// `v12`, `a12` or `s12`.
fn file_ref(s: &str) -> Option<(u8, usize)> {
    let (&t, rest) = s.as_bytes().split_first()?;
    if !matches!(t, b'v' | b'a' | b's') {
        return None;
    }
    Some((t, number(std::str::from_utf8(rest).ok()?)?))
}

/// Parses the last part of a request's path; `None` for a name the handler does not serve.
pub(crate) fn parse_name(s: &str) -> Option<Name> {
    match s {
        "index.m3u8" => return Some(Name::new(Kind::Master)),
        KEY_NAME => return Some(Name::new(Kind::Key)),
        _ => {}
    }
    let parts: Vec<&str> = s.split('.').collect();
    match parts.as_slice() {
        [first, ext] => {
            if *first == "poster" {
                let mut n = Name::new(Kind::Poster);
                n.ext = (*ext).to_owned();
                return (!ext.is_empty()).then_some(n);
            }
            let (t, i) = file_ref(first)?;
            let mut n = Name::new(Kind::File);
            n.i = i;
            match (t, *ext) {
                (b's', "m3u8") => n.kind = Kind::Subtitles,
                (b's', "vtt") => n.kind = Kind::Vtt,
                (b's', _) => return None,
                (_, e) if media_ext(e) => {
                    n.audio = t == b'a';
                    n.ext = e.to_owned();
                }
                _ => return None,
            }
            Some(n)
        }
        [first, mid, last] => {
            if *first == "index" {
                let mut n = Name::new(Kind::Master);
                n.c = Some(container_of(mid)?);
                return (*last == "m3u8").then_some(n);
            }
            let (t, i) = file_ref(first)?;
            if t == b's' {
                return None;
            }
            let mut n = Name::new(Kind::Media);
            n.audio = t == b'a';
            n.i = i;
            match *last {
                "m3u8" => n.c = Some(container_of(mid)?),
                "mp4" if *mid == "init" => {
                    n.kind = Kind::Init;
                    n.c = Some(C::Fmp4);
                }
                "m4s" | "ts" => {
                    n.kind = Kind::Segment;
                    n.n = number(mid)?;
                    n.c = Some(if *last == "ts" { C::Ts } else { C::Fmp4 });
                }
                _ => return None,
            }
            Some(n)
        }
        _ => None,
    }
}

fn file_ref_name(audio: bool, i: usize) -> String {
    format!("{}{i}", if audio { 'a' } else { 'v' })
}

/// The name of a media playlist.
pub(crate) fn media_name(audio: bool, i: usize, c: C) -> String {
    format!("{}.{}.m3u8", file_ref_name(audio, i), c.as_str())
}

/// The name of an initialization section.
pub(crate) fn init_name(audio: bool, i: usize) -> String {
    format!("{}.init.mp4", file_ref_name(audio, i))
}

/// The name of a segment.
pub(crate) fn segment_name(audio: bool, i: usize, n: usize, c: C) -> String {
    let ext = if c == C::Ts { "ts" } else { "m4s" };
    format!("{}.{n}.{ext}", file_ref_name(audio, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fit's `TestNames` (`units_test.go`).
    #[test]
    fn names() {
        let p = |s: &str| parse_name(s);
        assert_eq!(p("index.m3u8").unwrap().kind, Kind::Master);
        assert_eq!(p("index.ts.m3u8").unwrap().c, Some(C::Ts));
        assert_eq!(p("index.fmp4.m3u8").unwrap().c, Some(C::Fmp4));
        let n = p("a2.ts.m3u8").unwrap();
        assert!(n.kind == Kind::Media && n.audio && n.i == 2 && n.c == Some(C::Ts));
        let n = p("v0.init.mp4").unwrap();
        assert!(n.kind == Kind::Init && !n.audio);
        let n = p("v1.12.m4s").unwrap();
        assert!(n.kind == Kind::Segment && n.i == 1 && n.n == 12 && n.c == Some(C::Fmp4));
        assert_eq!(p("v1.12.ts").unwrap().c, Some(C::Ts));
        assert_eq!(p("s3.vtt").unwrap().kind, Kind::Vtt);
        assert_eq!(p("s3.m3u8").unwrap().kind, Kind::Subtitles);
        let n = p("a0.m4a").unwrap();
        assert!(n.kind == Kind::File && n.audio && n.ext == "m4a");
        assert_eq!(p("poster.jpg").unwrap().ext, "jpg");
        assert_eq!(p(KEY_NAME).unwrap().kind, Kind::Key);
        for bad in [
            "",
            "index.mp4.m3u8",
            "v01.m3u8",
            "v1.012.m4s",
            "x1.mp4",
            "v.mp4",
            "v1.txt",
            "s1.mp4",
            "s1.fmp4.m3u8",
            "v1.init.m4s",
            "v123456789.mp4",
            "v16777216.mp4",
            "poster.",
            "a.b.c.d",
            "v1.fmp4.m3u9",
        ] {
            assert_eq!(p(bad), None, "{bad}");
        }
        assert_eq!(media_name(true, 2, C::Ts), "a2.ts.m3u8");
        assert_eq!(init_name(false, 0), "v0.init.mp4");
        assert_eq!(segment_name(false, 1, 12, C::Fmp4), "v1.12.m4s");
        assert_eq!(segment_name(true, 0, 3, C::Ts), "a0.3.ts");
    }
}
