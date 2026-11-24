//! JSON Web Signatures for ACME (RFC 8555 §6.2, RFC 7515): an account's P-256 key, its JWK and
//! thumbprint (RFC 7638), and requests signed with ES256 in the flattened JSON serialization.
//! akio-crypto has the pieces (akio API request A6): ECDSA with the fixed-width `r || s`
//! signature JWS uses, SHA-256 and base64url.

use akio_crypto::base64::URL_SAFE_NO_PAD;
use akio_crypto::hash::Sha256;
use akio_crypto::pkey::{PrivateKey, PublicKey, SignatureAlgorithm};
use akit_config::value::write_string;

/// An account's key: P-256, signing with ES256.
pub(crate) struct AccountKey {
    key: PrivateKey,
    /// The public key as a JWK, its members in the order RFC 7638 hashes them.
    jwk: String,
    /// The base64url SHA-256 of `jwk` (RFC 7638), which key authorizations end with.
    thumbprint: String,
}

impl std::fmt::Debug for AccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountKey")
            .field("thumbprint", &self.thumbprint)
            .finish_non_exhaustive()
    }
}

impl AccountKey {
    /// A new P-256 key.
    pub(crate) fn generate() -> Result<AccountKey, String> {
        let k = akio_crypto::p256::SecretKey::generate().map_err(|e| e.to_string())?;
        AccountKey::new(PrivateKey::P256(k))
    }

    /// The key in a PKCS#8 PEM, as [`to_pem`](Self::to_pem) writes it.
    pub(crate) fn from_pem(pem: &[u8]) -> Result<AccountKey, String> {
        AccountKey::new(PrivateKey::from_pem(pem).map_err(|e| e.to_string())?)
    }

    fn new(key: PrivateKey) -> Result<AccountKey, String> {
        let PublicKey::P256(public) = key.public_key() else {
            return Err("the account key is not a P-256 key".into());
        };
        // The uncompressed SEC1 point: 0x04, then x and y, 32 bytes each.
        let point = public.to_uncompressed();
        let (Some(x), Some(y)) = (point.get(1..33), point.get(33..65)) else {
            return Err("the account key's point is not 65 bytes".into());
        };
        let jwk = format!(
            r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
            URL_SAFE_NO_PAD.encode(x),
            URL_SAFE_NO_PAD.encode(y)
        );
        let thumbprint = URL_SAFE_NO_PAD.encode(Sha256::digest(jwk.as_bytes()));
        Ok(AccountKey {
            key,
            jwk,
            thumbprint,
        })
    }

    /// The key as a PKCS#8 PEM.
    pub(crate) fn to_pem(&self) -> String {
        akio_crypto::pem::encode("PRIVATE KEY", &self.key.to_pkcs8_der())
    }

    /// The key authorization of a challenge's `token` (RFC 8555 §8.1): what HTTP-01 answers.
    pub(crate) fn key_authorization(&self, token: &str) -> String {
        format!("{token}.{}", self.thumbprint)
    }

    /// The body of a POST to `url`: `payload` (JSON, or empty for a POST-as-GET) signed, with
    /// `nonce`, and the account's URL (`kid`) or, before there is one, its JWK.
    pub(crate) fn sign(
        &self,
        url: &str,
        nonce: &str,
        kid: Option<&str>,
        payload: &str,
    ) -> Result<String, String> {
        let mut protected = String::from(r#"{"alg":"ES256","#);
        match kid {
            Some(kid) => {
                protected.push_str(r#""kid":"#);
                write_string(&mut protected, kid);
            }
            None => {
                protected.push_str(r#""jwk":"#);
                protected.push_str(&self.jwk);
            }
        }
        protected.push_str(r#","nonce":"#);
        write_string(&mut protected, nonce);
        protected.push_str(r#","url":"#);
        write_string(&mut protected, url);
        protected.push('}');
        let protected = URL_SAFE_NO_PAD.encode(protected.as_bytes());
        let payload = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        let input = format!("{protected}.{payload}");
        let der = self
            .key
            .sign(SignatureAlgorithm::EcdsaSha256, input.as_bytes())
            .map_err(|e| e.to_string())?;
        let sig = akio_crypto::p256::Signature::from_der(&der).map_err(|e| e.to_string())?;
        Ok(format!(
            r#"{{"protected":"{protected}","payload":"{payload}","signature":"{}"}}"#,
            URL_SAFE_NO_PAD.encode(sig.to_fixed())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akit_config::value::{Value, parse};

    fn field<'a>(v: &'a Value, k: &str) -> &'a str {
        match v.get(k) {
            Some(Value::String(s)) => s,
            other => panic!("{k}: {other:?}"),
        }
    }

    #[test]
    fn signs_what_a_ca_verifies() {
        let key = AccountKey::generate().unwrap();
        let again = AccountKey::from_pem(key.to_pem().as_bytes()).unwrap();
        assert_eq!(again.thumbprint, key.thumbprint);
        assert_eq!(key.thumbprint.len(), 43);
        assert!(key.key_authorization("tok").starts_with("tok."));

        let body = key
            .sign("https://ca/new-acct", "n0nce", None, r#"{"a":1}"#)
            .unwrap();
        let v = parse(&body).unwrap();
        let protected = URL_SAFE_NO_PAD.decode(field(&v, "protected")).unwrap();
        let header = parse(std::str::from_utf8(&protected).unwrap()).unwrap();
        assert_eq!(field(&header, "alg"), "ES256");
        assert_eq!(field(&header, "nonce"), "n0nce");
        assert_eq!(field(&header, "url"), "https://ca/new-acct");
        assert!(header.get("jwk").is_some() && header.get("kid").is_none());
        let payload = URL_SAFE_NO_PAD.decode(field(&v, "payload")).unwrap();
        assert_eq!(payload, br#"{"a":1}"#);
        // The signature verifies with the public key, as the CA checks it.
        let sig = URL_SAFE_NO_PAD.decode(field(&v, "signature")).unwrap();
        assert_eq!(sig.len(), 64);
        let der = akio_crypto::p256::Signature::from_fixed(&sig)
            .unwrap()
            .to_der();
        let input = format!("{}.{}", field(&v, "protected"), field(&v, "payload"));
        key.key
            .public_key()
            .verify(SignatureAlgorithm::EcdsaSha256, input.as_bytes(), &der)
            .unwrap();

        // With an account URL, a POST-as-GET.
        let body = key
            .sign("https://ca/x", "n", Some("https://ca/acct/1"), "")
            .unwrap();
        let v = parse(&body).unwrap();
        assert_eq!(field(&v, "payload"), "");
        let protected = URL_SAFE_NO_PAD.decode(field(&v, "protected")).unwrap();
        let header = parse(std::str::from_utf8(&protected).unwrap()).unwrap();
        assert_eq!(field(&header, "kid"), "https://ca/acct/1");
        assert!(header.get("jwk").is_none());
    }

    #[test]
    fn thumbprint_of_rfc_7638s_kind() {
        // The thumbprint is the SHA-256 of the JWK's required members in lexical order, with no
        // white space (RFC 7638 §3).
        let key = AccountKey::generate().unwrap();
        assert!(key.jwk.starts_with(r#"{"crv":"P-256","kty":"EC","x":""#));
        assert_eq!(
            key.thumbprint,
            URL_SAFE_NO_PAD.encode(Sha256::digest(key.jwk.as_bytes()))
        );
    }
}
