//! The members and the store's document, as fit's `cluster/members.go`: a member's name, labels,
//! URL, access and credentials, checked as fit's `Member.check`; what the API shows of one; and
//! the document the store's section `cluster` keeps, read and written as fit's, its parts akit
//! does not use yet kept as they were read (decision 041, 4).

use std::collections::BTreeMap;

use akit_config::value::Value;

use crate::json::{self, Obj, quoted};

/// The store's section the manager keeps its document in: fit's.
pub const SECTION: &str = "cluster";

/// The version of the document fit writes with roles; one later is a newer manager's, read and
/// never written over.
pub const DOC_VERSION: i64 = 3;
/// The version a document without roles is written as.
pub const DOC_VERSION_NO_ROLES: i64 = 2;

/// A member reached through its console, with a user name and a password.
pub const ACCESS_CONSOLE: &str = "console";
/// A member reached through its Admin API, with its bearer token.
pub const ACCESS_ADMIN_API: &str = "admin_api";
/// A member that links out to the manager (fit's I149, akit's I139b).
pub const ACCESS_LINK: &str = "link";

/// The bounds of what a member holds: fit's.
pub const MAX_NAME_BYTES: usize = 64;
/// See [`MAX_NAME_BYTES`].
pub const MAX_LABELS: usize = 32;
/// See [`MAX_NAME_BYTES`].
pub const MAX_LABEL_KEY: usize = 63;
/// See [`MAX_NAME_BYTES`].
pub const MAX_LABEL_VALUE: usize = 63;
/// See [`MAX_NAME_BYTES`].
pub const MAX_URL_BYTES: usize = 2048;
/// See [`MAX_NAME_BYTES`].
pub const MAX_USERNAME_BYTES: usize = 128;
/// See [`MAX_NAME_BYTES`].
pub const MAX_PASSWORD_BYTES: usize = 1024;
/// See [`MAX_NAME_BYTES`].
pub const MAX_TOKEN_BYTES: usize = 4096;
/// See [`MAX_NAME_BYTES`].
pub const MAX_CA_BYTES: usize = 64 << 10;
/// How many audit entries the document keeps, the oldest dropped first.
pub const MAX_AUDIT: usize = 1000;
/// About how many bytes the audit entries take at most, the oldest dropped first (fit's
/// `maxAuditBytes`).
pub const MAX_AUDIT_BYTES: usize = 2 << 20;
/// The largest body of a request to the module.
pub const MAX_BODY_BYTES: usize = 128 << 10;

/// How a member's certificate is verified: the system's CAs when both are empty, a PEM of CAs,
/// or the SHA-256 of its leaf, pinned once a person confirmed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tls {
    /// `ca`: the CAs to trust, PEM.
    pub ca: String,
    /// `fingerprint`: the leaf's SHA-256, as fit writes pins.
    pub fingerprint: String,
}

impl Tls {
    /// What verifies a member's certificate: `pin`, `ca` or `system`.
    #[must_use]
    pub fn verified(&self) -> &'static str {
        if !self.fingerprint.is_empty() {
            "pin"
        } else if !self.ca.is_empty() {
            "ca"
        } else {
            "system"
        }
    }

    fn json(&self) -> String {
        Obj::new()
            .str_omit("ca", &self.ca)
            .str_omit("fingerprint", &self.fingerprint)
            .end()
    }
}

/// A member as the store keeps it, with its credentials, which never leave the manager but to
/// the member. What akit does not read of fit's member (its `link`, I139b) is kept in `extra`.
#[derive(Clone, PartialEq, Eq)]
pub struct Member {
    /// Its name.
    pub name: String,
    /// Its labels.
    pub labels: BTreeMap<String, String>,
    /// Its URL.
    pub url: String,
    /// [`ACCESS_CONSOLE`], [`ACCESS_ADMIN_API`] or [`ACCESS_LINK`].
    pub access: String,
    /// The console user's name.
    pub username: String,
    /// The console user's password.
    pub password: String,
    /// The Admin API's token.
    pub token: String,
    /// How its certificate is verified.
    pub tls: Tls,
    /// When it was added and changed, as written.
    pub created: String,
    /// See [`created`](Member::created).
    pub updated: String,
    /// Members of fit's member akit keeps as they were read: `link`.
    pub extra: Vec<(String, Value)>,
}

