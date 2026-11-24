//! JSON Web Tokens as the `jwt` middleware verifies them, as fit's `middleware/auth` `jws.go` and
//! `jwk.go`: a JWS in compact serialization (RFC 7515) whose payload is a JSON object of claims
//! (RFC 7519), signed with RSA (RS256, RS384, RS512, PS256, PS384, PS512), ECDSA (ES256, ES384,
//! ES512), Ed25519 (EdDSA) or HMAC (HS256, HS384, HS512), each verified with `akio-crypto`.
//! `none` is never accepted, nor a header with `crit`, nor an algorithm that does not fit the
//! key: an HMAC key is a secret of the config's, never a public key.
//!
//! The keys come from PEM files (public keys and certificates) and an HMAC secret file, read when
//! the middleware is provisioned, and from a JWK set (RFC 7517), which [`crate::jwks`] fetches.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use akio_crypto::base64::URL_SAFE_NO_PAD as B64;
use akio_crypto::hash::{Hash, Sha256, Sha384, Sha512};
use akio_crypto::mac::Hmac;
use akio_crypto::pkey::PublicKey;
use akio_crypto::rsa::HashAlg;
use akio_crypto::{ed25519, p256, p384, p521, rsa};
use akit_config::value::{self, Value};

/// The largest token read, as fit's `maxTokenBytes`.
pub(crate) const MAX_TOKEN_BYTES: usize = 16 << 10;

/// A signing algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Alg {
    Rs256,
    Rs384,
    Rs512,
    Ps256,
    Ps384,
    Ps512,
    Es256,
    Es384,
    Es512,
    EdDsa,
    Hs256,
    Hs384,
    Hs512,
}

/// What an algorithm signs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// RSA PKCS#1 v1.5.
    Rsa,
    /// RSA-PSS, its salt as long as the hash.
    Pss,
    /// ECDSA, on the curve whose `r` and `s` are this many bytes each.
    Ec(usize),
    Ed25519,
    Hmac,
}

impl Alg {
    /// Every algorithm, in the order fit's `algorithms` enum lists them.
    pub(crate) const ALL: [Alg; 13] = [
        Alg::Rs256,
        Alg::Rs384,
        Alg::Rs512,
        Alg::Ps256,
        Alg::Ps384,
        Alg::Ps512,
        Alg::Es256,
        Alg::Es384,
        Alg::Es512,
        Alg::EdDsa,
        Alg::Hs256,
        Alg::Hs384,
        Alg::Hs512,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Alg::Rs256 => "RS256",
            Alg::Rs384 => "RS384",
            Alg::Rs512 => "RS512",
            Alg::Ps256 => "PS256",
            Alg::Ps384 => "PS384",
            Alg::Ps512 => "PS512",
            Alg::Es256 => "ES256",
            Alg::Es384 => "ES384",
            Alg::Es512 => "ES512",
            Alg::EdDsa => "EdDSA",
            Alg::Hs256 => "HS256",
            Alg::Hs384 => "HS384",
            Alg::Hs512 => "HS512",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Alg> {
        Alg::ALL.into_iter().find(|a| a.name() == name)
    }

    /// Its bit in a set of algorithms.
    pub(crate) fn bit(self) -> u16 {
        1u16 << (self as u16)
    }

    fn kind(self) -> Kind {
        match self {
            Alg::Rs256 | Alg::Rs384 | Alg::Rs512 => Kind::Rsa,
            Alg::Ps256 | Alg::Ps384 | Alg::Ps512 => Kind::Pss,
            Alg::Es256 => Kind::Ec(32),
            Alg::Es384 => Kind::Ec(48),
            Alg::Es512 => Kind::Ec(66),
            Alg::EdDsa => Kind::Ed25519,
            Alg::Hs256 | Alg::Hs384 | Alg::Hs512 => Kind::Hmac,
        }
    }

    fn hash(self) -> HashAlg {
        match self {
            Alg::Rs256 | Alg::Ps256 | Alg::Es256 | Alg::Hs256 | Alg::EdDsa => HashAlg::Sha256,
            Alg::Rs384 | Alg::Ps384 | Alg::Es384 | Alg::Hs384 => HashAlg::Sha384,
            Alg::Rs512 | Alg::Ps512 | Alg::Es512 | Alg::Hs512 => HashAlg::Sha512,
        }
    }
}

/// What a key verifies with.
#[derive(Clone)]
pub(crate) enum Material {
    Rsa(rsa::PublicKey),
    P256(p256::PublicKey),
    P384(p384::PublicKey),
    P521(p521::PublicKey),
    Ed25519(ed25519::VerifyingKey),
    /// An HMAC secret, of the config's.
    Secret(Vec<u8>),
}

/// A key tokens are verified with.
#[derive(Clone)]
pub(crate) struct Key {
    /// `kid`, or empty for none.
    pub(crate) id: String,
    /// The one algorithm it is for, or `None` for any of its type's.
    pub(crate) alg: Option<Alg>,
    pub(crate) material: Material,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.material {
            Material::Rsa(_) => "RSA",
            Material::P256(_) => "P-256",
            Material::P384(_) => "P-384",
            Material::P521(_) => "P-521",
            Material::Ed25519(_) => "Ed25519",
            Material::Secret(_) => "HMAC",
        };
        f.debug_struct("Key")
            .field("id", &self.id)
            .field("alg", &self.alg)
            .field("kind", &kind)
            .finish()
    }
}

