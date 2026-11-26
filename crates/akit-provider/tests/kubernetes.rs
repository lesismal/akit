//! The Kubernetes provider (I59, I60) against a fake API server, as fit's `kubernetes_test.go`
//! runs it (`TestProvider`, `TestProviderIngress`, `TestProviderUnauthorized`,
//! `TestProviderSetupFails`, `TestProviderStatus`): nothing is sent until every kind is listed,
//! a kind not served counts as empty, an older version of a kind is listed when the newer is not
//! served, lists come in pages, watches carry the changes, a watch too old makes a new list, an
//! ended watch is made again, a refused list is reported and tried again, an HTTPS listener's or
//! an Ingress's TLS Secret becomes a certificate file, and the statuses of the Gateway API's
//! objects are written back, once, and again after a write refused. The fake server is akio's
//! HTTP server: over TLS it speaks HTTP/2 (which the client uses, every request on one
//! connection) or, asked to, HTTP/1.1 alone; in the clear HTTP/1.1. On the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akio_http::message::Method;
use akio_http::{BodyStream, Context, Headers, Version};
use akit_config::value::{Value, parse};
use akit_config::{KubernetesIngress, KubernetesProvider};
use akit_provider::kubernetes::{Env, Kubernetes, Log, Timings};
use akit_provider::{Provider, Updates};

const TOKEN: &str = "test-token";
const INGRESSES: &str = "/apis/networking.k8s.io/v1/ingresses";
const CLASSES: &str = "/apis/networking.k8s.io/v1/ingressclasses";
const SERVICES: &str = "/api/v1/services";
const SLICES: &str = "/apis/discovery.k8s.io/v1/endpointslices";
const SECRETS: &str = "/api/v1/secrets";
const GW: &str = "/apis/gateway.networking.k8s.io/";
const GATEWAY_CLASSES: &str = "/apis/gateway.networking.k8s.io/v1/gatewayclasses";
const GATEWAYS: &str = "/apis/gateway.networking.k8s.io/v1/gateways";
const HTTP_ROUTES: &str = "/apis/gateway.networking.k8s.io/v1/httproutes";

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

/// What the fake API server holds.
#[derive(Default)]
struct State {
    rv: u64,
    /// By path, then namespace/name.
    objs: HashMap<String, BTreeMap<String, String>>,
    /// Paths answered 404.
    missing: BTreeSet<String>,
    watchers: HashMap<String, Vec<BodyStream>>,
    /// The changes stored at each path, by resource version, which a watch from an older
    /// version is sent first, as the API server does.
    events: HashMap<String, Vec<(u64, String)>>,
    lists: HashMap<String, usize>,
    watches: HashMap<String, usize>,
    /// The versions requests came over.
    versions: BTreeSet<&'static str>,
    /// The secrets' field selector, as each list or watch of secrets gave it.
    selectors: BTreeSet<String>,
    /// Status patches, by collection.
    patches: HashMap<String, usize>,
    /// How many status patches are yet to be refused with 409.
    conflicts: usize,
}

