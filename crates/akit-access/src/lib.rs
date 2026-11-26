#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's access lists (I85): which client addresses akit serves, by allow lists (whitelists)
//! and deny lists (blacklists) of IP addresses and CIDR prefixes, set for the whole config and for
//! each listener and route, where the most specific one that has a say decides. fit's package
//! `access`.
//!
//! A [`List`] is one place's allow and deny entries, compiled into sorted, merged ranges of each
//! address family that an address is looked up in by binary search, so that a list of thousands
//! of prefixes costs a lookup a few comparisons. A [`Policy`] is the lists that apply somewhere,
//! the most specific first: a route's, then its listener's, then the global one. Each list in
//! turn may decide:
//!
//! - an address its deny holds is refused;
//! - else an address its allow holds is served;
//! - else, when its allow has entries, the address is refused, since an allow list serves only
//!   what it holds;
//! - else the list has no say, and the next one decides.
//!
//! An address no list has a say on is served. A more specific list therefore overrides a less
//! specific one both ways: a route's allow serves an address the global deny refuses, and a
//! route's deny refuses one the global allow serves.
//!
//! List files (`allow_files`, `deny_files`) hold addresses and prefixes, ranges, eMule
//! `ipfilter.dat` lines and PeerGuardian `.p2p` lines, told apart line by line ([`read_file`]).
//! They are read when the config is compiled, on the control thread: never on an I/O thread
//! (rule 1). A lookup ([`Policy::allows`]) neither allocates nor locks (rules 12, 13).
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` (I85–I87).

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;

use akit_config::Access;

mod lists;
#[cfg(test)]
mod tests;

pub use lists::{DAT_BLOCK_LEVEL, MAX_FILE_BYTES, MAX_LINE_BYTES, Range, read_file};

/// The SHA-256 of a list file, as the graph records the certificate files' for `-watch-certs`.
pub type Sum = [u8; 32];

/// One place's allow and deny entries, compiled.
#[derive(Debug, Default)]
pub struct List {
    allow: Set,
    deny: Set,
}

/// What a list says of an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    NoSay,
    Allowed,
    Denied,
}

impl List {
    fn verdict(&self, a: IpAddr) -> Verdict {
        if self.deny.contains(a) {
            Verdict::Denied
        } else if self.allow.contains(a) {
            Verdict::Allowed
        } else if !self.allow.is_empty() {
            Verdict::Denied
        } else {
            Verdict::NoSay
        }
    }

    /// Whether it has no entries, and so never has a say.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// The list of `allow` and `deny`, ranges as [`Range::of_prefix`] and [`read_file`] make
    /// them.
    #[must_use]
    pub fn of_ranges(allow: Vec<Range>, deny: Vec<Range>) -> List {
        List {
            allow: Set::new(allow),
            deny: Set::new(deny),
        }
    }
}

/// Why an access does not compile: the field it is about, relative to the access (`allow[0]`,
/// `deny_files[1]`), and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The field, as `allow_files[0]`.
    pub field: String,
    /// What is wrong with it.
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.msg)
    }
}

impl std::error::Error for Error {}

/// Compiles `a`, reading its files; `None` for no `a` or an empty one. `files`, when given, is
/// given the SHA-256 of each file read, by path, for the config to be compiled again when one
/// changes. Runs on the control plane: it reads files.
///
/// # Errors
///
/// An entry that is not an address or prefix (akit-config checks them first), a file that
/// cannot be read, is larger than [`MAX_FILE_BYTES`] or holds a line that is none of the formats
/// [`read_file`] reads; each error says which field.
pub fn compile(
    a: Option<&Access>,
    mut files: Option<&mut BTreeMap<String, Sum>>,
) -> Result<Option<Arc<List>>, Vec<Error>> {
    let Some(a) = a else { return Ok(None) };
    let mut errors = Vec::new();
    let mut allow = Vec::new();
    let mut deny = Vec::new();
    for (name, entries, into) in [
        ("allow", &a.allow, &mut allow),
        ("deny", &a.deny, &mut deny),
    ] {
        for (i, s) in entries.iter().enumerate() {
            match akit_router::parse_prefix(s.trim()) {
                Ok(p) => into.push(Range::of_prefix(p.addr(), p.bits())),
                Err(e) => errors.push(Error {
                    field: format!("{name}[{i}]"),
                    msg: e.to_string(),
                }),
            }
        }
    }
    for (name, paths, into) in [
        ("allow_files", &a.allow_files, &mut allow),
        ("deny_files", &a.deny_files, &mut deny),
    ] {
        for (i, path) in paths.iter().enumerate() {
            match read_file(path) {
                Ok((rs, sum)) => {
                    if let Some(files) = files.as_deref_mut() {
                        files.insert(path.clone(), sum);
                    }
                    into.extend(rs);
                }
                Err(msg) => errors.push(Error {
                    field: format!("{name}[{i}]"),
                    msg,
                }),
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let l = List::of_ranges(allow, deny);
    Ok((!l.is_empty()).then(|| Arc::new(l)))
}

/// The lists that apply somewhere, the most specific first. A place with no policy serves every
/// address ([`allows`]).
#[derive(Debug)]
pub struct Policy {
    lists: Vec<Arc<List>>,
}

impl Policy {
    /// The policy of `lists`, the most specific first; missing lists, and empty ones, are left
    /// out. `None` when none is left.
    #[must_use]
    pub fn chain<'a>(
        lists: impl IntoIterator<Item = Option<&'a Arc<List>>>,
    ) -> Option<Arc<Policy>> {
        let lists: Vec<Arc<List>> = lists
            .into_iter()
            .flatten()
            .filter(|l| !l.is_empty())
            .cloned()
            .collect();
        (!lists.is_empty()).then(|| Arc::new(Policy { lists }))
    }

    /// Whether it serves `a`. An IPv4 address mapped into IPv6 counts as IPv4. Does not
    /// allocate.
    #[must_use]
    pub fn allows(&self, a: IpAddr) -> bool {
        let a = a.to_canonical();
        for l in &self.lists {
            match l.verdict(a) {
                Verdict::Allowed => return true,
                Verdict::Denied => return false,
                Verdict::NoSay => {}
            }
        }
        true
    }
}

/// Whether `p` serves `a`: no policy serves every address.
#[must_use]
pub fn allows(p: Option<&Policy>, a: IpAddr) -> bool {
    p.is_none_or(|p| p.allows(a))
}

/// A set of addresses: sorted, merged ranges of each family.
#[derive(Debug, Default)]
struct Set {
    v4: Vec<(u32, u32)>,
    v6: Vec<(u128, u128)>,
}

impl Set {
    /// The set of `rs`, ranges whose ends are of one family and in order, as
    /// [`Range::of_prefix`] and [`read_file`] make them.
    fn new(rs: Vec<Range>) -> Set {
        let mut s = Set::default();
        for r in rs {
            match (r.lo, r.hi) {
                (IpAddr::V4(lo), IpAddr::V4(hi)) => s.v4.push((u32::from(lo), u32::from(hi))),
                (IpAddr::V6(lo), IpAddr::V6(hi)) => s.v6.push((u128::from(lo), u128::from(hi))),
                // Not made by this crate: left out.
                _ => {}
            }
        }
        s.v4 = merge(s.v4);
        s.v6 = merge(s.v6);
        s
    }

    fn is_empty(&self) -> bool {
        self.v4.is_empty() && self.v6.is_empty()
    }

    fn contains(&self, a: IpAddr) -> bool {
        match a {
            IpAddr::V4(v) => holds(&self.v4, u32::from(v)),
            IpAddr::V6(v) => holds(&self.v6, u128::from(v)),
        }
    }
}

/// An address of one family as a number, for [`merge`] and [`holds`].
trait Bound: Copy + Ord {
    fn next(self) -> Option<Self>;
}

impl Bound for u32 {
    fn next(self) -> Option<u32> {
        self.checked_add(1)
    }
}

impl Bound for u128 {
    fn next(self) -> Option<u128> {
        self.checked_add(1)
    }
}

/// `rs` sorted, the ranges that overlap or touch merged.
fn merge<T: Bound>(mut rs: Vec<(T, T)>) -> Vec<(T, T)> {
    rs.sort_unstable_by_key(|r| r.0);
    let mut out: Vec<(T, T)> = Vec::with_capacity(rs.len());
    for r in rs {
        if let Some(last) = out.last_mut() {
            // The last range reaches the family's end, or this one starts within it or right
            // after it.
            if last.1.next().is_none_or(|n| r.0 <= n) {
                last.1 = last.1.max(r.1);
                continue;
            }
        }
        out.push(r);
    }
    out.shrink_to_fit();
    out
}

/// Whether the sorted, merged ranges `rs` hold `v`: the range before the first that starts past
/// `v` may.
fn holds<T: Bound>(rs: &[(T, T)], v: T) -> bool {
    let i = rs.partition_point(|r| r.0 <= v);
    i.checked_sub(1)
        .and_then(|j| rs.get(j))
        .is_some_and(|r| v <= r.1)
}