impl std::fmt::Debug for Member {
    // Credentials are never shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Member")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("access", &self.access)
            .field("username", &self.username)
            .field("password_set", &!self.password.is_empty())
            .field("token_set", &!self.token.is_empty())
            .finish_non_exhaustive()
    }
}

impl Member {
    /// Reads a member as fit's document holds it.
    #[must_use]
    pub fn from_value(v: &Value) -> Member {
        let mut m = Member {
            name: json::str_of(v, "name").to_owned(),
            labels: BTreeMap::new(),
            url: json::str_of(v, "url").to_owned(),
            access: json::str_of(v, "access").to_owned(),
            username: json::str_of(v, "username").to_owned(),
            password: json::str_of(v, "password").to_owned(),
            token: json::str_of(v, "token").to_owned(),
            tls: Tls::default(),
            created: json::str_of(v, "created").to_owned(),
            updated: json::str_of(v, "updated").to_owned(),
            extra: Vec::new(),
        };
        if let Some(Value::Object(ls)) = v.get("labels") {
            for (k, val) in ls {
                if let Value::String(s) = val {
                    m.labels.insert(k.clone(), s.clone());
                }
            }
        }
        if let Some(t) = v.get("tls") {
            m.tls = Tls {
                ca: json::str_of(t, "ca").to_owned(),
                fingerprint: json::str_of(t, "fingerprint").to_owned(),
            };
        }
        if let Value::Object(ms) = v {
            for (k, val) in ms {
                if !KNOWN.contains(&k.as_str()) {
                    m.extra.push((k.clone(), val.clone()));
                }
            }
        }
        m
    }

