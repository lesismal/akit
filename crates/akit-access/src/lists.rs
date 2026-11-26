//! List files: their lines, in the formats published for firewalls and peer-to-peer clients, as
//! fit's `access/lists.go` reads them.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use akio_crypto::hash::Sha256;

use crate::Sum;

/// The largest list file read (fit's `MaxFileBytes`), so that a mistaken path cannot have akit
/// read a huge file into memory: 64 MiB holds millions of entries.
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// The longest line of a list file (fit's `maxLineBytes`).
pub const MAX_LINE_BYTES: usize = 16 << 10;

/// The highest level of an eMule `ipfilter.dat` line that is listed: a line of a higher level is
/// left out, as qBittorrent leaves it out, and as eMule does at its default filter level.
pub const DAT_BLOCK_LEVEL: i64 = 127;

/// The addresses from `lo` to `hi`, both included: both of one family, IPv4 addresses never
/// mapped into IPv6, and `lo` not past `hi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// The first address.
    pub lo: IpAddr,
    /// The last address.
    pub hi: IpAddr,
}

impl Range {
    /// The addresses of the prefix `addr`/`bits` (fit's `RangeOf`): an IPv4 prefix written in
    /// IPv6 (`::ffff:10.0.0.0/104`) is IPv4's; bits past the address's length are its length.
    #[must_use]
    pub fn of_prefix(addr: IpAddr, bits: u8) -> Range {
        match addr {
            IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some() => {
                let v4 = v6.to_ipv4_mapped().unwrap_or(Ipv4Addr::UNSPECIFIED);
                Range::of_prefix(IpAddr::V4(v4), bits.saturating_sub(96))
            }
            IpAddr::V4(v4) => {
                let host = u32::MAX.checked_shr(u32::from(bits.min(32))).unwrap_or(0);
                let lo = u32::from(v4) & !host;
                Range {
                    lo: IpAddr::V4(Ipv4Addr::from(lo)),
                    hi: IpAddr::V4(Ipv4Addr::from(lo | host)),
                }
            }
            IpAddr::V6(v6) => {
                let host = u128::MAX.checked_shr(u32::from(bits.min(128))).unwrap_or(0);
                let lo = u128::from(v6) & !host;
                Range {
                    lo: IpAddr::V6(Ipv6Addr::from(lo)),
                    hi: IpAddr::V6(Ipv6Addr::from(lo | host)),
                }
            }
        }
    }
}

impl std::fmt::Display for Range {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.lo, self.hi)
    }
}

/// Reads the list file `path`, at most [`MAX_FILE_BYTES`], and returns the ranges it lists and
/// the file's SHA-256 (fit's `ReadFile`). Each line is one of ([`parse_line`] tells them apart
/// by the line's content, so that one file may mix them):
///
/// - an IP address or CIDR prefix, with what follows a `#` or `;` left out, as the lists
///   published for firewalls are written;
/// - a range, `first-last` or `first - last`;
/// - an eMule `ipfilter.dat` line, `first - last , level , name`: a line whose level is above
///   [`DAT_BLOCK_LEVEL`] is not listed, as qBittorrent and eMule (at its default filter level)
///   leave it out;
/// - a PeerGuardian `.p2p` line, `name:first-last`.
///
/// Blank lines and lines that begin with `#`, `;` or `//` are left out. Blocks: control plane
/// only.
///
/// # Errors
///
/// The file cannot be read or is too large, or a line is none of these: the message names the
/// file and the line.
pub fn read_file(path: &str) -> Result<(Vec<Range>, Sum), String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let too_large = || format!("{path}: larger than {MAX_FILE_BYTES} bytes");
    if f.metadata().is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        return Err(too_large());
    }
    let mut buf = Vec::new();
    f.take(MAX_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("read {path}: {e}"))?;
    if u64::try_from(buf.len()).map_or(true, |n| n > MAX_FILE_BYTES) {
        return Err(too_large());
    }
    let sum = Sha256::digest(&buf);
    let rs = parse_list(&buf, path)?;
    Ok((rs, sum))
}

/// The ranges the lines of `data`, a list file named `name`, list.
pub(crate) fn parse_list(data: &[u8], name: &str) -> Result<Vec<Range>, String> {
    let mut rs = Vec::new();
    let mut rest = data;
    let mut n = 0usize;
    while !rest.is_empty() {
        n = n.saturating_add(1);
        let (line, next) = match rest.iter().position(|&b| b == b'\n') {
            Some(i) => (
                rest.get(..i).unwrap_or_default(),
                rest.get(i.saturating_add(1)..),
            ),
            None => (rest, None),
        };
        rest = next.unwrap_or_default();
        if line.len() > MAX_LINE_BYTES {
            return Err(format!(
                "{name}:{n}: a line longer than {MAX_LINE_BYTES} bytes"
            ));
        }
        let line = String::from_utf8_lossy(line);
        match parse_line(&line) {
            Ok(Some(r)) => rs.push(r),
            Ok(None) => {}
            Err(e) => return Err(format!("{name}:{n}: {e}")),
        }
    }
    Ok(rs)
}

