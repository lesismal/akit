# 043: The media library: fit's members, akio-media's MP4 reader and HLS, fit's formats

[English](043-media-library.md) | [简体中文](043-media-library.zh-CN.md)

Status: accepted in I140 (2026-11-27), waiting for review ([decisions waiting for
review](../decisions-pending.md), rows 86–90); extended by the iterations of the Media library and
channels track (I140–I149) as they land.

## Context

fit's I131–I139 ([fit's decision 032](https://github.com/lesismal/fit/blob/main/docs/decisions/032-media-library.md))
added a media library: directories of MP4 files scanned into a catalog (`media_library`), served as
HLS on demand without transcoding (`vod`), played into the streaming hub as live channels and
pushed on to outside servers (`streaming.channels`, `push`), behind playback URLs that expire
(`media_auth`'s `url_signing`, on I119's `keys` and `media_tokens`), with AES-128 segments, a video
site's pages and the workbench's. fit asked fib for a reader of MP4 files that are not fragmented
(fib's A64) and for encryption in its live HLS muxer (fib's A69).

akio-media has both already: `akio_media::mp4` (akio ADR-097) is a sans-io reader of such files
(the `moov` found at the front or the back, every track's codec and a compact sample index, edit
lists applied as players apply them, every count checked against limits) and a planner that cuts
them into HLS segments at keyframes and muxes a segment as fMP4 or MPEG-TS; akio-media's `Hls` has
AES-128 and SAMPLE-AES with keys from a `KeyProvider`, rotated; akio-crypto has AES-128-CBC with
PKCS #7 padding. The streaming track (decision 040) left `media_pages`' `libraries`, `vod` and
`channels`, the policies' `keys` and `url_signing`, and the role's `channels` and `push` refused as
not supported yet for this track.

## Decision

1. **The members are read as fit's**, each in the iteration that runs it, and refused as not
   supported yet until then; `media_library` first (I140), with fit's fields, defaults, bounds and
   checks, in `akit-config`'s `medialib` module.
2. **The MP4 reader is akio-media's.** fit's I131 is done in akio: akit reads files with
   `akio_media::mp4` (`Locator`, `Movie`, `Plan`, `Segment`) and asks akio for what fit's reader
   does that akio's does not, by name, in akio requests A295–A300 (rule 21), with a sound stopgap
   or the field refused meanwhile.
3. **One crate, `akit-media`**, holds the library, `vod` and the channels beside the streaming role
   and the policies (no dependency of a new kind: akio crates at the pinned commit).
4. **Names are akit's, formats fit's** (decision 037): the metrics `akit_media_library_*`, an
   upload's temporary files `.akit-upload-*`; but what is a format stays fit's byte for byte, as
   the tokens' did (decision 040): an item's ID made from its path, the playback URL's fields and
   signature and the label its key is derived with, and the derivation of the AES-128 keys from a
   library's secret, so that a URL fit signs plays on akit and keys derived by either are the
   other's, given the same secret.
5. **Nothing blocks an I/O thread, as fit's**: a library scans on a thread of its own; serving reads
   files on a bounded file pool of the library's own (`file_workers`, `file_queue`,
   `file_timeout`), the request suspended meanwhile (`Context::suspend`); the catalog, the sample
   tables kept and the segments built are each bounded in bytes.

## Alternatives

- **akit's own MP4 reader**, ported from fib's `media/mp4`: two readers of one format in one stack,
  where akio's is fuzzed and carries the codecs akio-media's muxers take. Refused (rule 21).
- **akit's names on the formats** (another label for the URL signature and the keys): fit's and
  akit's URLs and keys would not be taken by each other for names nobody sees. Refused, as for the
  tokens.

## Consequences

- A library is scanned, played and signed the same by fit and akit; a site can move between them,
  or run both behind one API, given the same secret.
- Where akio-media's reader or planner answers otherwise than fib's (where a segment is cut, what a
  probe reads), akit answers as akio does; each such difference is recorded in the iteration that
  meets it, and asked of akio where fit's behaviour matters.

## I141: the catalog

- **Scans on threads of the library's own.** A library scans on a thread of its own, woken when
  asked, when a reload changes what it scans, and every `scan_interval` (a condition variable's
  timed wait: the logic is about time). Its probes run on at most `max_probes` scoped threads of the
  scan, not on the file pool as fit's do, so that a scan never sleeps to retry a job the pool
  refused and the pool's workers are left to the files served (decision row 88).
- **A probe parses the `moov` whole** with `Movie::parse` and keeps only what it describes; the
  movie is parsed again when its file is served. fib reads a `moov` but for its sample tables;
  akio's request A295 asks for that (decision row 88).
- **A movie a file is kept**, not a table a track: `Movie::parse` builds every track's index at
  once. The cache is bounded by `max_table_bytes` as fit's is, admitting a movie by the bytes its
  probe's parse found its indexes take.
- **The file pool's threads start as work comes**, up to `file_workers`, rather than all at once.
- **The Admin API's `/media/library`** is served by `akit-media`, which the binary registers as an
  endpoint of the Admin API (`akit_admin::Options::endpoints`), so that akit-admin does not depend
  on the media crate.
- **Where akio reads a file otherwise than fib**, the catalog follows akio (decision row 89).

## I142: video on demand

- **Segments are cut by akio-media's `mp4::Plan`** and muxed by its fMP4 and MPEG-TS muxers, from
  the movie the library keeps; a playlist carries a file's first video and first audio track of a
  codec served (fit's choice), the others given to the muxers as unknown tracks so that they leave
  them out. Where `Plan` cuts otherwise than fit's planner, akit follows `Plan` (decision row 90).
- **One clock, fit's**: every packet's time is moved so that the file's time 0 is at 1.4 s in both
  containers, so that one `X-TIMESTAMP-MAP` places the subtitles, as fit's.
- **The names, the multivariant playlist and the subtitles are fit's**, written as fit writes them;
  an H.264 codec string is written in fib's lower-case hex.
- **The files themselves and the poster go through akio-http's `Files`** of the item's root, which
  gives them ranges, conditional requests and `sendfile`.
- **A request waiting for the pool is suspended** (`Context::suspend`) and answered from the pool,
  or 503 past `timeout` by a deadline thread of akit-media's own until akio's A296.
