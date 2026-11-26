//! A role listener's self-signed certificate (fit's `certpin.SelfSigned` and `Keep`): made once,
//! kept beside the config, mode 0600, so that its fingerprint, which peers pin, outlives a
//! restart.

/// A self-signed certificate for `hosts`, valid ten years: its PEM and its key's PEM.
///
/// # Errors
///
/// No random bytes, or a host that is not one.
pub fn self_signed(hosts: &[String]) -> Result<(String, String), String> {
    let key = akio_crypto::p256::SecretKey::generate().map_err(|e| e.to_string())?;
    let key = akio_crypto::pkey::PrivateKey::P256(key);
    let hosts: Vec<&str> = hosts
        .iter()
        .map(String::as_str)
        .filter(|h| !h.is_empty())
        .collect();
    let mut p = akio_x509::CertificateParams::new(if hosts.is_empty() {
        vec!["localhost"]
    } else {
        hosts
    });
    let now = akit_roles::linktoken::unix_now();
    p.not_before = now.saturating_sub(3600);
    p.not_after = now.saturating_add(10 * 365 * 86_400);
    let cert = p.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((
        cert.to_pem(),
        akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
    ))
}

/// Splits a PEM file into its certificates and its key.
#[must_use]
pub fn split_pem(data: &str) -> (String, String) {
    let (mut cert, mut key, mut cur) = (String::new(), String::new(), String::new());
    let mut is_cert = false;
    for line in data.lines() {
        if line.starts_with("-----BEGIN ") {
            cur.clear();
            is_cert = line.contains("CERTIFICATE");
        }
        cur.push_str(line);
        cur.push('\n');
        if line.starts_with("-----END ") {
            if is_cert {
                cert.push_str(&cur);
            } else {
                key.push_str(&cur);
            }
            cur.clear();
        }
    }
    (cert, key)
}

/// The certificate and key kept at `path`, or a new self-signed one for `hosts` written there,
/// mode 0600; and whether it was made. Reads and writes a file: the control thread's.
///
/// # Errors
///
/// A file that holds no certificate and key, or that cannot be read or written.
pub fn keep(path: &str, hosts: &[String]) -> Result<(String, String, bool), String> {
    match std::fs::read_to_string(path) {
        Ok(data) => {
            let (cert, key) = split_pem(&data);
            if akio::tls::Certificate::from_pem(cert.as_bytes()).map_or(true, |c| c.is_empty())
                || akio::tls::PrivateKey::from_pem(key.as_bytes()).is_err()
            {
                return Err(format!("{path} does not hold a certificate and its key"));
            }
            Ok((cert, key, false))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (cert, key) = self_signed(hosts)?;
            write_private(path, &format!("{cert}{key}")).map_err(|e| format!("{path}: {e}"))?;
            Ok((cert, key, true))
        }
        Err(e) => Err(format!("{path}: {e}")),
    }
}

fn write_private(path: &str, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(data.as_bytes())?;
    f.sync_all()
}