/// Stores `o` at `path`, or deletes it, with a new resource version, and tells the watches.
fn store(s: &mut State, path: &str, mut o: Value, deleted: bool) -> String {
    s.rv += 1;
    let rv = s.rv;
    let Value::Object(ms) = &mut o else { panic!() };
    let Some((_, Value::Object(meta))) = ms.iter_mut().find(|(k, _)| k == "metadata") else {
        panic!("no metadata")
    };
    meta.retain(|(k, _)| k != "resourceVersion");
    meta.push(("resourceVersion".to_owned(), Value::String(rv.to_string())));
    let field = |k: &str| match o.get("metadata").and_then(|m| m.get(k)) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let key = format!("{}/{}", field("namespace"), field("name"));
    let data = compact(&o);
    let objs = s.objs.entry(path.to_owned()).or_default();
    let typ = if deleted {
        objs.remove(&key);
        "DELETED"
    } else if objs.insert(key, data.clone()).is_some() {
        "MODIFIED"
    } else {
        "ADDED"
    };
    let e = format!(r#"{{"type": "{typ}", "object": {data}}}"#);
    s.events
        .entry(path.to_owned())
        .or_default()
        .push((rv, e.clone()));
    if let Some(ws) = s.watchers.get_mut(path) {
        ws.retain(|w| w.write(format!("{e}\n").as_bytes()).is_ok());
    }
    data
}

/// Applies a JSON merge patch to an object's status, as the API server does: refused with 409
/// when the patch names another resource version than the object's.
fn patch(s: &mut State, path: &str, content_type: &str, body: &[u8], ctx: &mut Context) {
    let Some(path) = path.strip_suffix("/status") else {
        return ctx.text(400, r#"{"kind":"Status","code":400}"#);
    };
    if content_type != "application/merge-patch+json" {
        return ctx.text(400, r#"{"kind":"Status","code":400}"#);
    }
    let (coll, name) = path.rsplit_once('/').unwrap();
    let (coll, ns) = match coll.split_once("/namespaces/") {
        Some((before, rest)) => {
            let (ns, plural) = rest.split_once('/').unwrap();
            (format!("{before}/{plural}"), ns.to_owned())
        }
        None => (coll.to_owned(), String::new()),
    };
    let Ok(p) = parse(std::str::from_utf8(body).unwrap_or("")) else {
        return ctx.text(400, r#"{"kind":"Status","code":400}"#);
    };
    *s.patches.entry(coll.clone()).or_default() += 1;
    let Some(data) = s
        .objs
        .get(&coll)
        .and_then(|o| o.get(&format!("{ns}/{name}")))
        .cloned()
    else {
        return ctx.text(404, r#"{"kind":"Status","code":404}"#);
    };
    let mut o = parse(&data).unwrap();
    let rv = o
        .get("metadata")
        .and_then(|m| m.get("resourceVersion"))
        .cloned();
    let want = p
        .get("metadata")
        .and_then(|m| m.get("resourceVersion"))
        .cloned();
    if s.conflicts > 0 || rv != want {
        s.conflicts = s.conflicts.saturating_sub(1);
        return ctx.text(
            409,
            r#"{"kind":"Status","code":409,"message":"the object has been modified"}"#,
        );
    }
    let Value::Object(ms) = &mut o else { panic!() };
    if !ms.iter().any(|(k, _)| k == "status") {
        ms.push(("status".to_owned(), Value::Object(Vec::new())));
    }
    let Some((_, Value::Object(status))) = ms.iter_mut().find(|(k, _)| k == "status") else {
        panic!()
    };
    if let Some(Value::Object(ps)) = p.get("status") {
        for (k, v) in ps {
            status.retain(|(n, _)| n != k);
            if *v != Value::Null {
                status.push((k.clone(), v.clone()));
            }
        }
    }
    let data = store(s, &coll, o, false);
    ctx.text(200, data);
}

/// A fake Kubernetes API server: it lists the objects a test puts, in pages, and streams their
/// changes to watches, from the resource version each asks for.
struct ApiServer {
    engine: Engine,
    url: String,
    /// The CA its certificate is checked against, as PEM; none in the clear.
    ca: Option<Vec<u8>>,
    state: Arc<Mutex<State>>,
    dir: PathBuf,
}

fn query<'a>(q: &'a str, k: &str) -> Option<&'a str> {
    q.split('&')
        .find_map(|kv| kv.split_once('=').filter(|(n, _)| *n == k).map(|(_, v)| v))
}

fn serve(st: &Arc<Mutex<State>>, ctx: &mut Context) {
    let req = ctx.req();
    let version = if req.version == Version::Http2 {
        "h2"
    } else {
        "h1"
    };
    let auth = req.headers.get("authorization").unwrap_or("").to_owned();
    let content_type = req.headers.get("content-type").unwrap_or("").to_owned();
    let is_patch = req.method == Method::Patch;
    let body = req.body.clone();
    let (path, q) = req
        .target
        .split_once('?')
        .map_or((req.target.clone(), String::new()), |(p, q)| {
            (p.to_owned(), q.to_owned())
        });
    let mut s = st.lock().unwrap();
    s.versions.insert(version);
    if auth != format!("Bearer {TOKEN}") {
        drop(s);
        return ctx.text(
            401,
            r#"{"kind":"Status","code":401,"message":"Unauthorized"}"#,
        );
    }
    if is_patch {
        return patch(&mut s, &path, &content_type, &body, ctx);
    }
    if s.missing.contains(&path) {
        drop(s);
        return ctx.text(404, r#"{"kind":"Status","code":404}"#);
    }
    if let Some(sel) = query(&q, "fieldSelector") {
        s.selectors.insert(sel.to_owned());
    }
    if query(&q, "watch") == Some("1") {
        *s.watches.entry(path.clone()).or_default() += 1;
        let from: u64 = query(&q, "resourceVersion")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let missed: Vec<String> = s
            .events
            .get(&path)
            .map(|es| {
                es.iter()
                    .filter(|(rv, _)| *rv > from)
                    .map(|(_, e)| e.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut h = Headers::new();
        h.set("content-type", "application/json");
        let Some(body) = ctx.stream_response(200, h) else {
            return;
        };
        for e in missed {
            let _ = body.write(format!("{e}\n").as_bytes());
        }
        s.watchers.entry(path).or_default().push(body);
        return;
    }
    *s.lists.entry(path.clone()).or_default() += 1;
    let objs = s.objs.get(&path).cloned().unwrap_or_default();
    let from: usize = query(&q, "continue")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let limit: usize = query(&q, "limit")
        .and_then(|v| v.parse().ok())
        .filter(|l| *l > 0)
        .unwrap_or(objs.len());
    let to = (from + limit).min(objs.len());
    let items: Vec<&String> = objs
        .values()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect();
    let cont = if to < objs.len() {
        to.to_string()
    } else {
        String::new()
    };
    let body = format!(
        r#"{{"metadata": {{"resourceVersion": "{}", "continue": "{cont}"}}, "items": [{}]}}"#,
        s.rv,
        items
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",")
    );
    drop(s);
    ctx.text(200, body);
}

impl ApiServer {
    /// Over TLS with ALPN `alpn` (`h2` and `http/1.1`, or `http/1.1` alone), or in the clear.
    fn start(name: &str, alpn: Option<&[&str]>) -> ApiServer {
        let dir = std::env::temp_dir().join(format!("akit-k8s-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state: Arc<Mutex<State>> = Arc::default();
        let st = Arc::clone(&state);
        let service = akio_http::serve(move |ctx: &mut Context| serve(&st, ctx));
        let l = Listener::tcp("127.0.0.1:0".parse().unwrap())
            .backend(backend())
            .threads(2);
        let (l, ca, scheme) = match alpn {
            Some(alpn) => {
                let key = akio_crypto::pkey::PrivateKey::P256(
                    akio_crypto::p256::SecretKey::generate().unwrap(),
                );
                let ca = akio_x509::CertificateParams::ca("fake API server CA")
                    .self_signed(&key)
                    .unwrap();
                let leaf_key = akio_crypto::pkey::PrivateKey::P256(
                    akio_crypto::p256::SecretKey::generate().unwrap(),
                );
                let leaf = akio_x509::CertificateParams::new(["127.0.0.1", "localhost"])
                    .signed_by(&leaf_key.public_key(), &ca, &key)
                    .unwrap();
                let scfg = akio::tls::ServerConfig::builder(
                    akio::tls::Certificate::from_pem(leaf.to_pem().as_bytes()).unwrap(),
                    akio::tls::PrivateKey::from_pem(leaf_key.to_pkcs8_pem().as_bytes()).unwrap(),
                )
                .alpn(alpn)
                .build()
                .unwrap();
                (
                    l.serve(akio::tls::Tls::server(scfg, service)),
                    Some(ca.to_pem().into_bytes()),
                    "https",
                )
            }
            None => (l.serve(service), None, "http"),
        };
        let engine = Engine::builder().listen(l).build().unwrap();
        engine.start().unwrap();
        let url = format!("{scheme}://{}", engine.local_addrs()[0]);
        ApiServer {
            engine,
            url,
            ca,
            state,
            dir,
        }
    }

    /// Adds or changes an object at `path`, given as JSON with its metadata, and tells the
    /// watches.
    fn put(&self, path: &str, doc: &str) {
        self.event(path, doc, false);
    }

    fn del(&self, path: &str, doc: &str) {
        self.event(path, doc, true);
    }

    fn event(&self, path: &str, doc: &str, deleted: bool) {
        let mut s = self.state.lock().unwrap();
        store(&mut s, path, parse(doc).unwrap(), deleted);
    }

    /// The object at `path` of namespace `ns` and name `name`.
    fn object(&self, path: &str, ns: &str, name: &str) -> Value {
        let s = self.state.lock().unwrap();
        s.objs
            .get(path)
            .and_then(|o| o.get(&format!("{ns}/{name}")))
            .map_or(Value::Null, |d| parse(d).unwrap())
    }

    fn patch_count(&self, path: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .patches
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    /// Sends a raw event to the watches of `path`.
    fn send(&self, path: &str, e: &str) {
        let mut s = self.state.lock().unwrap();
        if let Some(ws) = s.watchers.get_mut(path) {
            ws.retain(|w| w.write(format!("{e}\n").as_bytes()).is_ok());
        }
    }

    /// Ends the watches of `path`.
    fn end_watches(&self, path: &str) {
        let ws = self
            .state
            .lock()
            .unwrap()
            .watchers
            .remove(path)
            .unwrap_or_default();
        for w in ws {
            w.finish();
        }
    }

    fn counts(&self, path: &str) -> (usize, usize) {
        let s = self.state.lock().unwrap();
        (
            s.lists.get(path).copied().unwrap_or(0),
            s.watches.get(path).copied().unwrap_or(0),
        )
    }

    /// Writes a kubeconfig for the server, with its CA as data and `token`, and returns its
    /// path.
    fn kubeconfig(&self, token: &str) -> String {
        let ca = self.ca.as_ref().map_or(String::new(), |ca| {
            format!(
                "\n    certificate-authority-data: {}",
                akio_crypto::base64::STANDARD.encode(ca)
            )
        });
        let doc = format!(
            r#"apiVersion: v1
kind: Config
current-context: test
clusters:
- name: test-cluster
  cluster:
    server: {}{ca}
users:
- name: test-user
  user: {{token: "{token}"}}
contexts:
- name: other
  context: {{cluster: nowhere, user: nobody}}
- name: test
  context:
    cluster: test-cluster
    user: test-user
"#,
            self.url
        );
        let p = self.dir.join(format!("kubeconfig-{token}"));
        std::fs::write(&p, doc).unwrap();
        p.display().to_string()
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        for (_, ws) in self.state.lock().unwrap().watchers.drain() {
            for w in ws {
                w.finish();
            }
        }
        self.engine.shutdown();
        self.engine.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn compact(v: &Value) -> String {
    let mut b = String::new();
    write(&mut b, v);
    b
}

fn write(b: &mut String, v: &Value) {
    match v {
        Value::Null => b.push_str("null"),
        Value::Bool(x) => b.push_str(if *x { "true" } else { "false" }),
        Value::Number(n) => b.push_str(n),
        Value::String(s) => akit_config::value::write_string(b, s),
        Value::Array(a) => {
            b.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write(b, x);
            }
            b.push(']');
        }
        Value::Object(ms) => {
            b.push('{');
            for (i, (k, x)) in ms.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                akit_config::value::write_string(b, k);
                b.push(':');
                write(b, x);
            }
            b.push('}');
        }
    }
}

fn opts(kubeconfig: String, ingress: KubernetesIngress) -> KubernetesProvider {
    KubernetesProvider {
        kubeconfig,
        context: String::new(),
        controller_name: akit_config::DEFAULT_KUBERNETES_CONTROLLER_NAME.to_owned(),
        namespaces: Vec::new(),
        listen_host: String::new(),
        addresses: Vec::new(),
        backends: akit_config::KubernetesBackends::Endpoints,
        max_objects: 10_000,
        request_timeout: Duration::from_secs(5),
        ingress: Some(ingress),
        certificates_dir: String::new(),
    }
}

const PORTS: KubernetesIngress = KubernetesIngress {
    http_port: 8000,
    https_port: 8443,
};

/// fit's `shorten`.
fn short() -> Timings {
    Timings {
        debounce: Duration::from_millis(30),
        resync: Duration::from_secs(3600),
        min_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(100),
    }
}

/// A provider running, the parts it sends, and the lines it logs.
struct Running {
    p: Kubernetes,
    ups: Updates,
    wake: Receiver<()>,
    lines: Arc<Mutex<Vec<String>>>,
}

fn env() -> Env {
    Env {
        service_account_token: PathBuf::from("/nonexistent"),
        service_account_ca: PathBuf::from("/nonexistent"),
        service_host: None,
        service_port: None,
        kubeconfig: None,
        home: None,
    }
}

fn run(o: KubernetesProvider) -> Running {
    run_with(o, short())
}

fn run_with(o: KubernetesProvider, t: Timings) -> Running {
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let l = Arc::clone(&lines);
    let log: Log = Arc::new(move |line: &str| l.lock().unwrap().push(line.to_owned()));
    let (tx, wake) = mpsc::sync_channel(1);
    let ups = Updates::new(move || {
        let _ = tx.try_send(());
    });
    let mut p = Kubernetes::new("k8s", o)
        .with_env(env())
        .with_timings(t)
        .with_log(log);
    p.start(ups.update("k8s")).unwrap();
    Running {
        p,
        ups,
        wake,
        lines,
    }
}

impl Running {
    /// The next part, decoded.
    fn next(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some((name, data)) = self.ups.take() {
                assert_eq!(&*name, "k8s");
                return parse(std::str::from_utf8(&data).unwrap()).unwrap();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no part came; logs:\n{}", self.logs());
            let _ = self.wake.recv_timeout(left.min(Duration::from_millis(50)));
        }
    }

    fn logs(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }

    fn wait_log(&self, what: &str, ok: impl Fn(&str) -> bool) {
        wait(what, || ok(&self.logs()));
    }

    /// Stops the provider, within 10 s.
    fn stop(&mut self) {
        let start = Instant::now();
        self.p.stop();
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "stop took {:?}",
            start.elapsed()
        );
    }
}

fn wait(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "{what}: not within 10s");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

const CLASS: &str =
    r#"{"metadata": {"name": "akit"}, "spec": {"controller": "lesismal.github.io/akit"}}"#;
const WEB: &str =
    r#"{"metadata": {"namespace": "default", "name": "web"}, "spec": {"ports": [{"port": 80}]}}"#;
const INGRESS: &str = r#"{"metadata": {"namespace": "default", "name": "i"}, "spec": {"ingressClassName": "akit",
  "rules": [{"http": {"paths": [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "web", "port": {"number": 80}}}}]}}]}}"#;

fn slice(name: &str, addr: &str) -> String {
    format!(
        r#"{{"metadata": {{"namespace": "default", "name": "{name}", "labels": {{"kubernetes.io/service-name": "web"}}}},
  "endpoints": [{{"addresses": ["{addr}"]}}], "ports": [{{"port": 8080}}]}}"#
    )
}

/// fit's `TestProvider` and `TestProviderIngress`, over HTTP/2: nothing is sent until every
/// kind is listed, lists come in pages, watches carry the changes, a watch too old makes a new
/// list, an ended watch is made again, from where it was.
#[test]
fn provider() {
    let a = ApiServer::start("provider", Some(&["h2", "http/1.1"]));
    a.put(CLASSES, CLASS);
    a.put(SERVICES, WEB);
    // More slices than a page holds.
    for i in 0..520 {
        a.put(
            SLICES,
            &slice(
                &format!("web-{i:03}"),
                &format!("10.1.{}.{}", i / 250, i % 250),
            ),
        );
    }
    let mut r = run(opts(a.kubeconfig(TOKEN), PORTS));
    let first = r.next();
    assert!(
        list(&first, "routes").is_empty(),
        "at the start: {}",
        compact(&first)
    );
    assert_eq!(a.counts(SLICES).0, 2, "pages of slices");

    // An Ingress comes: one upstream of every slice's endpoint.
    a.put(INGRESSES, INGRESS);
    let p1 = r.next();
    let routes = list(&p1, "routes");
    assert_eq!(routes.len(), 1, "{}", compact(&p1));
    assert_eq!(
        compact(routes[0].get("listeners").unwrap()),
        r#"["k8s-8000"]"#
    );
    assert_eq!(
        compact(routes[0].get("handler").unwrap()),
        r#"{"module":"proxy","params":{"upstream":"default.web.80"}}"#
    );
    let ups = list(&p1, "upstreams");
    assert_eq!((ups.len(), list(&ups[0], "addresses").len()), (1, 520));
    assert_eq!(
        compact(list(&p1, "listeners").first().unwrap()),
        r#"{"name":"k8s-8000","address":":8000","protocols":["http/1.1","h2c"]}"#
    );

    // The watch of slices is too old: they are listed again, as they are now, which the watch
    // never said.
    let (lists, _) = a.counts(SLICES);
    {
        let mut s = a.state.lock().unwrap();
        let mut objs = BTreeMap::new();
        objs.insert(
            "default/web-new".to_owned(),
            slice("web-new", "10.9.9.9").replace('\n', ""),
        );
        s.objs.insert(SLICES.to_owned(), objs);
    }
    a.send(
        SLICES,
        r#"{"type": "ERROR", "object": {"kind": "Status", "code": 410, "message": "too old"}}"#,
    );
    let p2 = r.next();
    assert_eq!(
        compact(
            list(&p2, "upstreams")
                .first()
                .unwrap()
                .get("addresses")
                .unwrap()
        ),
        r#"["10.9.9.9:8080"]"#
    );
    assert_eq!(a.counts(SLICES).0, lists + 1, "lists after a watch too old");

    // An ended watch is made again, from where it was: a change after it still comes.
    let (_, watches) = a.counts(INGRESSES);
    a.end_watches(INGRESSES);
    wait("an ended watch made again", || {
        a.counts(INGRESSES).1 > watches
    });
    a.del(INGRESSES, INGRESS);
    let p3 = r.next();
    assert!(
        list(&p3, "routes").is_empty(),
        "after the Ingress went: {}",
        compact(&p3)
    );

    r.stop();
    let logs = r.logs();
    assert!(
        logs.contains(&format!("provider k8s: watching the cluster at {}", a.url)),
        "{logs}"
    );
    let s = a.state.lock().unwrap();
    assert_eq!(s.versions.iter().copied().collect::<Vec<_>>(), ["h2"]);
    assert_eq!(
        s.selectors.iter().cloned().collect::<Vec<_>>(),
        ["type%3Dkubernetes.io%2Ftls"]
    );
    // Nothing more is sent once stopped.
    drop(s);
    a.put(INGRESSES, INGRESS);
    std::thread::sleep(Duration::from_millis(100));
    assert!(r.ups.take().is_none());
}

/// The same over HTTP/1.1: in the clear, and over TLS when the server chooses HTTP/1.1 in
/// ALPN. A kind the API server does not serve counts as empty, and is looked for again.
#[test]
fn provider_http1() {
    for (name, alpn) in [("h1", None), ("h1-tls", Some(&["http/1.1"][..]))] {
        let a = ApiServer::start(name, alpn);
        a.state.lock().unwrap().missing.insert(CLASSES.to_owned());
        a.put(SERVICES, WEB);
        a.put(SLICES, &slice("web-1", "10.1.0.1"));
        let mut r = run(opts(a.kubeconfig(TOKEN), PORTS));
        assert!(list(&r.next(), "routes").is_empty(), "{name}");
        r.wait_log(name, |l| {
            l.contains("provider k8s: ingressclasses are not served by the API server (is the Gateway API installed?); looking again every 100ms")
        });
        a.put(INGRESSES, INGRESS);
        // No class: no route yet; once the class is served, the Ingress is akit's.
        a.state.lock().unwrap().missing.clear();
        a.put(CLASSES, CLASS);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let p = r.next();
            if list(&p, "routes").len() == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "{name}: no route");
        }
        r.wait_log(name, |l| {
            l.contains("provider k8s: ingressclasses are served now")
        });
        r.stop();
        assert_eq!(
            a.state
                .lock()
                .unwrap()
                .versions
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            ["h1"],
            "{name}"
        );
    }
}

