//! Announces and scrapes as fit's tracker answers them (fib's `tracker.Server`): the HTTP query
//! read as fib reads it, the allow-list and the access checked, each announce's peers bounded,
//! the answers encoded, and BEP 15's UDP requests with connection IDs keyed by the role's secret.
//! The swarms are akio-bittorrent's `TrackerServer`'s.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;

use akio::{Acceptor, Conn, Handler};
use akio_bittorrent::bencode::{self, Value};
use akio_bittorrent::metainfo::InfoHash;
use akio_bittorrent::tracker::{
    AnnounceRequest, AnnounceResponse, Event, TrackerError, TrackerServer, udp,
};

use super::{REFUSED_ACCESS, REFUSED_PASSKEY, REFUSED_UNREGISTERED, Snap, TorrentStats, with_snap};

/// The peers an announce gets when it does not say: fib's `DefaultNumWant`.
pub(crate) const DEFAULT_NUMWANT: i64 = 50;
/// The most bytes of a UDP answer, clear of fragmentation on IPv6's minimum MTU and tunnels:
/// fib's `maxUDPReply`.
pub(crate) const MAX_UDP_REPLY: usize = 1400;
/// An announce answer's header over UDP: action, transaction, interval, leechers, seeders.
const UDP_ANNOUNCE_HEADER: usize = 20;
/// The most info hashes a UDP scrape names, bounded by its packet (BEP 15).
const MAX_UDP_SCRAPE: usize = 74;

/// The most URL data a UDP announce's options carry that is read (BEP 41): fib's `maxURLData`.
const MAX_URL_DATA: usize = 2048;

