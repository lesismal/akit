//! fit's `llm/internal/role`: what the three LLM roles share. A role's [`Base`] holds its identity
//! (its `id`, made at the first start and written back into its member, and its `name`), its
//! mesh (the member's `listen`, `peers`, `tokens`, `limits` and `access`), and its listener's
//! self-signed certificate, made once and kept beside the config.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use akit_config::Config;
use akit_config::roles::{LlmListen, LlmPeer, LlmRole};
use akit_config::value::Value;
use akit_roles::host::Host;
use akit_roles::linktoken::Registry;

use crate::mesh::{self, Limits, Mesh, Router};
use crate::model::{DialReq, Identity, ListenConfig};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The organization of a role's self-signed certificate: `akit llm <role>` (fit's `fit llm`).
#[must_use]
pub fn cert_org(role: &str) -> String {
    format!("akit llm {role}")
}

/// A new ID for a role: `<role>-<16 hex digits>`.
#[must_use]
pub fn new_id(role: &str) -> String {
    let mut b = [0u8; 8];
    let _ = akio_crypto::rand::fill_random(&mut b);
    format!("{role}-{}", akit_roles::linktoken::hex(&b))
}

#[derive(Default)]
struct State {
    id: String,
    name: String,
    applied: bool,
    /// The self-signed certificate and key made for the listener.
    generated: Option<(String, String)>,
}

