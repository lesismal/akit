//! fit's `mesh/listen.go` and `tls.go`: a role's listener, an akio listener of its own on the
//! listeners' engine serving [`PATH`](crate::wire::conn::PATH) alone; over TLS off loopback.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use akio::ListenerId;

use super::{Inner, Limits, Mesh, bad_request, lock};
use crate::json::Obj;
use crate::model::{ListenConfig, ROLE_LOCAL};
use crate::wire::{self, Credential, Error};

/// The listener as it runs.
#[derive(Default)]
pub(crate) struct Lst {
    cfg: ListenConfig,
    err: String,
    limits: Option<Limits>,
    id: Option<ListenerId>,
    listener: Option<wire::Listener>,
    addr: String,
    fp: String,
    /// The certificate served, replaced in place when its files change.
    store: Option<akio::tls::CertStore>,
    /// What is served (certificate, key), to tell a change.
    pem: Option<(String, String)>,
    /// The certificate's and key's files as last read, with their SHA-256 (zeros for one that
    /// could not be read): for `akit run -watch-certs`.
    files: Vec<(String, [u8; 32])>,
}

/// The listener as the pages show it: no password, no key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenState {
    /// Its address as configured.
    pub addr: String,
    /// Whether any password is set.
    pub has_password: bool,
    /// Over TLS.
    pub tls: bool,
    /// The certificate's file.
    pub cert_file: String,
    /// The key's file.
    pub key_file: String,
    /// A certificate the role makes.
    pub self_signed: bool,
    /// The SHA-256 of the certificate presented: what dialers pin.
    pub fingerprint: String,
    /// The last failure to listen.
    pub error: String,
}

impl ListenState {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("addr", &self.addr)
            .bool("has_password", self.has_password)
            .bool_omit("tls", self.tls)
            .str_omit("cert_file", &self.cert_file)
            .str_omit("key_file", &self.key_file)
            .bool_omit("self_signed", self.self_signed)
            .str_omit("fingerprint", &self.fingerprint)
            .str_omit("error", &self.error)
            .end()
    }
}

/// Checks and normalizes `host:port`; the host may be empty (every interface) only for a
/// listener (`need_host` false).
///
/// # Errors
///
/// A bad request.
pub fn validate_addr(addr: &str, need_host: bool) -> Result<String, Error> {
    let bad = || bad_request("address must be host:port");
    let (host, port) = addr.rsplit_once(':').ok_or_else(bad)?;
    if port.is_empty() || (need_host && host.is_empty()) {
        return Err(bad());
    }
    let bracketed = host.starts_with('[') && host.ends_with(']');
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if !bracketed && bare.contains(':') {
        return Err(bad()); // an IPv6 address needs brackets
    }
    let n: u16 = port
        .parse()
        .map_err(|_| bad_request(format!("invalid port {port:?}")))?;
    if need_host && n == 0 {
        return Err(bad_request(format!("invalid port {port:?}")));
    }
    Ok(if bare.contains(':') {
        format!("[{bare}]:{port}")
    } else {
        format!("{bare}:{port}")
    })
}

/// The SHA-256 of a PEM's first certificate, in hex.
#[must_use]
pub fn fingerprint(cert_pem: &str) -> String {
    akio::tls::Certificate::from_pem(cert_pem.as_bytes())
        .ok()
        .and_then(|c| {
            c.first()
                .map(|c| akit_roles::linktoken::hex(&akio_crypto::hash::Sha256::digest(c.as_der())))
        })
        .unwrap_or_default()
}

/// The SHA-256 of a file read, zeros for one that could not be.
fn file_sum(r: &std::io::Result<String>) -> [u8; 32] {
    r.as_ref()
        .map(|d| akio_crypto::hash::Sha256::digest(d.as_bytes()))
        .unwrap_or([0u8; 32])
}

/// Reads `cfg`'s certificate and key files, and records them in `files`.
fn read_files(
    cfg: &ListenConfig,
    files: &mut Vec<(String, [u8; 32])>,
) -> Result<(String, String), Error> {
    let c = std::fs::read_to_string(&cfg.cert_file);
    let k = std::fs::read_to_string(&cfg.key_file);
    *files = vec![
        (cfg.cert_file.clone(), file_sum(&c)),
        (cfg.key_file.clone(), file_sum(&k)),
    ];
    let bad = |e: std::io::Error| bad_request(format!("the listener's certificate: {e}"));
    Ok((c.map_err(bad)?, k.map_err(bad)?))
}