impl Key {
    pub(crate) fn new(material: Material) -> Key {
        Key {
            id: String::new(),
            alg: None,
            material,
        }
    }

    /// Whether the key may verify a signature of `alg`.
    pub(crate) fn fits(&self, alg: Alg) -> bool {
        if self.alg.is_some_and(|a| a != alg) {
            return false;
        }
        matches!(
            (&self.material, alg.kind()),
            (Material::Rsa(_), Kind::Rsa | Kind::Pss)
                | (Material::P256(_), Kind::Ec(32))
                | (Material::P384(_), Kind::Ec(48))
                | (Material::P521(_), Kind::Ec(66))
                | (Material::Ed25519(_), Kind::Ed25519)
                | (Material::Secret(_), Kind::Hmac)
        )
    }

    /// Whether `sig` is the key's signature of `signed` by `alg`, which fits it.
    pub(crate) fn verify(&self, alg: Alg, signed: &[u8], sig: &[u8]) -> bool {
        fn ec_hash<H: Hash>(
            verify: impl Fn(&[u8]) -> Result<(), akio_crypto::Error>,
            signed: &[u8],
        ) -> bool {
            verify(H::digest(signed).as_ref()).is_ok()
        }
        let hash = alg.hash();
        match (&self.material, alg.kind()) {
            (Material::Rsa(k), Kind::Rsa) => k.verify_pkcs1v15(hash, signed, sig).is_ok(),
            (Material::Rsa(k), Kind::Pss) => {
                k.verify_pss(hash, signed, sig, hash.output_len()).is_ok()
            }
            (Material::P256(k), Kind::Ec(n)) if sig.len() == n.saturating_mul(2) => {
                p256::Signature::from_fixed(sig).is_ok_and(|s| match alg {
                    Alg::Es256 => ec_hash::<Sha256>(|d| k.verify_prehash(d, &s), signed),
                    _ => false,
                })
            }
            (Material::P384(k), Kind::Ec(n)) if sig.len() == n.saturating_mul(2) => {
                p384::Signature::from_fixed(sig).is_ok_and(|s| match alg {
                    Alg::Es384 => ec_hash::<Sha384>(|d| k.verify_prehash(d, &s), signed),
                    _ => false,
                })
            }
            (Material::P521(k), Kind::Ec(n)) if sig.len() == n.saturating_mul(2) => {
                p521::Signature::from_fixed(sig).is_ok_and(|s| match alg {
                    Alg::Es512 => ec_hash::<Sha512>(|d| k.verify_prehash(d, &s), signed),
                    _ => false,
                })
            }
            (Material::Ed25519(k), Kind::Ed25519) => k.verify(signed, sig).is_ok(),
            (Material::Secret(secret), Kind::Hmac) => {
                // The whole tag, compared in constant time: `Hmac::verify` would take a prefix.
                let ok = |mac: &[u8]| mac.len() == sig.len() && akio_crypto::ct_eq(mac, sig);
                match hash {
                    HashAlg::Sha256 => ok(Hmac::<Sha256>::mac(secret, signed).as_ref()),
                    HashAlg::Sha384 => ok(Hmac::<Sha384>::mac(secret, signed).as_ref()),
                    _ => ok(Hmac::<Sha512>::mac(secret, signed).as_ref()),
                }
            }
            _ => false,
        }
    }
}

