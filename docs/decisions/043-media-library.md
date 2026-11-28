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

## I143: media tokens

- **The keyring and `media_tokens` are fit's I119**, which this track carries since the signed
  URLs build on it. A token is signed with the keyring's first key, or the key a batch names, and
  checked against every key; the key names are not in the token, as fit's.
- **The route must authenticate its callers**: akit-config refuses a `media_tokens` route without
  `jwt`, `basic_auth`, `forward_auth` or `client_cert` without `optional`, as fit's graph does,
  and a policy without the secret files a revoke rotates.
- **A batch's body is read whole by akio-http within the listener's bounds**, then held to
  `max_body_bytes` (413), where fit reads it streamed; it is at most 1 MiB.
- **A revoke** writes each new secret (32 random bytes, base64url) to a temporary file beside
  the secret file, mode 0600, synced, then renamed over it, on a thread of the handler's own with
  a queue of one; the request is suspended and answered from there, or 504 past
  `revoke_timeout` by akit-media's deadline thread until akio's A296.

## I144: playback URLs that expire

- **The URLs and grants are fit's byte for byte**: the key `HMAC-SHA256(sign key, "fit media
  url\0")`, the MAC over `fit-url-1`, the resource and the fields, its first 16 bytes in
  base64url, the query's order and escapes. A URL fit signs plays on akit and one akit signs on
  fit, given the same secret or keys and policy name; a unit test holds the query fit's `SignURL`
  gives. The cookie a grant goes in is `akit_media_grant` (decision 037), so a player that keeps
  cookies does not carry a grant from one gateway to the other on the same host.
- **A check is on the I/O thread with no callback**, as fit's: the MAC, the expiry, the address,
  `Referer` and `Origin`, then the session's entry under its shard's lock. It allocates a little
  (the query copied before the target is redacted, the grant's fields); fit's allocates nothing.
- **Tests name the time** rather than move a clock: the policy's checks and sessions take the
  time as an argument (`let_in`, `pass_on_at`, `sessions_at`), which the handlers give from the
  clock; the handlers' tests sign URLs already expired, or good for an hour.
- **A playlist that hands on a grant or a token is the viewer's own** (`private, no-store`), the
  media playlist the cache keeps sent with the grant's cookie when `cookie` is on, as fit's.
- **The Admin API's sessions and refusal counts** (fit's `admin/media.go`) come with the console's
  pages (I149); the policy has them now (`sessions`, `end_session`, `url_refusals`).

## I145: channels

- **The channels are fit's**: their states, their choices of item (the list, shuffle, the
  schedule's slots cutting in and joined), their timeline (each packet sent when its decode time
  falls due, the next item placed by its presentation after the last's, decode times never going
  back on a track), their Admin API and their metrics, named akit's (`akit_streaming_channel*`).
- **The timers run on akit-media's deadline thread**, one thread for every channel, until akio
  gives a handler a timer of its engine (A296); a channel's tick takes its lock for a few steps and
  hands packets to the hub, never blocking. Movies and samples are read on the library's pool.
- **Work that calls out runs once the channel's lock is released**: the library may answer a
  movie at once, on the caller's thread, which then takes the lock again; fit spawns a goroutine
  for it.
- **A time zone is loaded when the role applies**, from the system's zoneinfo files with
  akit-observe's reader, as the access log's and BitTorrent's schedules' are; akit-config checks
  its name's form. Slots fall on the time of day as written across a change of the clocks, as Go's
  `time.Date` puts them.
- **Tests move a clock** (`clock::Clock`): a channel's pace, its schedule and its retries are
  tested on the test's time, waiting for the library's pool to be idle, never for wall time.
- **The media libraries apply before the streaming role**, so that a channel finds its library at
  its first item.
- **A channel waits to be told, never retries on a timer**: where fit tries again later, akit's
  channel waits for the event that lets it go on (akit's timers are for what is about time, never
  a retry):
  - its library's first scan: the library tells the channels waiting once its first catalog is in
    place, or once it is removed (`Library::when_scanned`), where fit tries every 200 ms;
  - room in the library's file pool or sample tables, for a load or a read refused as busy: the
    library's room notice (`library::notice::Notice`) changes each time a job leaves the pool's
    queue, a movie's read ends, the pool is replaced or the library removed, and a channel waits
    for a change since it read the notice before trying, so that none is missed; fit tries again
    in 200 ms;
  - something to play, once every item failed or the list played out: the library's catalog
    replaced by a scan, a reload, or the schedule's next slot, where fit tries every 5s;
  - its stream, taken by another publisher or the hub full: the hub's `on_unpublish`, through the
    channels manager, where fit tries every 5s. A stream slot that frees without a publisher
    leaving (players waiting for a stream that never came giving up) does not tell it; a reload
    or the Admin API's start does.

## I146: push targets

