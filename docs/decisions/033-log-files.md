# 033: Log files, rotation and retention, as fit's

[English](033-log-files.md) | [简体中文](033-log-files.zh-CN.md)

Status: accepted in I81 (2026-11-26).

## Context

fit's own log and its access log can write to a file, and since fit's I155–I156 a file is rotated
by size and by time and its rotated files removed by age, number and total size, gzipped, in the
log's time zone ([fit's decision 034](https://github.com/lesismal/fit/blob/main/docs/decisions/034-log-files.md)).
akit's access log (I21) appended to one file for as long as akit ran and refused `rotate` as not
supported yet; akit had no `log` at all. The Logging track ports fit's I155–I157 in I81–I83.

## Decision

1. **A sink of akit's own, as fit's.** akit-observe's `Files` keeps one sink a path, shared by
   every log writing there (the access log, and akit's own loggers from I82), as fit's
   `logs.Files`. A write queues its buffer and returns; past a queue of 1 MiB (or 256 buffers) it
   is refused and counted, never waited for, so no I/O thread, request or logger waits for the
   disk (rules 1 and 11). A thread of the sink's own writes, rotates and sweeps; the access log's
   writer thread hands it each flushed buffer as it is, without a copy, and counts the lines of a
   refused one as dropped. Closing a sink writes what it queued and waits, at most 30 s, for the
   compressions of its files.
2. **fit's rotation, naming and retention, unchanged.** Before a write that would take the file
   past `max_bytes` (a buffer larger than that gets a file of its own, so the access log's unit is
   its flushed batch, as fit's I156 says) and at the first write in a later period of `interval`,
   a period a whole number of intervals from midnight in the log's zone (a period of whole days
   grouping days from 1970-01-01). The rotated name is `<path>.<yyyy-mm-ddThh-mm-ss.mmm>`, the
   rotation's time in UTC, `-1`, `-2` when taken, `.gz` once compressed. A rotation that fails
   keeps the file and is not tried again by size or time for a minute. Retention at open, after
   every rotation and every ten minutes: older than `max_age`, then past `max_files`, then the
   oldest past `max_total_bytes`; only names a rotation makes from that path, never the open file.
   `rotate`'s fields, defaults and checks are fit's `config.Rotate`'s.
3. **Compression on a thread of the set's own**, started with the first file compressed, a queue
   of 16 and a minute's wait at most (fit's `internal/filepool` with one worker): gzip by
   akio-deflate into a temporary file created exclusively, renamed, the plain file removed; a
   failure keeps the plain file and says why. No dependency is added (decision 001).
4. **No link is followed, without `O_NOFOLLOW`.** fit opens with `O_NOFOLLOW|O_NONBLOCK`. akio-sys
   has no `O_NOFOLLOW`, and akit has no `unsafe` and no `libc`; so a file is looked at without
   following a link (`symlink_metadata`), must be a regular file with one link, is opened with
   `O_NONBLOCK` (akio-sys's) so that a FIFO put at the name cannot hold the open, and must then be
   the same file (device and inode) the name held. A link swapped in between is opened but never
   written or read, and refused. A new file, and a compression's temporary file, are created with
   `O_EXCL` (`create_new`), which does not follow a link. This is what `O_NOFOLLOW` gives, so no
   akio request is made. A directory others may write is warned of, as fit warns.
5. **Zones from the system's zoneinfo**, as I21 decided for the access log: fit embeds Go's
   `time/tzdata`; akit reads `/usr/share/zoneinfo` and the like (akit-observe's `Zone`), so an
   IANA name needs a zoneinfo in a container, and an unknown name fails the config.
6. **A reload changes the rotation and the zone in place**, as fit's: the sink keeps its file and
   takes the new settings at its next write; its path, and a log's format, take a restart.

## Consequences

- A slow disk costs at most the queue: 1 MiB a file, behind the access log's own buffer.
- A file shared by two logs has one sink and one policy; I82 refuses a config that gives one
  path two `rotate`s or two zones, as fit's.
- `"rotate": {}` is a bounded policy of fit's defaults; a file output without `rotate` grows, and
  `akit run` says so when it starts.
- What to measure is in the dev plan's benchmark table (I81's row): a feature task runs no
  benchmark (rule 30).
