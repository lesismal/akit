//! fit's cluster management in the config (I130, fit's `config/cluster.go`): `admin.cluster`, which
//! makes this akit the manager of others, its members; `admin.cluster_member`, which links this akit
//! out to a manager; and `admin.node_id`, the ID a manager knows a member by. Read with fit's
//! fields, defaults, bounds and checks (`checkCluster`, `checkClusterMember`, the node ID's
//! pattern); what is left out is kept zero, and the methods give fit's defaults for it.

use std::time::Duration;

use crate::decode::Decoder;
use crate::error::{join_field, join_index};
use crate::json::Node;
use crate::model::Console;

/// `poll_interval`'s and `timeout`'s default, fit's `DefaultClusterPollInterval` and
/// `DefaultClusterTimeout`.
pub const DEFAULT_CLUSTER_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// See [`DEFAULT_CLUSTER_POLL_INTERVAL`].
pub const DEFAULT_CLUSTER_TIMEOUT: Duration = Duration::from_secs(10);
/// `max_nodes`' default, and its most.
pub const DEFAULT_CLUSTER_MAX_NODES: usize = 256;
/// See [`DEFAULT_CLUSTER_MAX_NODES`].
pub const MAX_CLUSTER_MAX_NODES: i64 = 10_000;
/// `max_concurrency`'s default, and its most.
pub const DEFAULT_CLUSTER_MAX_CONCURRENCY: usize = 8;
/// See [`DEFAULT_CLUSTER_MAX_CONCURRENCY`].
pub const MAX_CLUSTER_MAX_CONCURRENCY: i64 = 256;
/// `max_response_bytes`' default, its least and its most.
pub const DEFAULT_CLUSTER_MAX_RESPONSE_BYTES: u64 = 4 << 20;
/// See [`DEFAULT_CLUSTER_MAX_RESPONSE_BYTES`].
pub const MIN_CLUSTER_RESPONSE_BYTES: u64 = 16 << 10;
/// See [`DEFAULT_CLUSTER_MAX_RESPONSE_BYTES`].
pub const MAX_CLUSTER_RESPONSE_BYTES: i64 = 64 << 20;
/// The least `poll_interval` and `timeout`: a manager does not poll as fast as it can.
pub const MIN_CLUSTER_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// See [`MIN_CLUSTER_POLL_INTERVAL`].
pub const MIN_CLUSTER_TIMEOUT: Duration = Duration::from_secs(1);
/// The most `admins`.
pub const MAX_CLUSTER_ADMINS: usize = 64;
/// How long an enrollment token lasts when none is asked for, and the longest one may (fit's
/// `DefaultClusterEnrollTTL`, `MaxClusterEnrollTTL`).
pub const DEFAULT_CLUSTER_ENROLL_TTL: Duration = Duration::from_secs(24 * 3600);
/// See [`DEFAULT_CLUSTER_ENROLL_TTL`].
pub const MAX_CLUSTER_ENROLL_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// The path a manager's link listener takes members' WebSockets on: fit's, a protocol's path.
pub const CLUSTER_LINK_PATH: &str = "/cluster/link";
/// The prefix of an enrollment token a manager makes: fit's `fite1.`, renamed (decision 037; a
/// token moving between akit and fit has its prefix rewritten, decision 041).
pub const CLUSTER_ENROLL_PREFIX: &str = "akite1.";

/// `admin.cluster`, fit's `config.Cluster`: this akit manages others, its members, kept in the
/// store and served in the console's Cluster pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cluster {
    /// `poll_interval`: how often each member is read; zero is the default.
    pub poll_interval: Duration,
    /// `timeout`: the longest one call to a member takes; zero is the default.
    pub timeout: Duration,
    /// `max_nodes`: the most members kept; zero is the default.
    pub max_nodes: usize,
    /// `max_concurrency`: the most members polled at once; zero is the default.
    pub max_concurrency: usize,
    /// `max_response_bytes`: the largest answer read from a member; zero is the default.
    pub max_response_bytes: u64,
    /// `link_address`: where members that link out reach the manager; empty, off.
    pub link_address: String,
    /// `link_url`: the URL members reach `link_address` at; empty, made from the page's host.
    pub link_url: String,
    /// `admins`: console users who are the cluster's admins whatever the roles say.
    pub admins: Vec<String>,
}

impl Cluster {
    /// `poll_interval`, its default when left out.
    #[must_use]
    pub fn interval(&self) -> Duration {
        or(self.poll_interval, DEFAULT_CLUSTER_POLL_INTERVAL)
    }

    /// `timeout`, its default when left out.
    #[must_use]
    pub fn call_timeout(&self) -> Duration {
        or(self.timeout, DEFAULT_CLUSTER_TIMEOUT)
    }

