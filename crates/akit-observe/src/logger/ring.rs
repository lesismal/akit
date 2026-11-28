//! The ring of recent messages a log keeps (fit's `logs/ring.go`, I139c): the last
//! [`RING_SIZE`] messages written, each cut to [`RING_MESSAGE_BYTES`], numbered from 1 since akit
//! started, which the Admin API's `GET /logs/recent` answers and a manager tails across its
//! members after the last number it saw. A fixed ring under a lock: it never grows past its
//! size, and a message is added by the thread that logs it, as it is written.

use std::time::SystemTime;

use super::Level;

/// How many messages the ring keeps; one more removes the oldest.
pub const RING_SIZE: usize = 1000;

/// The most a message is as the ring keeps it, its attributes included; a longer one is cut.
pub const RING_MESSAGE_BYTES: usize = 2048;

/// How many [`Root::recent`](super::Root::recent) returns when its filter does not say.
pub const DEFAULT_RECENT: usize = 100;

/// A message the log wrote, as its ring keeps it: fit's `logs.Record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Numbers the messages the ring was given since akit started, from 1: a reader that keeps
    /// the last it saw asks for those after it ([`Filter::after`]).
    pub seq: u64,
    /// When it was written.
    pub time: SystemTime,
    /// Its level.
    pub level: Level,
    /// The logger's name, as a config names it.
    pub logger: &'static str,
    /// The message and its attributes, ` key=value` each, cut to [`RING_MESSAGE_BYTES`].
    pub message: String,
}

impl Record {
    /// The record as fit's JSON writes it: `{"seq","time","level","logger","message"}`, the time
    /// in RFC 3339 with its nanoseconds, in UTC.
    #[must_use]
    pub fn json(&self) -> String {
        let d = self
            .time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let mut b = Vec::with_capacity(self.message.len().saturating_add(96));
        b.extend_from_slice(b"{\"seq\":");
        b.extend_from_slice(self.seq.to_string().as_bytes());
        b.extend_from_slice(b",\"time\":\"");
        crate::time::rfc3339_nano(
            &mut b,
            i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            d.subsec_nanos(),
            0,
        );
        b.extend_from_slice(b"\",\"level\":\"");
        b.extend_from_slice(level_name(self.level).as_bytes());
        b.extend_from_slice(b"\",\"logger\":");
        crate::format::json_string(&mut b, self.logger.as_bytes());
        b.extend_from_slice(b",\"message\":");
        crate::format::json_string(&mut b, self.message.as_bytes());
        b.push(b'}');
        String::from_utf8(b).unwrap_or_default()
    }
}

/// Which of the ring's messages [`Root::recent`](super::Root::recent) returns: fit's
/// `logs.Filter`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// When set, the only logger whose messages are returned.
    pub logger: String,
    /// The least level returned; every level when `None`.
    pub level: Option<Level>,
    /// When set, text the message must hold, case ignored.
    pub contains: String,
    /// The most returned, the latest; [`DEFAULT_RECENT`] when 0, and never more than
    /// [`RING_SIZE`].
    pub limit: usize,
    /// When not 0, only the messages numbered after it; one past the last message's, as a
    /// reader's of an akit since restarted, is taken as 0.
    pub after: u64,
}

/// The ring itself, under the log's lock.
#[derive(Debug, Default)]
pub(crate) struct Ring {
    /// The records, [`RING_SIZE`] once full, written in a circle.
    recs: Vec<Record>,
    /// Where the next goes, once full.
    next: usize,
    /// The last record's number.
    seq: u64,
}

impl Ring {
    pub(crate) fn add(
        &mut self,
        time: SystemTime,
        level: Level,
        logger: &'static str,
        message: String,
    ) {
        self.seq = self.seq.saturating_add(1);
        let rec = Record {
            seq: self.seq,
            time,
            level,
            logger,
            message: clip_utf8(message, RING_MESSAGE_BYTES),
        };
        if self.recs.len() < RING_SIZE {
            if self.recs.capacity() == 0 {
                // Its whole ring at once, so that adding never grows it past.
                self.recs.reserve_exact(RING_SIZE);
            }
            self.recs.push(rec);
        } else if let Some(slot) = self.recs.get_mut(self.next) {
            *slot = rec;
            self.next = self.next.saturating_add(1) % RING_SIZE;
        }
    }

    pub(crate) fn last(&self) -> u64 {
        self.seq
    }

