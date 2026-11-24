//! Route matchers besides hosts and paths (I39): method, header, query, remote IP and CIDR,
//! regular expressions on the path, and fit's expressions combining them. As fit's
//! `router/matchers.go`: built when the config is compiled, then only read; matching does not
//! allocate.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use akio_regex::{Regex, RegexBuilder};

use crate::expr::{Expr, go_quote};
use crate::host_only;

/// What the matchers read of a request. [`Parts`] is one made of its parts; akit's listener
/// implements it for akio's request context.
pub trait Request {
    /// The method, as sent (methods are case-sensitive).
    fn method(&self) -> &str;
    /// The `Host` field's value (or HTTP/2's and HTTP/3's `:authority`), as sent.
    fn host(&self) -> &str;
    /// The canonical, percent-decoded path ([`crate::Canonical::decoded`]).
    fn path(&self) -> &[u8];
    /// The query as sent, without the `?`; empty when there is none.
    fn query(&self) -> &str;
    /// The connection's peer address, if it has an IP address.
    fn remote_ip(&self) -> Option<IpAddr>;
    /// Whether `f` holds for the value of one of the request's header fields named `name`
    /// (compared ignoring case); `f` is called on each such value, in order, until it does.
    fn any_header(&self, name: &str, f: impl FnMut(&str) -> bool) -> bool;
}

/// A [`Request`] made of its parts.
#[derive(Debug, Clone, Copy)]
pub struct Parts<'a> {
    /// The method.
    pub method: &'a str,
    /// The `Host` field's value.
    pub host: &'a str,
    /// The canonical, percent-decoded path.
    pub path: &'a [u8],
    /// The query, without the `?`.
    pub query: &'a str,
    /// The peer's address.
    pub remote_ip: Option<IpAddr>,
    /// The header fields, names and values, in order.
    pub headers: &'a [(&'a str, &'a str)],
}

impl<'a> Parts<'a> {
    /// A `GET` request for `host` and `path`, with no query, peer address or header fields.
    #[must_use]
    pub fn new(host: &'a str, path: &'a [u8]) -> Parts<'a> {
        Parts {
            method: "GET",
            host,
            path,
            query: "",
            remote_ip: None,
            headers: &[],
        }
    }
}

impl Request for Parts<'_> {
    fn method(&self) -> &str {
        self.method
    }
    fn host(&self) -> &str {
        self.host
    }
    fn path(&self) -> &[u8] {
        self.path
    }
    fn query(&self) -> &str {
        self.query
    }
    fn remote_ip(&self) -> Option<IpAddr> {
        self.remote_ip
    }
    fn any_header(&self, name: &str, mut f: impl FnMut(&str) -> bool) -> bool {
        self.headers
            .iter()
            .any(|(n, v)| n.eq_ignore_ascii_case(name) && f(v))
    }
}

/// Why a matcher could not be built; its message is fit's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub(crate) String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// A matcher: what a request must match, besides a route's hosts and paths, for the route to
/// take it.
#[derive(Debug, Clone)]
pub struct Matcher(pub(crate) Kind);

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// One of the methods, compared as written.
    Methods(Box<[Box<[u8]>]>),
    /// Every field: header fields.
    Headers(Box<[Field]>),
    /// Every field: query parameters.
    Query(Box<[Field]>),
    /// The peer's address is in one of the prefixes.
    RemoteIp(Box<[Prefix]>),
    /// One of the expressions matches the path, anywhere in it unless anchored.
    PathRegexp(Box<[Regex]>),
    /// Expressions only: one of the hosts, exact or `*.` (by suffix from the dot on), lower case.
    Host {
        exact: Box<[Box<[u8]>]>,
        wild: Box<[Box<[u8]>]>,
    },
    /// Expressions only: one of the paths, exactly.
    Path(Box<[Box<[u8]>]>),
    /// Expressions only: a path that starts with one of the prefixes.
    PathPrefix(Box<[Box<[u8]>]>),
    /// An expression: counts as one matcher.
    Expr(Box<Expr>),
}