/// The generations of key sets: each set has its own, so that what was verified with one set is
/// known not to hold for another (a token the cache keeps).
static GENERATIONS: AtomicU64 = AtomicU64::new(1);

/// The keys a `jwt` middleware verifies with at a time.
#[derive(Debug, Default)]
pub(crate) struct KeySet {
    /// Unique to this set, never 0.
    pub(crate) generation: u64,
    by_id: HashMap<String, Vec<Arc<Key>>>,
    /// Those without a `kid`, tried for every token.
    plain: Vec<Arc<Key>>,
}

impl KeySet {
    pub(crate) fn new<'a>(lists: impl IntoIterator<Item = &'a [Arc<Key>]>) -> KeySet {
        let mut s = KeySet {
            generation: GENERATIONS.fetch_add(1, Ordering::Relaxed),
            ..KeySet::default()
        };
        for k in lists.into_iter().flatten() {
            if k.id.is_empty() {
                s.plain.push(Arc::clone(k));
            } else {
                s.by_id.entry(k.id.clone()).or_default().push(Arc::clone(k));
            }
        }
        s
    }

    /// The keys a token with `kid` may be verified with, and whether a key has that `kid`.
    pub(crate) fn lookup(&self, kid: &str) -> (impl Iterator<Item = &Arc<Key>>, bool) {
        let by_id = if kid.is_empty() {
            None
        } else {
            self.by_id.get(kid)
        };
        let known = kid.is_empty() || by_id.is_some();
        (by_id.into_iter().flatten().chain(&self.plain), known)
    }
}

// ---- tokens ----

/// A JWT taken apart, its signature not yet verified.
#[derive(Debug, Clone)]
pub(crate) struct Token {
    /// The header's `alg`, as written.
    pub(crate) alg: String,
    /// The header's `kid`, or empty.
    pub(crate) kid: String,
    /// The header and payload, as signed.
    pub(crate) signed: String,
    pub(crate) payload: Vec<u8>,
    pub(crate) sig: Vec<u8>,
}

/// Why a token was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Malformed, or no key it names verifies it: fit's `errMalformed`.
    Malformed,
    /// Its algorithm is not allowed: `errAlgorithm`.
    Algorithm,
    /// No key of the set has its `kid`, or there are no keys yet: `errUnknownKey`.
    UnknownKey,
    /// Its claims do not hold: `errClaims`.
    Claims,
}

/// Takes a compact JWS apart, as fit's `parseToken`.
pub(crate) fn parse_token(s: &str) -> Result<Token, Refusal> {
    if s.len() > MAX_TOKEN_BYTES {
        return Err(Refusal::Malformed);
    }
    let (Some(first), Some(last)) = (s.find('.'), s.rfind('.')) else {
        return Err(Refusal::Malformed);
    };
    let (Some(head), Some(body), Some(sig), Some(signed)) = (
        s.get(..first),
        s.get(first.saturating_add(1)..last),
        s.get(last.saturating_add(1)..),
        s.get(..last),
    ) else {
        return Err(Refusal::Malformed);
    };
    if first == last || body.contains('.') {
        return Err(Refusal::Malformed);
    }
    let decode = |part: &str| B64.decode(part).map_err(|_| Refusal::Malformed);
    let header = decode(head)?;
    let payload = decode(body)?;
    let sig = decode(sig)?;
    let header = std::str::from_utf8(&header).map_err(|_| Refusal::Malformed)?;
    let Ok(Value::Object(members)) = value::parse(header) else {
        return Err(Refusal::Malformed);
    };
    let mut t = Token {
        alg: String::new(),
        kid: String::new(),
        signed: signed.to_owned(),
        payload,
        sig,
    };
    for (k, v) in members {
        match (k.as_str(), v) {
            ("alg", Value::String(a)) => t.alg = a,
            ("kid", Value::String(k)) => t.kid = k,
            ("alg" | "kid", Value::Null) => {}
            ("alg" | "kid", _) => return Err(Refusal::Malformed),
            // No extension is understood, so none may be critical.
            ("crit", Value::Null) => {}
            ("crit", _) => return Err(Refusal::Malformed),
            _ => {}
        }
    }
    Ok(t)
}

