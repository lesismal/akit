//! Where the API server is and how to authenticate to it (fit's `loadConfig`): the kubeconfig
//! the config names, or the pod's service account inside a cluster, or `$KUBECONFIG`, or
//! `~/.kube/config`. Read on the provider's thread, never an I/O thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akit_config::value::{self, Value};
use akit_config::{Format, KubernetesProvider, Source};

/// The files and variables the provider looks at to find the cluster; fields so that tests can
/// point them elsewhere.
#[derive(Debug, Clone)]
pub struct Env {
    /// The service account's token, inside a pod.
    pub service_account_token: PathBuf,
    /// The service account's CA, inside a pod.
    pub service_account_ca: PathBuf,
    /// `$KUBERNETES_SERVICE_HOST`, set inside a pod.
    pub service_host: Option<String>,
    /// `$KUBERNETES_SERVICE_PORT`.
    pub service_port: Option<String>,
    /// `$KUBECONFIG`.
    pub kubeconfig: Option<String>,
    /// `$HOME`.
    pub home: Option<PathBuf>,
}

impl Env {
    /// The process's: its environment and the service account's usual files.
    #[must_use]
    pub fn of_process() -> Env {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Env {
            service_account_token: PathBuf::from(
                "/var/run/secrets/kubernetes.io/serviceaccount/token",
            ),
            service_account_ca: PathBuf::from(
                "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt",
            ),
            service_host: var("KUBERNETES_SERVICE_HOST"),
            service_port: var("KUBERNETES_SERVICE_PORT"),
            kubeconfig: var("KUBECONFIG"),
            home: var("HOME").map(PathBuf::from),
        }
    }
}

/// A bearer token, given or read from a file, which is read again a minute after it was last
/// read, since projected tokens rotate.
#[derive(Debug)]
pub(crate) struct Token {
    fixed: String,
    file: Option<PathBuf>,
    cached: String,
    read: Option<Instant>,
}

/// How long a token read from a file is used before it is read again.
const TOKEN_REREAD: Duration = Duration::from_secs(60);
/// The largest token file read.
const MAX_TOKEN_BYTES: u64 = 64 << 10;
/// The largest kubeconfig, CA or client certificate file read.
const MAX_FILE_BYTES: u64 = 4 << 20;

impl Token {
    fn fixed(t: &str) -> Token {
        Token {
            fixed: t.to_owned(),
            file: None,
            cached: String::new(),
            read: None,
        }
    }

