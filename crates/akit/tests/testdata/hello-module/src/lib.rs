#![forbid(unsafe_code)]
//! A module built into akit with `akit build`: the middleware `hello`, which adds `X-Hello` to
//! every response of its routes. docs/modules.md walks through it.
//!
//! A module crate exports `MODULES` at its root: the list `akit build`'s generated `main` hands
//! akit's registry. Each entry is an ID (its kind's namespace, `http.middleware` here, and the
//! name configs know it by), a doc line and a constructor.

use std::sync::Arc;

use akit_runtime::akio_http::{BodyMode, Context, Request, Response, Service};
use akit_runtime::config::Handler as Params;
use akit_runtime::config::value::Value;
use akit_runtime::{
    Handler, HandlerParams, Info, Middleware, Provision, ProvisionError, Provisioned,
};

/// The modules of this crate, for akit's registry.
pub static MODULES: &[Info<HandlerParams>] = &[Info::middleware(
    "http.middleware.hello",
    "Adds X-Hello to every response of its routes.",
    new,
)];

/// The longest greeting taken.
const MAX_GREETING: usize = 256;

/// The middleware, compiled: what serving takes, built once, when the config is.
struct Hello {
    value: Arc<str>,
}

/// Checks the params and builds the middleware, when the config is compiled; requests only read
/// what it built. Its params come as written in the config ([`Params::Module`]): it decodes them
/// as strictly as akit decodes its own, every field it does not know an error.
fn new(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let Params::Module(m) = p.params() else {
        return Err(ProvisionError::new(
            "the hello middleware was given another's params",
        ));
    };
    let mut greeting = String::from("world");
    match &m.params {
        None => {}
        Some(Value::Object(members)) => {
            for (key, value) in members {
                match (key.as_str(), value) {
                    ("greeting", Value::String(s)) if s.is_empty() => {}
                    ("greeting", Value::String(s)) => greeting.clone_from(s),
                    ("greeting", _) => {
                        return Err(ProvisionError::field("greeting", "expected a string"));
                    }
                    (key, _) => return Err(ProvisionError::field(key, "unknown field")),
                }
            }
        }
        Some(_) => return Err(ProvisionError::new("expected an object")),
    }
    if greeting.len() > MAX_GREETING {
        return Err(ProvisionError::field(
            "greeting",
            format!("is longer than {MAX_GREETING} bytes"),
        ));
    }
    if greeting.contains(|c: char| c.is_control()) {
        return Err(ProvisionError::field("greeting", "holds a control character"));
    }
    Ok(Provisioned::new(Arc::new(Hello {
        value: greeting.into(),
    }) as Arc<dyn Middleware>))
}

impl Middleware for Hello {
    /// Called once, when the route is compiled.
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Greeted {
            value: Arc::clone(&self.value),
            next,
        })
    }
}

/// A route's handler behind the middleware. It runs on an akio I/O thread for every request, so
/// it never blocks: no I/O, no waiting, no lock held for long.
struct Greeted {
    value: Arc<str>,
    next: Arc<dyn Handler>,
}

impl Service for Greeted {
    fn call(&self, ctx: &mut Context) {
        let value = Arc::clone(&self.value);
        // Every response of the request, whenever the handler writes it.
        ctx.add_response_filter(move |resp: &mut Response, _: &Request| {
            resp.headers.set("X-Hello", &value);
        });
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Greeted {}
