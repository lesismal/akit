//! The `jwt` middleware through akit's listener on akio, with fit's tests
//! (`middleware/auth/jwt_test.go`, and the jwt parts of `cover_test.go`) ported, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`): every algorithm with keys from PEM files
//! and a secret, keys of the wrong kind or size, `none`, an RSA public key taken for an HMAC
//! secret, the claims (`exp`, `nbf`, `iss`, `aud`, the leeway) and malformed tokens, where the
//! token is found, `claims_headers` and the fields a client may not forge, the params akit
//! refuses; a JWK set fetched when akit starts (a request before waits for it), rotated (a token
//! of a new kid has it fetched again and waits; a dropped key no longer verifies; unknown kids
//! fetch it at most every `MIN_REFETCH`), refreshed, failing to load (the keys there were kept),
//! out of reach (requests refused, then let in once it is back); a client that goes while its
//! request waits; and bodies through `jwt` to the `proxy` handler, a request that waits for the
//! set among them.
//!
//! The RSA keys are `tests/data/jwt-rsa1.key` and `jwt-rsa2.key`, made for these tests only
//! (`openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048`): akio-crypto does not make
//! RSA keys. The JWK set is served by a test server on plain threads. The route's handler is a
//! test one, registered as `respond`: it answers with the fields `claims_headers` sets, as an
//! application behind CGI would read them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::{Backend, Engine, UringConfig};
use akio_crypto::base64::URL_SAFE_NO_PAD as B64;
use akio_crypto::hash::{Sha256, Sha384, Sha512};
use akio_crypto::mac::Hmac;
use akio_crypto::pkey::{PrivateKey, PublicKey};
use akio_crypto::rsa::HashAlg;
use akio_crypto::{ed25519, p256, p384, p521};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Info, Registry, compile};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

/// The test handler: fit's `claimsEcho`, as far as akit has it.
mod fields {
    use std::sync::Arc;

    use akio_http::{Context, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    /// What an application behind CGI reads as field `name`: fit's `asCGI`.
    fn as_cgi(h: &akio_http::Headers, name: &str) -> String {
        let fold = |s: &str| s.replace('_', "-").to_ascii_lowercase();
        let mut values: Vec<&str> = h
            .iter()
            .filter(|(n, _)| fold(n) == fold(name))
            .map(|(_, v)| v)
            .collect();
        values.sort_unstable();
        values.join(",")
    }

    struct Fields;

    impl akio_http::Service for Fields {
        fn call(&self, ctx: &mut Context) {
            let body = {
                let h = &ctx.req().headers;
                format!(
                    "x-sub={} x-roles={} x-missing={}",
                    as_cgi(h, "X-Sub"),
                    as_cgi(h, "X-Roles"),
                    as_cgi(h, "X-Missing")
                )
            };
            ctx.respond(Response::text(200, body));
        }
    }

    impl Handler for Fields {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Fields) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] =
        &[Info::handler("http.handlers.respond", "Fields.", new)];
}

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[
        fields::MODULES as &[Info<HandlerParams>],
        akit_middleware::MODULES,
        akit_proxy::MODULES,
    ])
    .unwrap()
}

/// akit serving `json`.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry(), &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = engine.local_addrs()[0];
        Akit {
            engine,
            addr,
            _cell: cell,
        }
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A config with one route for each of `routes`, (path, jwt params), and a route `/open`
/// without jwt.
fn doc(routes: &[(&str, String)]) -> String {
    let mut rs: Vec<String> = routes
        .iter()
        .map(|(path, params)| {
            format!(
                r#"{{"match": {{"path": ["{path}"]}}, "middleware": [{{"module": "jwt", "params": {params}}}],
                    "handler": {{"module": "respond"}}}}"#
            )
        })
        .collect();
    rs.push(r#"{"match": {"path": ["/open"]}, "handler": {"module": "respond"}}"#.into());
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": [{}]}}"#,
        rs.join(",")
    )
}

// ---- keys and tokens ----

/// One key of each kind, as fit's `testKeys`.
struct Keyring {
    rsa: PrivateKey,
    rsa2: PrivateKey,
    p256: PrivateKey,
    p384: PrivateKey,
    p521: PrivateKey,
    ed: PrivateKey,
    secret: Vec<u8>,
}

fn rsa_key(file: &str) -> PrivateKey {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(file);
    let pem = std::fs::read(path).unwrap();
    let der = akio_crypto::pem::decode_label(&pem, "PRIVATE KEY").unwrap();
    PrivateKey::from_pkcs8_der(&der[0]).unwrap()
}

fn keyring() -> Keyring {
    Keyring {
        rsa: rsa_key("jwt-rsa1.key"),
        rsa2: rsa_key("jwt-rsa2.key"),
        p256: PrivateKey::P256(p256::SecretKey::generate().unwrap()),
        p384: PrivateKey::P384(p384::SecretKey::generate().unwrap()),
        p521: PrivateKey::P521(p521::SecretKey::generate().unwrap()),
        ed: PrivateKey::Ed25519(ed25519::SigningKey::generate().unwrap()),
        secret: b"0123456789abcdef0123456789abcdef".to_vec(),
    }
}