/// The passkey of a request's path and query (fib's `passkeyOf`): its `passkey` parameter, or
/// else the path segment before the last one when that one begins with `announce` or `scrape`,
/// as in `/0123abcd/announce`, where private trackers put their users' passkeys.
pub(crate) fn passkey_of(path: &str, query: &str) -> String {
    if let Some(q) = parse_query(query) {
        let k = first(&q, "passkey");
        if !k.is_empty() {
            return k.to_owned();
        }
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    let Some((rest, last)) = path.rsplit_once('/') else {
        return String::new();
    };
    if !last.starts_with("announce") && !last.starts_with("scrape") {
        return String::new();
    }
    rest.rsplit('/').next().unwrap_or("").to_owned()
}

/// The target `uri` with its passkey replaced by `REDACTED` (fit's `redacted`): its `passkey`
/// parameter and, with `in_path`, the path segment before its last `announce` or `scrape` one;
/// `None` when it has none.
pub(crate) fn redacted(uri: &str, in_path: bool) -> Option<String> {
    let (mut path, query) = match uri.split_once('?') {
        Some((p, q)) => (p.to_owned(), Some(q)),
        None => (uri.to_owned(), None),
    };
    let mut changed = false;
    if in_path {
        if let Some(i) = path.rfind('/').filter(|i| *i > 0) {
            let last = path.get(i.saturating_add(1)..).unwrap_or("");
            if last.starts_with("announce") || last.starts_with("scrape") {
                let head = path.get(..i).unwrap_or("");
                let j = head.rfind('/').map_or(0, |j| j.saturating_add(1));
                let seg = head.get(j..).unwrap_or("");
                if !seg.is_empty() && seg != "REDACTED" {
                    path = format!(
                        "{}REDACTED{}",
                        path.get(..j).unwrap_or(""),
                        path.get(i..).unwrap_or("")
                    );
                    changed = true;
                }
            }
        }
    }
    let query = query.map(|q| {
        q.split('&')
            .map(|kv| match kv.split_once('=') {
                Some(("passkey", v)) if v != "REDACTED" => {
                    changed = true;
                    "passkey=REDACTED"
                }
                _ => kv,
            })
            .collect::<Vec<_>>()
            .join("&")
    });
    if !changed {
        return None;
    }
    Some(match query {
        Some(q) => format!("{path}?{q}"),
        None => path,
    })
}

/// The URL data of a UDP announce's options (BEP 41), after its 98 bytes: what the client sends
/// of the tracker URL's path and query; `""` without.
fn url_data(data: &[u8]) -> String {
    let mut out = Vec::new();
    let mut b = data.get(98..).unwrap_or(&[]);
    while let Some((&kind, rest)) = b.split_first() {
        match kind {
            0 => break,
            1 => b = rest,
            2 => {
                let Some((&n, rest)) = rest.split_first() else {
                    break;
                };
                let n = usize::from(n);
                let Some(chunk) = rest.get(..n) else { break };
                if out.len().saturating_add(n) > MAX_URL_DATA {
                    break;
                }
                out.extend_from_slice(chunk);
                b = rest.get(n..).unwrap_or(&[]);
            }
            _ => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A bencoded failure, BEP 3's refusal.
pub(crate) fn failure(reason: &str) -> Vec<u8> {
    bencode::encode(&Value::dict().with("failure reason", reason))
}

/// An announce as the tracker serves it, over either protocol.
#[derive(Debug, Clone)]
pub(crate) struct Announce {
    pub(crate) info_hash: [u8; 20],
    pub(crate) peer_id: [u8; 20],
    pub(crate) port: u16,
    /// -1 when not said: the peer counts as a leecher.
    pub(crate) left: i64,
    pub(crate) event: Event,
    /// -1 for the default.
    pub(crate) numwant: i64,
    pub(crate) compact: bool,
    /// The families of the `ipv4` and `ipv6` parameters (BEP 7), which an HTTP peer gets peers
    /// of besides its own.
    pub(crate) want_v4: bool,
    pub(crate) want_v6: bool,
}

/// A query's values, percent-decoded as Go's `url.ParseQuery` decodes them (`+` a space), in
/// the order given; `None` for a malformed escape or a `;`, as Go refuses it.
fn parse_query(q: &str) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut out = Vec::new();
    for kv in q.split('&') {
        if kv.is_empty() {
            continue;
        }
        if kv.contains(';') {
            return None;
        }
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        out.push((unescape(k)?, unescape(v)?));
    }
    Some(out)
}

fn unescape(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        match c {
            b'%' => {
                let hex = b.get(i.saturating_add(1)..i.saturating_add(3))?;
                let txt = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(txt, 16).ok()?);
                i = i.saturating_add(3);
            }
            b'+' => {
                out.push(b' ');
                i = i.saturating_add(1);
            }
            _ => {
                out.push(c);
                i = i.saturating_add(1);
            }
        }
    }
    Some(out)
}

fn values<'q>(q: &'q [(Vec<u8>, Vec<u8>)], key: &str) -> impl Iterator<Item = &'q [u8]> {
    q.iter()
        .filter(move |(k, _)| k == key.as_bytes())
        .map(|(_, v)| v.as_slice())
}

fn first<'q>(q: &'q [(Vec<u8>, Vec<u8>)], key: &str) -> &'q str {
    values(q, key)
        .next()
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or("")
}

fn hash20(v: &[u8]) -> Option<[u8; 20]> {
    <[u8; 20]>::try_from(v).ok()
}

/// Reads an HTTP announce's query as fib's `parseAnnounceQuery`; the error is the failure reason
/// the client is told.
pub(crate) fn parse_announce(query: &str) -> Result<Announce, &'static str> {
    let q = parse_query(query).ok_or("invalid query string")?;
    let one = |key: &str, err: &'static str| {
        let mut it = values(&q, key);
        match (it.next(), it.next()) {
            (Some(v), None) => hash20(v).ok_or(err),
            _ => Err(err),
        }
    };
    let info_hash = one("info_hash", "invalid info_hash")?;
    let peer_id = one("peer_id", "invalid peer_id")?;
    let port = first(&q, "port")
        .parse::<u16>()
        .map_err(|_| "invalid port")?;
    let number = |key: &str, default: i64| -> Result<i64, &'static str> {
        let s = first(&q, key);
        if s.is_empty() {
            return Ok(default);
        }
        s.parse::<i64>().map_err(|_| "invalid number")
    };
    number("uploaded", 0)?;
    number("downloaded", 0)?;
    let left = number("left", -1)?;
    let numwant = match first(&q, "numwant") {
        "" => -1,
        s => s
            .parse::<i32>()
            .map(|n| i64::from(n).max(-1))
            .map_err(|_| "invalid number")?,
    };
    let event = match first(&q, "event") {
        "" | "empty" => Event::None,
        "started" => Event::Started,
        "stopped" => Event::Stopped,
        "completed" => Event::Completed,
        _ => return Err("invalid event"),
    };
    let family = |key: &str| {
        first(&q, key)
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .ok()
    };
    let addr = |key: &str| {
        first(&q, key)
            .parse::<SocketAddr>()
            .ok()
            .map(|a| a.ip())
            .or_else(|| family(key))
    };
    Ok(Announce {
        info_hash,
        peer_id,
        port,
        left,
        event,
        numwant,
        compact: first(&q, "compact") != "0",
        want_v4: addr("ipv4").is_some_and(|a| a.to_canonical().is_ipv4()),
        want_v6: addr("ipv6").is_some_and(|a| !a.to_canonical().is_ipv4()),
    })
}