/// fit's `TestProviderUnauthorized`: a list refused is reported and tried again, and nothing is
/// sent meanwhile.
#[test]
fn unauthorized() {
    let a = ApiServer::start("unauthorized", Some(&["h2", "http/1.1"]));
    let mut r = run(opts(a.kubeconfig("wrong"), PORTS));
    r.wait_log("two refusals of each kind", |l| {
        [
            "ingressclasses",
            "ingresses",
            "services",
            "endpointslices",
            "secrets",
        ]
        .iter()
        .all(|k| {
            l.matches(&format!("listing {k}: GET /")).count() >= 2
                && l.contains("401 Unauthorized: Unauthorized; trying again in")
        })
    });
    assert!(r.ups.take().is_none(), "a part while unauthorized");
    r.stop();
}

/// fit's `TestProviderSetupFails`: with no cluster to reach, it says so and tries again, and
/// stops at once; a server that does not answer is said too.
#[test]
fn setup_fails() {
    let mut r = run(opts("/nonexistent/kubeconfig".to_owned(), PORTS));
    r.wait_log("no kubeconfig", |l| {
        l.contains("provider k8s: kubeconfig: /nonexistent/kubeconfig: ")
            && l.contains("; trying again in 100ms")
    });
    r.stop();

    // A server nobody listens on.
    let dir = std::env::temp_dir().join(format!("akit-k8s-setup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kc = dir.join("kubeconfig");
    std::fs::write(
        &kc,
        "current-context: c\nclusters: [{name: c, cluster: {server: \"https://127.0.0.1:1\", insecure-skip-tls-verify: true}}]\nusers: [{name: u, user: {token: t}}]\ncontexts: [{name: c, context: {cluster: c, user: u}}]\n",
    )
    .unwrap();
    let mut r = run(opts(kc.display().to_string(), PORTS));
    r.wait_log("no server", |l| {
        l.contains("provider k8s: connecting to https://127.0.0.1:1: ")
            && l.matches("; trying again in").count() >= 2
    });
    assert!(r.ups.take().is_none());
    r.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

fn cert(names: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let key =
        akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
    let cert = akio_x509::CertificateParams::new(names)
        .self_signed(&key)
        .unwrap();
    (cert.to_pem().into_bytes(), key.to_pkcs8_pem().into_bytes())
}

fn files(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// An Ingress's TLS: its Secret's chain and key written to certificates_dir, named by the
/// HTTPS listener, its hosts' HTTP redirected; the file removed once the Secret goes. Without
/// certificates_dir, a directory of the provider's own, removed when it stops.
#[test]
fn ingress_tls() {
    let a = ApiServer::start("tls", Some(&["h2", "http/1.1"]));
    let (crt, key) = cert(&["shop.example.com"]);
    let b64 = |b: &[u8]| akio_crypto::base64::STANDARD.encode(b);
    let secret = format!(
        r#"{{"metadata": {{"namespace": "default", "name": "shop-tls"}}, "type": "kubernetes.io/tls", "data": {{"tls.crt": "{}", "tls.key": "{}"}}}}"#,
        b64(&crt),
        b64(&key)
    );
    a.put(CLASSES, CLASS);
    a.put(SERVICES, WEB);
    a.put(SLICES, &slice("web-1", "10.1.0.1"));
    a.put(SECRETS, &secret);
    a.put(
        INGRESSES,
        r#"{"metadata": {"namespace": "default", "name": "shop", "annotations": {"akit.lesismal.github.io/ssl-redirect": "true"}},
  "spec": {"ingressClassName": "akit", "tls": [{"hosts": ["shop.example.com"], "secretName": "shop-tls"}],
  "rules": [{"host": "shop.example.com", "http": {"paths": [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "web", "port": {"number": 80}}}}]}}]}}"#,
    );
    let certs = a.dir.join("certs");
    let mut o = opts(a.kubeconfig(TOKEN), PORTS);
    o.certificates_dir = certs.display().to_string();
    let mut r = run(o);
    let p = r.next();
    let ls = list(&p, "listeners");
    assert_eq!(ls.len(), 2, "{}", compact(&p));
    let file = match ls[1].get("tls").and_then(|t| t.get("certificates")) {
        Some(Value::Array(cs)) => match cs[0].get("cert_file") {
            Some(Value::String(f)) => f.clone(),
            _ => panic!(),
        },
        _ => panic!("{}", compact(&p)),
    };
    assert!(file.starts_with(&certs.display().to_string()), "{file}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        [&crt[..], b"\n", &key[..]].concat()
    );
    // The part is a config akit takes, its certificate file loaded.
    akit_config::from_json(&compact(&p), None).unwrap();
    let names: Vec<String> = list(&p, "routes")
        .iter()
        .map(|r| compact(r.get("name").unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            r#""default.shop.ing.r0.p0""#,
            r#""default.shop.ing.r0.p0.redirect""#
        ]
    );

    // The Secret goes: HTTP alone, the file removed.
    a.del(SECRETS, &secret);
    let p = r.next();
    assert_eq!(list(&p, "listeners").len(), 1, "{}", compact(&p));
    wait("the file removed", || files(&certs).is_empty());
    r.wait_log("the Secret missing", |l| {
        l.contains("provider k8s: ingress default/shop: tls 0: there is no secret default/shop-tls of type kubernetes.io/tls")
    });
    r.stop();
    // certificates_dir is the user's: kept.
    assert!(certs.exists());

    // A directory of the provider's own: removed when it stops.
    a.put(SECRETS, &secret);
    let mut r = run(opts(a.kubeconfig(TOKEN), PORTS));
    let p = r.next();
    let own = match list(&p, "listeners")[1]
        .get("tls")
        .and_then(|t| t.get("certificates"))
    {
        Some(Value::Array(cs)) => match cs[0].get("cert_file") {
            Some(Value::String(f)) => PathBuf::from(f).parent().unwrap().to_owned(),
            _ => panic!(),
        },
        _ => panic!("{}", compact(&p)),
    };
    assert!(own.exists());
    r.stop();
    assert!(!own.exists(), "{}", own.display());
}

