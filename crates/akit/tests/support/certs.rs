//! Certificates for tests: a CA, and leaf certificates it signs for some names, written as PEM
//! files.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use akio_crypto::pkey::PrivateKey;
use akio_x509::{Certificate, CertificateParams};

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
        // Readable by its owner alone, as a key file should be: akit warns of one that is not
        // (I80b), which would be noise in the tests' output.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&k, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        (c, k)
    }
}

/// A fresh directory under the target's test directory.
pub fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