/// The info hashes of an HTTP scrape, at most `max`, those not 20 bytes long skipped, as fib's
/// `parseScrapeQuery`.
pub(crate) fn parse_scrape(query: &str, max: usize) -> Result<Vec<[u8; 20]>, &'static str> {
    let q = parse_query(query).ok_or("invalid query string")?;
    Ok(values(&q, "info_hash")
        .filter_map(hash20)
        .take(max)
        .collect())
}

/// The statistics of torrent `h` in `server`'s store.
pub(crate) fn torrent_stats(server: &TrackerServer, h: [u8; 20]) -> TorrentStats {
    let st = server.stats(&InfoHash(h));
    TorrentStats {
        info_hash: hex(&h),
        seeders: st.complete,
        leechers: st.incomplete,
        completed: st.downloaded,
        // A swarm is dropped with its last peer: one in the store has a peer.
        tracked: st.complete > 0 || st.incomplete > 0,
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(
        String::with_capacity(b.len().saturating_mul(2)),
        |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        },
    )
}

fn compact(out: &mut Vec<u8>, a: SocketAddr) {
    match canonical(a) {
        SocketAddr::V4(v4) => out.extend_from_slice(&v4.ip().octets()),
        SocketAddr::V6(v6) => out.extend_from_slice(&v6.ip().octets()),
    }
    out.extend_from_slice(&a.port().to_be_bytes());
}

fn canonical(a: SocketAddr) -> SocketAddr {
    SocketAddr::new(a.ip().to_canonical(), a.port())
}

impl Snap {
    /// Whether the allow-list takes `h`; a refusal is counted.
    fn allowed(&self, h: &[u8; 20]) -> bool {
        match &self.allow {
            Some(a) if !a.has(h) => {
                self.counters.refuse(REFUSED_UNREGISTERED);
                false
            }
            _ => true,
        }
    }

    /// A private tracker's check of `passkey` for each of `hashes` (fit's `authorize`): it must
    /// play `bittorrent/<info hash in hex>` in the tracker's `media_auth` policy. A lookup, never
    /// a bcrypt check: on the I/O thread. A refusal is counted and is the failure reason.
    pub(crate) fn authorize(&self, hashes: &[[u8; 20]], passkey: &str) -> Result<(), String> {
        let Some(g) = &self.auth else {
            return Ok(());
        };
        let refuse = |why: &str| {
            self.counters.refuse(REFUSED_PASSKEY);
            Err(why.to_owned())
        };
        let Some(p) = g.policy() else {
            return refuse("the tracker's passkeys are not available");
        };
        for h in hashes {
            let stream = format!("bittorrent/{}", hex(h));
            if let Err(e) = p.allow_passkey(&stream, passkey) {
                use akit_media::auth::Refusal;
                return refuse(match e {
                    Refusal::NoCredentials => "a passkey is required",
                    Refusal::Forbidden => "the passkey may not use this torrent",
                    Refusal::BadCredentials | Refusal::Pending => "unknown passkey",
                    Refusal::Busy | Refusal::Unavailable => {
                        "the tracker's passkeys are not available"
                    }
                });
            }
        }
        Ok(())
    }