/// What every LLM role has.
pub struct Base {
    /// `hub`, `node` or `proxy`.
    pub role: &'static str,
    /// The host the role runs on.
    pub host: Host,
    /// The role's mesh.
    pub mesh: Mesh,
    st: Arc<Mutex<State>>,
    rpc: Arc<Mutex<String>>,
    /// The sweep of expired tokens: its generation, so that only the last one armed goes on.
    sweep: Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for Base {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Base")
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// The member's ID and name changes written back: `id` set when it is not.
fn write_id(host: &Host, id: String) {
    host.edit(
        Box::new(move |m: &mut Value| {
            let Value::Object(ms) = m else {
                return Err("the member is not an object".to_owned());
            };
            if !ms.iter().any(|(k, _)| k == "id") {
                ms.insert(0, ("id".to_owned(), Value::String(id)));
            }
            Ok(())
        }),
        Box::new(|r| {
            if let Err(e) = r {
                akit_observe::warn!(
                    Llm,
                    "the role's ID could not be written into the config: {e}"
                );
            }
        }),
    );
}

/// The listener's self-signed certificate written into the member, for a config kept nowhere
/// else.
fn write_cert(host: &Host, cert: String, key: String) {
    host.edit(
        Box::new(move |m: &mut Value| {
            let Value::Object(ms) = m else {
                return Ok(());
            };
            let Some((_, Value::Object(l))) = ms.iter_mut().find(|(k, _)| k == "listen") else {
                return Ok(());
            };
            let self_signed = l
                .iter()
                .any(|(k, v)| k == "self_signed" && *v == Value::Bool(true));
            let has = l
                .iter()
                .any(|(k, v)| k == "cert" && !matches!(v, Value::String(s) if s.is_empty()));
            if self_signed && !has {
                l.retain(|(k, _)| k != "cert" && k != "key");
                l.push(("cert".to_owned(), Value::String(cert)));
                l.push(("key".to_owned(), Value::String(key)));
            }
            Ok(())
        }),
        Box::new(|r| {
            if let Err(e) = r {
                akit_observe::warn!(
                    Llm,
                    "keeping the listener's self-signed certificate in the config: {e}"
                );
            }
        }),
    );
}

impl Base {
    /// A role's base on `host`, serving `router`'s methods.
    #[must_use]
    pub fn new(host: Host, role: &'static str, router: Router) -> Base {
        let st: Arc<Mutex<State>> = Arc::default();
        let hostname = akit_config::roles::host_name();
        let started = akit_roles::linktoken::unix_now();
        let s2 = Arc::clone(&st);
        let lst: Arc<Mutex<String>> = Arc::default();
        let l2 = Arc::clone(&lst);
        let identity = Arc::new(move || {
            let g = lock(&s2);
            Identity {
                role: role.to_owned(),
                id: g.id.clone(),
                name: if g.name.is_empty() {
                    hostname.clone()
                } else {
                    g.name.clone()
                },
                hostname: hostname.clone(),
                version: mesh::version(),
                pid: i64::from(std::process::id()),
                http_url: String::new(),
                rpc_addr: lock(&l2).clone(),
                started_at: started,
            }
        });
        let tokens = Registry::new(
            Some(bind_writer(&host)),
            Arc::new(akit_roles::linktoken::unix_now),
        );
        let mesh = Mesh::with_tokens(
            mesh::Config {
                identity,
                accept_roles: akit_config::roles::llm_peer_roles(role)
                    .iter()
                    .map(|r| (*r).to_owned())
                    .collect(),
                engine: Arc::clone(&host.engine),
            },
            router,
            tokens,
        );
        Base {
            role,
            host,
            mesh,
            st,
            rpc: lst,
            sweep: Arc::default(),
        }
    }

    /// Arms the sweep that closes the links of expired tokens, at the next expiry (a minute at
    /// most), again after each; armed anew on every apply, since the tokens may have changed,
    /// the earlier one then going no further.
    fn arm_sweep(&self) {
        use std::sync::atomic::Ordering;
        let epoch = self.sweep.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        sweep_after(self.mesh.tokens(), Arc::downgrade(&self.sweep), epoch);
    }

    /// The role's ID.
    #[must_use]
    pub fn id(&self) -> String {
        lock(&self.st).id.clone()
    }

    /// Moves the base to `c`, the role's member, and `config`, the whole config (its global
    /// access): the identity, the access policy, the tokens, the listener and the peers. The
    /// first apply of a member without an ID makes one and writes it back. Binds and reads
    /// files: the control thread's.
    ///
    /// # Errors
    ///
    /// The access policy, the listener and the peers' failures, joined; the rest is applied.
    pub fn apply(&self, c: &LlmRole, config: &Config) -> Result<(), String> {
        let mut errs = Vec::new();
        let (old, id, write) = {
            let mut g = lock(&self.st);
            let first = !g.applied;
            g.applied = true;
            let old = g.id.clone();
            if !c.id.is_empty() {
                g.id.clone_from(&c.id);
            } else if g.id.is_empty() {
                g.id = new_id(self.role);
            }
            g.name.clone_from(&c.name);
            (old, g.id.clone(), c.id.is_empty() && first)
        };
        if !old.is_empty() && old != id {
            // Peers know the role by its ID: they say hello again.
            for p in self.mesh.peers() {
                self.mesh.kick(&p.identity.id);
            }
        }
        if write {
            write_id(&self.host, id);
        }
        match policy(c.access.as_ref(), config.access.as_ref()) {
            Ok(p) => self.mesh.set_filter(p),
            Err(e) => errs.push(e),
        }
        self.mesh.set_limits(Limits::of(c.limits.as_ref()));
        let mut listen = c.listen.as_ref().map(listen_config).unwrap_or_default();
        if let Err(e) = self.self_signed(&mut listen) {
            akit_observe::warn!(
                Llm,
                "llm.{}: making the listener's self-signed certificate: {e}",
                self.role
            );
        }
        let peers: Vec<DialReq> = c.peers.iter().map(dial_req).collect();
        // The tokens before the listener, so that it says it has none only when so.
        self.mesh.tokens().set(&c.tokens);
        self.arm_sweep();
        if let Err(e) = self.mesh.configure(&listen, &peers) {
            errs.push(e);
        }
        // The address the identity gives: the listener's, once it listens.
        *lock(&self.rpc) = self.mesh.listen_addr();
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    /// Gives a `self_signed` listener without a certificate one: made once, kept beside the
    /// config (`akit.llm.<role>.pem`) so that it outlives a restart and its peers' pins stay
    /// good, or written into the member when nothing is kept beside the config.
    fn self_signed(&self, l: &mut ListenConfig) -> Result<(), String> {
        if !l.self_signed || !l.cert_file.is_empty() || !l.cert.is_empty() {
            if !l.cert.is_empty() {
                lock(&self.st).generated = None;
            }
            return Ok(());
        }
        let have = lock(&self.st).generated.clone();
        let (cert, key) = match have {
            Some(ck) => ck,
            None => {
                let mut hosts = vec!["localhost".to_owned(), akit_config::roles::host_name()];
                if let Some((h, _)) = l.addr.rsplit_once(':') {
                    let h = h.trim_start_matches('[').trim_end_matches(']');
                    if !h.is_empty() && !hosts.iter().any(|x| x == h) {
                        hosts.push(h.to_owned());
                    }
                }
                let (c, k) = match self.host.kept_file("pem") {
                    Some(path) => {
                        let (c, k, made) = crate::tls::keep(&path, &hosts)?;
                        akit_observe::info!(
                            Llm,
                            "llm.{}: {} the listener's self-signed certificate, kept in {path}; its fingerprint: {}",
                            self.role,
                            if made { "made" } else { "read" },
                            mesh::fingerprint(&c)
                        );
                        (c, k)
                    }
                    None => {
                        let (c, k) = crate::tls::self_signed(&hosts)?;
                        write_cert(&self.host, c.clone(), k.clone());
                        (c, k)
                    }
                };
                lock(&self.st).generated = Some((c.clone(), k.clone()));
                (c, k)
            }
        };
        l.cert = cert;
        l.key = key;
        Ok(())
    }

    /// Drains the mesh for `grace` at most, then stops it. Waits: the control thread's.
    pub fn stop(&self, grace: Duration) {
        self.sweep.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.mesh.shutdown(grace);
    }
}

fn sweep_after(
    tokens: Registry,
    epochs: std::sync::Weak<std::sync::atomic::AtomicU64>,
    epoch: u64,
) {
    let Some(d) = tokens.next_sweep() else {
        return; // no token expires: the next apply arms it again
    };
    let _ = akio::timer::after_send(d.max(Duration::from_secs(1)), move || {
        let Some(g) = epochs.upgrade() else { return };
        if g.load(std::sync::atomic::Ordering::Acquire) != epoch {
            return;
        }
        tokens.sweep();
        sweep_after(tokens, epochs, epoch);
    });
}

/// A token's first member written into its record, as fit's `OnBind`.
fn bind_writer(host: &Host) -> akit_roles::linktoken::OnBind {
    let host = host.clone();
    Arc::new(move |token: &str, member: &str| {
        let (token, member) = (token.to_owned(), member.to_owned());
        // The edit hands itself to the control thread at once.
        {
            host.edit(
                Box::new(move |m: &mut Value| {
                    let Value::Object(ms) = m else { return Ok(()) };
                    let Some((_, Value::Array(ts))) = ms.iter_mut().find(|(k, _)| k == "tokens")
                    else {
                        return Ok(());
                    };
                    for t in ts.iter_mut() {
                        let Value::Object(f) = t else { continue };
                        if !f
                            .iter()
                            .any(|(k, v)| k == "id" && matches!(v, Value::String(s) if *s == token))
                        {
                            continue;
                        }
                        match f.iter_mut().find(|(k, _)| k == "members") {
                            Some((_, Value::Array(a))) => {
                                if !a
                                    .iter()
                                    .any(|v| matches!(v, Value::String(s) if *s == member))
                                {
                                    a.push(Value::String(member.clone()));
                                }
                            }
                            _ => f.push((
                                "members".to_owned(),
                                Value::Array(vec![Value::String(member.clone())]),
                            )),
                        }
                    }
                    Ok(())
                }),
                Box::new(|_| {}),
            );
        }
    })
}

/// The access policy of a role: its own, then the global one.
///
/// # Errors
///
/// A list that does not compile.
pub fn policy(
    own: Option<&akit_config::Access>,
    global: Option<&akit_config::Access>,
) -> Result<Option<Arc<akit_access::Policy>>, String> {
    let err = |es: Vec<akit_access::Error>| {
        es.iter()
            .map(|e| format!("access.{}: {}", e.field, e.msg))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let own = akit_access::compile(own, None).map_err(err)?;
    let global = akit_access::compile(global, None).map_err(err)?;
    Ok(akit_access::Policy::chain([own.as_ref(), global.as_ref()]))
}

/// The mesh's listener of `l`.
#[must_use]
pub fn listen_config(l: &LlmListen) -> ListenConfig {
    ListenConfig {
        addr: l.address.clone(),
        password: l.password.clone(),
        node_password: l.node_password.clone(),
        proxy_password: l.proxy_password.clone(),
        hub_password: l.hub_password.clone(),
        cert_file: l.cert_file.clone(),
        key_file: l.key_file.clone(),
        self_signed: l.self_signed,
        cert: l.cert.clone(),
        key: l.key.clone(),
    }
}

/// The mesh's dial target of `p`.
#[must_use]
pub fn dial_req(p: &LlmPeer) -> DialReq {
    DialReq {
        addr: p.address.clone(),
        password: p.password.clone(),
        token: p.token.clone(),
        role: p.role.clone(),
        pin: p.pin.clone(),
        ca_file: p.ca_file.clone(),
        server_name: p.server_name.clone(),
    }
}
