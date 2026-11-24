//! A listener's certificates, indexed by the names they are for, and the choice of one per
//! handshake: fit's `tlsmgr.Static`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::IpAddr;

use akio::tls::{CertAlert, CertDone, CertResolver, CertifiedKey, ClientHelloInfo};
use akio_crypto::pkey::PublicKey;
use akio_x509::GeneralName;

/// The SHA-256 of a file's contents.
pub type Sum = [u8; 32];

/// The key a certificate is for, as far as choosing it goes: which signature schemes and, in
/// TLS 1.2, which cipher suites a client must offer to take it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// ECDSA, with the TLS 1.3 scheme of its curve (`ecdsa_secp256r1_sha256` and the like).
    Ecdsa(u16),
    Ed25519,
    Ed448,
    /// `rsaEncryption`.
    Rsa,
    /// `id-RSASSA-PSS`.
    RsaPss,
    /// One akit cannot tell: taken to suit every client.
    Other,
}

const ECDSA_SCHEMES: [u16; 3] = [0x0403, 0x0503, 0x0603];
const PSS_RSAE: [u16; 3] = [0x0804, 0x0805, 0x0806];
const PSS_PSS: [u16; 3] = [0x0809, 0x080a, 0x080b];
const PKCS1: [u16; 3] = [0x0401, 0x0501, 0x0601];
/// TLS 1.2 suites (the AEAD ones akio-tls has) by the key that signs their key exchange.
const ECDHE_ECDSA_SUITES: [u16; 3] = [0xc02b, 0xc02c, 0xcca9];
const ECDHE_RSA_SUITES: [u16; 3] = [0xc02f, 0xc030, 0xcca8];

impl Kind {
    fn of(key: &PublicKey) -> Kind {
        match key {
            PublicKey::P256(_) => Kind::Ecdsa(0x0403),
            PublicKey::P384(_) => Kind::Ecdsa(0x0503),
            PublicKey::P521(_) => Kind::Ecdsa(0x0603),
            PublicKey::Ed25519(_) => Kind::Ed25519,
            PublicKey::Ed448(_) => Kind::Ed448,
            PublicKey::Rsa(_) => Kind::Rsa,
            PublicKey::RsaPss(..) => Kind::RsaPss,
            _ => Kind::Other,
        }
    }

    /// Whether a client offering `schemes` and `suites` can take a certificate of this kind, as
    /// Go's `ClientHelloInfo.SupportsCertificate` decides it.
    fn suits(self, schemes: &[u16], suites: &[u16], tls13: bool) -> bool {
        let scheme = |s: u16| match self {
            Kind::Ecdsa(own) => s == own || (!tls13 && ECDSA_SCHEMES.contains(&s)),
            Kind::Ed25519 => s == 0x0807,
            Kind::Ed448 => s == 0x0808,
            Kind::Rsa => PSS_RSAE.contains(&s) || (!tls13 && PKCS1.contains(&s)),
            Kind::RsaPss => PSS_PSS.contains(&s),
            Kind::Other => true,
        };
        if !schemes.iter().any(|s| scheme(*s)) {
            return false;
        }
        if tls13 {
            return true;
        }
        let wanted: &[u16] = match self {
            Kind::Rsa | Kind::RsaPss => &ECDHE_RSA_SUITES,
            Kind::Other => return true,
            _ => &ECDHE_ECDSA_SUITES,
        };
        suites.iter().any(|s| wanted.contains(s))
    }
}

/// What a handshake offers that choosing a certificate looks at.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hello<'a> {
    pub(crate) name: Option<&'a str>,
    pub(crate) schemes: &'a [u16],
    pub(crate) suites: &'a [u16],
    pub(crate) tls13: bool,
    /// The ALPN protocols offered, in the client's order.
    pub(crate) alpn: &'a [&'a [u8]],
}

impl<'a> From<&ClientHelloInfo<'a>> for Hello<'a> {
    fn from(h: &ClientHelloInfo<'a>) -> Hello<'a> {
        Hello {
            name: h.server_name(),
            schemes: h.signature_schemes(),
            suites: h.cipher_suites(),
            tls13: h.version() == "TLSv1_3",
            alpn: h.alpn(),
        }
    }
}