/// A listener's TLS: its configuration, the store it serves its certificate from, what it
/// serves, and its fingerprint.
type ServerTls = (
    akio::tls::ServerConfig,
    akio::tls::CertStore,
    (String, String),
    String,
);

/// The TLS a listener of `cfg` takes links with; `None` without TLS. Reads the certificate's
/// files, recorded in `files`: the control thread's.
fn server_tls(
    cfg: &ListenConfig,
    files: &mut Vec<(String, [u8; 32])>,
) -> Result<Option<ServerTls>, Error> {
    files.clear();
    let (cert, key) = if !cfg.cert_file.is_empty() {
        read_files(cfg, files)?
    } else if !cfg.cert.is_empty() {
        (cfg.cert.clone(), cfg.key.clone())
    } else if cfg.self_signed {
        return Err(bad_request("self_signed: no certificate has been made yet"));
    } else {
        return Ok(None);
    };
    let bad = |e: String| bad_request(format!("the listener's certificate: {e}"));
    let chain =
        akio::tls::Certificate::from_pem(cert.as_bytes()).map_err(|e| bad(e.to_string()))?;
    if chain.is_empty() {
        return Err(bad("no certificate".to_owned()));
    }
    let k = akio::tls::PrivateKey::from_pem(key.as_bytes()).map_err(|e| bad(e.to_string()))?;
    let store = akio::tls::CertStore::new();
    let sc = akio::tls::ServerConfig::builder(chain, k)
        .tls13_only(true)
        .session_tickets(false)
        .cert_resolver(Arc::new(store.clone()))
        .build()
        .map_err(|e| bad(e.to_string()))?;
    let fp = fingerprint(&cert);
    Ok(Some((sc, store, (cert, key), fp)))
}

impl Mesh {
    /// Replaces the listener: the one running stops (its inbound links closed; their dialers link
    /// again) and, unless `cfg.addr` is empty, a new one starts. Setting what runs, with the same
    /// bounds, does nothing. A listener off loopback needs TLS. The configuration is kept even
    /// when it cannot bind: the error is returned and shown in [`Mesh::listen_state`]. Binds a
    /// socket and reads files: the control thread's.
    ///
    /// # Errors
    ///
    /// A bad address, certificate or bind.
    pub fn set_listen(&self, cfg: &ListenConfig) -> Result<(), Error> {
        let mut cfg = cfg.clone();
        cfg.addr = cfg.addr.trim().to_owned();
        if !cfg.addr.is_empty() {
            cfg.addr = validate_addr(&cfg.addr, false)?;
            let host = cfg.addr.rsplit_once(':').map_or("", |(h, _)| h);
            if !akit_config::roles::loopback(host) && !cfg.tls() {
                return Err(bad_request(format!(
                    "{} is not a loopback address: set cert_file and key_file, or self_signed, so that links are taken over TLS and peers can check whose listener they reach",
                    cfg.addr
                )));
            }
        }
        let inner = &self.0;
        let mut l = lock(&inner.lst);
        if inner.stopped.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let limits = *lock(&inner.limits);
        let running = l.id.is_some();
        if cfg == l.cfg
            && l.err.is_empty()
            && running == !cfg.addr.is_empty()
            && (!running || l.limits == Some(limits))
        {
            if running && !cfg.cert_file.is_empty() {
                reread(&mut l, &cfg);
            }
            return Ok(());
        }
        inner.stop_listener_locked(&mut l);
        l.cfg = cfg.clone();
        l.err.clear();
        l.limits = Some(limits);
        l.fp.clear();
        l.store = None;
        l.pem = None;
        l.files.clear();
        if cfg.addr.is_empty() {
            akit_observe::info!(Llm, "mesh: not listening");
            return Ok(());
        }
        let fail = |l: &mut Lst, e: Error| {
            l.err = e.to_string();
            Err(e)
        };
        let mut files = Vec::new();
        let tls = server_tls(&cfg, &mut files);
        l.files = files;
        let tls = match tls {
            Ok(t) => t,
            Err(e) => return fail(&mut l, e),
        };
        let creds = inner.credentials(&cfg);
        let Some(me) = inner.me.upgrade() else {
            return Err(Error::Closed);
        };
        let listener = wire::Listener::new(inner.listener_config(&me, creds.clone(), limits));
        let addr: SocketAddr = match resolve_listen(&cfg.addr) {
            Ok(a) => a,
            Err(e) => return fail(&mut l, e),
        };
        let (tls_cfg, fp) = match tls {
            Some((c, store, pem, fp)) => {
                l.store = Some(store);
                l.pem = Some(pem);
                (Some(c), fp)
            }
            None => (None, String::new()),
        };
        let al = listener.serve(addr, tls_cfg, inner.peer_addr_filter());
        let id = match inner.cfg.engine.add_listener(al) {
            Ok(id) => id,
            Err(e) => return fail(&mut l, Error::Other(format!("listen {}: {e}", cfg.addr))),
        };
        l.addr = inner
            .cfg
            .engine
            .listeners()
            .into_iter()
            .find(|i| i.id == id)
            .map_or_else(|| cfg.addr.clone(), |i| i.local_addr.to_string());
        l.id = Some(id);
        l.listener = Some(listener);
        l.fp.clone_from(&fp);
        if fp.is_empty() {
            akit_observe::info!(
                Llm,
                "mesh listening: addr={} path={}",
                l.addr,
                wire::conn::PATH
            );
        } else {
            akit_observe::info!(
                Llm,
                "mesh listening over TLS (wss://); peers pin its certificate by this fingerprint, or check it with ca_file: addr={} path={} fingerprint={fp}",
                l.addr,
                wire::conn::PATH
            );
        }
        if creds.is_empty() && inner.tokens.is_empty() {
            akit_observe::warn!(
                Llm,
                "mesh: the listener has no password and no token yet: it takes no link until its pages, its Admin API module or akit token issue one: addr={}",
                l.addr
            );
        }
        for c in &creds {
            if c.roles.len() > 1 {
                akit_observe::warn!(
                    Llm,
                    "mesh: the listener takes one password from several roles: whoever holds it may link as any of them; give each role a password of its own: addr={} roles={}",
                    l.addr,
                    c.roles.join(",")
                );
            }
        }
        Ok(())
    }