    /// `max_nodes`, its default when left out.
    #[must_use]
    pub fn nodes(&self) -> usize {
        if self.max_nodes == 0 {
            DEFAULT_CLUSTER_MAX_NODES
        } else {
            self.max_nodes
        }
    }

    /// `max_concurrency`, its default when left out.
    #[must_use]
    pub fn concurrency(&self) -> usize {
        if self.max_concurrency == 0 {
            DEFAULT_CLUSTER_MAX_CONCURRENCY
        } else {
            self.max_concurrency
        }
    }

    /// `max_response_bytes`, its default when left out.
    #[must_use]
    pub fn response_bytes(&self) -> u64 {
        if self.max_response_bytes == 0 {
            DEFAULT_CLUSTER_MAX_RESPONSE_BYTES
        } else {
            self.max_response_bytes
        }
    }
}

fn or(d: Duration, default: Duration) -> Duration {
    if d.is_zero() { default } else { d }
}

/// `admin.cluster_member`, fit's `config.ClusterMember`: this akit links out to a manager, which
/// manages it through the link.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ClusterMember {
    /// `manager`: the manager's link URL, `wss://host:port/cluster/link`.
    pub manager: String,
    /// `fingerprint`: the SHA-256 of the manager's link certificate.
    pub fingerprint: String,
    /// `token`: the enrollment token, used once.
    pub token: String,
    /// `token_file`: a file holding it.
    pub token_file: String,
}

impl std::fmt::Debug for ClusterMember {
    // The token is never shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterMember")
            .field("manager", &self.manager)
            .field("fingerprint", &self.fingerprint)
            .field("token", &(!self.token.is_empty()).then_some("[redacted]"))
            .field("token_file", &self.token_file)
            .finish()
    }
}

/// Whether `id` is a node ID, fit's `nodeIDPattern`: 1 to 64 of letters, digits, '.', '_' and
/// '-', starting with a letter or digit.
#[must_use]
pub fn valid_node_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// Why `raw` is not a manager's link URL (wss, a host, no user, query or fragment), or `None`:
/// fit's `CheckClusterLinkURL`.
#[must_use]
pub fn check_link_url(raw: &str) -> Option<String> {
    let bad = || {
        Some(format!(
            "{raw:?} is not a wss:// URL without a user, a query or a fragment, as \
             wss://manager.example.com:7444{CLUSTER_LINK_PATH}"
        ))
    };
    let Some(rest) = raw
        .get(..6)
        .filter(|s| s.eq_ignore_ascii_case("wss://"))
        .and_then(|_| raw.get(6..))
    else {
        return bad();
    };
    if rest.contains(['?', '#']) {
        return bad();
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return bad();
    }
    None
}

/// Whether `s` is a SHA-256 written as fit writes pins: 64 hex digits, either case, with or
/// without colons, spaces around it ignored (fit's `ValidFingerprint`).
#[must_use]
pub fn valid_fingerprint(s: &str) -> bool {
    let h: Vec<u8> = s.trim().bytes().filter(|b| *b != b':').collect();
    h.len() == 64 && h.iter().all(u8::is_ascii_hexdigit)
}

/// Why `tok` is not an enrollment token ([`CLUSTER_ENROLL_PREFIX`], 16 hex digits, a dot and a
/// secret of 22 to 86 base64url characters), or `None`: fit's `CheckClusterEnrollToken`.
#[must_use]
pub fn check_enroll_token(tok: &str) -> Option<String> {
    let ok = tok
        .strip_prefix(CLUSTER_ENROLL_PREFIX)
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(id, secret)| {
            crate::roles::valid_link_token_id(id)
                && (22..=86).contains(&secret.len())
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
    (!ok).then(|| {
        format!(
            "not an enrollment token: one is {CLUSTER_ENROLL_PREFIX}<16 hex digits>.<secret>, as \
             the manager's Add by enrollment makes it"
        )
    })
}

/// Whether `address` is a host and a non-empty port, as Go's `net.SplitHostPort` takes it.
fn host_and_port(address: &str) -> bool {
    let port = if let Some(rest) = address.strip_prefix('[') {
        match rest.split_once(']') {
            Some((_, after)) => after.strip_prefix(':'),
            None => None,
        }
    } else {
        match address.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => Some(port),
            _ => None,
        }
    };
    port.is_some_and(|p| !p.is_empty() && !p.contains(':'))
}

const CLUSTER_FIELDS: &[&str] = &[
    "poll_interval",
    "timeout",
    "max_nodes",
    "max_concurrency",
    "max_response_bytes",
    "link_address",
    "link_url",
    "admins",
];
const CLUSTER_MEMBER_FIELDS: &[&str] = &["manager", "fingerprint", "token", "token_file"];

