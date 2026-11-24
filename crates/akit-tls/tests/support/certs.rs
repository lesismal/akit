//! Certificates for tests: a CA, and leaf certificates it signs for some names, written as PEM
//! files.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use akio_crypto::pkey::PrivateKey;
use akio_x509::{Certificate, CertificateParams, Extension, ext};

/// A certificate authority that signs leaf certificates.
pub struct Ca {
    pub cert: Certificate,
    key: PrivateKey,
}

fn new_key() -> PrivateKey {
    PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap())
}

impl Ca {
    pub fn new(name: &str) -> Ca {
        let key = new_key();
        let cert = CertificateParams::ca(name).self_signed(&key).unwrap();
        Ca { cert, key }
    }

    /// Writes the CA's certificate to `dir/<file>`.
    pub fn write(&self, dir: &Path, file: &str) -> PathBuf {
        let p = dir.join(file);
        std::fs::write(&p, self.cert.to_pem()).unwrap();
        p
    }

    /// A leaf for `names`, written to `dir/<stem>.pem` and `dir/<stem>.key`.
    pub fn leaf(&self, dir: &Path, stem: &str, names: &[&str]) -> (PathBuf, PathBuf) {
        let key = new_key();
        let cert = CertificateParams::new(names)
            .signed_by(&key.public_key(), &self.cert, &self.key)
            .unwrap();
        let (c, k) = (
            dir.join(format!("{stem}.pem")),
            dir.join(format!("{stem}.key")),
        );
        std::fs::write(&c, cert.to_pem()).unwrap();
        std::fs::write(&k, key.to_pkcs8_pem()).unwrap();
        (c, k)
    }
}

impl Ca {
    /// A certificate made from `p` for `key`, signed by this CA.
    pub fn sign(&self, p: &CertificateParams, key: &PrivateKey) -> Certificate {
        p.signed_by(&key.public_key(), &self.cert, &self.key)
            .unwrap()
    }

    /// Its private key, to sign OCSP responses with.
    pub fn key(&self) -> &PrivateKey {
        &self.key
    }

    /// A leaf for `names` that names `responder` as its OCSP responder (authorityInfoAccess),
    /// written with this CA's certificate after it to `dir/<stem>.pem`, its key to
    /// `dir/<stem>.key`; and the leaf.
    pub fn leaf_with_ocsp(
        &self,
        dir: &Path,
        stem: &str,
        names: &[&str],
        responder: &str,
    ) -> (PathBuf, PathBuf, Certificate) {
        let key = new_key();
        let mut p = CertificateParams::new(names);
        p.custom_extensions.push(aia_ocsp(responder));
        let cert = p
            .signed_by(&key.public_key(), &self.cert, &self.key)
            .unwrap();
        let (c, k) = (
            dir.join(format!("{stem}.pem")),
            dir.join(format!("{stem}.key")),
        );
        std::fs::write(&c, format!("{}{}", cert.to_pem(), self.cert.to_pem())).unwrap();
        std::fs::write(&k, key.to_pkcs8_pem()).unwrap();
        (c, k, cert)
    }
}

/// DER: `tag`, a short or long length, `content`.
fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    let n = content.len();
    if n < 0x80 {
        v.push(u8::try_from(n).unwrap());
    } else if n < 0x100 {
        v.extend_from_slice(&[0x81, u8::try_from(n).unwrap()]);
    } else {
        v.extend_from_slice(&[0x82]);
        v.extend_from_slice(&u16::try_from(n).unwrap().to_be_bytes());
    }
    v.extend_from_slice(content);
    v
}

/// authorityInfoAccess with one OCSP access description: `url`.
fn aia_ocsp(url: &str) -> Extension {
    let mut ad = tlv(0x06, ext::AD_OCSP);
    ad.extend_from_slice(&tlv(0x86, url.as_bytes()));
    Extension {
        oid: ext::AUTHORITY_INFO_ACCESS.to_vec(),
        critical: false,
        value: tlv(0x30, &tlv(0x30, &ad)),
    }
}

/// A fresh directory under the target's test directory.
pub fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