    /// The member as the store keeps it: fit's `Member`, its empty members left out as Go's
    /// `omitempty` leaves them.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new().str("name", &self.name);
        if !self.labels.is_empty() {
            o = o.raw("labels", &labels_json(&self.labels));
        }
        o = o
            .str("url", &self.url)
            .str("access", &self.access)
            .str_omit("username", &self.username)
            .str_omit("password", &self.password)
            .str_omit("token", &self.token);
        if self.tls != Tls::default() {
            o = o.raw("tls", &self.tls.json());
        }
        for (k, v) in &self.extra {
            o = o.raw(k, &json::compact(v));
        }
        o.raw("created", &quoted(&self.created))
            .raw("updated", &quoted(&self.updated))
            .end()
    }

    /// What the API shows of the member, fit's `MemberView`: credentials only as whether they
    /// are set.
    #[must_use]
    pub fn view(&self) -> String {
        let mut o = Obj::new()
            .str("name", &self.name)
            .raw("labels", &labels_json(&self.labels))
            .str("url", &self.url)
            .str("access", &self.access)
            .str_omit("username", &self.username)
            .bool("password_set", !self.password.is_empty())
            .bool("token_set", !self.token.is_empty())
            .raw("tls", &self.tls.json());
        if let Some(l) = self.link() {
            o = o.raw("link", &link_view(l));
        }
        o.raw("created", &quoted(&self.created))
            .raw("updated", &quoted(&self.updated))
            .end()
    }

    /// Its `link`, a member that links out to the manager.
    #[must_use]
    pub fn link(&self) -> Option<&Value> {
        self.extra.iter().find(|(k, _)| k == "link").map(|(_, v)| v)
    }

    /// Whether `a` and `b` reach the member the same way, so that a session of one serves the
    /// other: fit's `sameLink`.
    #[must_use]
    pub fn same_link(&self, b: &Member) -> bool {
        self.url == b.url
            && self.access == b.access
            && self.username == b.username
            && self.password == b.password
            && self.token == b.token
            && self.tls == b.tls
            && self.link() == b.link()
    }

    /// Checks the member as one the manager can reach: fit's `Member.check`.
    ///
    /// # Errors
    ///
    /// The field refused, and why.
    pub fn check(&self) -> Result<(), FieldError> {
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES || !name_ok(&self.name) {
            return Err(field(
                "name",
                format!(
                    "{:?} is not a name: 1 to {MAX_NAME_BYTES} letters, digits, '_', '.' and '-', \
                     starting with a letter or digit",
                    self.name
                ),
            ));
        }
        if self.labels.len() > MAX_LABELS {
            return Err(field(
                "labels",
                format!("{} labels, more than {MAX_LABELS}", self.labels.len()),
            ));
        }
        for (k, v) in &self.labels {
            if k.len() > MAX_LABEL_KEY || !label_ok(k) {
                return Err(field(
                    "labels",
                    format!(
                        "{k:?} is not a label: 1 to {MAX_LABEL_KEY} letters, digits, '_', '.', '/' \
                         and '-', starting and ending with a letter or digit"
                    ),
                ));
            }
            if v.len() > MAX_LABEL_VALUE || !v.is_empty() && !label_ok(v) {
                return Err(field(
                    "labels",
                    format!(
                        "the value of {k:?} is not 0 to {MAX_LABEL_VALUE} letters, digits, '_', \
                         '.', '/' and '-', starting and ending with a letter or digit"
                    ),
                ));
            }
        }
        if self.access == ACCESS_LINK {
            if self.link().is_none() {
                return Err(field(
                    "link",
                    "the linked member has neither a credential nor an enrollment token: make it \
                     a token",
                ));
            }
            if !self.url.is_empty()
                || !self.username.is_empty()
                || !self.password.is_empty()
                || !self.token.is_empty()
                || self.tls != Tls::default()
            {
                return Err(field(
                    "access",
                    "a linked member has no URL, credentials or TLS of its own: it is reached \
                     through its link",
                ));
            }
            return Ok(());
        }
        check_url(&self.url)?;
        match self.access.as_str() {
            ACCESS_CONSOLE => {
                if self.username.is_empty()
                    || self.username.len() > MAX_USERNAME_BYTES
                    || !printable(&self.username)
                {
                    return Err(field(
                        "username",
                        format!(
                            "the console's user name is 1 to {MAX_USERNAME_BYTES} printable \
                             characters"
                        ),
                    ));
                }
                if self.password.is_empty() || self.password.len() > MAX_PASSWORD_BYTES {
                    return Err(field(
                        "password",
                        format!(
                            "the console user's password is 1 to {MAX_PASSWORD_BYTES} bytes of UTF-8"
                        ),
                    ));
                }
            }
            ACCESS_ADMIN_API => {
                if self.token.is_empty()
                    || self.token.len() > MAX_TOKEN_BYTES
                    || !printable(&self.token)
                    || self.token.contains(' ')
                {
                    return Err(field(
                        "token",
                        format!(
                            "the Admin API's token is 1 to {MAX_TOKEN_BYTES} printable characters \
                             without spaces"
                        ),
                    ));
                }
            }
            other => {
                return Err(field(
                    "access",
                    format!(
                        "{other:?} is not an access: {ACCESS_CONSOLE} or {ACCESS_ADMIN_API} (or \
                         {ACCESS_LINK}, added by enrollment)"
                    ),
                ));
            }
        }
        if !self.tls.ca.is_empty() && !self.tls.fingerprint.is_empty() {
            return Err(field(
                "tls",
                "ca and fingerprint are two ways to trust the member's certificate: set one",
            ));
        }
        if !self.tls.ca.is_empty() {
            if self.tls.ca.len() > MAX_CA_BYTES {
                return Err(field(
                    "tls.ca",
                    format!("the CAs are more than {MAX_CA_BYTES} bytes"),
                ));
            }
            ca_roots(&self.tls.ca).map_err(|e| field("tls.ca", e))?;
        }
        if !self.tls.fingerprint.is_empty()
            && akit_config::roles::parse_pin(&self.tls.fingerprint).is_none()
        {
            return Err(field(
                "tls.fingerprint",
                format!(
                    "{:?} is not a SHA-256 fingerprint: 64 hex digits, with or without colons",
                    self.tls.fingerprint
                ),
            ));
        }
        if self.tls != Tls::default() && !self.url.starts_with("https://") {
            return Err(field(
                "tls",
                "a member reached over http has no certificate to verify",
            ));
        }
        Ok(())
    }
}

/// The members of fit's `Member` akit reads.
const KNOWN: &[&str] = &[
    "name", "labels", "url", "access", "username", "password", "token", "tls", "created", "updated",
];

fn labels_json(labels: &BTreeMap<String, String>) -> String {
    let mut o = Obj::new();
    for (k, v) in labels {
        o = o.str(k, v);
    }
    o.end()
}