    /// Serves an announce from `from` with `passkey`: the store's answer, its peers of the
    /// families `from` asks for, or the failure reason.
    pub(crate) fn announce(
        &self,
        from: IpAddr,
        a: &Announce,
        udp: bool,
        passkey: &str,
    ) -> Result<AnnounceResponse, String> {
        if !self.allowed(&a.info_hash) {
            return Err("unregistered torrent".to_owned());
        }
        self.authorize(&[a.info_hash], passkey)?;
        self.counters.announces.fetch_add(1, Ordering::Relaxed);
        let src = from.to_canonical();
        let v4 = src.is_ipv4();
        let mut want = if a.numwant < 0 {
            DEFAULT_NUMWANT
        } else {
            a.numwant
        };
        want = want.min(i64::try_from(self.numwant).unwrap_or(i64::MAX));
        if udp {
            let size: usize = if v4 { 6 } else { 18 };
            let room = MAX_UDP_REPLY
                .saturating_sub(UDP_ANNOUNCE_HEADER)
                .checked_div(size)
                .unwrap_or(0);
            want = want.min(i64::try_from(room).unwrap_or(i64::MAX));
        }
        let left = u64::try_from(a.left).unwrap_or(u64::MAX);
        let mut req = AnnounceRequest::new(InfoHash(a.info_hash), a.peer_id, a.port, left);
        req.event = a.event;
        req.num_want = i32::try_from(want).unwrap_or(i32::MAX);
        let mut r = self.server.announce(src, &req).map_err(|e| match e {
            TrackerError::Failure(m) if m == "tracker full" => "tracker is full".to_owned(),
            TrackerError::Failure(m) => m,
            other => other.to_string(),
        })?;
        // Over UDP the answer's format is the request's family; over HTTP a peer gets peers of
        // each family it has an address of.
        let (want_v4, want_v6) = if udp {
            (v4, !v4)
        } else {
            (v4 || a.want_v4, !v4 || a.want_v6)
        };
        r.peers.retain(|p| {
            if p.ip().to_canonical().is_ipv4() {
                want_v4
            } else {
                want_v6
            }
        });
        Ok(r)
    }