    /// The listener's certificate and key files as last read, with their SHA-256 (zeros for one
    /// that could not be read): `Role::files`, which `akit run -watch-certs` watches.
    #[must_use]
    pub fn files(&self) -> Vec<(String, [u8; 32])> {
        lock(&self.0.lst).files.clone()
    }

    /// Where the listener listens, its port chosen when the configuration's was 0; empty when
    /// it does not.
    #[must_use]
    pub fn listen_addr(&self) -> String {
        lock(&self.0.lst).addr.clone()
    }

    /// The listener's configuration, passwords included.
    #[must_use]
    pub fn listen_config(&self) -> ListenConfig {
        lock(&self.0.lst).cfg.clone()
    }

    /// The listener as the pages show it.
    #[must_use]
    pub fn listen_state(&self) -> ListenState {
        let l = lock(&self.0.lst);
        ListenState {
            addr: l.cfg.addr.clone(),
            has_password: l.cfg.has_password(),
            tls: l.cfg.tls(),
            cert_file: l.cfg.cert_file.clone(),
            key_file: l.cfg.key_file.clone(),
            self_signed: l.cfg.self_signed,
            fingerprint: l.fp.clone(),
            error: l.err.clone(),
        }
    }
}

/// Reads a running listener's certificate files again (fit's listener reads them at each
/// handshake once they changed): a certificate that changed is served to the handshakes from
/// now on, in place, the listener and its links left alone; one that does not load leaves the
/// running one, and is logged.
fn reread(l: &mut Lst, cfg: &ListenConfig) {
    let mut files = Vec::new();
    let read = read_files(cfg, &mut files);
    l.files = files;
    let keep = |why: String| {
        akit_observe::warn!(
            Llm,
            "mesh: the listener's certificate: {why}; the running certificate stays: addr={}",
            cfg.addr
        );
    };
    let pem = match read {
        Ok(p) => p,
        Err(e) => return keep(e.to_string()),
    };
    if l.pem.as_ref() == Some(&pem) {
        return;
    }
    let Some(store) = &l.store else { return };
    match akio::tls::CertifiedKey::from_pem(pem.0.as_bytes(), pem.1.as_bytes()) {
        Ok(key) => {
            store.set_default(Some(key));
            l.fp = fingerprint(&pem.0);
            l.pem = Some(pem);
            akit_observe::info!(
                Llm,
                "mesh: the listener's certificate read again; peers pin it by this fingerprint: addr={} fingerprint={}",
                l.addr,
                l.fp
            );
        }
        Err(e) => keep(e.to_string()),
    }
}