/// A provider of the Gateway API alone, without Ingresses.
fn gateway_opts(kubeconfig: String) -> KubernetesProvider {
    KubernetesProvider {
        ingress: None,
        ..opts(kubeconfig, PORTS)
    }
}

const GW_CLASS: &str =
    r#"{"metadata": {"name": "akit"}, "spec": {"controllerName": "lesismal.github.io/akit"}}"#;

/// fit's `TestProvider`, for the Gateway API: nothing is sent until every kind is listed, a
/// kind served in neither of its versions counts as empty, lists come in pages, a route comes
/// and goes through its watch, and no Ingresses are watched without `ingress`.
#[test]
fn gateway_provider() {
    let a = ApiServer::start("gateway", Some(&["h2", "http/1.1"]));
    {
        let mut s = a.state.lock().unwrap();
        s.missing.insert(format!("{GW}v1beta1/referencegrants"));
        s.missing.insert(format!("{GW}v1/referencegrants"));
    }
    a.put(GATEWAY_CLASSES, GW_CLASS);
    a.put(
        GATEWAYS,
        r#"{"metadata": {"namespace": "default", "name": "gw"}, "spec": {"gatewayClassName": "akit", "listeners": [{"name": "http", "port": 8000, "protocol": "HTTP"}]}}"#,
    );
    a.put(SERVICES, WEB);
    for i in 0..520 {
        a.put(
            SLICES,
            &slice(
                &format!("web-{i:03}"),
                &format!("10.1.{}.{}", i / 250, i % 250),
            ),
        );
    }
    let mut r = run(gateway_opts(a.kubeconfig(TOKEN)));
    let first = r.next();
    assert_eq!(
        compact(first.get("listeners").unwrap()),
        r#"[{"name":"k8s-8000","address":":8000","protocols":["http/1.1","h2c"]}]"#
    );
    assert!(list(&first, "routes").is_empty() && list(&first, "upstreams").is_empty());
    assert_eq!(a.counts(SLICES).0, 2, "pages of slices");

    // A route comes: one upstream of every slice's endpoint.
    let route = r#"{"metadata": {"namespace": "default", "name": "r"}, "spec": {"parentRefs": [{"name": "gw"}], "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}}"#;
    a.put(HTTP_ROUTES, route);
    let p1 = r.next();
    assert_eq!(list(&p1, "routes").len(), 1, "{}", compact(&p1));
    let ups = list(&p1, "upstreams");
    assert_eq!((ups.len(), list(&ups[0], "addresses").len()), (1, 520));
    // The route goes.
    a.del(HTTP_ROUTES, route);
    let p2 = r.next();
    assert!(list(&p2, "routes").is_empty(), "{}", compact(&p2));
    r.stop();
    let logs = r.logs();
    assert!(
        logs.contains("provider k8s: referencegrants are not served by the API server"),
        "{logs}"
    );
    // Without ingress, no Ingresses are asked for.
    assert_eq!(a.counts(INGRESSES), (0, 0));
    assert_eq!(a.counts(CLASSES), (0, 0));
}