struct Entry {
    key: CertifiedKey,
    kind: Kind,
}

/// A listener's certificates, indexed by the names they are for so that a handshake finds its
/// own without looking at the others: fit's `tlsmgr.Static`. It does not change once loaded; a
/// reload loads another, which the listeners serve from then on (see [`crate`]).
///
/// It is a [`CertResolver`] (the trait fit's `tlsmgr.Manager` is to Go's `GetCertificate`): it
/// answers from memory, in the handshake's call, and never blocks.
pub struct Certs {
    entries: Vec<Entry>,
    /// For each name a certificate is for (a host name in lower case, or an IP address), the
    /// certificates for it in the order given; `wildcard` holds the same for the parent of each
    /// `*.` name.
    exact: HashMap<String, Vec<usize>>,
    wildcard: HashMap<String, Vec<usize>>,
    /// The SHA-256 of each certificate and key file loaded, by path; empty for certificates not
    /// loaded from files.
    files: BTreeMap<String, Sum>,
}

impl fmt::Debug for Certs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Certs")
            .field("certificates", &self.entries.len())
            .field("files", &self.files.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The names a certificate's leaf is for (its subjectAltName's DNS names, in lower case, and IP
/// addresses), and the kind of its key.
fn leaf_names(key: &CertifiedKey) -> Result<(Vec<String>, Kind), String> {
    let Some(der) = key.chain_der().next() else {
        return Err("no certificate".into());
    };
    let leaf = akio_x509::Certificate::from_der(der).map_err(|e| e.to_string())?;
    let names = leaf
        .subject_alt_names()
        .iter()
        .filter_map(|n| match n {
            GeneralName::Dns(d) => Some(d.to_ascii_lowercase()),
            GeneralName::Ip(b) => ip(b).map(|a| a.to_string()),
            _ => None,
        })
        .collect();
    let kind = leaf.public_key().map_or(Kind::Other, |k| Kind::of(&k));
    Ok((names, kind))
}

pub(crate) fn ip(b: &[u8]) -> Option<IpAddr> {
    if let Ok(v4) = <[u8; 4]>::try_from(b) {
        return Some(IpAddr::from(v4));
    }
    <[u8; 16]>::try_from(b).ok().map(IpAddr::from)
}

fn push_once(list: &mut Vec<usize>, i: usize) {
    if list.last() != Some(&i) {
        list.push(i);
    }
}

fn normalize(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

impl Certs {
    /// Certificates in the order given, the first the default: fit's `tlsmgr.NewStatic`.
    ///
    /// # Errors
    ///
    /// The index of a certificate whose leaf does not parse, and why.
    pub fn new(keys: Vec<CertifiedKey>) -> Result<Certs, (usize, String)> {
        let mut c = Certs {
            entries: Vec::with_capacity(keys.len()),
            exact: HashMap::new(),
            wildcard: HashMap::new(),
            files: BTreeMap::new(),
        };
        for (i, key) in keys.into_iter().enumerate() {
            let (names, kind) = leaf_names(&key).map_err(|e| (i, e))?;
            c.add(key, kind, &names);
        }
        Ok(c)
    }

    pub(crate) fn with_files(mut self, files: BTreeMap<String, Sum>) -> Certs {
        self.files = files;
        self
    }

    fn add(&mut self, key: CertifiedKey, kind: Kind, names: &[String]) {
        let i = self.entries.len();
        self.entries.push(Entry { key, kind });
        for name in names {
            match name.strip_prefix("*.") {
                Some(parent) => push_once(self.wildcard.entry(parent.to_owned()).or_default(), i),
                None => push_once(self.exact.entry(name.clone()).or_default(), i),
            }
        }
    }

    /// How many certificates there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No certificates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The certificates, in the order given.
    pub fn certificates(&self) -> impl Iterator<Item = &CertifiedKey> {
        self.entries.iter().map(|e| &e.key)
    }

    /// The SHA-256 of each file the certificates were loaded from, by path.
    #[must_use]
    pub fn files(&self) -> &BTreeMap<String, Sum> {
        &self.files
    }

    /// Whether a certificate is for `name`, exactly or by a wildcard.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        let name = normalize(name);
        if self.exact.get(&name).is_some_and(|l| !l.is_empty()) {
            return true;
        }
        name.split_once('.')
            .is_some_and(|(_, parent)| self.wildcard.get(parent).is_some_and(|l| !l.is_empty()))
    }

    /// The certificate a handshake gets: the first of those for the name the client asks for
    /// that the client can take, or the first for that name if it can take none; the first of all
    /// when none is for that name or the client names none. `None` with no certificates.
    #[must_use]
    pub fn get(&self, hello: &ClientHelloInfo<'_>) -> Option<CertifiedKey> {
        self.choose(&Hello::from(hello))
            .and_then(|i| self.entries.get(i))
            .map(|e| e.key.clone())
    }

    /// The `i`th certificate.
    pub(crate) fn key(&self, i: usize) -> Option<&CertifiedKey> {
        self.entries.get(i).map(|e| &e.key)
    }

    /// The index of the certificate for `hello` (fit's `Static.GetCertificate`).
    pub(crate) fn choose(&self, hello: &Hello<'_>) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let name = match hello.name {
            Some(n) if self.entries.len() > 1 => normalize(n),
            _ => return Some(0),
        };
        let none: &[usize] = &[];
        let mut exact = self.exact.get(&name).map_or(none, Vec::as_slice);
        let mut wild = name
            .split_once('.')
            .and_then(|(_, parent)| self.wildcard.get(parent))
            .map_or(none, Vec::as_slice);
        // The only certificate for the name is served whatever the client supports.
        match (exact, wild) {
            ([i], []) | ([], [i]) => return Some(*i),
            _ => {}
        }
        // The two lists are each in the order given; walk them as one.
        let mut first = None;
        loop {
            let i = match (exact.split_first(), wild.split_first()) {
                (Some((e, rest)), Some((w, _))) if e < w => {
                    exact = rest;
                    *e
                }
                (_, Some((w, rest))) => {
                    wild = rest;
                    *w
                }
                (Some((e, rest)), None) => {
                    exact = rest;
                    *e
                }
                (None, None) => break,
            };
            first.get_or_insert(i);
            if self
                .entries
                .get(i)
                .is_some_and(|e| e.kind.suits(hello.schemes, hello.suites, hello.tls13))
            {
                return Some(i);
            }
        }
        Some(first.unwrap_or(0))
    }
}

