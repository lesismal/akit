//! The module registry: registration, provisioning from config, cleanup on release and on a failed
//! compile, the errors fit gives, and a provisioned chain serving a request on akio.
#![allow(missing_docs)]

use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    time::Duration,
};

use akio_http::Context;
use akit_runtime::{
    Env, Handler, HandlerService, Info, LoadError, Middleware, Modules, Provision, ProvisionError,
    Provisioned, Registry,
    module::{HANDLERS, Kind, MIDDLEWARE, RegistryError},
    traits::{AkioMiddleware, chain},
};

/// A stand-in for a module's params as a config writes them (I04 uses akit-config's).
#[derive(Clone, Default)]
struct Params {
    text: String,
    fail: bool,
    /// Where the test modules record their cleanups.
    log: Arc<Mutex<Vec<String>>>,
}

impl Params {
    fn new(text: &str, log: &Arc<Mutex<Vec<String>>>) -> Params {
        Params {
            text: text.into(),
            fail: false,
            log: Arc::clone(log),
        }
    }
}

struct Respond {
    body: String,
}

impl akio_http::Service for Respond {
    fn call(&self, ctx: &mut Context) {
        ctx.text(200, self.body.clone());
    }
}

impl Handler for Respond {}

fn new_respond(p: &Provision<'_, Params>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let params = p.params();
    if params.fail {
        return Err(ProvisionError::field("text", "is refused"));
    }
    if params.text.is_empty() {
        return Err(ProvisionError::new("something failed"));
    }
    let log = Arc::clone(&params.log);
    let id = p.id();
    Ok(Provisioned::new(Arc::new(Respond {
        body: params.text.clone(),
    }) as Arc<dyn Handler>)
    .on_cleanup(move || log.lock().unwrap().push(format!("{id} cleaned"))))
}

fn new_tag(p: &Provision<'_, Params>) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let value = p.params().text.clone();
    let log = Arc::clone(&p.params().log);
    let mw = AkioMiddleware::new(move |ctx: &mut Context, next: &dyn akio_http::Service| {
        ctx.add_response_header("X-Tag", &value);
        next.call(ctx);
    });
    let tag = p.params().text.clone();
    Ok(Provisioned::new(Arc::new(mw) as Arc<dyn Middleware>)
        .on_cleanup(move || log.lock().unwrap().push(format!("tag {tag} cleaned"))))
}

static TEST_MODULES: &[Info<Params>] = &[
    Info::handler(
        "http.handlers.respond",
        "Answers with fixed text.",
        new_respond,
    ),
    Info::middleware("http.middleware.tag", "Adds X-Tag.", new_tag),
];

static MORE_MODULES: &[Info<Params>] = &[Info::handler(
    "http.handlers.echo",
    "Another handler.",
    new_respond,
)];

fn registry() -> Registry<Params> {
    Registry::new(&[TEST_MODULES, MORE_MODULES]).unwrap()
}

fn log() -> Arc<Mutex<Vec<String>>> {
    Arc::new(Mutex::new(Vec::new()))
}

fn logged(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().unwrap().clone()
}

#[test]
fn registers_looks_up_and_lists() {
    let r = registry();
    let ids: Vec<_> = r.iter().map(|i| i.id).collect();
    assert_eq!(
        ids,
        [
            "http.handlers.echo",
            "http.handlers.respond",
            "http.middleware.tag"
        ]
    );
    let info = r.lookup("http.handlers.respond").unwrap();
    assert_eq!(
        (info.namespace(), info.name(), info.new.kind()),
        (HANDLERS, "respond", Kind::Handler)
    );
    assert!(r.lookup("http.handlers.nope").is_none());
    let names: Vec<_> = r.namespace(MIDDLEWARE).map(|i| i.name()).collect();
    assert_eq!(names, ["tag"]);
}

#[test]
fn provisions_from_config_and_cleans_up_on_release() {
    let r = registry();
    let log = log();
    let mut modules = Modules::new();
    let env = Env::default();
    let h = r
        .handler("respond", &Params::new("hi", &log), &env, &mut modules)
        .unwrap();
    let mw = r
        .middleware("tag", &Params::new("a", &log), &env, &mut modules)
        .unwrap();
    let _route = chain(&[mw], h);
    assert_eq!(
        modules.ids().collect::<Vec<_>>(),
        ["http.handlers.respond", "http.middleware.tag"]
    );
    assert!(logged(&log).is_empty(), "nothing cleaned up while in use");
    modules.release();
    // Last provisioned first, each once.
    assert_eq!(
        logged(&log),
        ["tag a cleaned", "http.handlers.respond cleaned"]
    );
}

#[test]
fn dropping_the_modules_cleans_up_once() {
    let r = registry();
    let log = log();
    {
        let mut modules = Modules::new();
        r.handler(
            "echo",
            &Params::new("x", &log),
            &Env::default(),
            &mut modules,
        )
        .unwrap();
        assert_eq!(modules.len(), 1);
    }
    assert_eq!(logged(&log), ["http.handlers.echo cleaned"]);
}

#[test]
fn an_unknown_module_is_an_error_naming_it() {
    let r = registry();
    let mut modules = Modules::new();
    let err = r
        .handler("nope", &Params::default(), &Env::default(), &mut modules)
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        r#"there is no module "nope" in http.handlers (it has "echo", "respond")"#
    );
    // A middleware is not a handler: it is not found among handlers.
    let err = r
        .handler("tag", &Params::default(), &Env::default(), &mut modules)
        .err()
        .unwrap();
    assert!(matches!(err, LoadError::Unknown { ref name, .. } if &**name == "tag"));
    let err = r
        .middleware("respond", &Params::default(), &Env::default(), &mut modules)
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        r#"there is no module "respond" in http.middleware (it has "tag")"#
    );
    let empty = Registry::<Params>::new(&[]).unwrap();
    let err = empty
        .middleware("x", &Params::default(), &Env::default(), &mut modules)
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        r#"there is no module "x" in http.middleware"#
    );
    assert!(modules.is_empty());
}