    fn file(p: PathBuf) -> Token {
        Token {
            fixed: String::new(),
            file: Some(p),
            cached: String::new(),
            read: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn file_for_tests(p: PathBuf) -> Token {
        Token::file(p)
    }

    /// Makes the token read `d` ago.
    #[cfg(test)]
    pub(crate) fn age(&mut self, d: Duration) {
        self.read = self.read.and_then(|r| r.checked_sub(d));
    }

    /// The token, read again from its file when a minute has passed; the last one read while
    /// the file cannot be.
    pub(crate) fn get(&mut self) -> Result<String, String> {
        let Some(file) = &self.file else {
            return Ok(self.fixed.clone());
        };
        if self.cached.is_empty() || self.read.is_none_or(|r| r.elapsed() > TOKEN_REREAD) {
            match read_file(file, MAX_TOKEN_BYTES) {
                Ok(data) => {
                    self.cached = String::from_utf8_lossy(&data).trim().to_owned();
                    self.read = Some(Instant::now());
                }
                Err(e) if self.cached.is_empty() => return Err(e),
                // The last one, until the file is back.
                Err(_) => {}
            }
        }
        Ok(self.cached.clone())
    }
}

/// Where the API server is and how to authenticate to it.
#[derive(Debug)]
pub(crate) struct RestConfig {
    /// The API server's URL, without a trailing `/`.
    pub(crate) server: String,
    /// TLS to it, for an `https` server.
    pub(crate) tls: Option<TlsParts>,
    pub(crate) token: Option<Token>,
    /// Where the config came from, for messages.
    pub(crate) r#where: String,
}

impl RestConfig {
    /// What reaching the cluster this way exposes off loopback (I80b, the threat model's L4): an
    /// `http://` server, or one whose certificate is not verified, carries the bearer token and
    /// the TLS Secrets' private keys akit reads where a machine on the path can read them. Said
    /// once the cluster is found; allowed, as in fit.
    pub(crate) fn exposed(&self) -> Vec<String> {
        if akit_config::hardening::loopback(&self.server) {
            return Vec::new();
        }
        let what = if self.token.is_some() {
            "its bearer token and the TLS Secrets' private keys akit reads"
        } else {
            "the TLS Secrets' private keys akit reads"
        };
        match &self.tls {
            None => vec![format!(
                "the cluster's API server {} is plain HTTP off loopback ({}): {what} cross the network in the clear",
                self.server, self.r#where
            )],
            Some(t) if t.insecure => vec![format!(
                "the cluster's API server {} is not verified (insecure-skip-tls-verify, {}): {what} go to whoever answers in its place",
                self.server, self.r#where
            )],
            Some(_) => Vec::new(),
        }
    }
}

/// What the client's TLS is made of; made into configs for HTTP/2 and HTTP/1.1.
#[derive(Debug, Clone, Default)]
pub(crate) struct TlsParts {
    /// The CAs, as PEM; the system's when `None`.
    pub(crate) ca: Option<Vec<u8>>,
    pub(crate) insecure: bool,
    pub(crate) server_name: String,
    /// The client certificate's chain and key, as PEM.
    pub(crate) client: Option<(Vec<u8>, Vec<u8>)>,
}

impl TlsParts {
    /// A client config asking for `alpn`.
    pub(crate) fn config(&self, alpn: &[&str]) -> Result<ClientConfig, String> {
        let roots = match &self.ca {
            Some(pem) => {
                let mut r = RootCerts::empty();
                r.add_pem(pem)
                    .map_err(|e| format!("certificate-authority: {e}"))?;
                r
            }
            None => RootCerts::system().map_err(|e| format!("the system's CAs: {e}"))?,
        };
        let mut b = ClientConfig::builder()
            .roots(roots)
            .alpn(alpn.iter().copied());
        if let Some((crt, key)) = &self.client {
            let chain =
                Certificate::from_pem(crt).map_err(|e| format!("client-certificate: {e}"))?;
            let key = PrivateKey::from_pem(key).map_err(|e| format!("client-key: {e}"))?;
            b = b.client_cert(chain, key);
        }
        if self.insecure {
            b = b.insecure_skip_verify();
        }
        b.build().map_err(|e| e.to_string())
    }
}

fn read_file(p: &Path, max: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut data = Vec::new();
    f.take(max.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > max {
        return Err(format!("{}: larger than {max} bytes", p.display()));
    }
    Ok(data)
}

/// Finds how to reach the cluster, as fit's `loadConfig`.
pub(crate) fn load(opts: &KubernetesProvider, env: &Env) -> Result<RestConfig, String> {
    if !opts.kubeconfig.is_empty() {
        return from_kubeconfig(Path::new(&opts.kubeconfig), &opts.context);
    }
    if let Some(host) = &env.service_host {
        return in_cluster(host, env.service_port.as_deref(), env);
    }
    let path = match &env.kubeconfig {
        // As kubectl: the first of the list.
        Some(list) => PathBuf::from(list.split(':').next().unwrap_or_default()),
        None => match &env.home {
            Some(home) => home.join(".kube").join("config"),
            None => {
                return Err(
                    "no kubeconfig: not in a cluster, no $KUBECONFIG, and no home directory"
                        .to_owned(),
                );
            }
        },
    };
    from_kubeconfig(&path, &opts.context)
}

fn in_cluster(host: &str, port: Option<&str>, env: &Env) -> Result<RestConfig, String> {
    let ca = read_file(&env.service_account_ca, MAX_FILE_BYTES)
        .map_err(|e| format!("in the cluster: {e}"))?;
    if Certificate::from_pem(&ca).map_or(true, |c| c.is_empty()) {
        return Err(format!(
            "in the cluster: {} holds no certificate",
            env.service_account_ca.display()
        ));
    }
    let mut token = Token::file(env.service_account_token.clone());
    token.get().map_err(|e| format!("in the cluster: {e}"))?;
    let port = port.filter(|p| !p.is_empty()).unwrap_or("443");
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Ok(RestConfig {
        server: format!("https://{host}:{port}"),
        tls: Some(TlsParts {
            ca: Some(ca),
            ..TlsParts::default()
        }),
        token: Some(token),
        r#where: "the pod's service account".to_owned(),
    })
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut v = v;
    for k in path {
        match v.get(k) {
            Some(x) => v = x,
            None => return "",
        }
    }
    match v {
        Value::String(s) => s,
        _ => "",
    }
}

fn named<'a>(v: &'a Value, list: &str, name: &str) -> Option<&'a Value> {
    match v.get(list) {
        Some(Value::Array(a)) => a.iter().find(|x| str_at(x, &["name"]) == name),
        _ => None,
    }
}

/// Reads the kubeconfig at `path` and takes its context named `context`, or its current one.
pub(crate) fn from_kubeconfig(path: &Path, context: &str) -> Result<RestConfig, String> {
    let shown = path.display();
    let data = read_file(path, MAX_FILE_BYTES).map_err(|e| format!("kubeconfig: {e}"))?;
    let text = String::from_utf8(data).map_err(|_| format!("kubeconfig {shown}: not UTF-8"))?;
    let json = value::json_of(&[Source {
        name: None,
        format: Format::Yaml,
        src: &text,
    }])
    .map_err(|e| format!("kubeconfig {shown}: {e}"))?;
    let kc = value::parse(&json).map_err(|e| format!("kubeconfig {shown}: {e}"))?;
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let file = |p: &str| -> Option<PathBuf> {
        if p.is_empty() {
            None
        } else if Path::new(p).is_absolute() {
            Some(PathBuf::from(p))
        } else {
            Some(dir.join(p))
        }
    };
    let context = if context.is_empty() {
        str_at(&kc, &["current-context"])
    } else {
        context
    };
    if context.is_empty() {
        return Err(format!(
            "kubeconfig {shown}: no current-context, and no context given"
        ));
    }
    let Some(ctx) = named(&kc, "contexts", context) else {
        return Err(format!("kubeconfig {shown}: no context {context:?}"));
    };
    let (cluster_name, user_name) = (
        str_at(ctx, &["context", "cluster"]),
        str_at(ctx, &["context", "user"]),
    );
    let Some(cluster) = named(&kc, "clusters", cluster_name) else {
        return Err(format!("kubeconfig {shown}: no cluster {cluster_name:?}"));
    };
    let server = str_at(cluster, &["cluster", "server"]);
    if server.is_empty() {
        return Err(format!(
            "kubeconfig {shown}: cluster {cluster_name:?} has no server"
        ));
    }
    let mut tls = TlsParts {
        insecure: matches!(
            cluster
                .get("cluster")
                .and_then(|c| c.get("insecure-skip-tls-verify")),
            Some(Value::Bool(true))
        ),
        server_name: str_at(cluster, &["cluster", "tls-server-name"]).to_owned(),
        ..TlsParts::default()
    };
    tls.ca = pem_of(
        str_at(cluster, &["cluster", "certificate-authority-data"]),
        file(str_at(cluster, &["cluster", "certificate-authority"])),
    )
    .map_err(|e| {
        format!("kubeconfig {shown}: cluster {cluster_name:?}: certificate-authority: {e}")
    })?;
    if let Some(ca) = &tls.ca {
        if Certificate::from_pem(ca).map_or(true, |c| c.is_empty()) {
            return Err(format!(
                "kubeconfig {shown}: cluster {cluster_name:?}: certificate-authority holds no certificate"
            ));
        }
    }
    let mut token = None;
    if let Some(user) = named(&kc, "users", user_name) {
        let u = user.get("user").unwrap_or(&Value::Null);
        if u.get("exec").is_some_and(|v| *v != Value::Null)
            || u.get("auth-provider").is_some_and(|v| *v != Value::Null)
        {
            return Err(format!(
                "kubeconfig {shown}: user {user_name:?} authenticates with a plugin (exec or auth-provider), which akit does not run: use a token, a client certificate, or akit inside the cluster"
            ));
        }
        let crt = pem_of(
            str_at(u, &["client-certificate-data"]),
            file(str_at(u, &["client-certificate"])),
        )
        .map_err(|e| format!("kubeconfig {shown}: user {user_name:?}: client-certificate: {e}"))?;
        let key = pem_of(
            str_at(u, &["client-key-data"]),
            file(str_at(u, &["client-key"])),
        )
        .map_err(|e| format!("kubeconfig {shown}: user {user_name:?}: client-key: {e}"))?;
        if let Some(c) = &crt {
            if let Err(e) = Certificate::from_pem(c) {
                return Err(format!(
                    "kubeconfig {shown}: user {user_name:?}: client-certificate: {e}"
                ));
            }
        }
        if let Some(k) = &key {
            if let Err(e) = PrivateKey::from_pem(k) {
                return Err(format!(
                    "kubeconfig {shown}: user {user_name:?}: client-key: {e}"
                ));
            }
        }
        match (crt, key) {
            (None, None) => {}
            (Some(c), Some(k)) => tls.client = Some((c, k)),
            _ => {
                return Err(format!(
                    "kubeconfig {shown}: user {user_name:?}: a client certificate needs its key, and a key its certificate"
                ));
            }
        }
        let (t, tf) = (str_at(u, &["token"]), str_at(u, &["tokenFile"]));
        if !t.is_empty() {
            token = Some(Token::fixed(t));
        } else if let Some(p) = file(tf) {
            token = Some(Token::file(p));
        }
    }
    let server = server.trim_end_matches('/').to_owned();
    let https = server.starts_with("https://");
    if https {
        // Checked now, so that a mistake is said once, at the start.
        tls.config(&["h2", "http/1.1"])
            .map_err(|e| format!("kubeconfig {shown}: {e}"))?;
    }
    Ok(RestConfig {
        r#where: format!("kubeconfig {shown}, context {context}"),
        server,
        tls: https.then_some(tls),
        token,
    })
}

/// PEM given as base64 data, or read from a file, or `None` when neither is given.
fn pem_of(data: &str, path: Option<PathBuf>) -> Result<Option<Vec<u8>>, String> {
    if !data.is_empty() {
        return akio_crypto::base64::STANDARD
            .decode(data.trim())
            .map(Some)
            .map_err(|_| "not base64".to_owned());
    }
    match path {
        Some(p) => read_file(&p, MAX_FILE_BYTES).map(Some),
        None => Ok(None),
    }
}