/// The condition of type `typ` in `cs`, as "status reason"; "" when there is none.
fn condition(cs: Option<&Value>, typ: &str) -> String {
    let s = |c: &Value, k: &str| match c.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    match cs {
        Some(Value::Array(cs)) => cs
            .iter()
            .find(|c| s(c, "type") == typ)
            .map(|c| format!("{} {}", s(c, "status"), s(c, "reason")))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// fit's `TestProviderStatus`: the statuses of the class, the Gateway and its listeners, and
/// routes, written once and then left; a write refused made again; a TLSRoute at the version
/// the server has; and an HTTPS listener's certificate written to a file, and removed with its
/// Secret.
#[test]
fn gateway_status() {
    let a = ApiServer::start("status", Some(&["h2", "http/1.1"]));
    {
        let mut s = a.state.lock().unwrap();
        s.missing.insert(format!("{GW}v1/tlsroutes"));
        s.missing.insert(format!("{GW}v1alpha3/tlsroutes"));
    }
    let tls_path = format!("{GW}v1alpha2/tlsroutes");
    let (crt, key) = cert(&["a.example.com"]);
    let b64 = |b: &[u8]| akio_crypto::base64::STANDARD.encode(b);
    let secret = format!(
        r#"{{"metadata": {{"namespace": "default", "name": "a"}}, "type": "kubernetes.io/tls", "data": {{"tls.crt": "{}", "tls.key": "{}"}}}}"#,
        b64(&crt),
        b64(&key)
    );
    a.put(SECRETS, &secret);
    a.put(GATEWAY_CLASSES, GW_CLASS);
    a.put(
        GATEWAYS,
        r#"{"metadata": {"namespace": "default", "name": "gw", "generation": 1}, "spec": {"gatewayClassName": "akit", "listeners": [
  {"name": "http", "port": 8000, "protocol": "HTTP"},
  {"name": "https", "port": 8443, "protocol": "HTTPS", "tls": {"certificateRefs": [{"name": "a"}]}},
  {"name": "pass", "port": 9443, "protocol": "TLS", "tls": {"mode": "Passthrough"}}]}}"#,
    );
    a.put(SERVICES, WEB);
    a.put(SLICES, &slice("web-1", "10.1.0.1"));
    a.put(
        HTTP_ROUTES,
        r#"{"metadata": {"namespace": "default", "name": "r", "generation": 1}, "spec": {"parentRefs": [{"name": "gw", "sectionName": "http"}],
  "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}}"#,
    );
    a.put(
        &tls_path,
        r#"{"metadata": {"namespace": "default", "name": "db", "generation": 1}, "spec": {"parentRefs": [{"name": "gw"}],
  "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}}"#,
    );
    let certs = a.dir.join("certs");
    let mut o = gateway_opts(a.kubeconfig(TOKEN));
    o.certificates_dir = certs.display().to_string();
    let resync = Duration::from_millis(200);
    let mut r = run_with(o, Timings { resync, ..short() });

    let route_status = |path: &str, name: &str| {
        let o = a.object(path, "default", name);
        match o.get("status").and_then(|s| s.get("parents")) {
            Some(Value::Array(ps)) if ps.len() == 1 => {
                let cs = ps[0].get("conditions");
                format!(
                    "{}, {}",
                    condition(cs, "Accepted"),
                    condition(cs, "ResolvedRefs")
                )
            }
            _ => String::new(),
        }
    };
    let status_of = |path: &str, ns: &str, name: &str| {
        a.object(path, ns, name)
            .get("status")
            .cloned()
            .unwrap_or(Value::Null)
    };
    wait("the statuses", || {
        let gs = status_of(GATEWAYS, "default", "gw");
        let cs = status_of(GATEWAY_CLASSES, "", "akit");
        condition(gs.get("conditions"), "Programmed") == "True Programmed"
            && condition(cs.get("conditions"), "Accepted") == "True Accepted"
            && route_status(HTTP_ROUTES, "r") == "True Accepted, True ResolvedRefs"
            && route_status(&tls_path, "db") == "True Accepted, True ResolvedRefs"
    });
    let gs = status_of(GATEWAYS, "default", "gw");
    let http = r#"[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute"},{"group":"gateway.networking.k8s.io","kind":"GRPCRoute"}]"#;
    for l in list(&gs, "listeners") {
        let name = compact(l.get("name").unwrap());
        let want = match name.as_str() {
            r#""http""# => format!("1 {http}"),
            r#""https""# => format!("0 {http}"),
            _ => r#"1 [{"group":"gateway.networking.k8s.io","kind":"TLSRoute"}]"#.to_owned(),
        };
        let got = format!(
            "{} {}",
            compact(l.get("attachedRoutes").unwrap()),
            compact(l.get("supportedKinds").unwrap())
        );
        assert_eq!(got, want, "listener {name}");
    }

    // The certificate is in a file of its own, which the part names.
    let fs = files(&certs);
    assert_eq!(fs.len(), 1, "{fs:?}");
    assert!(fs[0].starts_with("default.a.") && fs[0].ends_with(".pem"));
    let file = certs.join(&fs[0]);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        [&crt[..], b"\n", &key[..]].concat()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let file_s = file.display().to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let p = r.next();
        let ls = list(&p, "listeners");
        if ls.len() == 3
            && compact(ls[1].get("tls").unwrap_or(&Value::Null)).contains(&format!("\"{file_s}\""))
        {
            akit_config::from_json(&compact(&p), None).unwrap();
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the HTTPS listener: {}",
            compact(&p)
        );
    }

    // Written once: resyncs and the watches of what was written write nothing more. The writes
    // are let settle first, two resyncs in a row with none.
    let count = || {
        a.patch_count(GATEWAY_CLASSES)
            + a.patch_count(GATEWAYS)
            + a.patch_count(HTTP_ROUTES)
            + a.patch_count(&tls_path)
    };
    let mut before = count();
    let deadline = Instant::now() + resync * 30;
    let mut quiet = 0;
    while quiet < 2 {
        std::thread::sleep(resync);
        // The parts each resync sends are not read here: drop them.
        while r.ups.take().is_some() {}
        let n = count();
        if n == before {
            quiet += 1;
            continue;
        }
        assert!(
            Instant::now() < deadline,
            "status writes did not settle: {} more",
            n - before
        );
        quiet = 0;
        before = n;
    }
    std::thread::sleep(resync * 3);
    assert_eq!(count(), before, "status writes with nothing changed");

    // A write refused is made again: the next resync finds the status still to write.
    a.state.lock().unwrap().conflicts = 1;
    a.put(
        HTTP_ROUTES,
        r#"{"metadata": {"namespace": "default", "name": "r", "generation": 2}, "spec": {"parentRefs": [{"name": "gw", "sectionName": "http"}],
  "rules": [{"backendRefs": [{"name": "gone", "port": 80}]}]}}"#,
    );
    wait("the route's status after a refused write", || {
        route_status(HTTP_ROUTES, "r") == "True Accepted, False BackendNotFound"
    });

    // The Secret gone: the listener is not served, and its file goes.
    a.del(SECRETS, &secret);
    wait("the certificate file removed", || !file.exists());
    wait("the HTTPS listener's status", || {
        compact(&status_of(GATEWAYS, "default", "gw"))
            .contains(r#""reason":"InvalidCertificateRef""#)
    });
    r.stop();
    assert!(!r.logs().contains("writing the status of"), "{}", r.logs());
}