/// What signs a token.
#[derive(Clone, Copy)]
enum Signer<'a> {
    Key(&'a PrivateKey),
    Secret(&'a [u8]),
    None,
}

impl Keyring {
    /// The key `alg` signs with, as fit's `signer`.
    fn signer(&self, alg: &str) -> Signer<'_> {
        match alg {
            "ES256" => Signer::Key(&self.p256),
            "ES384" => Signer::Key(&self.p384),
            "ES512" => Signer::Key(&self.p521),
            "EdDSA" => Signer::Key(&self.ed),
            a if a.starts_with("HS") => Signer::Secret(&self.secret),
            _ => Signer::Key(&self.rsa),
        }
    }
}

fn hash_of(alg: &str) -> HashAlg {
    match &alg[2..] {
        "384" => HashAlg::Sha384,
        "512" => HashAlg::Sha512,
        _ => HashAlg::Sha256,
    }
}

/// A token with `header` and `claims` (JSON), signed by `alg`'s way with `signer`.
fn sign_raw(alg: &str, signer: Signer<'_>, header: &str, claims: &str) -> String {
    let signed = format!("{}.{}", B64.encode(header), B64.encode(claims));
    let msg = signed.as_bytes();
    let sig: Vec<u8> = match (signer, &alg[..2]) {
        (Signer::None, _) => Vec::new(),
        (Signer::Secret(s), _) => match hash_of(alg) {
            HashAlg::Sha384 => Hmac::<Sha384>::mac(s, msg).as_ref().to_vec(),
            HashAlg::Sha512 => Hmac::<Sha512>::mac(s, msg).as_ref().to_vec(),
            _ => Hmac::<Sha256>::mac(s, msg).as_ref().to_vec(),
        },
        (Signer::Key(PrivateKey::Rsa(k)), "RS") => k.sign_pkcs1v15(hash_of(alg), msg).unwrap(),
        (Signer::Key(PrivateKey::Rsa(k)), "PS") => k.sign_pss(hash_of(alg), msg).unwrap(),
        (Signer::Key(PrivateKey::P256(k)), _) => match hash_of(alg) {
            HashAlg::Sha384 => k.sign_with::<Sha384>(msg).to_fixed(),
            _ => k.sign_with::<Sha256>(msg).to_fixed(),
        },
        (Signer::Key(PrivateKey::P384(k)), _) => k.sign_with::<Sha384>(msg).to_fixed(),
        (Signer::Key(PrivateKey::P521(k)), _) => k.sign_with::<Sha512>(msg).to_fixed(),
        (Signer::Key(PrivateKey::Ed25519(k)), _) => k.sign(msg).to_vec(),
        _ => panic!("no way to sign {alg}"),
    };
    format!("{signed}.{}", B64.encode(sig))
}

