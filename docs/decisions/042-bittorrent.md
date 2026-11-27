# 042: BitTorrent: fit's roles on akio-bittorrent

[English](042-bittorrent.md) | [简体中文](042-bittorrent.zh-CN.md)

Status: accepted in I150–I151 (2026-11-27), waiting for review ([decisions waiting for
review](../decisions-pending.md), rows 75–79); extended by the iterations of the BitTorrent track
(I150–I159) as they land.

## Context

fit's I196–I210 added two roles to fit's config ([fit's decision
037](https://github.com/lesismal/fit/blob/main/docs/decisions/037-bittorrent.md)):
`bittorrent.client`, a client that downloads and seeds torrents kept in its `data_dir`, and
`bittorrent.tracker`, a tracker over HTTP (the `bt_tracker` handler) and UDP; with `bt_stream`,
publishing, the Admin API's `/bittorrent/...`, the pages and RSS. fit built them on fib's
`bittorrent` packages after asking fib for what they lacked (fib's A75–A79, A85–A88). akio has a
BitTorrent crate of its own, akio-bittorrent: bencoding, v1, v2 and hybrid torrents, magnet links,
HTTP and UDP trackers (client and server), the peer wire with MSE, uTP, the DHT, LSD, web seeds,
and a client with fast resume, wire compatible with libtorrent and aria2.

## Decision

1. **akit's roles run on akio-bittorrent**, in a crate of their own, `akit-bittorrent`, which the
   binary registers: its roles (`TRACKER`, the client from I152) and its handler modules
   (`bt_tracker`; `bt_stream` and `bt_publish` with their iterations), listed in `STANDARD` beside
   akit's other module crates. akio-bittorrent is akio's: a new dependency of the same repository
   and pin as every akio crate (rule 22), with this note (rule 20).
2. **The roles are fit's**: the config's fields, defaults, bounds and checks (I150), the
   answers and refusals on the wire (fit's failure reasons, `min interval`, 50 peers by default,
   a UDP answer within 1,400 bytes, a forged UDP connection ID dropped), the console's routes, and
   the client's state files in `data_dir` (`torrents.json`, `torrents/<id>.torrent`) so that a
   data directory moves between fit and akit (I152).
3. **What akio-bittorrent lacks is asked of akio** (A283–A288), each with a stopgap only where it
   is sound: the client on its own engine until A283; the client's `access`, `max_conns`,
   `max_half_open` and `disk_queue_bytes` refused as not supported yet until A284 (row 76); the
   tracker's `max_peers` refused and a change of its store's bounds making a new store until A285
   (rows 76, 77); the tracker's HTTP and UDP sides answered by akit's own code, fib's rules, on
   akio-bittorrent's `TrackerServer` as the store, until akio's answer fit's (A285, row 78).
4. **Tests stay on loopback**: two akio-bittorrent clients, fit's own binary (`AKIT_FIT_BIN`),
   aria2c where installed; the DHT and LSD off; never a public tracker or DHT router.

5. **I153, as fit's I198**: the manager is a thread of the role's own that looks every second,
   as fit's does, and at once when a magnet link's metadata comes; the queue and the disk check
   are also applied in each API call that changes the list. Every torrent is added paused and
   started by the schedule. `min_free_bytes` reads akio's `fs::space` (`available`, a process
   that is not root) of the torrent's directory or its nearest ancestor; where it cannot be
   told, it is not checked. A .torrent URL is fetched on akio's HTTP client on the listeners'
   engine, its redirects followed by the role so that one to a magnet link adds that link.
   `watch_dir` looks at most 100,000 entries of a directory a look. A category's `limits` and
   `media_library` are refused until I154 and the media library's track.

6. **I154, as fit's I199**: a category's limits and a torrent's own are its rates and seed
   limits through akio-bittorrent's per-torrent ones, own over category field by field; their
   `max_conns` and `upload_slots` are refused until akio's A284, as are the client's `access`
   and its connection limits (I154b). The alternative rates follow fit's rules (a range that
   runs past midnight holds into the next day, `24:00` and an ending `00:00` are midnight at the
   end, the API's `on` and `off` override the schedules until `auto`, saved in `torrents.json`'s
   `alt_speed`); the zone is loaded when the role applies, a name that does not load refusing the
   apply, as a log's `time_zone` is checked when the log opens.

## Consequences

- A config fit runs that sets a field akio cannot give yet is refused by name, not run without it.
- The tracker's swarms start empty when its bounds change, as fit's did before fib's A85.
- `akit build -without akit-bittorrent` leaves `bt_tracker` out of a build.
