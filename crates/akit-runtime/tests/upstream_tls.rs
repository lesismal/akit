//! An h2 upstream's TLS files in the graph (I54): read when the config is compiled, a file that
//! does not load failing it with the field's path (fit's `TestCompileFails`), and each file
//! watched with the listeners' certificate files, so that a change compiles a new pool that reads
//! it again; and the pools a reload shares (fit's `TestSharedPools`).
#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use akio_http::Context;
use akit_config::{Config, from_json};
use akit_runtime::{
    Env, Graph, Handler, HandlerParams, Info, Previous, Provision, ProvisionError, Provisioned,
    Registry, compile,
};

struct Nothing;

impl akio_http::Service for Nothing {
    fn call(&self, ctx: &mut Context) {
        ctx.text(200, "");
    }
}

impl Handler for Nothing {}

fn new_nothing(
    _: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    Ok(Provisioned::new(Arc::new(Nothing) as Arc<dyn Handler>))
}

static MODULES: &[Info<HandlerParams>] = &[Info::handler(
    "http.handlers.respond",
    "Answers nothing in particular.",
    new_nothing,
)];

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[MODULES]).unwrap()
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A new self-signed certificate for `name`, its PEM written to `cert` and its key's to `key`.
fn write_pair(name: &str, cert: &Path, key: &Path) {
    let (c, k) = akio_x509::generate_simple_self_signed([name]).unwrap();
    std::fs::write(cert, c.to_pem()).unwrap();
    std::fs::write(key, k.to_pkcs8_pem()).unwrap();
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// Two upstreams, `a` over HTTP/1.1 and `b` over h2 with `b_fields`, and a route.
fn config(b_fields: &str) -> Config {
    from_json(
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "upstreams": [{{"name": "a", "addresses": ["127.0.0.1:1"]}},
                              {{"name": "b", "addresses": ["127.0.0.1:2"], "protocol": "h2"{b_fields}}}],
                "routes": [{{"handler": {{"module": "respond"}}}}]}}"#
        ),
        None,
    )
    .unwrap()
}