    pub(crate) fn recent(&self, f: &Filter) -> Vec<Record> {
        let limit = match f.limit {
            0 => DEFAULT_RECENT,
            n => n.min(RING_SIZE),
        };
        let contains = f.contains.to_lowercase();
        let after = if f.after > self.seq { 0 } else { f.after };
        let n = self.recs.len();
        let mut out = Vec::new();
        // Newest first: the one before `next` once full, the last pushed before.
        for i in 0..n {
            if out.len() >= limit {
                break;
            }
            let at = self
                .next
                .saturating_add(n)
                .saturating_sub(1)
                .saturating_sub(i)
                .checked_rem(n)
                .unwrap_or(0);
            let Some(rec) = self.recs.get(at) else { break };
            if rec.seq <= after {
                break;
            }
            if (!f.logger.is_empty() && rec.logger != f.logger)
                || f.level.is_some_and(|l| rec.level < l)
                || (!contains.is_empty() && !rec.message.to_lowercase().contains(&contains))
            {
                continue;
            }
            out.push(rec.clone());
        }
        out.reverse();
        out
    }
}

/// The level's name as a config writes it.
#[must_use]
pub fn level_name(l: Level) -> &'static str {
    match l {
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error => "error",
    }
}

/// `s` cut to at most `n` bytes, on a character's boundary.
fn clip_utf8(mut s: String, n: usize) -> String {
    if s.len() <= n {
        return s;
    }
    let mut at = n;
    while at > 0 && !s.is_char_boundary(at) {
        at = at.saturating_sub(1);
    }
    s.truncate(at);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_of(n: usize) -> Ring {
        let mut r = Ring::default();
        for i in 0..n {
            let level = if i % 10 == 9 {
                Level::Error
            } else {
                Level::Info
            };
            let logger = if i % 2 == 0 { "server" } else { "tls" };
            r.add(SystemTime::now(), level, logger, format!("line {i}"));
        }
        r
    }

    fn msgs(rs: &[Record]) -> Vec<&str> {
        rs.iter().map(|r| r.message.as_str()).collect()
    }

    // fit's TestRing: the latest, filtered, after a number, the ring's size kept.
    #[test]
    fn keeps_the_latest() {
        let r = ring_of(RING_SIZE.saturating_add(10));
        let all = r.recent(&Filter {
            limit: RING_SIZE.saturating_mul(2),
            ..Filter::default()
        });
        assert_eq!(all.len(), RING_SIZE);
        assert_eq!(all.first().map(|r| r.message.as_str()), Some("line 10"));
        assert_eq!(all.last().map(|r| r.message.as_str()), Some("line 1009"));
        assert_eq!(r.last(), 1010);
        assert_eq!(all.last().map(|r| r.seq), Some(1010));
        let some = r.recent(&Filter::default());
        assert_eq!(some.len(), DEFAULT_RECENT);
        let after = r.recent(&Filter {
            after: 1007,
            ..Filter::default()
        });
        assert_eq!(msgs(&after), ["line 1007", "line 1008", "line 1009"]);
        // A number past the last, an akit restarted: from the start.
        let past = r.recent(&Filter {
            after: 5000,
            limit: 2,
            ..Filter::default()
        });
        assert_eq!(msgs(&past), ["line 1008", "line 1009"]);
        let errors = r.recent(&Filter {
            level: Some(Level::Warn),
            limit: 3,
            ..Filter::default()
        });
        assert_eq!(msgs(&errors), ["line 989", "line 999", "line 1009"]);
        let tls = r.recent(&Filter {
            logger: "tls".to_owned(),
            contains: "LINE 100".to_owned(),
            ..Filter::default()
        });
        assert_eq!(
            msgs(&tls),
            [
                "line 1001",
                "line 1003",
                "line 1005",
                "line 1007",
                "line 1009"
            ]
        );
        let few = ring_of(3);
        assert_eq!(
            msgs(&few.recent(&Filter::default())),
            ["line 0", "line 1", "line 2"]
        );
        assert!(Ring::default().recent(&Filter::default()).is_empty());
    }

    #[test]
    fn cut_and_written() {
        let mut r = Ring::default();
        r.add(
            SystemTime::UNIX_EPOCH,
            Level::Warn,
            "server",
            "é".repeat(RING_MESSAGE_BYTES),
        );
        let rec = r.recent(&Filter::default()).remove(0);
        assert_eq!(rec.message.len(), RING_MESSAGE_BYTES);
        let mut r = Ring::default();
        r.add(
            SystemTime::UNIX_EPOCH,
            Level::Warn,
            "server",
            "a \"b\"".to_owned(),
        );
        let rec = r.recent(&Filter::default()).remove(0);
        assert_eq!(
            rec.json(),
            r#"{"seq":1,"time":"1970-01-01T00:00:00Z","level":"warn","logger":"server","message":"a \"b\""}"#
        );
    }
}