/// A token's claims: the payload's members.
pub(crate) type Claims = Vec<(String, Value)>;

impl Token {
    /// The payload as claims: a JSON object, numbers kept as written.
    pub(crate) fn claims(&self) -> Result<Claims, Refusal> {
        let text = std::str::from_utf8(&self.payload).map_err(|_| Refusal::Malformed)?;
        match value::parse(text) {
            Ok(Value::Object(members)) => Ok(members),
            _ => Err(Refusal::Malformed),
        }
    }
}

/// The claim `name` of `claims`.
pub(crate) fn claim<'c>(claims: &'c Claims, name: &str) -> Option<&'c Value> {
    claims.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

/// A NumericDate claim, as fit's `numericDate`: a finite JSON number.
pub(crate) fn numeric_date(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

/// `v` as Go's `json.Marshal` writes a decoded value: compact, an object's members sorted by key,
/// numbers as written, strings escaped with HTML escaping on.
pub(crate) fn go_json(v: &Value) -> String {
    let mut out = String::new();
    write_go_json(&mut out, v);
    out
}

fn write_go_json(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::String(s) => write_go_string(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_go_json(out, x);
            }
            out.push(']');
        }
        Value::Object(ms) => {
            let mut sorted: Vec<&(String, Value)> = ms.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            out.push('{');
            for (i, (k, x)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_go_string(out, k);
                out.push(':');
                write_go_json(out, x);
            }
            out.push('}');
        }
    }
}