/// What the API shows of a member's link, fit's `LinkView`: never a hash.
fn link_view(l: &Value) -> String {
    let t = |k: &str| json::str_of(l, k).to_owned();
    let mut o = Obj::new();
    if !t("enrolled").is_empty() {
        o = o.str("enrolled", &t("enrolled"));
    }
    o = o
        .bool("credential_set", !t("credential_hash").is_empty())
        .bool("token_waiting", !t("token_hash").is_empty());
    for k in ["token_made", "token_expires"] {
        if !t(k).is_empty() {
            o = o.str(k, &t(k));
        }
    }
    o.end()
}

/// A member refused for one of its fields: fit's `FieldError`, `{"field", "error"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// The field.
    pub field: String,
    /// Why.
    pub msg: String,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.msg)
    }
}

impl FieldError {
    /// As the API answers it.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .str("field", &self.field)
            .str("error", &self.msg)
            .end()
    }
}

fn field(f: &str, msg: impl Into<String>) -> FieldError {
    FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    }
}

fn name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

fn label_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.last().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'-'))
}

fn printable(s: &str) -> bool {
    s.chars().all(|c| c >= ' ' && c != '\u{7f}')
}

/// Whether `host` is `localhost` or a loopback address.
#[must_use]
pub fn loopback(host: &str) -> bool {
    akit_config::roles::loopback(host)
}

/// Checks a member's URL as fit's `checkURL`: https, or http to a loopback address, whose
/// credentials do not cross a network; a host, no user, query or fragment.
///
/// # Errors
///
/// Why it is not one.
pub fn check_url(raw: &str) -> Result<(), FieldError> {
    if raw.is_empty() || raw.len() > MAX_URL_BYTES {
        return Err(field(
            "url",
            format!("the URL is 1 to {MAX_URL_BYTES} bytes"),
        ));
    }
    let bad = || {
        field(
            "url",
            format!("{raw:?} is not an http or https URL without a user, a query or a fragment"),
        )
    };
    let (scheme, rest) = raw.split_once("://").ok_or_else(bad)?;
    if rest.contains(['?', '#']) || rest.chars().any(char::is_whitespace) {
        return Err(bad());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(bad());
    }
    let host = host_of(authority).ok_or_else(bad)?;
    match scheme {
        "https" => Ok(()),
        "http" if loopback(&host) => Ok(()),
        "http" => Err(field(
            "url",
            format!(
                "{raw:?} is http to a host that is not a loopback address: use https, so that \
                 credentials do not cross a network in the clear"
            ),
        )),
        _ => Err(field("url", format!("{raw:?} is not an http or https URL"))),
    }
}

/// The host of an authority, `host[:port]` or `[v6][:port]`, without brackets.
#[must_use]
pub fn host_of(authority: &str) -> Option<String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        if !(after.is_empty() || after.strip_prefix(':').is_some_and(port_ok)) {
            return None;
        }
        return Some(h.to_owned());
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => port_ok(p).then(|| h.to_owned()),
        Some(_) => None,
        None => Some(authority.to_owned()),
    }
    .filter(|h| !h.is_empty())
}

fn port_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit())
}

/// The CAs of `pem`, at least one.
///
/// # Errors
///
/// No certificate, or one that does not parse.
pub fn ca_roots(pem: &str) -> Result<akio::tls::RootCerts, String> {
    if !pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err("no PEM certificate".to_owned());
    }
    let mut roots = akio::tls::RootCerts::empty();
    let n = roots
        .add_pem(pem.as_bytes())
        .map_err(|e| format!("a certificate does not parse: {e}"))?;
    if n == 0 {
        return Err("no PEM certificate".to_owned());
    }
    Ok(roots)
}

