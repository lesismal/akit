//! The tracker's console (I151, fit's `bittorrent/tracker/api.go`): its statistics, read-only, at
//! `GET /api/stats`, `/api/torrents` (the allow-list's, paged by `offset` and `limit`, at most
//! 1,000) and `/api/torrents/<hash>`. A public tracker's torrents are listed once akio-bittorrent
//! can walk its store (akio's A285); a private tracker's passkeys come with `auth` (I159).

use std::fmt::Write as _;

use akio_http::Context;
use akit_config::bittorrent::parse_info_hash;
use akit_config::value::write_string;
use akit_roles::pages::{Module, fail, json, no_endpoint};

use super::{Stats, TorrentStats, stats_of, torrent_of, with_snap};

/// fit's `MaxListed`: the most torrents a page of the list holds.
pub(crate) const MAX_LISTED: usize = 1000;

/// The role's module of the pages.
#[derive(Debug)]
pub(crate) struct Pages {
    key: usize,
}

impl Pages {
    pub(crate) fn new(key: usize) -> Pages {
        Pages { key }
    }
}

const NOT_RUNNING: &str = "bittorrent.tracker: the tracker does not run";

fn stats_json(s: &Stats) -> String {
    let mut b = String::from("{");
    let _ = write!(
        b,
        "\"torrents\":{},\"announces\":{},\"scrapes\":{},\"refused\":{{",
        s.torrents, s.announces, s.scrapes
    );
    for (i, (name, n)) in s.refused.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(&mut b, name);
        let _ = write!(b, ":{n}");
    }
    let _ = write!(b, "}},\"private\":{},\"allowed\":{}", s.private, s.allowed);
    if let Some(u) = s.udp {
        b.push_str(",\"udp\":");
        write_string(&mut b, &u.to_string());
    }
    let _ = writeln!(b, ",\"max_torrents\":{}}}", s.max_torrents);
    b
}

fn torrent_json(b: &mut String, t: &TorrentStats) {
    b.push_str("{\"info_hash\":");
    write_string(b, &t.info_hash);
    let _ = write!(
        b,
        ",\"seeders\":{},\"leechers\":{},\"completed\":{},\"tracked\":{}}}",
        t.seeders, t.leechers, t.completed, t.tracked
    );
}

fn query_usize(target: &str, key: &str) -> usize {
    let q = target.split_once('?').map_or("", |(_, q)| q);
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0)
}

impl Module for Pages {
    fn id(&self) -> &'static str {
        super::ID
    }

    fn serve(&self, ctx: &mut Context, _user: &str, path: &str) {
        let get = ctx.req().method == akio_http::Method::Get;
        let r = self.answer(get, &ctx.req().target, path);
        ctx.respond(r);
    }
}

impl Pages {
    /// The answer to a request of `path`, its target `target`, a GET when `get`.
    pub(crate) fn answer(&self, get: bool, target: &str, path: &str) -> akio_http::Response {
        if !get {
            return no_endpoint();
        }
        match path {
            "/api/stats" => match stats_of(self.key) {
                Some(s) => json(200, stats_json(&s)),
                None => fail(503, NOT_RUNNING),
            },
            "/api/torrents" => {
                let offset = query_usize(target, "offset");
                let mut limit = query_usize(target, "limit");
                if limit == 0 || limit > MAX_LISTED {
                    limit = MAX_LISTED;
                }
                self.list(offset, limit)
            }
            "/api/passkey" => fail(
                404,
                "bittorrent.tracker: the tracker is public: it takes no passkeys",
            ),
            p => match p.strip_prefix("/api/torrents/") {
                Some(h) => match parse_info_hash(h) {
                    None => fail(400, "not an info hash: 40 hex digits (v1) or 64 (v2)"),
                    Some(h) => match torrent_of(self.key, h) {
                        Some(t) => {
                            let mut b = String::new();
                            torrent_json(&mut b, &t);
                            b.push('\n');
                            json(200, b)
                        }
                        None => fail(503, NOT_RUNNING),
                    },
                },
                None => no_endpoint(),
            },
        }
    }
}

impl Pages {
    /// A page of the allow-list's torrents with their statistics.
    fn list(&self, offset: usize, limit: usize) -> akio_http::Response {
        with_snap(self.key, |s| {
            let Some(s) = s else {
                return fail(503, NOT_RUNNING);
            };
            let Some(a) = &s.allow else {
                return fail(
                    501,
                    "bittorrent.tracker: a public tracker's torrents are listed once akio can \
                     walk its store (akio's A285); a torrent is read by its info hash",
                );
            };
            let from = offset.min(a.list.len());
            let to = from.saturating_add(limit).min(a.list.len());
            let mut b = String::from("{\"torrents\":[");
            for (i, h) in a.list.get(from..to).unwrap_or(&[]).iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                torrent_json(&mut b, &super::serve::torrent_stats(&s.server, *h));
            }
            let _ = writeln!(
                b,
                "],\"total\":{},\"source\":\"allow_list\"}}",
                a.list.len()
            );
            json(200, b)
        })
    }
}
