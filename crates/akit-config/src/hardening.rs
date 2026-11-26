//! What a config exposes that akit warns of, and runs anyway (I80b; the threat model's L3 and L4,
//! [decision 035](../../../docs/decisions/035-admin-api-redacts-secrets.md)):
//!
//! - **Secret files open to others** ([`secret_files`]): a private key, token or password file
//!   the config names (`key_file`, `tls_key_file`, `token_file`, `password_file`,
//!   `hmac_secret_file`, a provider's `kubeconfig`) that its group or other users may read or
//!   write. fit reads them as they are, and so does akit; the warning says which to `chmod 600`.
//! - **Credentials and keys in the clear off loopback** ([`cleartext`]): an `http://` ACME
//!   directory; a shared storage (which holds the certificates' private keys) reached without TLS;
//!   a Consul token or an etcd password sent over `http://`; Docker's API over `tcp://`. Each is
//!   allowed, as in fit, when the address is a loopback one or a Unix socket.
//!
//! Neither refuses anything: a config fit runs, akit runs. Reading a file's metadata blocks: this
//! is control plane code (rules.md rule 1).

use std::net::IpAddr;

use crate::value::Value;
use crate::{Config, DockerEndpoint, ProviderKind, SharedStorageKind};

/// The warnings `config`, which `json` is as standard JSON, gets: [`secret_files`], then
/// [`cleartext`].
#[must_use]
pub fn warnings(config: &Config, json: &str) -> Vec<String> {
    let mut w = secret_files(json);
    w.extend(cleartext(config));
    w
}

/// The members whose values name files holding secrets.
const SECRET_FILES: &[&str] = &[
    "key_file",
    "tls_key_file",
    "token_file",
    "password_file",
    "hmac_secret_file",
    "kubeconfig",
];

/// A warning for each file a member of [`SECRET_FILES`] names in `json` that its group or others
/// may read or write, once a file, at the first place that names it. A file that cannot be
/// read is left for the config's own checks to report. Nothing on a system without Unix modes.
#[must_use]
pub fn secret_files(json: &str) -> Vec<String> {
    let Ok(root) = crate::value::parse(json) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = Vec::new();
    collect(&root, "", false, &mut found);
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for (path, file) in &found {
        if seen.contains(&file.as_str()) {
            continue;
        }
        seen.push(file);
        if let Some(mode) = open_to_others(file) {
            out.push(format!(
                "{path}: {file} holds a secret and is open to its group or others \
                 (mode {mode:04o}); make it readable by akit's user alone (chmod 600)"
            ));
        }
    }
    out
}

/// The paths and values of every string under a member of [`SECRET_FILES`] in `v`.
fn collect(v: &Value, path: &str, secret: bool, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) if secret && !s.is_empty() => out.push((path.to_owned(), s.clone())),
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                collect(x, &format!("{path}[{i}]"), secret, out);
            }
        }
        Value::Object(ms) => {
            for (k, x) in ms {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                collect(x, &p, SECRET_FILES.contains(&k.as_str()), out);
            }
        }
        _ => {}
    }
}

/// The permission bits of `file` when its group or others have any.
#[cfg(unix)]
fn open_to_others(file: &str) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(file).ok()?.permissions().mode() & 0o7777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
fn open_to_others(_file: &str) -> Option<u32> {
    None
}

/// The host of `url` (`scheme://[user@]host[:port][/...]`, or `host[:port]`), without brackets.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(r) = hostport.strip_prefix('[') {
        return r.split_once(']').map_or(r, |(h, _)| h);
    }
    hostport.rsplit_once(':').map_or(hostport, |(h, _)| h)
}