/// What I04's compile does with a route: load its handler and middleware in order, and give up on
/// the first error, dropping what it loaded.
fn compile(
    r: &Registry<Params>,
    route: &[(&str, &str, Params)],
) -> Result<(Arc<dyn Handler>, Modules), LoadError> {
    let mut modules = Modules::new();
    let env = Env::default();
    let mut mws = Vec::new();
    let mut handler = None;
    for (kind, name, params) in route {
        if *kind == "mw" {
            mws.push(r.middleware(name, params, &env, &mut modules)?);
        } else {
            handler = Some(r.handler(name, params, &env, &mut modules)?);
        }
    }
    Ok((chain(&mws, handler.unwrap()), modules))
}

#[test]
fn a_provisioning_failure_cleans_up_what_was_provisioned() {
    let r = registry();
    let log = log();
    let failing = Params {
        fail: true,
        ..Params::new("x", &log)
    };
    let err = compile(
        &r,
        &[
            ("mw", "tag", Params::new("a", &log)),
            ("mw", "tag", Params::new("b", &log)),
            ("handler", "respond", failing),
        ],
    )
    .err()
    .unwrap();
    assert_eq!(err.to_string(), "is refused");
    let LoadError::Provision { id, error } = &err else {
        panic!("{err:?}")
    };
    assert_eq!(
        (*id, error.field_name()),
        ("http.handlers.respond", Some("text"))
    );
    assert_eq!(logged(&log), ["tag b cleaned", "tag a cleaned"]);

    // An unknown name midway cleans up alike, and an error about the params as a whole has no
    // field.
    let log2 = self::log();
    let err = compile(
        &r,
        &[
            ("mw", "tag", Params::new("c", &log2)),
            ("mw", "nope", Params::new("d", &log2)),
        ],
    )
    .err()
    .unwrap();
    assert!(matches!(err, LoadError::Unknown { .. }));
    assert_eq!(logged(&log2), ["tag c cleaned"]);
    let err = compile(&r, &[("handler", "respond", Params::new("", &log2))])
        .err()
        .unwrap();
    assert_eq!(err.to_string(), "something failed");
    assert!(matches!(&err, LoadError::Provision { error, .. } if error.field_name().is_none()));
}

fn new_none(_: &Provision<'_, ()>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    Err(ProvisionError::new("unused"))
}

fn new_none_mw(_: &Provision<'_, ()>) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    Err(ProvisionError::new("unused"))
}

#[test]
fn registry_mistakes_are_errors() {
    static MALFORMED: &[Info<()>] = &[Info::handler("Handlers.x", "", new_none)];
    static NO_NAMESPACE: &[Info<()>] = &[Info::handler("respond", "", new_none)];
    static EMPTY_WORD: &[Info<()>] = &[Info::handler("http..x", "", new_none)];
    static TWICE: &[Info<()>] = &[
        Info::handler("http.handlers.a", "", new_none),
        Info::handler("http.handlers.a", "", new_none),
    ];
    static WRONG_NAMESPACE: &[Info<()>] = &[Info::middleware("http.handlers.m", "", new_none_mw)];
    static NESTED: &[Info<()>] = &[Info::handler("http.handlers.nested.x", "", new_none)];
    let err = |sets: &[&'static [Info<()>]]| Registry::new(sets).err().unwrap();
    assert_eq!(
        err(&[MALFORMED]).to_string(),
        r#"module: malformed ID "Handlers.x""#
    );
    assert_eq!(err(&[NO_NAMESPACE]), RegistryError::Malformed("respond"));
    assert_eq!(err(&[EMPTY_WORD]), RegistryError::Malformed("http..x"));
    assert_eq!(
        err(&[TWICE]).to_string(),
        "module: http.handlers.a is registered twice"
    );
    // Twice across two crates' lists, too.
    let one: &'static [Info<()>] = &TWICE[..1];
    assert_eq!(
        err(&[one, one]),
        RegistryError::Duplicate("http.handlers.a")
    );
    assert_eq!(
        err(&[WRONG_NAMESPACE]).to_string(),
        "module: http.handlers.m is a middleware, which belongs in http.middleware"
    );
    assert!(matches!(err(&[NESTED]), RegistryError::Namespace { .. }));
}

/// A provisioned route serves a request on akio: the chain runs on the I/O thread, middleware
/// first, and the modules are cleaned up once the server is done with them.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_provisioned_chain_serves_on_akio() {
    use akio::{Backend, Config, Engine, Listener, UringConfig};

    let backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    let r = registry();
    let log = log();
    let (route, modules) = compile(
        &r,
        &[
            ("mw", "tag", Params::new("outer", &log)),
            ("handler", "respond", Params::new("hello", &log)),
        ],
    )
    .unwrap();
    let e = Engine::builder()
        .config({
            let mut v = Config::default();
            v.shutdown_timeout = Duration::from_millis(500);
            v
        })
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend)
                .threads(1)
                .serve(akio_http::serve(HandlerService(route))),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    let mut s = TcpStream::connect(e.local_addrs()[0]).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /hello HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    e.shutdown();
    e.wait();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.to_ascii_lowercase().contains("x-tag: outer"), "{resp}");
    assert!(resp.ends_with("hello"), "{resp}");
    drop(e);
    modules.release();
    assert_eq!(
        logged(&log),
        ["http.handlers.respond cleaned", "tag outer cleaned"]
    );
}