    /// Answers an HTTP request to `target` from `peer`: the status and the body, as fit's
    /// `bt_tracker` and fib's `Server.ServeHTTP` answer it.
    pub(crate) fn http(&self, peer: SocketAddr, target: &str) -> (u16, &'static str, Vec<u8>) {
        if !akit_access::allows(self.policy.as_deref(), peer.ip()) {
            self.counters.refuse(REFUSED_ACCESS);
            return (200, "text/plain", failure("refused"));
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let last = path.rsplit('/').next().unwrap_or("");
        let passkey = || {
            if self.auth.is_some() {
                passkey_of(path, query)
            } else {
                String::new()
            }
        };
        if last.starts_with("announce") {
            let body = match parse_announce(query) {
                Err(e) => failure(e),
                Ok(a) => match self.announce(peer.ip(), &a, false, &passkey()) {
                    Err(e) => failure(&e),
                    Ok(r) => self.encode_announce(&r, a.compact, peer.ip()),
                },
            };
            (200, "text/plain", body)
        } else if last.starts_with("scrape") {
            let body = match parse_scrape(query, self.scrape) {
                Err(e) => failure(e),
                Ok(hashes) => {
                    let allowed: Vec<[u8; 20]> = hashes
                        .iter()
                        .copied()
                        .filter(|h| self.allow.as_ref().is_none_or(|a| a.has(h)))
                        .collect();
                    match self.authorize(&allowed, &passkey()) {
                        Err(e) => failure(&e),
                        Ok(()) => self.scrape_http(&hashes),
                    }
                }
            };
            (200, "text/plain", body)
        } else {
            (404, "text/plain; charset=utf-8", b"not found\n".to_vec())
        }
    }

    fn encode_announce(&self, r: &AnnounceResponse, compact_peers: bool, src: IpAddr) -> Vec<u8> {
        let mut d = Value::dict()
            .with("complete", i64::from(r.seeders.unwrap_or(0)))
            .with("incomplete", i64::from(r.leechers.unwrap_or(0)))
            .with("interval", i64::from(self.interval));
        if self.min_interval > 0 {
            d.insert("min interval", i64::from(self.min_interval));
        }
        if compact_peers {
            let mut v4 = Vec::new();
            let mut v6 = Vec::new();
            for p in &r.peers {
                if p.ip().to_canonical().is_ipv4() {
                    compact(&mut v4, *p);
                } else {
                    compact(&mut v6, *p);
                }
            }
            d.insert("peers", v4);
            if !v6.is_empty() {
                d.insert("peers6", v6);
            }
        } else {
            let list: Vec<Value> = r
                .peers
                .iter()
                .map(|p| {
                    Value::dict()
                        .with("ip", p.ip().to_canonical().to_string())
                        .with("port", i64::from(p.port()))
                })
                .collect();
            d.insert("peers", list);
        }
        let ext = match src.to_canonical() {
            IpAddr::V4(v4) => v4.octets().to_vec(),
            IpAddr::V6(v6) => v6.octets().to_vec(),
        };
        d.insert("external ip", ext);
        bencode::encode(&d)
    }

    fn scrape_http(&self, hashes: &[[u8; 20]]) -> Vec<u8> {
        self.counters.scrapes.fetch_add(1, Ordering::Relaxed);
        let mut files = BTreeMap::new();
        for h in hashes {
            if !self.allowed(h) {
                continue;
            }
            let st = torrent_stats(&self.server, *h);
            if st.tracked {
                files.insert(
                    h.to_vec(),
                    Value::dict()
                        .with("complete", i64::from(st.seeders))
                        .with("downloaded", i64::from(st.completed))
                        .with("incomplete", i64::from(st.leechers)),
                );
            }
        }
        bencode::encode(&Value::dict().with("files", Value::Dict(files)))
    }

    /// The connection ID of a UDP client at `addr` in the minute `epoch`: an HMAC of both under
    /// the role's secret, as fib's `connectionID`, so that no state is kept per client.
    fn conn_id(&self, addr: SocketAddr, epoch: u64) -> u64 {
        let mut b = [0u8; 26];
        let ip = match addr.ip().to_canonical() {
            IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
            IpAddr::V6(v6) => v6.octets(),
        };
        if let Some(d) = b.get_mut(..8) {
            d.copy_from_slice(&epoch.to_be_bytes());
        }
        if let Some(d) = b.get_mut(8..24) {
            d.copy_from_slice(&ip);
        }
        if let Some(d) = b.get_mut(24..) {
            d.copy_from_slice(&addr.port().to_be_bytes());
        }
        let mac = akio_crypto::mac::Hmac::<akio_crypto::hash::Sha256>::mac(&self.secret[..], &b);
        let mut w = [0u8; 8];
        if let Some(s) = mac.get(..8) {
            w.copy_from_slice(s);
        }
        u64::from_be_bytes(w)
    }

    fn valid_conn_id(&self, addr: SocketAddr, id: u64) -> bool {
        let m = minute();
        // Both computed, whichever matches: the comparison takes the same time.
        let now = self.conn_id(addr, m) == id;
        let before = self.conn_id(addr, m.saturating_sub(1)) == id;
        now | before
    }

    /// Answers one UDP datagram from `from`, as fib's `serveUDP`: `None` for one that gets no
    /// answer (unreadable, or with a connection ID that is not the client's, which may be
    /// forged).
    pub(crate) fn udp(&self, from: SocketAddr, data: &[u8]) -> Option<Vec<u8>> {
        let header = |action: u32, tid: u32| {
            let mut b = Vec::with_capacity(MAX_UDP_REPLY);
            b.extend_from_slice(&action.to_be_bytes());
            b.extend_from_slice(&tid.to_be_bytes());
            b
        };
        let error = |tid: u32, msg: &str| {
            let mut b = header(udp::ERROR, tid);
            b.extend_from_slice(msg.as_bytes());
            b
        };
        match udp::parse_request(data).ok()? {
            udp::Request::Connect { tid } => {
                let mut b = header(udp::CONNECT, tid);
                b.extend_from_slice(&self.conn_id(from, minute()).to_be_bytes());
                Some(b)
            }
            udp::Request::Announce { conn_id, tid, req } => {
                if !self.valid_conn_id(from, conn_id) {
                    return None;
                }
                let a = Announce {
                    info_hash: req.info_hash.0,
                    peer_id: req.peer_id,
                    port: req.port,
                    left: i64::try_from(req.left).unwrap_or(i64::MAX),
                    event: req.event,
                    numwant: i64::from(req.num_want).max(-1),
                    compact: true,
                    want_v4: false,
                    want_v6: false,
                };
                // A private tracker's passkey rides in the URL data the client sends from the
                // tracker URL it was given (BEP 41).
                let passkey = if self.auth.is_some() {
                    let u = url_data(data);
                    let (p, q) = u.split_once('?').unwrap_or((&u, ""));
                    passkey_of(p, q)
                } else {
                    String::new()
                };
                match self.announce(from.ip(), &a, true, &passkey) {
                    Err(e) => Some(error(tid, &e)),
                    Ok(r) => {
                        let mut b = header(udp::ANNOUNCE, tid);
                        b.extend_from_slice(&self.interval.to_be_bytes());
                        b.extend_from_slice(&r.leechers.unwrap_or(0).to_be_bytes());
                        b.extend_from_slice(&r.seeders.unwrap_or(0).to_be_bytes());
                        for p in &r.peers {
                            let size = if p.ip().to_canonical().is_ipv4() {
                                6
                            } else {
                                18
                            };
                            if b.len().saturating_add(size) > MAX_UDP_REPLY {
                                break;
                            }
                            compact(&mut b, *p);
                        }
                        Some(b)
                    }
                }
            }
            udp::Request::Scrape {
                conn_id,
                tid,
                hashes,
            } => {
                if !self.valid_conn_id(from, conn_id) {
                    return None;
                }
                // A UDP scrape carries no URL data (BEP 41): a private tracker refuses it, unless
                // its policy makes play public.
                let allowed: Vec<[u8; 20]> = hashes
                    .iter()
                    .map(|h| h.0)
                    .filter(|h| self.allow.as_ref().is_none_or(|a| a.has(h)))
                    .collect();
                if let Err(e) = self.authorize(&allowed, "") {
                    return Some(error(tid, &e));
                }
                self.counters.scrapes.fetch_add(1, Ordering::Relaxed);
                let mut b = header(udp::SCRAPE, tid);
                for h in hashes.iter().take(MAX_UDP_SCRAPE) {
                    // A torrent not allowed is answered as one not tracked: zeros.
                    let (seeders, completed, leechers) = if self.allowed(&h.0) {
                        let st = torrent_stats(&self.server, h.0);
                        (st.seeders, st.completed, st.leechers)
                    } else {
                        (0, 0, 0)
                    };
                    b.extend_from_slice(&seeders.to_be_bytes());
                    b.extend_from_slice(&completed.to_be_bytes());
                    b.extend_from_slice(&leechers.to_be_bytes());
                }
                Some(b)
            }
            _ => None,
        }
    }
}

fn minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 60)
}

/// The UDP binding's acceptor: each client's datagrams go to the tracker of now, so that a reload
/// takes over without a new binding.
#[derive(Debug, Clone)]
pub(crate) struct UdpAcceptor {
    pub(crate) key: usize,
}

/// One UDP client.
#[derive(Debug)]
pub(crate) struct UdpClient {
    key: usize,
}

impl Acceptor for UdpAcceptor {
    type Handler = UdpClient;

    fn on_open(&self, _c: &Conn<UdpClient>) -> UdpClient {
        UdpClient { key: self.key }
    }
}

impl Handler for UdpClient {
    fn on_data(&mut self, c: &Conn<Self>, data: &[u8]) {
        let from = c.peer_addr();
        // While the role does not run, datagrams are dropped.
        if let Some(out) = with_snap(self.key, |s| s.and_then(|s| s.udp(from, data))) {
            let _ = c.send(&out);
        }
    }
}