#[test]
fn a_file_that_does_not_load_fails_the_config() {
    let d = dir("runtime-upstream-tls-fails");
    let missing = d.join("missing.pem");
    let e = compile(
        &config(&format!(r#", "tls_ca_file": "{}""#, s(&missing))),
        &registry(),
        &Env::default(),
    )
    .unwrap_err();
    assert_eq!(e.as_slice().len(), 1, "{e}");
    assert_eq!(e.as_slice()[0].path, "upstreams[1].tls_ca_file");
    assert!(e.as_slice()[0].msg.contains(&s(&missing)), "{e}");
    // A key that is not the certificate's: at tls_cert_file.
    let (c, k, other) = (d.join("c.pem"), d.join("c.key"), d.join("o.key"));
    write_pair("akit", &c, &k);
    write_pair("other", &d.join("o.pem"), &other);
    let e = compile(
        &config(&format!(
            r#", "tls_cert_file": "{}", "tls_key_file": "{}""#,
            s(&c),
            s(&other)
        )),
        &registry(),
        &Env::default(),
    )
    .unwrap_err();
    assert_eq!(e.as_slice()[0].path, "upstreams[1].tls_cert_file", "{e}");
    assert!(e.as_slice()[0].msg.contains("does not match"), "{e}");
}

#[test]
fn the_files_are_watched() {
    let d = dir("runtime-upstream-tls-watched");
    let (ca, ca_key) = (d.join("ca.pem"), d.join("ca.key"));
    write_pair("upstream CA", &ca, &ca_key);
    let with_ca = format!(r#", "tls_ca_file": "{}""#, s(&ca));
    let g = compile(&config(&with_ca), &registry(), &Env::default()).unwrap();
    let before = g
        .cert_files()
        .get(&s(&ca))
        .copied()
        .expect("the CA file is not watched");
    assert_eq!(g.cert_files().len(), 1);
    // The CA file changes: the next config reads it again.
    write_pair("another CA", &ca, &ca_key);
    let g = compile(&config(&with_ca), &registry(), &Env::default()).unwrap();
    assert_ne!(g.cert_files()[&s(&ca)], before);
    // A client certificate and its key: watched too.
    let (c, k) = (d.join("client.pem"), d.join("client.key"));
    write_pair("akit", &c, &k);
    let g = compile(
        &config(&format!(
            r#"{with_ca}, "tls_cert_file": "{}", "tls_key_file": "{}""#,
            s(&c),
            s(&k)
        )),
        &registry(),
        &Env::default(),
    )
    .unwrap();
    for f in [&ca, &c, &k] {
        assert!(g.cert_files().contains_key(&s(f)), "{f:?} is not watched");
    }
    // tls_server_name alone reads no file.
    let g = compile(
        &config(r#", "tls_server_name": "app.internal""#),
        &registry(),
        &Env::default(),
    )
    .unwrap();
    assert!(g.cert_files().is_empty());
}

/// `config` compiled to replace `prev`, as a reload compiles it.
fn compile_after(config: &Config, prev: Option<&Graph>) -> Graph {
    let env = Env::default().with_previous(prev.map(Previous::of));
    compile(config, &registry(), &env).unwrap()
}

/// fit's `TestSharedPools`: a graph takes over the pools of the upstreams it leaves as they are,
/// TLS files included, and each pool closes when the last graph holding it lets go.
#[test]
fn unchanged_upstreams_share_their_pools() {
    let g1 = compile_after(&config(""), None);
    let g2 = compile_after(&config(r#", "max_conns": 9"#), Some(&g1));
    let (a1, a2) = (g1.upstream("a").unwrap(), g2.upstream("a").unwrap());
    let (b1, b2) = (g1.upstream("b").unwrap(), g2.upstream("b").unwrap());
    assert!(a1.ptr_eq(a2), "a, unchanged, was not shared");
    assert!(!b1.ptr_eq(b2), "b, changed, was shared");
    let (a2, b1) = (a2.clone(), b1.clone());
    // g1 retired: its pool of b closes, the one it shares with g2 does not.
    drop(g1);
    assert!(b1.is_closing() && !a2.is_closing());

    let d = dir("runtime-upstream-shared");
    let (ca, ca_key) = (d.join("ca.pem"), d.join("ca.key"));
    write_pair("a", &ca, &ca_key);
    let with_ca = format!(r#", "tls_ca_file": "{}""#, s(&ca));
    let g3 = compile_after(&config(&with_ca), Some(&g2));
    let g4 = compile_after(&config(&with_ca), Some(&g3));
    let b4 = g4.upstream("b").unwrap().clone();
    assert!(
        g3.upstream("b").unwrap().ptr_eq(&b4),
        "an unchanged upstream with a CA file was not shared"
    );
    // The CA file changes: a new pool, which reads it again.
    write_pair("b", &ca, &ca_key);
    let g5 = compile_after(&config(&with_ca), Some(&g4));
    assert!(
        !g5.upstream("b").unwrap().ptr_eq(&b4),
        "a changed CA file kept the old pool"
    );
    assert!(
        g5.cert_files().contains_key(&s(&ca)),
        "the CA file is not watched"
    );

    // A client certificate and its key: new ones make a new pool.
    let (c, k) = (d.join("client.pem"), d.join("client.key"));
    write_pair("client", &c, &k);
    let with_cert = format!(
        r#"{with_ca}, "tls_cert_file": "{}", "tls_key_file": "{}""#,
        s(&c),
        s(&k)
    );
    let g6 = compile_after(&config(&with_cert), Some(&g5));
    assert!(
        g6.cert_files().contains_key(&s(&k)),
        "the key file is not watched"
    );
    write_pair("other", &c, &k);
    let g7 = compile_after(&config(&with_cert), Some(&g6));
    assert!(
        !g7.upstream("b").unwrap().ptr_eq(g6.upstream("b").unwrap()),
        "a changed client certificate kept the old pool"
    );
    // a, the same throughout, is one pool from g2 to g7; it closes with the last of them.
    assert!(g7.upstream("a").unwrap().ptr_eq(&a2));
    for g in [g2, g3, g4, g5, g6] {
        drop(g);
        assert!(!a2.is_closing());
    }
    assert!(b4.is_closing());
    drop(g7);
    assert!(a2.is_closing());
    // Without a previous graph (a dry run, akit validate), nothing is shared.
    let g8 = compile_after(&config(""), None);
    assert!(!g8.upstream("a").unwrap().ptr_eq(&a2));
}