/// A SHA-256 written as fit writes pins: lower-case hex, its bytes apart by colons.
#[must_use]
pub fn format_fingerprint(sum: &[u8]) -> String {
    let mut out = String::with_capacity(sum.len().saturating_mul(3));
    for (i, b) in sum.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// A member as the API takes it, to add or replace one: a password or token left empty keeps
/// the one kept. fit's `Input`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Input {
    /// `name`.
    pub name: String,
    /// `labels`.
    pub labels: BTreeMap<String, String>,
    /// `url`.
    pub url: String,
    /// `access`.
    pub access: String,
    /// `username`.
    pub username: String,
    /// `password`.
    pub password: String,
    /// `token`.
    pub token: String,
    /// `tls`.
    pub tls: Tls,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Input")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}

impl Input {
    /// Reads an input from a request's JSON body, as Go's decoder would: members of the wrong
    /// type refuse it.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn from_value(v: &Value) -> Result<Input, String> {
        let Value::Object(ms) = v else {
            return Err("the body is not an object".to_owned());
        };
        let mut i = Input::default();
        for (k, val) in ms {
            let s = |slot: &mut String| match val {
                Value::String(x) => {
                    slot.clone_from(x);
                    Ok(())
                }
                Value::Null => Ok(()),
                _ => Err(format!("{k} is not a string")),
            };
            match k.as_str() {
                "name" => s(&mut i.name)?,
                "url" => s(&mut i.url)?,
                "access" => s(&mut i.access)?,
                "username" => s(&mut i.username)?,
                "password" => s(&mut i.password)?,
                "token" => s(&mut i.token)?,
                "labels" => match val {
                    Value::Object(ls) => {
                        for (lk, lv) in ls {
                            match lv {
                                Value::String(x) => {
                                    i.labels.insert(lk.clone(), x.clone());
                                }
                                _ => return Err(format!("labels.{lk} is not a string")),
                            }
                        }
                    }
                    Value::Null => {}
                    _ => return Err("labels is not an object".to_owned()),
                },
                "tls" => match val {
                    Value::Object(_) => {
                        i.tls.ca = json::str_of(val, "ca").to_owned();
                        i.tls.fingerprint = json::str_of(val, "fingerprint").to_owned();
                    }
                    Value::Null => {}
                    _ => return Err("tls is not an object".to_owned()),
                },
                // Go's decoder leaves out members it does not know.
                _ => {}
            }
        }
        Ok(i)
    }

    /// The member this input makes, taking the credentials `old` keeps where it leaves them
    /// empty (`None` for a new member), checked; `now` stamps it. fit's `Input.member`.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn member(&self, old: Option<&Member>, now: &str) -> Result<Member, FieldError> {
        let mut m = Member {
            name: self.name.trim().to_owned(),
            labels: self.labels.clone(),
            url: self.url.trim().to_owned(),
            access: self.access.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            token: self.token.clone(),
            tls: Tls {
                ca: self.tls.ca.trim().to_owned(),
                fingerprint: self.tls.fingerprint.trim().to_owned(),
            },
            created: now.to_owned(),
            updated: now.to_owned(),
            extra: Vec::new(),
        };
        if m.access.is_empty() {
            m.access = if old.is_some_and(|o| o.access == ACCESS_LINK) {
                ACCESS_LINK.to_owned()
            } else {
                ACCESS_CONSOLE.to_owned()
            };
        }
        if m.access == ACCESS_LINK {
            let Some(o) = old.filter(|o| o.access == ACCESS_LINK) else {
                return Err(field(
                    "access",
                    "a member that links out to the manager is added by enrollment, not by its URL",
                ));
            };
            m.url.clear();
            m.username.clear();
            m.password.clear();
            m.token.clear();
            m.tls = Tls::default();
            m.created.clone_from(&o.created);
            m.extra.clone_from(&o.extra);
            m.check()?;
            return Ok(m);
        }
        if let Some(o) = old {
            m.created.clone_from(&o.created);
            if m.password.is_empty() && m.access == ACCESS_CONSOLE && o.access == ACCESS_CONSOLE {
                m.password.clone_from(&o.password);
            }
            if m.token.is_empty() && m.access == ACCESS_ADMIN_API && o.access == ACCESS_ADMIN_API {
                m.token.clone_from(&o.token);
            }
        }
        // A credential of the other access is not kept.
        match m.access.as_str() {
            ACCESS_CONSOLE => m.token.clear(),
            ACCESS_ADMIN_API => {
                m.username.clear();
                m.password.clear();
            }
            _ => {}
        }
        m.check()?;
        if let Some(pin) = akit_config::roles::parse_pin(&m.tls.fingerprint) {
            m.tls.fingerprint = format_fingerprint(&pin);
        }
        Ok(m)
    }
}