/// A header field or query parameter the request must have, with one of `values` when there
/// are any.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    /// `None` when it can never be present: a header name that is not UTF-8, or `Host` (fit's
    /// server takes `Host` out of the header fields).
    name: Option<Box<str>>,
    /// The query parameter's name, as bytes.
    key: Box<[u8]>,
    values: Box<[Box<[u8]>]>,
}

impl Matcher {
    /// A request whose method is one of `methods`, compared as written: methods are
    /// case-sensitive (fit's `Methods`).
    #[must_use]
    pub fn methods<S: AsRef<str>>(methods: &[S]) -> Matcher {
        Matcher(Kind::Methods(
            methods
                .iter()
                .map(|m| m.as_ref().as_bytes().into())
                .collect(),
        ))
    }

    /// A request that has every header named in `fields`, with, for a name given values, one of
    /// them as the value of one of its fields of that name. Names ignore case; values do not
    /// (fit's `Headers`).
    #[must_use]
    pub fn headers<N: AsRef<str>, V: AsRef<str>>(fields: &[(N, Vec<V>)]) -> Matcher {
        Matcher(Kind::Headers(
            fields
                .iter()
                .map(|(n, vs)| Field::new(n.as_ref().as_bytes(), vs.iter().map(|v| v.as_ref())))
                .collect(),
        ))
    }

    /// A request whose query has every key in `fields`, with, for a key given values, one of
    /// them as one of its values; keys and values are compared once unescaped (fit's `Query`).
    #[must_use]
    pub fn query<N: AsRef<str>, V: AsRef<str>>(fields: &[(N, Vec<V>)]) -> Matcher {
        Matcher(Kind::Query(
            fields
                .iter()
                .map(|(n, vs)| Field::new(n.as_ref().as_bytes(), vs.iter().map(|v| v.as_ref())))
                .collect(),
        ))
    }

    /// A request from an address in one of `nets`, each an IP address or a CIDR prefix, as in
    /// `10.0.0.0/8`; an IPv4 address mapped into IPv6 counts as IPv4 (fit's `RemoteIP`).
    ///
    /// # Errors
    ///
    /// A net that is neither, as [`parse_prefix`] says.
    pub fn remote_ip<S: AsRef<str>>(nets: &[S]) -> Result<Matcher, Error> {
        let nets = nets
            .iter()
            .map(|s| parse_prefix(s.as_ref()))
            .collect::<Result<_, _>>()?;
        Ok(Matcher(Kind::RemoteIp(nets)))
    }

    /// A request whose path one of `exprs` matches, anywhere in it unless anchored with `^` and
    /// `$` (fit's `PathRegexp`). The expressions are `akio-regex`'s, whose matching time is
    /// linear in the path, with Go's ASCII `\d`, `\w`, `\s` and `\b`.
    ///
    /// # Errors
    ///
    /// An expression that does not compile.
    pub fn path_regexp<S: AsRef<str>>(exprs: &[S]) -> Result<Matcher, Error> {
        let res = exprs
            .iter()
            .map(|s| regexp(s.as_ref().as_bytes()))
            .collect::<Result<_, _>>()?;
        Ok(Matcher(Kind::PathRegexp(res)))
    }

    /// A request that expression `s` matches (fit's `ParseExpr`; see [`Expr`]).
    ///
    /// # Errors
    ///
    /// An expression that does not parse, or one of whose matchers cannot be built.
    pub fn expr(s: &str) -> Result<Matcher, Error> {
        Ok(Matcher(Kind::Expr(Box::new(Expr::parse(s)?))))
    }