/// A token of `claims`, its header `alg` and, if not empty, `kid`: fit's `sign`.
fn sign(alg: &str, kid: &str, signer: Signer<'_>, claims: &str) -> String {
    let header = if kid.is_empty() {
        format!(r#"{{"alg":"{alg}","typ":"JWT"}}"#)
    } else {
        format!(r#"{{"alg":"{alg}","typ":"JWT","kid":"{kid}"}}"#)
    };
    sign_raw(alg, signer, &header, claims)
}

fn unix_now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// fit's `valid`: alice's, for an hour.
fn valid() -> String {
    format!(r#"{{"sub":"alice","exp":{}}}"#, unix_now() + 3600)
}

/// A fresh directory for a test's files, under the target's.
fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("jwt-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `k`'s public keys as PEM files and its secret, in `d`: fit's `keyFiles`.
fn key_files(d: &Path, k: &Keyring) -> (Vec<String>, String) {
    let mut pubs = Vec::new();
    for (i, key) in [&k.rsa, &k.p256, &k.p384, &k.p521, &k.ed]
        .into_iter()
        .enumerate()
    {
        let p = d.join(format!("pub{i}.pem"));
        let pem = akio_crypto::pem::encode("PUBLIC KEY", &key.public_key().to_spki_der());
        std::fs::write(&p, pem).unwrap();
        pubs.push(path_str(&p));
    }
    let secret = d.join("secret");
    std::fs::write(&secret, &k.secret).unwrap();
    (pubs, path_str(&secret))
}

fn json_list(list: &[String]) -> String {
    let quoted: Vec<String> = list.iter().map(|s| format!("{s:?}")).collect();
    format!("[{}]", quoted.join(","))
}

/// The JWK of `key`'s public key, with `kid`: fit's `jwkOf`.
fn jwk_of(key: &PrivateKey, kid: &str) -> String {
    match key.public_key() {
        PublicKey::Rsa(k) => {
            let e = k.exponent().to_be_bytes();
            let skip = e.iter().take_while(|&&b| b == 0).count();
            format!(
                r#"{{"kty":"RSA","kid":"{kid}","n":"{}","e":"{}"}}"#,
                B64.encode(k.modulus()),
                B64.encode(&e[skip..])
            )
        }
        PublicKey::Ed25519(k) => format!(
            r#"{{"kty":"OKP","kid":"{kid}","crv":"Ed25519","x":"{}"}}"#,
            B64.encode(k.as_bytes())
        ),
        other => {
            let point = other.raw_key();
            let (crv, size) = match point.len() {
                65 => ("P-256", 32),
                97 => ("P-384", 48),
                _ => ("P-521", 66),
            };
            format!(
                r#"{{"kty":"EC","kid":"{kid}","crv":"{crv}","x":"{}","y":"{}"}}"#,
                B64.encode(&point[1..=size]),
                B64.encode(&point[1 + size..])
            )
        }
    }
}

// ---- the client ----

/// A response as the client saw it.
#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A request or response head: its first line's second word (a target or a status), and its
/// fields.
struct Head {
    second: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn get(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }
}

fn read_head(r: &mut impl BufRead) -> Option<Head> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split(' ');
    parts.next()?;
    let second = parts.next()?.trim().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':')?;
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    Some(Head { second, headers })
}

/// A `GET` of `path` on a connection of its own, with `headers`.
fn get(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: example.com\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    (&s).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let head = read_head(&mut r).expect("a response");
    let length: usize = head.get("content-length").parse().unwrap_or(0);
    let mut body = vec![0; length];
    r.read_exact(&mut body).unwrap();
    Resp {
        status: head.second.parse().unwrap(),
        headers: head.headers,
        body: String::from_utf8(body).unwrap(),
    }
}

/// A `GET` of `path` with `token` after `Bearer`.
fn bearer(addr: SocketAddr, path: &str, token: &str) -> Resp {
    get(addr, path, &[("Authorization", &format!("Bearer {token}"))])
}

/// The least time between fetches of a JWK set in these tests: one for all, since the setting is
/// the process's and the tests run at once.
const MIN_REFETCH: Duration = Duration::from_millis(50);

const INVALID: &str = r#"Bearer realm="akit", error="invalid_token""#;

// ---- fit's tests ----

// fit's TestJWTAlgorithms.
#[test]
fn jwt_algorithms() {
    let k = keyring();
    let d = dir("algorithms");
    let (pubs, secret) = key_files(&d, &k);
    let akit = Akit::start(&doc(&[
        (
            "/all",
            format!(
                r#"{{"public_key_files": {}, "hmac_secret_file": "{secret}"}}"#,
                json_list(&pubs)
            ),
        ),
        (
            "/only",
            format!(
                r#"{{"public_key_files": {}, "algorithms": ["ES256"]}}"#,
                json_list(&pubs)
            ),
        ),
    ]));
    for alg in akit_config::JWT_ALGORITHMS {
        let r = bearer(akit.addr, "/all", &sign(alg, "", k.signer(alg), &valid()));
        assert_eq!(r.status, 200, "{alg}: {r:?}");
    }

    // A key of the wrong size or kind, another key, none, and the RSA public key taken for an
    // HMAC secret.
    let pem_rsa = std::fs::read(&pubs[0]).unwrap();
    for (what, token) in [
        (
            "ES384 by a P-256 key",
            sign("ES384", "", Signer::Key(&k.p256), &valid()),
        ),
        (
            "RS256 by another key",
            sign("RS256", "", Signer::Key(&k.rsa2), &valid()),
        ),
        ("none", sign("none", "", Signer::None, &valid())),
        (
            "HS256 by the RSA public key",
            sign("HS256", "", Signer::Secret(&pem_rsa), &valid()),
        ),
    ] {
        let r = bearer(akit.addr, "/all", &token);
        assert_eq!(r.status, 401, "{what}");
        assert_eq!(r.header("www-authenticate"), Some(INVALID), "{what}");
        assert_eq!(r.body, "Unauthorized\n");
    }

    let only = |alg: &str| bearer(akit.addr, "/only", &sign(alg, "", k.signer(alg), &valid()));
    assert_eq!(only("ES256").status, 200, "allowed");
    assert_eq!(only("RS256").status, 401, "not allowed");
    assert_eq!(get(akit.addr, "/open", &[]).status, 200);
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}

// fit's TestJWTClaims, through the server, with its clock.
#[test]
fn jwt_claims() {
    let k = keyring();
    let d = dir("claims");
    let (_, secret) = key_files(&d, &k);
    let akit = Akit::start(&doc(&[(
        "/p",
        format!(
            r#"{{"hmac_secret_file": "{secret}", "issuer": "https://issuer.example",
                 "audience": ["api", "web"], "leeway": "30s"}}"#
        ),
    )]));
    let now = unix_now();
    let base = |more: &str| {
        let mut c = vec![
            r#""sub":"alice""#.to_owned(),
            r#""iss":"https://issuer.example""#.to_owned(),
            r#""aud":"api""#.to_owned(),
            format!(r#""exp":{}"#, now + 60),
        ];
        for m in more.split(';').filter(|m| !m.is_empty()) {
            let name = m.split(':').next().unwrap();
            c.retain(|x| !x.starts_with(name));
            if !m.ends_with(":-") {
                c.push(m.to_owned());
            }
        }
        format!("{{{}}}", c.join(","))
    };
    let hs = |claims: &str| sign("HS256", "", Signer::Secret(&k.secret), claims);
    for (what, claims, code) in [
        ("valid", base(""), 200),
        (
            "expired within the leeway",
            base(&format!(r#""exp":{}"#, now - 20)),
            200,
        ),
        ("expired", base(&format!(r#""exp":{}"#, now - 40)), 401),
        ("no exp", base(r#""exp":-"#), 401),
        (
            "exp a string",
            base(&format!(r#""exp":"{}""#, now + 60)),
            401,
        ),
        (
            "fractional exp",
            base(&format!(r#""exp":{}.5"#, now + 60)),
            200,
        ),
        (
            "nbf within the leeway",
            base(&format!(r#""nbf":{}"#, now + 20)),
            200,
        ),
        ("not yet", base(&format!(r#""nbf":{}"#, now + 40)), 401),
        (
            "another issuer",
            base(r#""iss":"https://evil.example""#),
            401,
        ),
        ("no issuer", base(r#""iss":-"#), 401),
        ("audiences", base(r#""aud":["other","web"]"#), 200),
        ("another audience", base(r#""aud":["other"]"#), 401),
        ("no audience", base(r#""aud":-"#), 401),
    ] {
        let r = bearer(akit.addr, "/p", &hs(&claims));
        assert_eq!(r.status, code, "{what}: {claims}");
    }

    let good = hs(&base(""));
    let parts: Vec<&str> = good.split('.').collect();
    let evil = B64.encode(base(r#""sub":"root""#));
    let enc = |s: &str| B64.encode(s);
    for (what, token) in [
        ("tampered", format!("{}.{evil}.{}", parts[0], parts[2])),
        ("two parts", format!("{}.{}", parts[0], parts[1])),
        ("four parts", format!("{good}.x")),
        ("bad base64", format!("{}.!!.{}", parts[0], parts[2])),
        (
            "crit",
            sign_raw(
                "HS256",
                Signer::Secret(&k.secret),
                r#"{"alg":"HS256","crit":["exp"]}"#,
                &base(""),
            ),
        ),
        (
            "not JSON",
            format!("{}.{}.{}", enc("{"), parts[1], parts[2]),
        ),
        (
            "too long",
            format!("{}.{}.{}", "a".repeat(16 << 10), parts[1], parts[2]),
        ),
        (
            "claims no object",
            sign("HS256", "", Signer::Secret(&k.secret), "[1]"),
        ),
    ] {
        let r = bearer(akit.addr, "/p", &token);
        assert_eq!(r.status, 401, "{what}");
        assert_eq!(r.header("www-authenticate"), Some(INVALID), "{what}");
    }

    // A token kept verified is still checked against the clock: one that expires while kept is
    // refused once it has. The clock is read again here: the cases above can take seconds on a
    // loaded host, and this token must be within the leeway when it is first sent.
    let soon = hs(&base(&format!(r#""exp":{}"#, unix_now() - 29)));
    assert_eq!(
        bearer(akit.addr, "/p", &soon).status,
        200,
        "within the leeway"
    );
    thread::sleep(Duration::from_millis(2100));
    assert_eq!(bearer(akit.addr, "/p", &soon).status, 401, "expired since");
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}

// fit's TestJWTTokenFrom.
#[test]
fn jwt_token_from() {
    let k = keyring();
    let d = dir("token-from");
    let (_, secret) = key_files(&d, &k);
    let route = |path: &'static str, from: &str| {
        (
            path,
            format!(r#"{{"hmac_secret_file": "{secret}", "token_from": "{from}"}}"#),
        )
    };
    let akit = Akit::start(&doc(&[
        route("/header", "header"),
        route("/field", "header:x-token"),
        route("/cookie", "cookie:session"),
        route("/query", "query:access_token"),
    ]));
    let token = sign("HS256", "", Signer::Secret(&k.secret), &valid());
    let a = akit.addr;
    let r = get(
        a,
        "/header",
        &[("Authorization", &format!("bearer  {token}"))],
    );
    assert_eq!(r.status, 200, "Authorization, in lower case");
    let r = get(a, "/header", &[]);
    assert_eq!(r.status, 401, "no token");
    assert_eq!(r.header("www-authenticate"), Some(r#"Bearer realm="akit""#));
    let basic = akio_crypto::base64::STANDARD.encode(format!("alice:{token}"));
    let r = get(
        a,
        "/header",
        &[("Authorization", &format!("Basic {basic}"))],
    );
    assert_eq!(r.status, 401, "Basic");

    assert_eq!(
        get(a, "/field", &[("X-Token", &token)]).status,
        200,
        "a field"
    );
    let r = get(
        a,
        "/cookie",
        &[("Cookie", &format!("a=1; session={token}"))],
    );
    assert_eq!(r.status, 200, "a cookie");
    assert_eq!(bearer(a, "/cookie", &token).status, 401, "no cookie");
    let r = get(a, &format!("/query?access_token={token}"), &[]);
    assert_eq!(r.status, 200, "a query");
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}

/// fit's `spellings`.
fn spellings(name: &str) -> Vec<String> {
    let under = name.replace('-', "_");
    vec![
        under.clone(),
        under.to_ascii_lowercase(),
        under.to_ascii_uppercase(),
        name.replacen('-', "_", 1),
    ]
}

// fit's TestJWTClaimsHeaders.
#[test]
fn jwt_claims_headers() {
    let k = keyring();
    let d = dir("claims-headers");
    let (_, secret) = key_files(&d, &k);
    let akit = Akit::start(&doc(&[(
        "/p",
        format!(
            r#"{{"hmac_secret_file": "{secret}",
                 "claims_headers": {{"x-sub": "sub", "X-Roles": "roles", "X-Missing": "nope"}}}}"#
        ),
    )]));
    let exp = unix_now() + 3600;
    let token = sign(
        "HS256",
        "",
        Signer::Secret(&k.secret),
        &format!(r#"{{"sub":"alice\r\nX-Evil: 1","roles":["admin","dev"],"exp":{exp}}}"#),
    );
    let auth = format!("Bearer {token}");
    let mut forged: Vec<(String, &str)> = vec![
        ("X-Roles".into(), "forged"),
        ("X-Missing".into(), "forged"),
        ("X-Sub".into(), "forged"),
    ];
    for n in ["X-Roles", "X-Sub", "X-Missing"] {
        forged.extend(spellings(n).into_iter().map(|s| (s, "forged")));
    }
    let mut headers: Vec<(&str, &str)> = forged.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    headers.push(("Authorization", &auth));
    let r = get(akit.addr, "/p", &headers);
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            200,
            r#"x-sub=alice??X-Evil: 1 x-roles=["admin","dev"] x-missing="#
        )
    );
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}

/// The errors of a config whose only route has a jwt of `params`: the config's, else those of
/// compiling it.
fn errors_of(params: &str) -> String {
    let json = doc(&[("/p", params.to_owned())]);
    match akit_config::from_json(&json, None) {
        Err(e) => e.to_string(),
        Ok(c) => match compile(&c, &registry(), &Env::new(EngineSlot::new())) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        },
    }
}

// fit's TestJWTParams and the params of TestJWTMore.
#[test]
fn jwt_params() {
    let k = keyring();
    let d = dir("params");
    let (pubs, secret) = key_files(&d, &k);
    let short = d.join("short");
    std::fs::write(&short, "too short").unwrap();
    let large = d.join("large.pem");
    std::fs::write(&large, vec![0u8; (1 << 20) + 1]).unwrap();
    let (short, large) = (path_str(&short), path_str(&large));
    let pub0 = &pubs[0];
    for (params, want) in [
        ("{}".to_owned(), "a key is needed"),
        (
            r#"{"jwks_url": "ftp://x/keys"}"#.to_owned(),
            "not an http or https URL",
        ),
        (
            r#"{"jwks_url": "https:///keys"}"#.to_owned(),
            "not an http or https URL",
        ),
        (
            format!(r#"{{"public_key_files": ["{secret}"]}}"#),
            "no PEM public key",
        ),
        (
            r#"{"public_key_files": ["/nonexistent"]}"#.to_owned(),
            "public_key_files[0]",
        ),
        (
            format!(r#"{{"hmac_secret_file": "{short}"}}"#),
            "at least 32 are needed",
        ),
        (
            format!(r#"{{"public_key_files": ["{pub0}"], "token_from": "body"}}"#),
            "is not header",
        ),
        (
            format!(r#"{{"public_key_files": ["{pub0}"], "token_from": "cookie:"}}"#),
            "is not header",
        ),
        (
            format!(r#"{{"public_key_files": ["{pub0}"], "claims_headers": {{"a b": "sub"}}}}"#),
            "not a field name",
        ),
        (
            format!(r#"{{"public_key_files": ["{pub0}"], "algorithms": ["none"]}}"#),
            r#""none" is not one of"#,
        ),
        (
            format!(r#"{{"hmac_secret_file": "{secret}", "claims_headers": {{"X-Sub": ""}}}}"#),
            "names no claim",
        ),
        (
            format!(r#"{{"public_key_files": ["{large}"]}}"#),
            "larger than 1 MiB",
        ),
        (
            r#"{"jwks_url": "https://a.example/k", "jwks_refresh": "1m", "jwks_refresh_interval": "1m"}"#
                .to_owned(),
            r#""jwks_refresh" is the old name of "jwks_refresh_interval""#,
        ),
        (
            format!(r#"{{"hmac_secret_file": "{secret}", "leeway": "soon"}}"#),
            "not a duration",
        ),
        (
            format!(r#"{{"hmac_secret_file": "{secret}", "jwks": "x"}}"#),
            "unknown field",
        ),
    ] {
        let errs = errors_of(&params);
        assert!(errs.contains(want), "{params}: {errs:?}, want {want:?}");
    }
    // Without params.
    let json = r#"{"listeners": [{"name": "web", "address": ":8080"}],
        "routes": [{"middleware": [{"module": "jwt"}], "handler": {"module": "respond"}}]}"#;
    let errs = akit_config::from_json(json, None).unwrap_err().to_string();
    assert!(errs.contains("a key is needed"), "{errs}");

    // What is read, and the defaults; the old name of jwks_refresh_interval.
    let json = doc(&[(
        "/p",
        r#"{"jwks_url": "https://idp.example/keys", "jwks_refresh": "5m",
            "token_from": "cookie:s", "claims_headers": {"X-B": "b", "X-A": "a"}}"#
            .to_owned(),
    )]);
    let c = akit_config::from_json(&json, None).unwrap();
    assert_eq!(
        c.routes[0].middleware[0],
        akit_config::Handler::Jwt(akit_config::Jwt {
            jwks_url: "https://idp.example/keys".into(),
            jwks_refresh_interval: Duration::from_secs(300),
            public_key_files: Vec::new(),
            hmac_secret_file: String::new(),
            algorithms: Vec::new(),
            issuer: String::new(),
            audience: Vec::new(),
            leeway: Duration::ZERO,
            token_from: akit_config::TokenFrom::Cookie("s".into()),
            claims_headers: vec![("X-B".into(), "b".into()), ("X-A".into(), "a".into())],
        })
    );
    // A JWK set and no other key is enough, and nothing is fetched without an engine (akit
    // validate).
    assert_eq!(
        errors_of(r#"{"jwks_url": "http://127.0.0.1:1/keys"}"#),
        "",
        "jwks_url alone"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ---- the JWK set ----

/// What the JWK set server answers.
#[derive(Clone)]
enum Answer {
    Set(Vec<u8>),
    Status(u16),
    /// More than the middleware reads.
    Large,
    /// A head promising more than the body sent before the connection closes.
    Cut,
}

/// A JWK set that a test changes, served on plain threads, counting the fetches: fit's
/// `jwksServer`.
struct JwksServer {
    addr: SocketAddr,
    answer: Arc<Mutex<(Answer, Duration)>>,
    fetches: Arc<AtomicUsize>,
}

impl JwksServer {
    fn start() -> JwksServer {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let answer = Arc::new(Mutex::new((Answer::Set(b"{}".to_vec()), Duration::ZERO)));
        let fetches = Arc::new(AtomicUsize::new(0));
        let (a, f) = (answer.clone(), fetches.clone());
        // Runs as long as the test process: each test has its own.
        thread::spawn(move || {
            for s in l.incoming().flatten() {
                let (a, f) = (a.clone(), f.clone());
                thread::spawn(move || serve_jwks(s, &a, &f));
            }
        });
        JwksServer {
            addr,
            answer,
            fetches,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/keys", self.addr)
    }

    fn serve(&self, keys: &[String]) {
        let set = format!(r#"{{"keys": [{}]}}"#, keys.join(","));
        self.answer.lock().unwrap().0 = Answer::Set(set.into_bytes());
    }

    fn answer(&self, a: Answer) {
        self.answer.lock().unwrap().0 = a;
    }

    fn delay(&self, d: Duration) {
        self.answer.lock().unwrap().1 = d;
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }

    /// Waits until the set was fetched `n` times.
    fn wait_fetches(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.fetches() < n {
            assert!(
                Instant::now() < deadline,
                "{} fetches, want {n}",
                self.fetches()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn serve_jwks(s: TcpStream, answer: &Mutex<(Answer, Duration)>, fetches: &AtomicUsize) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    while read_head(&mut r).is_some() {
        fetches.fetch_add(1, Ordering::SeqCst);
        let (a, delay) = answer.lock().unwrap().clone();
        thread::sleep(delay);
        let out = match a {
            Answer::Set(body) => [
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes(),
                body,
            ]
            .concat(),
            Answer::Status(code) => {
                format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\n\r\n").into_bytes()
            }
            Answer::Large => {
                let n = (1 << 20) + 10;
                [
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\n\r\n").into_bytes(),
                    vec![b' '; n],
                ]
                .concat()
            }
            Answer::Cut => {
                let _ = w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc");
                return;
            }
        };
        if w.write_all(&out).is_err() {
            return;
        }
    }
}

// fit's TestJWKSRotation: the set is fetched when akit starts, a request before it waiting for it;
// a token naming a new kid has it fetched again, and waits for it; a key the set dropped no
// longer verifies; unknown kids have it fetched at most every MIN_REFETCH.
#[test]
fn jwks_rotation() {
    let min_refetch = MIN_REFETCH;
    akit_middleware::set_jwks_min_refetch(min_refetch);
    let k = keyring();
    let s = JwksServer::start();
    s.serve(&[jwk_of(&k.rsa, "one")]);
    s.delay(Duration::from_millis(200));
    let akit = Akit::start(&doc(&[("/p", format!(r#"{{"jwks_url": "{}"}}"#, s.url()))]));
    let a = akit.addr;

    let one = sign("RS256", "one", Signer::Key(&k.rsa), &valid());
    assert_eq!(bearer(a, "/p", &one).status, 200, "before the first fetch");
    s.delay(Duration::ZERO);
    assert_eq!(bearer(a, "/p", &one).status, 200, "kid one");
    assert_eq!(s.fetches(), 1);

    // The keys rotate: a token of the new key has the set fetched again.
    s.serve(&[jwk_of(&k.p256, "two")]);
    let two = sign("ES256", "two", Signer::Key(&k.p256), &valid());
    assert_eq!(
        bearer(a, "/p", &two).status,
        200,
        "kid two, after a rotation"
    );
    assert_eq!(s.fetches(), 2);
    assert_eq!(bearer(a, "/p", &one).status, 401, "kid one, dropped");
    let forged = sign("RS256", "two", Signer::Key(&k.rsa), &valid());
    assert_eq!(
        bearer(a, "/p", &forged).status,
        401,
        "kid two by another key"
    );

    // Unknown kids have it fetched at most every MIN_REFETCH.
    let before = s.fetches();
    let start = Instant::now();
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let t = sign(
                "RS256",
                &format!("unknown{i}"),
                Signer::Key(&k.rsa),
                &valid(),
            );
            thread::spawn(move || bearer(a, "/p", &t).status)
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), 401, "an unknown kid");
    }
    let n = s.fetches() - before;
    let most = (start.elapsed().as_millis() / min_refetch.as_millis()) as usize + 2;
    assert!(
        n <= most,
        "{n} fetches for unknown kids in {:?}",
        start.elapsed()
    );
    drop(akit);
}

// fit's TestJWKSRefresh: fetched every jwks_refresh_interval; a set that fails to load keeps the
// keys there were.
#[test]
fn jwks_refresh() {
    akit_middleware::set_jwks_min_refetch(MIN_REFETCH);
    let k = keyring();
    let s = JwksServer::start();
    s.serve(&[jwk_of(&k.ed, "ed")]);
    let akit = Akit::start(&doc(&[(
        "/p",
        format!(
            r#"{{"jwks_url": "{}", "jwks_refresh_interval": "50ms"}}"#,
            s.url()
        ),
    )]));
    let ed = || sign("EdDSA", "ed", Signer::Key(&k.ed), &valid());
    assert_eq!(bearer(akit.addr, "/p", &ed()).status, 200, "ed");
    s.wait_fetches(4);

    // Sets that fail to load, one after another: the keys there were are kept.
    for bad in [
        Answer::Set(b"{".to_vec()),
        Answer::Status(500),
        Answer::Large,
        Answer::Cut,
    ] {
        s.answer(bad);
        let n = s.fetches();
        s.wait_fetches(n + 2);
        assert_eq!(
            bearer(akit.addr, "/p", &ed()).status,
            200,
            "after a failed fetch"
        );
    }
    drop(akit);
}

// A JWK set out of reach, then back: requests are refused meanwhile (each waits for a fetch,
// which fails), and let in once a fetch brings the set.
#[test]
fn jwks_outage() {
    akit_middleware::set_jwks_min_refetch(MIN_REFETCH);
    let k = keyring();
    let s = JwksServer::start();
    s.answer(Answer::Status(503));
    let akit = Akit::start(&doc(&[("/p", format!(r#"{{"jwks_url": "{}"}}"#, s.url()))]));
    let ed = || sign("EdDSA", "ed", Signer::Key(&k.ed), &valid());
    for _ in 0..3 {
        let r = bearer(akit.addr, "/p", &ed());
        assert_eq!(r.status, 401, "the set out of reach");
        assert_eq!(r.header("www-authenticate"), Some(INVALID));
    }
    s.serve(&[jwk_of(&k.ed, "ed")]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while bearer(akit.addr, "/p", &ed()).status != 200 {
        assert!(Instant::now() < deadline, "not let in once the set is back");
        thread::sleep(Duration::from_millis(20));
    }

    // Nothing listening: refused, and the route without jwt answers meanwhile.
    // Port 1: nothing serves it, and no test binds it.
    let closed = SocketAddr::from(([127, 0, 0, 1], 1));
    let akit2 = Akit::start(&doc(&[(
        "/p",
        format!(r#"{{"jwks_url": "http://{closed}/keys"}}"#),
    )]));
    assert_eq!(bearer(akit2.addr, "/p", &ed()).status, 401);
    assert_eq!(get(akit2.addr, "/open", &[]).status, 200);
    drop((akit, akit2));
}

// fit's TestJWTClientGoneWaiting: a request that waits for the set, for its token's unknown
// kid, and whose client goes meanwhile; the next requests are served.
#[test]
fn jwks_client_gone_waiting() {
    akit_middleware::set_jwks_min_refetch(MIN_REFETCH);
    let k = keyring();
    let s = JwksServer::start();
    s.serve(&[jwk_of(&k.ed, "ed")]);
    let akit = Akit::start(&doc(&[("/p", format!(r#"{{"jwks_url": "{}"}}"#, s.url()))]));
    let ed = sign("EdDSA", "ed", Signer::Key(&k.ed), &valid());
    assert_eq!(bearer(akit.addr, "/p", &ed).status, 200, "ed");
    s.delay(Duration::from_millis(300));
    let fetched = s.fetches();
    let c = TcpStream::connect(akit.addr).unwrap();
    let other = sign("EdDSA", "other", Signer::Key(&k.ed), &valid());
    (&c).write_all(
        format!("GET /p HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer {other}\r\n\r\n")
            .as_bytes(),
    )
    .unwrap();
    s.wait_fetches(fetched + 1);
    drop(c);
    thread::sleep(Duration::from_millis(500));
    s.delay(Duration::ZERO);
    assert_eq!(bearer(akit.addr, "/p", &ed).status, 200, "served after");
    drop(akit);
}

// Bodies through jwt to the proxy: one of 200 KiB with a key from a file, and one whose token
// waits for the JWK set (its body read whole first, so that it can wait).
#[test]
fn jwt_then_proxy() {
    akit_middleware::set_jwks_min_refetch(MIN_REFETCH);
    let k = keyring();
    let d = dir("proxy");
    let (_, secret) = key_files(&d, &k);
    let s = JwksServer::start();
    s.serve(&[jwk_of(&k.ed, "ed")]);
    let app = TcpListener::bind("127.0.0.1:0").unwrap();
    let app_addr = app.local_addr().unwrap();
    thread::spawn(move || {
        for s in app.incoming().flatten() {
            thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut w = s;
                while let Some(req) = read_head(&mut r) {
                    let n: usize = req.get("Content-Length").parse().unwrap_or(0);
                    let mut body = vec![0; n];
                    if r.read_exact(&mut body).is_err() {
                        return;
                    }
                    let text = format!("sub={} body={n}", req.get("X-Sub"));
                    let out = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{text}",
                        text.len()
                    );
                    if w.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    let route = |path: &str, params: String| {
        format!(
            r#"{{"match": {{"path": ["{path}"]}}, "middleware": [{{"module": "jwt", "params": {params}}}],
                "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}"#
        )
    };
    let akit = Akit::start(&format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "upstreams": [{{"name": "app", "addresses": ["{app_addr}"]}}],
            "routes": [{}, {}]}}"#,
        route(
            "/file",
            format!(r#"{{"hmac_secret_file": "{secret}", "claims_headers": {{"X-Sub": "sub"}}}}"#)
        ),
        route(
            "/jwks",
            format!(
                r#"{{"jwks_url": "{}", "claims_headers": {{"X-Sub": "sub"}}}}"#,
                s.url()
            )
        ),
    ));
    let post = |path: &str, token: &str, n: usize| {
        let c = TcpStream::connect(akit.addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer {token}\r\nX-Sub: forged\r\nContent-Length: {n}\r\n\r\n"
        );
        (&c).write_all(head.as_bytes()).unwrap();
        (&c).write_all(&vec![b'x'; n]).unwrap();
        let mut r = BufReader::new(c);
        let head = read_head(&mut r).unwrap();
        let len: usize = head.get("Content-Length").parse().unwrap_or(0);
        let mut body = vec![0; len];
        r.read_exact(&mut body).unwrap();
        (head.second, String::from_utf8(body).unwrap())
    };
    let n = 200 << 10;
    let hs = sign("HS256", "", Signer::Secret(&k.secret), &valid());
    assert_eq!(
        post("/file", &hs, n),
        ("200".to_owned(), format!("sub=alice body={n}"))
    );
    let ed = sign("EdDSA", "ed", Signer::Key(&k.ed), &valid());
    assert_eq!(
        post("/jwks", &ed, n),
        ("200".to_owned(), format!("sub=alice body={n}"))
    );
    // A token of a kid the set does not have yet waits, its body read whole, for the set that
    // brings it.
    s.serve(&[jwk_of(&k.ed, "ed"), jwk_of(&k.p256, "new")]);
    let new = sign("ES256", "new", Signer::Key(&k.p256), &valid());
    assert_eq!(
        post("/jwks", &new, n),
        ("200".to_owned(), format!("sub=alice body={n}"))
    );
    let (status, _) = post(
        "/jwks",
        &sign("ES256", "newer", Signer::Key(&k.p256), &valid()),
        n,
    );
    assert_eq!(status, "401");
    drop(akit);
    let _ = std::fs::remove_dir_all(&d);
}