/// Parses a line of a list file (fit's `parseLine`): `None` when it lists nothing (a blank line,
/// a comment, or an `ipfilter.dat` line of a level above [`DAT_BLOCK_LEVEL`]). It tells the
/// formats apart by the line's content:
///
/// ```text
/// 203.0.113.0/24 # a comment      an address or prefix
/// 203.0.113.1-203.0.113.9         a range ("first - last" too)
/// 001.002.003.004 - 001.002.003.255 , 000 , Some name   ipfilter.dat
/// Some name:1.2.3.4-1.2.3.255     PeerGuardian .p2p
/// ```
///
/// The addresses of a range may have leading zeros in IPv4, as `ipfilter.dat` writes them.
pub(crate) fn parse_line(line: &str) -> Result<Option<Range>, String> {
    let s = line.trim();
    if s.is_empty() || s.starts_with('#') || s.starts_with(';') || s.starts_with("//") {
        return Ok(None);
    }
    // An address or a prefix, with a comment after it.
    let plain = match s.find(['#', ';']) {
        Some(i) => s.get(..i).unwrap_or_default().trim(),
        None => s,
    };
    if let Ok(p) = akit_router::parse_prefix(plain) {
        return Ok(Some(Range::of_prefix(p.addr(), p.bits())));
    }
    // A bare range.
    if !plain.contains(',') && plain.contains('-') {
        if let Ok(r) = parse_range(plain) {
            return Ok(Some(r));
        }
    }
    let mut dat_err = None;
    let mut fields = s.splitn(3, ',');
    if let (Some(first), Some(level)) = (fields.next(), fields.next()) {
        if first.contains('-') {
            match parse_range(first) {
                Ok(r) => {
                    let level = level.trim();
                    return match level.parse::<i64>() {
                        Ok(l) if (0..=255).contains(&l) => Ok((l <= DAT_BLOCK_LEVEL).then_some(r)),
                        _ => Err(format!(
                            "{}: an ipfilter.dat line whose level {} is not 0 to 255",
                            quote(s),
                            quote(level)
                        )),
                    };
                }
                Err(e) => dat_err = Some(e),
            }
        }
    }
    if let Some(i) = s.rfind(':').filter(|&i| i > 0) {
        if let Ok(r) = parse_range(s.get(i.saturating_add(1)..).unwrap_or_default()) {
            return Ok(Some(r));
        }
    }
    if let Some(e) = dat_err {
        return Err(format!("{}: {e}", quote(s)));
    }
    Err(format!(
        "{} is not an IP address, a CIDR prefix, a range, an ipfilter.dat line or a .p2p line",
        quote(s)
    ))
}

/// Parses `first-last`, spaces allowed around the dash: an IPv6 address has no `-`, so the
/// first one splits the range.
fn parse_range(s: &str) -> Result<Range, String> {
    let Some((lo, hi)) = s.split_once('-') else {
        return Err("no '-' between two addresses".to_owned());
    };
    let lo = parse_addr(lo.trim())?;
    let hi = parse_addr(hi.trim())?;
    if lo.is_ipv4() != hi.is_ipv4() {
        return Err(format!("the range {lo}-{hi} mixes IPv4 and IPv6"));
    }
    if hi < lo {
        return Err(format!("the range {lo}-{hi} ends before it begins"));
    }
    Ok(Range { lo, hi })
}

/// Parses an address: IPv4 with leading zeros allowed in each part, as `ipfilter.dat` writes
/// them, or IPv6 without a zone (an IPv4 address mapped into IPv6 taken as IPv4).
fn parse_addr(s: &str) -> Result<IpAddr, String> {
    let bad = || format!("{} is not an IP address", quote(s));
    if s.contains(':') {
        let a: Ipv6Addr = s.parse().map_err(|_| bad())?;
        return Ok(IpAddr::V6(a).to_canonical());
    }
    let mut b = [0u8; 4];
    let mut parts = s.split('.');
    for slot in &mut b {
        let p = parts.next().ok_or_else(bad)?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        *slot = p.parse::<u8>().map_err(|_| bad())?;
    }
    if parts.next().is_some() {
        return Err(bad());
    }
    Ok(IpAddr::V4(Ipv4Addr::from(b)))
}

/// `s` quoted as Go's `%q` writes it, shortened to 120 bytes (fit's `clip`).
fn quote(s: &str) -> String {
    const MAX: usize = 120;
    if s.len() <= MAX {
        return akit_router::go_quote(s.as_bytes());
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let mut q = akit_router::go_quote(s.get(..end).unwrap_or_default().as_bytes());
    // Go's clip shortens before quoting: the "..." is inside the quotes.
    q.pop();
    q.push_str("...\"");
    q
}
