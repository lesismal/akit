//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. What the certificate
//! manager reads from the network and the shared storage. OCSP responses and certificates are
//! akio-x509's to parse, fuzzed in akio (`x509_ocsp`, `x509_parse`).

/// `tls_redis_reply`: see [`crate::shared::fuzz`].
pub fn tls_redis_reply(data: &[u8]) {
    crate::shared::fuzz::redis_reply(data);
}

/// `tls_acme`: see [`crate::acme::fuzz`].
pub fn tls_acme(data: &[u8]) {
    crate::acme::fuzz::acme(data);
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::expect_used,
        clippy::panic
    )]

    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_tls_redis_reply() {
        replay("tls_redis_reply", tls_redis_reply);
    }

    #[test]
    fn corpus_tls_acme() {
        replay("tls_acme", tls_acme);
    }
}