/// The fields that changed from `a` to `b`, credentials by name only: fit's `changes`.
#[must_use]
pub fn changes(a: &Member, b: &Member) -> String {
    let mut out = Vec::new();
    for (changed, name) in [
        (a.name != b.name, "name"),
        (a.labels != b.labels, "labels"),
        (a.url != b.url, "url"),
        (a.access != b.access, "access"),
        (a.username != b.username, "username"),
        (a.password != b.password, "password"),
        (a.token != b.token, "token"),
        (a.tls.ca != b.tls.ca, "tls.ca"),
        (a.tls.fingerprint != b.tls.fingerprint, "tls.fingerprint"),
        (a.link() != b.link(), "link.credential"),
    ] {
        if changed {
            out.push(name);
        }
    }
    if out.is_empty() {
        "nothing".to_owned()
    } else {
        out.join(", ")
    }
}

/// Says what a member is, without its credentials: fit's `describe`.
#[must_use]
pub fn describe(m: &Member) -> String {
    if m.access == ACCESS_LINK {
        return "linking out to the manager, by enrollment".to_owned();
    }
    let mut s = format!(
        "{} by {}, certificate by {}",
        m.url,
        m.access,
        m.tls.verified()
    );
    if m.access == ACCESS_CONSOLE {
        s.push_str(&format!(", as {:?}", m.username));
    }
    s
}

/// The store's section, fit's `document`: the members akit reads and changes, the audit log it
/// appends to, the revision, and every other member kept as it was read, in its place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    /// The version it was read as.
    pub version: i64,
    /// The members.
    pub members: Vec<Member>,
    /// The audit log, oldest first, each entry as written.
    pub audit: Vec<Value>,
    /// Counts the document's changes (fit's I148).
    pub revision: u64,
    /// The other members of the document, in their order: `groups`, `templates`, `rollouts`,
    /// `link_cert`, `access` and what a later fit adds.
    pub rest: Vec<(String, Value)>,
}

/// What reading the section found.
#[derive(Debug)]
pub enum Read {
    /// A document this akit writes.
    Ok(Document),
    /// A document a newer manager wrote, or one with what akit does not manage yet: shown, never
    /// written over, with why.
    ReadOnly(Document, String),
    /// Not the manager's document: left as it is, with why.
    Bad(String),
}

impl Document {
    /// Reads the store's section, or an empty document for none: fit's `readDocument`. A
    /// document of a later version, or one that sets the users' roles, which akit does not
    /// enforce before I139, is read-only (decision row 73).
    #[must_use]
    pub fn read(data: Option<&[u8]>) -> Read {
        let Some(data) = data.filter(|d| !d.is_empty()) else {
            return Read::Ok(Document {
                version: DOC_VERSION_NO_ROLES,
                ..Document::default()
            });
        };
        let Some(Value::Object(ms)) = json::parse(data) else {
            return Read::Bad(format!(
                "the store's section {SECTION:?} is not the manager's document"
            ));
        };
        let mut d = Document::default();
        for (k, v) in ms {
            match k.as_str() {
                "version" => {
                    d.version =
                        json::int_of(&Value::Object(vec![(k.clone(), v.clone())]), "version");
                }
                "members" => match v {
                    Value::Array(a) => d.members = a.iter().map(Member::from_value).collect(),
                    Value::Null => {}
                    _ => {
                        return Read::Bad(format!(
                            "the store's section {SECTION:?} is not the manager's document: its \
                             members are not a list"
                        ));
                    }
                },
                "audit" => {
                    if let Value::Array(a) = v {
                        d.audit = a;
                    }
                }
                "revision" => {
                    d.revision =
                        json::uint_of(&Value::Object(vec![(k.clone(), v.clone())]), "revision");
                }
                _ => d.rest.push((k, v)),
            }
        }
        if d.version > DOC_VERSION {
            let why = format!(
                "the store's members were written by a newer manager (document version {}; this \
                 akit writes {DOC_VERSION}): they are shown, and changed only by that manager",
                d.version
            );
            return Read::ReadOnly(d, why);
        }
        if d.rest
            .iter()
            .any(|(k, v)| k == "access" && *v != Value::Null)
        {
            return Read::ReadOnly(
                d,
                "the store's document sets the users' roles, which akit enforces from I139: the \
                 members are shown, and changed only by a manager that enforces them"
                    .to_owned(),
            );
        }
        Read::Ok(d)
    }