impl CertResolver for Certs {
    fn resolve(&self, hello: &ClientHelloInfo<'_>, done: CertDone) {
        match self.get(hello) {
            Some(k) => done.complete(k),
            None => done.fail(CertAlert::UnrecognizedName),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const P256: u16 = 0x0403;
    const RSA_PSS: u16 = 0x0804;

    fn hello<'a>(name: Option<&'a str>, schemes: &'a [u16], tls13: bool) -> Hello<'a> {
        Hello {
            name,
            schemes,
            suites: &[0xc02b, 0xc02f],
            tls13,
            alpn: &[],
        }
    }

    /// Certificates of the given kinds and names, without keys: only `choose` is tested here.
    fn certs(list: &[(Kind, &[&str])]) -> Certs {
        let mut c = Certs {
            entries: Vec::new(),
            exact: HashMap::new(),
            wildcard: HashMap::new(),
            files: BTreeMap::new(),
        };
        for (kind, names) in list {
            let i = c.entries.len();
            // A placeholder: `choose` looks at the kind and the index only.
            c.entries.push(Entry {
                key: placeholder(),
                kind: *kind,
            });
            for n in *names {
                match n.strip_prefix("*.") {
                    Some(p) => push_once(c.wildcard.entry(p.to_owned()).or_default(), i),
                    None => push_once(c.exact.entry((*n).to_owned()).or_default(), i),
                }
            }
        }
        c
    }

    fn placeholder() -> CertifiedKey {
        use akio_x509::CertificateParams;
        thread_local! {
            static KEY: CertifiedKey = {
                let key = akio_crypto::pkey::PrivateKey::P256(
                    akio_crypto::p256::SecretKey::generate().unwrap(),
                );
                let cert = CertificateParams::new(["placeholder"]).self_signed(&key).unwrap();
                CertifiedKey::from_pem(cert.to_pem().as_bytes(), key.to_pkcs8_pem().as_bytes())
                    .unwrap()
            };
        }
        KEY.with(Clone::clone)
    }

    #[test]
    fn chooses_as_fit() {
        let ec = Kind::Ecdsa(P256);
        let c = certs(&[
            (ec, &["a.example"]),
            (ec, &["b.example", "*.w.example"]),
            (Kind::Rsa, &["d.example"]),
            (ec, &["d.example"]),
            (ec, &["x.w.example"]),
        ]);
        let pick = |n: Option<&str>, s: &[u16]| c.choose(&hello(n, s, true));
        // No name, or a name none is for: the first.
        assert_eq!(pick(None, &[P256]), Some(0));
        assert_eq!(pick(Some("nothing.example"), &[P256]), Some(0));
        assert_eq!(pick(Some("b.example"), &[P256]), Some(1));
        assert_eq!(pick(Some("B.Example."), &[P256]), Some(1));
        // By the wildcard, one label deep only.
        assert_eq!(pick(Some("y.w.example"), &[P256]), Some(1));
        assert_eq!(pick(Some("z.y.w.example"), &[P256]), Some(0));
        // Two for the name: the first the client can take, or the first.
        assert_eq!(pick(Some("d.example"), &[P256]), Some(3));
        assert_eq!(pick(Some("d.example"), &[RSA_PSS]), Some(2));
        assert_eq!(pick(Some("d.example"), &[0x0807]), Some(2));
        // An exact name and a wildcard, walked in the order given.
        assert_eq!(pick(Some("x.w.example"), &[P256]), Some(1));
        assert_eq!(pick(Some("x.w.example"), &[RSA_PSS]), Some(1));

        // One certificate: served whatever the name.
        let one = certs(&[(ec, &["a.example"])]);
        assert_eq!(one.choose(&hello(Some("b.example"), &[], true)), Some(0));
        assert_eq!(certs(&[]).choose(&hello(None, &[], true)), None);

        assert!(c.has("A.example."));
        assert!(c.has("q.w.example"));
        assert!(!c.has("w.example"));
        assert!(!c.has("q.example"));
    }

    #[test]
    fn suits_as_go() {
        let ec = Kind::Ecdsa(P256);
        // TLS 1.3: the curve is part of the scheme.
        assert!(ec.suits(&[P256], &[], true));
        assert!(!ec.suits(&[0x0503], &[], true));
        // TLS 1.2: any ECDSA scheme, and an ECDHE-ECDSA suite.
        assert!(ec.suits(&[0x0503], &[0xc02b], false));
        assert!(!ec.suits(&[P256], &[0xc02f], false));
        assert!(Kind::Rsa.suits(&[0x0401], &[0xc02f], false));
        assert!(!Kind::Rsa.suits(&[0x0401], &[0xc02f], true));
        assert!(!Kind::Rsa.suits(&[RSA_PSS], &[0xc02b], false));
        assert!(Kind::RsaPss.suits(&[0x0809], &[], true));
        assert!(!Kind::RsaPss.suits(&[RSA_PSS], &[], true));
        assert!(Kind::Ed25519.suits(&[0x0807], &[0xcca9], false));
        assert!(Kind::Other.suits(&[1], &[], false));
    }

    #[test]
    fn ip_sans() {
        assert_eq!(ip(&[127, 0, 0, 1]), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(ip(&[0; 16]), Some("::".parse().unwrap()));
        assert_eq!(ip(&[1, 2, 3]), None);
    }
}