    /// Whether `r` matches. Does not allocate.
    #[must_use]
    pub fn matches<R: Request + ?Sized>(&self, r: &R) -> bool {
        match &self.0 {
            Kind::Methods(ms) => {
                let m = r.method().as_bytes();
                ms.iter().any(|x| **x == *m)
            }
            Kind::Headers(fs) => fs.iter().all(|f| {
                let Some(name) = &f.name else { return false };
                r.any_header(name, |v| {
                    f.values.is_empty() || f.values.iter().any(|w| **w == *v.as_bytes())
                })
            }),
            Kind::Query(fs) => {
                let q = r.query().as_bytes();
                fs.iter().all(|f| query_has(q, &f.key, &f.values))
            }
            Kind::RemoteIp(ps) => r
                .remote_ip()
                .is_some_and(|a| ps.iter().any(|p| p.contains(a.to_canonical()))),
            Kind::PathRegexp(res) => {
                let p = r.path();
                match std::str::from_utf8(p) {
                    Ok(s) => res.iter().any(|re| re.is_match(s)),
                    Err(_) => res
                        .iter()
                        .any(|re| re.is_match_fmt(format_args!("{}", Lossy(p)))),
                }
            }
            Kind::Host { exact, wild } => {
                let h = host_only(r.host().as_bytes());
                let h = h.strip_suffix(b".").unwrap_or(h);
                if exact.iter().any(|x| x.eq_ignore_ascii_case(h)) {
                    return true;
                }
                match h.iter().position(|&c| c == b'.') {
                    Some(i) if i > 0 => {
                        let s = h.get(i..).unwrap_or_default();
                        wild.iter().any(|w| w.eq_ignore_ascii_case(s))
                    }
                    _ => false,
                }
            }
            Kind::Path(ps) => {
                let p = r.path();
                ps.iter().any(|x| **x == *p)
            }
            Kind::PathPrefix(ps) => {
                let p = r.path();
                ps.iter().any(|x| p.starts_with(x))
            }
            Kind::Expr(e) => e.matches(r),
        }
    }
}

impl Field {
    fn new<'a>(name: &[u8], values: impl Iterator<Item = &'a str>) -> Field {
        let header = std::str::from_utf8(name)
            .ok()
            .filter(|n| !n.eq_ignore_ascii_case("host"))
            .map(Box::from);
        Field {
            name: header,
            key: name.into(),
            values: values.map(|v| v.as_bytes().into()).collect(),
        }
    }

    /// [`Field::new`] from an expression's arguments, which may not be UTF-8.
    pub(crate) fn from_bytes(name: &[u8], values: &[Box<[u8]>]) -> Field {
        let mut f = Field::new(name, std::iter::empty());
        f.values = values.into();
        f
    }
}

/// Builds `Kind::Headers` or `Kind::Query` of one field from an expression's arguments.
pub(crate) fn one_field(name: &[u8], values: &[Box<[u8]>]) -> Box<[Field]> {
    Box::new([Field::from_bytes(name, values)])
}

/// Compiles a path regexp as fit's `PathRegexp` does, with Go's ASCII classes.
pub(crate) fn regexp(s: &[u8]) -> Result<Regex, Error> {
    let bad = |why: &dyn fmt::Display| {
        Error(format!(
            "{} is not a valid regular expression: {why}",
            go_quote(s)
        ))
    };
    let text = std::str::from_utf8(s).map_err(|_| bad(&"invalid UTF-8"))?;
    RegexBuilder::new(text)
        .ascii_classes(true)
        .build()
        .map_err(|e| bad(&e))
}

/// A path that is not UTF-8, written as Go's regexp reads it: each byte of an invalid
/// sequence as U+FFFD.
struct Lossy<'a>(&'a [u8]);

impl fmt::Display for Lossy<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
            for _ in chunk.invalid() {
                f.write_str("\u{FFFD}")?;
            }
        }
        Ok(())
    }
}

/// Whether `raw`, a URL's escaped query, has `key`, with one of `values` when there are any
/// (fit's `queryHas`).
fn query_has(raw: &[u8], key: &[u8], values: &[Box<[u8]>]) -> bool {
    // As Go's `strings.Cut` loop: no pair after a trailing `&`, none in an empty query.
    if raw.is_empty() {
        return false;
    }
    let raw = raw.strip_suffix(b"&").unwrap_or(raw);
    raw.split(|&c| c == b'&').any(|pair| {
        let (k, v) = match pair.iter().position(|&c| c == b'=') {
            Some(i) => (
                pair.get(..i).unwrap_or_default(),
                pair.get(i.saturating_add(1)..).unwrap_or_default(),
            ),
            None => (pair, &[][..]),
        };
        unescaped_is(k, key) && (values.is_empty() || values.iter().any(|w| unescaped_is(v, w)))
    })
}