    /// The document as the store keeps it, fit's `marshal`: version 2 (akit never writes
    /// roles before I139), every list present, the parts akit does not use as they were read.
    #[must_use]
    pub fn marshal(&self) -> Vec<u8> {
        let mut b = Obj::new().num("version", DOC_VERSION_NO_ROLES);
        let mut members = String::from("[");
        for (i, m) in self.members.iter().enumerate() {
            if i > 0 {
                members.push(',');
            }
            members.push_str(&m.to_json());
        }
        members.push(']');
        b = b.raw("members", &members);
        for list in ["groups", "templates", "rollouts"] {
            let v = self
                .rest
                .iter()
                .find(|(k, _)| k == list)
                .map(|(_, v)| v)
                .filter(|v| **v != Value::Null);
            b = b.raw(list, &v.map_or_else(|| "[]".to_owned(), json::compact));
        }
        let audit = Value::Array(self.audit.clone());
        b = b.raw("audit", &json::compact(&audit));
        for (k, v) in &self.rest {
            if !matches!(k.as_str(), "groups" | "templates" | "rollouts") {
                b = b.raw(k, &json::compact(v));
            }
        }
        if self.revision > 0 {
            b = b.num("revision", self.revision);
        }
        b.end().into_bytes()
    }

    /// The index of member `name`.
    #[must_use]
    pub fn index(&self, name: &str) -> Option<usize> {
        self.members.iter().position(|m| m.name == name)
    }