/// Whether `url`'s host is `localhost` or a loopback address: what reaches no network.
#[must_use]
pub fn loopback(url: &str) -> bool {
    let host = host_of(url);
    host.eq_ignore_ascii_case("localhost")
        || host
            .get(host.len().saturating_sub(10)..)
            .is_some_and(|t| t.eq_ignore_ascii_case(".localhost"))
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether `url` is plain HTTP to a host off loopback.
fn http_off_loopback(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
        && !loopback(url)
}

/// A warning for each credential, key or certificate `config` sends in the clear to an address
/// off loopback.
#[must_use]
pub fn cleartext(config: &Config) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(acme) = &config.acme {
        if http_off_loopback(&acme.directory) {
            out.push(format!(
                "acme.directory {} is plain HTTP off loopback: the account's requests and the \
                 certificates it obtains cross the network in the clear; use https://",
                acme.directory
            ));
        }
        if let Some(s) = &acme.shared_storage {
            let keys = "the certificates' private keys it stores";
            match &s.kind {
                SharedStorageKind::Redis(r) if !r.tls && !loopback(&r.address) => {
                    let what = if r.password_file.is_empty() {
                        keys.to_owned()
                    } else {
                        format!("its password and {keys}")
                    };
                    out.push(format!(
                        "acme.shared_storage.redis at {} without tls: {what} cross the network \
                         in the clear; set tls",
                        r.address
                    ));
                }
                SharedStorageKind::Etcd(e) => {
                    for ep in e.endpoints.iter().filter(|ep| http_off_loopback(ep)) {
                        let what = if e.password_file.is_empty() {
                            keys.to_owned()
                        } else {
                            format!("its password and {keys}")
                        };
                        out.push(format!(
                            "acme.shared_storage.etcd endpoint {ep}: {what} cross the network in \
                             the clear; use https://"
                        ));
                    }
                }
                SharedStorageKind::Consul(c) if http_off_loopback(&c.endpoint) => {
                    let what = if c.token_file.is_empty() {
                        keys.to_owned()
                    } else {
                        format!("its token and {keys}")
                    };
                    out.push(format!(
                        "acme.shared_storage.consul endpoint {}: {what} cross the network in \
                         the clear; use https://",
                        c.endpoint
                    ));
                }
                _ => {}
            }
        }
    }
    for (i, p) in config.providers.iter().enumerate() {
        let at = format!("providers[{i}].{}", p.kind.name());
        match &p.kind {
            ProviderKind::Docker(d) => {
                if let DockerEndpoint::Tcp(addr) = &d.endpoint {
                    if !loopback(addr) {
                        out.push(format!(
                            "{at}.endpoint tcp://{addr}: Docker's API, which decides what akit \
                             serves, is read over plain HTTP off loopback, unauthenticated"
                        ));
                    }
                }
            }
            ProviderKind::Consul(c) => consul(&mut out, &at, &c.conn),
            ProviderKind::ConsulKv(c) => consul(&mut out, &at, &c.conn),
            ProviderKind::Etcd(e) if !e.password_file.is_empty() => {
                for ep in e.endpoints.iter().filter(|ep| http_off_loopback(ep)) {
                    out.push(format!(
                        "{at} endpoint {ep}: its password is sent in the clear; use https://"
                    ));
                }
            }
            _ => {}
        }
    }
    out
}