fn resolve_listen(addr: &str) -> Result<SocketAddr, Error> {
    let a = match addr.strip_prefix(':') {
        Some(port) => format!("0.0.0.0:{port}"),
        None => addr.to_owned(),
    };
    let a = a.replacen("localhost:", "127.0.0.1:", 1);
    a.parse::<SocketAddr>()
        .map_err(|_| bad_request(format!("{addr}: not an address to listen on")))
}

impl Inner {
    /// The passwords a listener of `cfg` takes, one per distinct password with the roles whose
    /// it is; a role without one is not taken.
    fn credentials(&self, cfg: &ListenConfig) -> Vec<Credential> {
        let mut out: Vec<Credential> = Vec::new();
        for role in &self.cfg.accept_roles {
            let pw = cfg.password_for(role);
            if pw.is_empty() || role == ROLE_LOCAL {
                continue;
            }
            match out.iter_mut().find(|c| c.password == pw) {
                Some(c) => c.roles.push(role.clone()),
                None => out.push(Credential {
                    password: pw.to_owned(),
                    roles: vec![role.clone()],
                }),
            }
        }
        out
    }

    /// The roles a link that proved a token may say hello as: every role taken.
    fn token_roles(&self) -> Vec<String> {
        self.cfg
            .accept_roles
            .iter()
            .filter(|r| *r != ROLE_LOCAL)
            .cloned()
            .collect()
    }

    fn listener_config(
        &self,
        me: &Arc<Inner>,
        creds: Vec<Credential>,
        limits: Limits,
    ) -> wire::ListenerConfig {
        let (m1, m2, m3, m4) = (
            Arc::downgrade(me),
            Arc::downgrade(me),
            Arc::downgrade(me),
            Arc::downgrade(me),
        );
        wire::ListenerConfig {
            session: limits.session(),
            timing: limits.timing(),
            credentials: creds,
            tokens: Some(Arc::new(self.tokens.clone())),
            token_roles: self.token_roles(),
            max_conns: limits.max_conns,
            max_pending: 0,
            max_pending_per_ip: 0,
            accept: Some(Arc::new(move || match m1.upgrade() {
                Some(m) if !m.draining.load(Ordering::Acquire) => Ok(()),
                _ => Err("shutting down".to_owned()),
            })),
            handlers: Arc::new(move |wc| match m2.upgrade() {
                Some(m) => m.handler_of(wc, None),
                None => Arc::new(super::dial_closed()),
            }),
            on_auth_failed: Some(Arc::new(|remote: &str| {
                akit_observe::warn!(
                    Llm,
                    "mesh: authentication failed (wrong password or token): remote={remote}"
                );
            })),
            on_open: Some(Arc::new(move |wc| {
                if let Some(m) = m3.upgrade() {
                    m.await_hello(wc);
                }
            })),
            on_close: Some(Arc::new(move |wc, e| {
                if let Some(m) = m4.upgrade() {
                    m.disconnected(wc, Some(e));
                }
            })),
        }
    }

    pub(crate) fn stop_listener(&self) {
        let mut l = lock(&self.lst);
        self.stop_listener_locked(&mut l);
    }

    fn stop_listener_locked(&self, l: &mut Lst) {
        let Some(id) = l.id.take() else { return };
        let _ = self.cfg.engine.remove_listener(id);
        if let Some(x) = l.listener.take() {
            x.close_all();
        }
        l.addr.clear();
        self.close_conns(|c| !c.out);
    }

    /// Has the running listener take connections by `p` from now on.
    pub(crate) fn refilter(&self, p: Option<Arc<akit_access::Policy>>) {
        let l = lock(&self.lst);
        let Some(id) = l.id else { return };
        let _ = match p {
            Some(p) => self
                .cfg
                .engine
                .set_listener_filter(id, move |a: &SocketAddr| p.allows(a.ip())),
            None => self.cfg.engine.clear_listener_filter(id),
        };
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(validate_addr("127.0.0.1:80", true).unwrap(), "127.0.0.1:80");
        assert_eq!(validate_addr("[::1]:80", true).unwrap(), "[::1]:80");
        assert_eq!(validate_addr(":0", false).unwrap(), ":0");
        assert!(validate_addr(":80", true).is_err());
        assert!(validate_addr("h:0", true).is_err());
        assert!(validate_addr("h", true).is_err());
        assert!(validate_addr("h:x", true).is_err());
        assert!(validate_addr("::1:80", true).is_err());
        assert!(validate_addr("h:", false).is_err());
    }
}