/// Whether `s`, query-escaped, unescapes to `want`; an invalid escape matches nothing (fit's
/// `unescapedIs`).
fn unescaped_is(s: &[u8], want: &[u8]) -> bool {
    let mut want = want.iter();
    let mut i = s.iter();
    while let Some(&c) = i.next() {
        let c = match c {
            b'+' => b' ',
            b'%' => {
                let (Some(&h), Some(&l)) = (i.next(), i.next()) else {
                    return false;
                };
                match (unhex(h), unhex(l)) {
                    (Some(h), Some(l)) => (h << 4) | l,
                    _ => return false,
                }
            }
            c => c,
        };
        if want.next() != Some(&c) {
            return false;
        }
    }
    want.next().is_none()
}

fn unhex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(c.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(c.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}

/// An IP prefix: an address and the number of its leading bits that count, as Go's
/// `netip.Prefix`. Its address has its other bits zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    addr: IpAddr,
    bits: u8,
}

impl Prefix {
    /// The address, its bits past [`bits`](Self::bits) zero.
    #[must_use]
    pub fn addr(&self) -> IpAddr {
        self.addr
    }

    /// How many leading bits count.
    #[must_use]
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// Whether `a` is in the prefix: of the same family, with the same leading bits.
    #[must_use]
    pub fn contains(&self, a: IpAddr) -> bool {
        match (self.addr, a) {
            (IpAddr::V4(p), IpAddr::V4(a)) => {
                let m = mask32(self.bits);
                u32::from(a) & m == u32::from(p)
            }
            (IpAddr::V6(p), IpAddr::V6(a)) => {
                let m = mask128(self.bits);
                u128::from(a) & m == u128::from(p)
            }
            _ => false,
        }
    }
}

impl fmt::Display for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.bits)
    }
}

fn mask32(bits: u8) -> u32 {
    u32::MAX
        .checked_shl(32u32.saturating_sub(u32::from(bits)))
        .unwrap_or(0)
}

fn mask128(bits: u8) -> u128 {
    u128::MAX
        .checked_shl(128u32.saturating_sub(u32::from(bits)))
        .unwrap_or(0)
}

/// Parses an IP address, as a prefix of its full length, or a CIDR prefix, as fit's
/// `ParsePrefix`: an IPv4 address mapped into IPv6 is IPv4, an IPv4 prefix written in IPv6 is
/// refused, and a prefix's address is masked to its bits.
///
/// # Errors
///
/// `s` is neither (or has an IPv6 zone).
pub fn parse_prefix(s: &str) -> Result<Prefix, Error> {
    let bad = || {
        Error(format!(
            "{} is not an IP address or a CIDR prefix",
            go_quote(s.as_bytes())
        ))
    };
    let Some((addr, bits)) = s.rsplit_once('/') else {
        let a: IpAddr = s.parse().map_err(|_| bad())?;
        let a = a.to_canonical();
        let bits = if a.is_ipv4() { 32 } else { 128 };
        return Ok(Prefix { addr: a, bits });
    };
    let a: IpAddr = addr.parse().map_err(|_| bad())?;
    // Go's: decimal digits without a leading zero, at most the address's length.
    let ok_digits = !bits.is_empty()
        && bits.bytes().all(|c| c.is_ascii_digit())
        && (bits.len() == 1 || !bits.starts_with('0'));
    let bits: u8 = if ok_digits {
        bits.parse().map_err(|_| bad())?
    } else {
        return Err(bad());
    };
    match a {
        IpAddr::V4(v4) if bits <= 32 => Ok(Prefix {
            addr: IpAddr::V4(Ipv4Addr::from(u32::from(v4) & mask32(bits))),
            bits,
        }),
        IpAddr::V6(v6) if bits <= 128 => {
            if v6.to_ipv4_mapped().is_some() {
                return Err(Error(format!(
                    "{} is an IPv4 prefix written in IPv6: write it in IPv4",
                    go_quote(s.as_bytes())
                )));
            }
            Ok(Prefix {
                addr: IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask128(bits))),
                bits,
            })
        }
        _ => Err(bad()),
    }
}