fn consul(out: &mut Vec<String>, at: &str, c: &crate::ConsulConn) {
    if !c.token_file.is_empty() && http_off_loopback(&c.endpoint) {
        out.push(format!(
            "{at}.endpoint {}: its token is sent in the clear; use https://",
            c.endpoint
        ));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn loopback_hosts() {
        for u in [
            "http://localhost:8500",
            "http://127.0.0.1:2379",
            "http://[::1]:2379/x",
            "127.0.0.2:6379",
            "localhost",
            "http://user:pw@127.0.0.1/",
            "http://a.localhost:1",
        ] {
            assert!(loopback(u), "{u}");
        }
        for u in [
            "http://10.0.0.1:8500",
            "http://consul.internal",
            "redis.example.com:6379",
            "http://[2001:db8::1]:1",
            "http://127.0.0.1.example.com/",
            "http://localhost@evil.example.com/",
        ] {
            assert!(!loopback(u), "{u}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_open_to_others() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("akit-hardening-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, mode: u32| {
            let p = dir.join(name);
            std::fs::write(&p, "x").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p.to_str().unwrap().to_owned()
        };
        let (open, group, private, cert) = (
            file("open.key", 0o644),
            file("group.token", 0o640),
            file("private.key", 0o600),
            file("cert.pem", 0o644),
        );
        let json = format!(
            r#"{{"listeners": [{{"tls": {{"certificates": [
                {{"cert_file": "{cert}", "key_file": "{open}"}},
                {{"cert_file": "{cert}", "key_file": "{private}"}},
                {{"cert_file": "{cert}", "key_file": "{open}"}}]}}}}],
              "admin": {{"token_file": "{group}"}},
              "providers": [{{"etcd": {{"password_file": "/nonexistent/akit-hardening"}}}}]}}"#
        );
        let w = secret_files(&json);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[0].starts_with(&format!(
                "listeners[0].tls.certificates[0].key_file: {open} holds a secret and is open to \
                 its group or others (mode 0644)"
            )),
            "{w:?}"
        );
        assert!(
            w[1].starts_with(&format!("admin.token_file: {group} ")) && w[1].contains("0640"),
            "{w:?}"
        );
        assert!(secret_files("not json").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn credentials_in_the_clear() {
        let warned = |doc: &str| cleartext(&crate::from_json(doc, None).unwrap());
        // Loopback, TLS and https are quiet, as are the defaults.
        for doc in [
            r#"{"acme": {"directory": "http://127.0.0.1:14000/dir"}}"#,
            r#"{"acme": {"directory": "https://ca.example.com/dir"}}"#,
            r#"{"acme": {"shared_storage": {"redis": {"address": "127.0.0.1:6379", "password_file": "p"}}}}"#,
            r#"{"acme": {"shared_storage": {"redis": {"address": "redis.example.com:6379", "tls": true}}}}"#,
            r#"{"acme": {"shared_storage": {"etcd": {"endpoints": ["https://etcd.example.com:2379"]}}}}"#,
            r#"{"providers": [{"docker": {}}, {"consul": {}}, {"etcd": {}}]}"#,
            r#"{"providers": [{"docker": {"endpoint": "tcp://127.0.0.1:2375"}}]}"#,
            r#"{"providers": [{"consul": {"endpoint": "http://consul.example.com:8500"}}]}"#,
            r#"{"providers": [{"consul_kv": {"endpoint": "https://consul.example.com:8501", "token_file": "t"}}]}"#,
        ] {
            assert_eq!(warned(doc), Vec::<String>::new(), "{doc}");
        }
        for (doc, want) in [
            (
                r#"{"acme": {"directory": "http://ca.example.com/dir"}}"#,
                "acme.directory http://ca.example.com/dir is plain HTTP off loopback",
            ),
            (
                r#"{"acme": {"shared_storage": {"redis": {"address": "redis.example.com:6379", "password_file": "p"}}}}"#,
                "acme.shared_storage.redis at redis.example.com:6379 without tls: its password and the certificates' private keys",
            ),
            (
                r#"{"acme": {"shared_storage": {"etcd": {"endpoints": ["http://127.0.0.1:2379", "http://10.0.0.2:2379"]}}}}"#,
                "acme.shared_storage.etcd endpoint http://10.0.0.2:2379: the certificates' private keys",
            ),
            (
                r#"{"acme": {"shared_storage": {"consul": {"endpoint": "http://10.0.0.3:8500", "token_file": "t"}}}}"#,
                "acme.shared_storage.consul endpoint http://10.0.0.3:8500: its token and",
            ),
            (
                r#"{"providers": [{"docker": {"endpoint": "tcp://10.0.0.4:2375"}}]}"#,
                "providers[0].docker.endpoint tcp://10.0.0.4:2375: Docker's API",
            ),
            (
                r#"{"providers": [{"consul": {"endpoint": "http://consul.example.com:8500", "token_file": "t"}}]}"#,
                "providers[0].consul.endpoint http://consul.example.com:8500: its token is sent in the clear",
            ),
            (
                r#"{"providers": [{"etcd": {"endpoints": ["http://etcd.example.com:2379"], "username": "u", "password_file": "p"}}]}"#,
                "providers[0].etcd endpoint http://etcd.example.com:2379: its password is sent in the clear",
            ),
        ] {
            let w = warned(doc);
            assert_eq!(w.len(), 1, "{doc}: {w:?}");
            assert!(w[0].starts_with(want), "{doc}: {w:?}");
        }
    }
}