/// `s` as a JSON string, as Go's `encoding/json` escapes it by default.
fn write_go_string(out: &mut String, s: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---- keys from files ----

/// The public keys and certificates of a PEM file, as fit's `parsePublicKeys`: `PUBLIC KEY`,
/// `RSA PUBLIC KEY` and `CERTIFICATE` blocks, others skipped.
pub(crate) fn parse_public_keys(data: &[u8]) -> Result<Vec<Arc<Key>>, String> {
    let blocks =
        akio_crypto::pem::decode(data).map_err(|e| format!("a malformed PEM block: {e}"))?;
    let mut keys = Vec::new();
    for (label, der) in blocks {
        let public = match label.as_str() {
            "PUBLIC KEY" => PublicKey::from_spki_der(&der).map_err(|e| e.to_string())?,
            "RSA PUBLIC KEY" => rsa::PublicKey::from_pkcs1_der(&der)
                .map(PublicKey::Rsa)
                .map_err(|e| e.to_string())?,
            "CERTIFICATE" => akio_x509::Certificate::from_der(der)
                .and_then(|c| c.public_key())
                .map_err(|e| e.to_string())?,
            _ => continue,
        };
        let material = match public {
            PublicKey::Rsa(k) => Material::Rsa(k),
            PublicKey::P256(k) => Material::P256(k),
            PublicKey::P384(k) => Material::P384(k),
            PublicKey::P521(k) => Material::P521(k),
            PublicKey::Ed25519(k) => Material::Ed25519(k),
            _ => return Err(format!("a {label} of this type cannot verify a token")),
        };
        keys.push(Arc::new(Key::new(material)));
    }
    if keys.is_empty() {
        return Err("no PEM public key or certificate found".into());
    }
    Ok(keys)
}

// ---- JWK sets ----

/// The keys of a JWK set that can verify a signature, as fit's `parseJWKS`: keys for
/// encryption, of another type, for an HMAC algorithm or that do not parse are left out, as a
/// set may hold keys for others; a set with none left is an error.
pub(crate) fn parse_jwks(data: &[u8]) -> Result<Vec<Arc<Key>>, String> {
    let text = std::str::from_utf8(data).map_err(|_| "the JWK set is not UTF-8".to_owned())?;
    let set = value::parse(text).map_err(|e| e.to_string())?;
    let Value::Object(_) = set else {
        return Err("the JWK set is not a JSON object".into());
    };
    let list = match set.get("keys") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(a)) => a.as_slice(),
        Some(_) => return Err("the JWK set's keys are not an array".into()),
    };
    let mut keys = Vec::new();
    for j in list {
        let field = |name: &str| -> Result<&str, String> {
            match j.get(name) {
                None | Some(Value::Null) => Ok(""),
                Some(Value::String(s)) => Ok(s),
                Some(_) => Err(format!("a JWK's {name} is not a string")),
            }
        };
        if !matches!(j, Value::Object(_) | Value::Null) {
            return Err("a JWK is not a JSON object".into());
        }
        let (kty, kid, usage, alg) = (field("kty")?, field("kid")?, field("use")?, field("alg")?);
        let (n, e, crv, x, y) = (
            field("n")?,
            field("e")?,
            field("crv")?,
            field("x")?,
            field("y")?,
        );
        if !usage.is_empty() && usage != "sig" {
            continue;
        }
        let alg = if alg.is_empty() {
            None
        } else {
            match Alg::from_name(alg) {
                Some(a) if a.kind() != Kind::Hmac => Some(a),
                _ => continue,
            }
        };
        if let Some(material) = jwk_public(kty, n, e, crv, x, y) {
            keys.push(Arc::new(Key {
                id: kid.to_owned(),
                alg,
                material,
            }));
        }
    }
    if keys.is_empty() {
        return Err("the JWK set has no key that verifies signatures".into());
    }
    Ok(keys)
}