/// `admin.cluster`, as fit's `config.Cluster` and the parts of `checkCluster` that need only it,
/// with where each of `admins` is written; [`check_cluster`] checks the rest against
/// `admin.console`.
pub(crate) fn cluster(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<(Cluster, Vec<usize>)> {
    let mut o = d.object(n, path)?;
    let mut c = Cluster::default();
    for (key, slot, min) in [
        (
            "poll_interval",
            &mut c.poll_interval,
            MIN_CLUSTER_POLL_INTERVAL,
        ),
        ("timeout", &mut c.timeout, MIN_CLUSTER_TIMEOUT),
    ] {
        if let Some(v) = o.take(key) {
            let p = o.at(key);
            if let Some(t) = d.duration(v, &p) {
                if !t.is_zero() && t < min {
                    d.error(
                        v.off,
                        &p,
                        format!(
                            "{} is below {}",
                            crate::duration::format(t),
                            crate::duration::format(min)
                        ),
                    );
                }
                *slot = t;
            }
        }
    }
    for (key, slot, max) in [
        ("max_nodes", &mut c.max_nodes, MAX_CLUSTER_MAX_NODES),
        (
            "max_concurrency",
            &mut c.max_concurrency,
            MAX_CLUSTER_MAX_CONCURRENCY,
        ),
    ] {
        if let Some(v) = o.take(key) {
            if let Some(n) = d
                .int(v, &o.at(key), 0, max)
                .and_then(|n| usize::try_from(n).ok())
            {
                *slot = n;
            }
        }
    }
    if let Some(v) = o.take("max_response_bytes") {
        let p = o.at("max_response_bytes");
        if let Some(b) = d
            .int(v, &p, 0, MAX_CLUSTER_RESPONSE_BYTES)
            .and_then(|n| u64::try_from(n).ok())
        {
            if b != 0 && b < MIN_CLUSTER_RESPONSE_BYTES {
                d.error(
                    v.off,
                    &p,
                    format!("{b} is below {MIN_CLUSTER_RESPONSE_BYTES}"),
                );
            }
            c.max_response_bytes = b;
        }
    }
    if let Some(v) = o.take("link_address") {
        let p = o.at("link_address");
        if let Some(s) = d.string(v, &p) {
            if !s.is_empty() && !host_and_port(&s) {
                d.error(
                    v.off,
                    &p,
                    format!("{s:?} is not a host and a port, as \":7444\""),
                );
            }
            c.link_address = s;
        }
    }
    if let Some(v) = o.take("link_url") {
        let p = o.at("link_url");
        if let Some(s) = d.string(v, &p) {
            if !s.is_empty() {
                if let Some(msg) = check_link_url(&s) {
                    d.error(v.off, &p, msg);
                } else if c.link_address.is_empty() {
                    d.error(
                        v.off,
                        &p,
                        "link_url names where members reach link_address: set link_address too",
                    );
                }
            }
            c.link_url = s;
        }
    }
    let mut offs = Vec::new();
    if let Some(v) = o.take("admins") {
        let p = o.at("admins");
        for (off, s) in d.strings(v, &p) {
            offs.push(off);
            c.admins.push(s);
        }
        offs.push(v.off);
    }
    o.finish(d, CLUSTER_FIELDS, &[]);
    Some((c, offs))
}

/// The rest of fit's `checkCluster`: the console the members are served in, and the admins,
/// console users each named once (fit's `checkClusterAdmins`). `off` is `admin.cluster`'s,
/// `offs` where each admin is written and, last, the list.
pub(crate) fn check_cluster(
    d: &mut Decoder<'_>,
    c: &Cluster,
    console: Option<&Console>,
    off: usize,
    offs: &[usize],
    path: &str,
) {
    if console.is_none() {
        d.error(
            off,
            path,
            "a manager's members are served in the console's workbench: set admin.console too",
        );
    }
    let at = join_field(path, "admins");
    if c.admins.len() > MAX_CLUSTER_ADMINS {
        d.error(
            offs.last().copied().unwrap_or(off),
            &at,
            format!("{} admins, more than {MAX_CLUSTER_ADMINS}", c.admins.len()),
        );
        return;
    }
    for (i, name) in c.admins.iter().enumerate() {
        let p = join_index(&at, i);
        let off = offs.get(i).copied().unwrap_or(off);
        if name.is_empty() {
            d.error(off, &p, "an admin is a console user's name");
        } else if c.admins.get(..i).is_some_and(|s| s.contains(name)) {
            d.error(off, &p, format!("{name:?} is named twice"));
        } else if console.is_some_and(|c| !c.users.iter().any(|u| &u.username == name)) {
            d.error(
                off,
                &p,
                format!("{name:?} is not a console user (admin.console.users)"),
            );
        }
    }
}

/// `admin.cluster_member`, as fit's `config.ClusterMember` and `checkClusterMember`.
pub(crate) fn cluster_member(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<ClusterMember> {
    let mut o = d.object(n, path)?;
    let mut m = ClusterMember::default();
    if let Some(v) = o.require(d, "manager") {
        let p = o.at("manager");
        if let Some(s) = d.string(v, &p) {
            if let Some(msg) = check_link_url(&s).filter(|_| !s.is_empty()) {
                d.error(v.off, &p, msg);
            }
            m.manager = s;
        }
    }
    if let Some(v) = o.require(d, "fingerprint") {
        let p = o.at("fingerprint");
        if let Some(s) = d.string(v, &p) {
            if !s.is_empty() && !valid_fingerprint(&s) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{s:?} is not the SHA-256 of a certificate: 64 hex digits, with or \
                         without colons"
                    ),
                );
            }
            m.fingerprint = s;
        }
    }
    let mut token_off = n.off;
    if let Some(v) = o.take("token") {
        token_off = v.off;
        m.token = d.string(v, &o.at("token")).unwrap_or_default();
    }
    if let Some(v) = o.take("token_file") {
        m.token_file = d.string(v, &o.at("token_file")).unwrap_or_default();
    }
    let tp = join_field(path, "token");
    if !m.token.is_empty() && !m.token_file.is_empty() {
        d.error(
            token_off,
            &tp,
            "token and token_file are two places for one token: set one",
        );
    } else if !m.token.is_empty() {
        if let Some(msg) = check_enroll_token(&m.token) {
            d.error(token_off, &tp, msg);
        }
    }
    o.finish(d, CLUSTER_MEMBER_FIELDS, &[]);
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        // fit's TestClusterDefaults.
        let c = Cluster::default();
        assert_eq!(c.interval(), Duration::from_secs(10));
        assert_eq!(c.call_timeout(), Duration::from_secs(10));
        assert_eq!(c.nodes(), DEFAULT_CLUSTER_MAX_NODES);
        assert_eq!(c.concurrency(), DEFAULT_CLUSTER_MAX_CONCURRENCY);
        assert_eq!(c.response_bytes(), DEFAULT_CLUSTER_MAX_RESPONSE_BYTES);
        let c = Cluster {
            poll_interval: Duration::from_secs(60),
            timeout: Duration::from_secs(3),
            max_nodes: 5,
            max_concurrency: 2,
            max_response_bytes: 1 << 20,
            ..Cluster::default()
        };
        assert_eq!(
            (
                c.interval(),
                c.call_timeout(),
                c.nodes(),
                c.concurrency(),
                c.response_bytes()
            ),
            (
                Duration::from_secs(60),
                Duration::from_secs(3),
                5,
                2,
                1 << 20
            )
        );
    }

    #[test]
    fn helpers() {
        // fit's TestClusterLinkSettings' helpers, with akit's token prefix.
        let secret = "A".repeat(43);
        assert!(check_enroll_token(&format!("akite1.0123456789abcdef.{secret}")).is_none());
        assert!(check_enroll_token(&format!("fite1.0123456789abcdef.{secret}")).is_some());
        assert!(check_enroll_token(&format!("akitt1.0123456789abcdef.{secret}")).is_some());
        assert!(
            check_enroll_token(&format!("akite1.0123456789abcdef.{}", "!".repeat(43))).is_some()
        );
        assert!(check_enroll_token("akite1.0123456789abcdef.short").is_some());
        assert!(valid_fingerprint(&format!("{}cd", "Ab:".repeat(31))));
        assert!(!valid_fingerprint(&"zz".repeat(32)));
        assert!(!valid_fingerprint("ab"));
        assert!(check_link_url("wss://u@m:1/x").is_some());
        assert!(check_link_url("wss://m:1/x?y").is_some());
        assert!(check_link_url("wss://m:1/x#y").is_some());
        assert!(check_link_url("https://m:1/x").is_some());
        assert!(check_link_url("wss:///x").is_some());
        assert!(check_link_url(&format!("wss://m:1{CLUSTER_LINK_PATH}")).is_none());
        assert!(valid_node_id("a"));
        assert!(valid_node_id(&"a".repeat(64)));
        assert!(!valid_node_id(&"a".repeat(65)));
        assert!(!valid_node_id("-a"));
        assert!(!valid_node_id("a b"));
        assert!(!valid_node_id(""));
        assert!(host_and_port(":7444"));
        assert!(host_and_port("0.0.0.0:7444"));
        assert!(host_and_port("[::]:7444"));
        assert!(!host_and_port("7444"));
        assert!(!host_and_port("m:"));
        assert!(!host_and_port("::1"));
    }
}