    /// Appends `entry`, dropping the oldest past [`MAX_AUDIT`] entries or about
    /// [`MAX_AUDIT_BYTES`]: fit's `trimAudit`.
    pub fn append_audit(&mut self, entry: Value) {
        self.audit.push(entry);
        let excess = self.audit.len().saturating_sub(MAX_AUDIT);
        self.audit.drain(..excess);
        let mut total: usize = self
            .audit
            .iter()
            .map(|e| json::compact(e).len())
            .fold(0, usize::saturating_add);
        while total > MAX_AUDIT_BYTES && self.audit.len() > 1 {
            let first = json::compact(&self.audit.remove(0)).len();
            total = total.saturating_sub(first);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn input(v: &str) -> Input {
        Input::from_value(&json::parse(v.as_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn members_checked_as_fit_checks_them() {
        let now = "2026-11-27T00:00:00Z";
        let ok = input(
            r#"{"name": "edge-1", "url": "https://e1:2020", "username": "ada", "password": "pw", "labels": {"region": "eu"}}"#,
        );
        let m = ok.member(None, now).unwrap();
        assert_eq!(m.access, ACCESS_CONSOLE);
        assert_eq!(m.tls.verified(), "system");
        for (body, field_name, want) in [
            (
                r#"{"name": "-x", "url": "https://e", "username": "a", "password": "p"}"#,
                "name",
                "is not a name",
            ),
            (
                r#"{"name": "x", "url": "http://e1", "username": "a", "password": "p"}"#,
                "url",
                "not a loopback address",
            ),
            (
                r#"{"name": "x", "url": "https://u@e1", "username": "a", "password": "p"}"#,
                "url",
                "without a user",
            ),
            (
                r#"{"name": "x", "url": "https://e1/?q", "username": "a", "password": "p"}"#,
                "url",
                "without a user",
            ),
            (
                r#"{"name": "x", "url": "ftp://e1", "username": "a", "password": "p"}"#,
                "url",
                "not an http or https URL",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "password": "p"}"#,
                "username",
                "user name",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "username": "a"}"#,
                "password",
                "password",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "access": "admin_api"}"#,
                "token",
                "token",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "access": "ssh"}"#,
                "access",
                "is not an access",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "access": "link"}"#,
                "access",
                "by enrollment",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "username": "a", "password": "p", "tls": {"ca": "x", "fingerprint": "y"}}"#,
                "tls",
                "set one",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "username": "a", "password": "p", "tls": {"ca": "x"}}"#,
                "tls.ca",
                "no PEM",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "username": "a", "password": "p", "tls": {"fingerprint": "ab"}}"#,
                "tls.fingerprint",
                "SHA-256",
            ),
            (
                r#"{"name": "x", "url": "http://127.0.0.1:1", "username": "a", "password": "p", "tls": {"fingerprint": "ab"}}"#,
                "tls.fingerprint",
                "SHA-256",
            ),
            (
                r#"{"name": "x", "url": "https://e1", "username": "a", "password": "p", "labels": {"-k": "v"}}"#,
                "labels",
                "is not a label",
            ),
        ] {
            let e = input(body).member(None, now).unwrap_err();
            assert_eq!(e.field, field_name, "{body}: {e}");
            assert!(e.msg.contains(want), "{body}: {e}");
        }
        // http to loopback; a pin written as fit writes pins.
        let pin = "AB".repeat(32);
        let m = input(&format!(
            r#"{{"name": "x", "url": "https://[::1]:2020", "access": "admin_api", "token": "t", "tls": {{"fingerprint": "{pin}"}}}}"#
        ))
        .member(None, now)
        .unwrap();
        assert_eq!(m.tls.fingerprint, vec!["ab"; 32].join(":"));
        assert!(input(r#"{"name": "x", "url": "http://localhost:2020", "access": "admin_api", "token": "t"}"#).member(None, now).is_ok());
        // An empty password keeps the one kept; a change of access drops the other's.
        let kept = m.clone();
        let changed = input(r#"{"name": "x", "url": "https://[::1]:2020", "access": "admin_api"}"#)
            .member(Some(&kept), "later")
            .unwrap();
        assert_eq!(changed.token, "t");
        assert_eq!(changed.created, now);
        assert_eq!(changes(&kept, &changed), "tls.fingerprint");
    }

    #[test]
    fn document_keeps_what_akit_does_not_use() {
        let fit_doc = br#"{"version":2,"members":[{"name":"a","labels":{"r":"eu"},"url":"https://a","access":"admin_api","token":"t","tls":{"fingerprint":"aa"},"created":"2026-11-06T01:02:03.5Z","updated":"2026-11-06T01:02:03.5Z"},
            {"name":"l","url":"","access":"link","link":{"credential_id":"c","credential_hash":"h"},"created":"x","updated":"x"}],
            "groups":[{"name":"g","selector":"r=eu"}],"templates":[],"rollouts":[],"audit":[{"at":"x","user":"u","action":"node.add"}],
            "link_cert":{"cert":"C","key":"K"},"revision":7}"#;
        let Read::Ok(mut d) = Document::read(Some(fit_doc)) else {
            panic!("not read");
        };
        assert_eq!(d.members.len(), 2);
        assert_eq!(d.revision, 7);
        assert!(d.members[1].link().is_some());
        d.append_audit(json::parse(br#"{"action":"node.update"}"#).unwrap());
        let out = String::from_utf8(d.marshal()).unwrap();
        for part in [
            r#""groups":[{"name":"g","selector":"r=eu"}]"#,
            r#""link_cert":{"cert":"C","key":"K"}"#,
            r#""link":{"credential_id":"c","credential_hash":"h"}"#,
            r#""tls":{"fingerprint":"aa"}"#,
            r#""revision":7"#,
            r#""version":2"#,
            r#"{"action":"node.update"}"#,
        ] {
            assert!(out.contains(part), "{out} lacks {part}");
        }
        let Read::Ok(again) = Document::read(Some(out.as_bytes())) else {
            panic!("not read again");
        };
        assert_eq!(again, d);
        // Roles, or a newer version, leave it read-only; what is not a document is refused.
        assert!(matches!(
            Document::read(Some(br#"{"version":3,"members":[],"access":{"users":[]}}"#)),
            Read::ReadOnly(..)
        ));
        assert!(matches!(
            Document::read(Some(br#"{"version":4,"members":[]}"#)),
            Read::ReadOnly(..)
        ));
        assert!(matches!(Document::read(Some(b"[1]")), Read::Bad(_)));
        assert!(matches!(Document::read(None), Read::Ok(_)));
    }

    #[test]
    fn audit_bounded() {
        let mut d = Document::default();
        for i in 0..MAX_AUDIT + 5 {
            d.append_audit(Value::Number(i.to_string()));
        }
        assert_eq!(d.audit.len(), MAX_AUDIT);
        assert_eq!(d.audit[0], Value::Number("5".into()));
    }
}