- **An SRT push is akit's own on akio-media's SRT session**: `srt::dial` with a handler of the
  push's, which hears the handshake and the close, its stream muxed into MPEG-TS as `srt::push`
  muxes it. akio-media's `srt::push` has no reconnect and tells its owner nothing; the push retries
  on the manager's clock, fit's backoff, which the config states.
- **An RTMP push is akio-media's**, which tells its owner nothing either (akio API request A298):
  its retries are the client's `Reconnect`, with `retry_min` and `retry_max`, and a watch of its
  stream (a subscription that waits) starts a client again when a publisher comes back, which the
  client does not survive. No timer polls a client. Its state is read from the client; its failures
  and bytes are not counted, and a target refusing it for its credentials ends it, until A298.
- **Dials run on akit-media's pool**: they resolve host names, which blocks, and `srt::dial` waits
  briefly for its socket; the session's callbacks hand their work there too, off the session's
  lock, which closing it takes.
- **`rtmps://` is refused as not supported yet** until akio-media's RTMP client dials over TLS
  (A297), rather than accepted and never connected.
- **A push URL is a secret**: the errors and the config's `Debug` show its scheme and host, and the
  Admin API its scheme, host and app.

## I147: encrypted segments

- **The keys are fit's, byte for byte**: the same library name and secret give the same item and
  channel keys, the same IVs (the media sequence number) and the same keys' ID, so a key fetched
  from fit decrypts akit's segments and the other way round. A vod segment's ETag carries the
  keys' ID, and its cache key the encryption, so a new secret makes new segments and ETags.
- **A channel is encrypted by akio-media's muxer** (`HlsEncryption`), which encrypts its segments
  and parts and serves its keys at `key-<n>.key` beside them, authorized by the handler's `auth`
  before akio sees the request. A channel's two formats are two muxers with keys of their own, so
  the MPEG-TS muxer names its keys `key-<n>.key?format=ts` and the handler routes a key by that
  query, as it routes a playlist. fit names a key `key-<session>-<n>.key` and serves it itself; a
  player follows the playlist's `URI` either way, so the names need not match.
- **A session is a muxer's**: a channel's key is derived of `<stream>/<session>` and its number, as
  fit's, and a muxer that starts again (its number back at 0) takes a new random session, so its
  keys and IVs never repeat.
- **A change of encryption makes new muxers**: the handler keeps a channel's encrypted services
  with what they were made for (the hub, the library, the keys' ID, the rotation) and makes new
  ones at the first request after any of these changes, so a channel is never served clear past
  the reload that encrypts it, nor under old keys past one that changes them; fit ends its muxers
  at the next playlist request for the same reasons.
- **Without `auth` a handler serves no key of an encrypted channel** (403), since anyone could take
  it; a channel whose library or keys are not loaded answers 503 with `Retry-After: 1`, never
  clear.

## I148: the video site

- **The API answers fit's bodies**: the library and player pages are fit's, copied at the pin, so
  `api/library`, `api/library/<name>` and `api/play` answer what fit's do, member for member; an
  interop run compares them with fit's binary under the same config.
- **The checks of `libraries` are the config's**: whether each names a `media_library`, and once,
  needs the config's libraries, which akit-config has once every file is read, as it checks
  `vod`'s `library`; the rest (`vod`'s template, `channels` without `hls`) are the handler's, as
  fit's are its provision's. Either way the config is refused with fit's messages.
- **The guide is `docs/guides/media-library.md`**, beside akit's other guides, rather than
  `docs/media-library.md`, where fit keeps it; its configs are checked by `akit validate` with every
  other guide's.

## I149: the console's media pages

- **The pages are fit's, their endpoints akit-media's**: the console's `gw.medialib` module was
  copied with the other pages at the pin; `akit_media::workbench` answers what it asks of the Admin
  API, as fit's `admin/media.go` does, the same members and refusals.
- **The endpoints read the running config the Admin API keeps**, parsed when they are asked, and
  the listeners' bound ports from the engine, on the crate's pool; the console's
  Content-Security-Policy reads the same, again only when the config's version changes.
- **The console's user and host reach them as `Akit-Console-User` and `Akit-Console-Host`**
  (fit's `Fit-Console-User` and `Fit-Console-Host`, renamed, decision 037), set by the console on
  what it passes, never taken from the browser; a caller with the Admin API's token may set them.
- **An upload's files keep fit's names** (`.fit-upload-<id>.part` and `.json`, `.fit-tmp-` for a
  file written whole), as the `fitstore` file keeps its magic: an upload begun on fit goes on on
  akit, and the other way round, from the same `upload_dir`.
- **A `PATCH`'s body is held, not blocked on**: fit's handler blocks its goroutine while a piece is
  written; akit's holds the body's reads (akio's `BodyReads::hold`) and lets them go once the
  piece is on disk, gathering what a read already under way brings, so at most 512 KiB and one
  read are kept. A body cut short ends the write with what was written, as fit's read error does.
- **The errors of the files say what fit's say without fit's `medialib:` prefix**, as akit's other
  library errors do; their statuses are fit's.