/// A JWK's public key, as fit's `jwk.public`, or `None`.
fn jwk_public(kty: &str, n: &str, e: &str, crv: &str, x: &str, y: &str) -> Option<Material> {
    match kty {
        "RSA" => {
            let (n, e) = (B64.decode(n).ok()?, B64.decode(e).ok()?);
            // RSA keys below 2048 bits are refused.
            if n.len() < 256 || e.is_empty() || e.len() > 4 {
                return None;
            }
            rsa::PublicKey::from_components(&n, &e)
                .ok()
                .map(Material::Rsa)
        }
        "EC" => {
            let size = match crv {
                "P-256" => 32,
                "P-384" => 48,
                "P-521" => 66,
                _ => return None,
            };
            let (x, y) = (B64.decode(x).ok()?, B64.decode(y).ok()?);
            if x.len() != size || y.len() != size {
                return None;
            }
            let mut point = Vec::with_capacity(size.saturating_mul(2).saturating_add(1));
            point.push(4);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            // A point not on the curve is refused.
            match size {
                32 => p256::PublicKey::from_sec1(&point).ok().map(Material::P256),
                48 => p384::PublicKey::from_sec1(&point).ok().map(Material::P384),
                _ => p521::PublicKey::from_sec1(&point).ok().map(Material::P521),
            }
        }
        "OKP" => {
            let x = B64.decode(x).ok()?;
            if crv != "Ed25519" || x.len() != 32 {
                return None;
            }
            ed25519::VerifyingKey::from_bytes(&x)
                .ok()
                .map(Material::Ed25519)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    // fit's TestKeySetLookup.
    #[test]
    fn key_set_lookup() {
        let secret = |id: &str| {
            Arc::new(Key {
                id: id.to_owned(),
                alg: None,
                material: Material::Secret(vec![0; 32]),
            })
        };
        let s = KeySet::new([&[secret("a"), secret("")][..]]);
        let (keys, known) = s.lookup("a");
        assert_eq!((keys.count(), known), (2, true));
        let (keys, known) = s.lookup("b");
        assert_eq!((keys.count(), known), (1, false));
        let (keys, known) = s.lookup("");
        assert_eq!((keys.count(), known), (1, true));
        assert_ne!(s.generation, KeySet::new([]).generation);
    }

    // Part of fit's TestJWS.
    #[test]
    fn tokens_taken_apart() {
        let enc = |s: &str| B64.encode(s);
        let h = enc(r#"{"alg":"HS256","kid":"k"}"#);
        let t = parse_token(&format!("{h}.e30.AA")).unwrap();
        assert_eq!((t.alg.as_str(), t.kid.as_str()), ("HS256", "k"));
        assert_eq!(t.payload, b"{}");
        assert_eq!(t.sig, [0]);
        assert_eq!(t.signed, format!("{h}.e30"));
        for bad in [
            "!!.e30.AA".to_owned(),
            format!("{h}.!!.AA"),
            format!("{h}.e30.!!"),
            format!("{h}.e30"),
            format!("{h}.e30.AA.AA"),
            format!("{}.e30.AA", enc("{")),
            format!("{}.e30.AA", enc(r#"{"alg":5}"#)),
            format!("{}.e30.AA", enc(r#"{"alg":"HS256","crit":["exp"]}"#)),
            format!("{}.e30.AA", enc("[]")),
            format!("{}.e30.AA", "a".repeat(MAX_TOKEN_BYTES)),
        ] {
            assert_eq!(parse_token(&bad).unwrap_err(), Refusal::Malformed, "{bad}");
        }
        for payload in ["null", "{} {}", "[]", "[1]"] {
            let t = Token {
                alg: String::new(),
                kid: String::new(),
                signed: String::new(),
                payload: payload.as_bytes().to_vec(),
                sig: Vec::new(),
            };
            assert!(t.claims().is_err(), "{payload}");
        }
    }

    #[test]
    fn algorithms_named() {
        for a in Alg::ALL {
            assert_eq!(Alg::from_name(a.name()), Some(a));
        }
        assert_eq!(Alg::from_name("none"), None);
        assert_eq!(Alg::from_name("hs256"), None);
        let names: Vec<&str> = Alg::ALL.iter().map(|a| a.name()).collect();
        assert_eq!(names, akit_config::JWT_ALGORITHMS);
    }

    // Part of fit's TestJWS: keys of another alg, of the wrong size and signatures of the wrong
    // length.
    #[test]
    fn keys_fit_their_algorithms() {
        let ec = p256::SecretKey::generate().unwrap();
        let k = Key::new(Material::P256(ec.public_key()));
        assert!(k.fits(Alg::Es256));
        assert!(!k.fits(Alg::Es384) && !k.fits(Alg::Rs256) && !k.fits(Alg::Hs256));
        assert!(!k.verify(Alg::Es256, b"x", b"short"));
        let sig = ec.sign(b"x").to_fixed();
        assert!(k.verify(Alg::Es256, b"x", &sig));
        assert!(!k.verify(Alg::Es256, b"y", &sig));
        let pinned = Key {
            alg: Some(Alg::Es256),
            ..k.clone()
        };
        assert!(pinned.fits(Alg::Es256));
        let secret = Key::new(Material::Secret(
            b"0123456789abcdef0123456789abcdef".to_vec(),
        ));
        assert!(secret.fits(Alg::Hs384) && !secret.fits(Alg::EdDsa));
        let mac = Hmac::<Sha256>::mac(b"0123456789abcdef0123456789abcdef", b"x");
        assert!(secret.verify(Alg::Hs256, b"x", mac.as_ref()));
        assert!(
            !secret.verify(Alg::Hs256, b"x", &mac.as_ref()[..16]),
            "truncated"
        );
    }

    #[test]
    fn claims_written_as_go_writes_them() {
        let input = "{\"b\":[1,\"x<y>&\u{2028}\"],\"a\":{\"z\":null,\"y\":true},\"n\":1.50}";
        let v = value::parse(input).unwrap();
        let want = "{\"a\":{\"y\":true,\"z\":null},\"b\":[1,\"x\\u003cy\\u003e\\u0026\\u2028\"],\"n\":1.50}";
        assert_eq!(go_json(&v), want);
        assert_eq!(
            go_json(&Value::String("a\"\\\n\u{1}\u{8}".into())),
            "\"a\\\"\\\\\\n\\u0001\\b\""
        );
    }

    #[test]
    fn numeric_dates() {
        let n = |s: &str| Value::Number(s.into());
        assert_eq!(numeric_date(Some(&n("1700000000"))), Some(1.7e9));
        assert_eq!(numeric_date(Some(&n("1.5"))), Some(1.5));
        assert_eq!(numeric_date(Some(&n("1e400"))), None);
        assert_eq!(numeric_date(Some(&Value::String("1".into()))), None);
        assert_eq!(numeric_date(None), None);
    }

    // fit's TestJWKPublic.
    #[test]
    fn jwk_public_keys() {
        let ec = p384::SecretKey::generate().unwrap().public_key();
        let point = ec.to_uncompressed();
        let (x, y) = (B64.encode(&point[1..49]), B64.encode(&point[49..]));
        assert!(jwk_public("EC", "", "", "P-384", &x, &y).is_some());
        assert!(
            jwk_public("EC", "", "", "P-192", &x, &y).is_none(),
            "another curve"
        );
        assert!(
            jwk_public("EC", "", "", "P-384", "AA", &y).is_none(),
            "a short point"
        );
        assert!(
            jwk_public("EC", "", "", "P-384", &x, &x).is_none(),
            "off the curve"
        );
        let ed = ed25519::SigningKey::generate().unwrap().verifying_key();
        let ex = B64.encode(ed.as_bytes());
        assert!(jwk_public("OKP", "", "", "Ed25519", &ex, "").is_some());
        assert!(jwk_public("OKP", "", "", "X25519", &ex, "").is_none());
        let n = B64.encode([0xc5u8; 256]);
        assert!(jwk_public("RSA", &n, "AQAB", "", "", "").is_some());
        assert!(
            jwk_public("RSA", &n, &B64.encode([4]), "", "", "").is_none(),
            "even"
        );
        assert!(jwk_public("RSA", &B64.encode([0xc5u8; 128]), "AQAB", "", "", "").is_none());
        assert!(jwk_public("", "", "", "", "", "").is_none(), "no type");
    }

    #[test]
    fn jwk_sets_parsed() {
        let ed = ed25519::SigningKey::generate().unwrap().verifying_key();
        let ex = B64.encode(ed.as_bytes());
        let n = B64.encode([0xc5u8; 256]);
        let set = format!(
            r#"{{"keys": [
              {{"kty": "OKP", "kid": "ed", "crv": "Ed25519", "x": "{ex}"}},
              {{"kty": "RSA", "kid": "rsa", "n": "{n}", "e": "AQAB", "alg": "RS256", "use": "sig"}},
              {{"kty": "RSA", "kid": "enc", "use": "enc", "n": "{n}", "e": "AQAB"}},
              {{"kty": "RSA", "kid": "hs", "alg": "HS256", "n": "{n}", "e": "AQAB"}},
              {{"kty": "RSA", "kid": "other", "alg": "XX1", "n": "{n}", "e": "AQAB"}},
              {{"kty": "oct", "kid": "secret", "k": "c2VjcmV0"}}
            ]}}"#
        );
        let keys = parse_jwks(set.as_bytes()).unwrap();
        let ids: Vec<&str> = keys.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(ids, ["ed", "rsa"]);
        assert_eq!(keys[1].alg, Some(Alg::Rs256));
        for bad in [
            r#"{"keys": []}"#,
            r#"{"keys": [{"kty": "oct", "k": "AA"}]}"#,
            "[",
            "[]",
            r#"{"keys": 5}"#,
            r#"{"keys": [{"kty": 5}]}"#,
            r#"{"keys": [5]}"#,
        ] {
            assert!(parse_jwks(bad.as_bytes()).is_err(), "{bad}");
        }
    }
}
