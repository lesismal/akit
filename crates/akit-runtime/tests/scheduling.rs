//! akit's listeners on akio's event loops (A303): `akit_runtime::scheduled` chooses them where
//! akio's default, on epoll/kqueue, is work stealing; `AKIO_DEFAULT_SCHEDULING` set leaves the
//! choice to akio.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::ErrorKind;

use akio::{Backend, Engine, Listener};
use akio_http::Context;

fn listener() -> Listener {
    Listener::tcp("127.0.0.1:0".parse().unwrap())
        .backend(Backend::Epoll)
        .threads(1)
        .serve(akio_http::serve(|ctx: &mut Context| ctx.text(200, "")))
}

/// akio resizes the threads of a listener on event loops and refuses a work-stealing one
/// (`Unsupported`): the listener `scheduled` returns is resized, one left to akio's default is
/// not.
#[test]
fn listeners_run_on_event_loops() {
    if std::env::var_os("AKIO_DEFAULT_SCHEDULING").is_some_and(|v| !v.is_empty()) {
        // The variable chooses, as `scheduled` documents.
        return;
    }
    let e = Engine::builder()
        .listen(akit_runtime::scheduled(listener()))
        .build()
        .unwrap();
    e.start().unwrap();
    let ours = e.listeners()[0].id;
    e.set_listener_threads(ours, 2).unwrap();
    let unchosen = e.add_listener(listener()).unwrap();
    let err = e.set_listener_threads(unchosen, 2).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported, "{err}");
    e.shutdown();
    e.wait();
}
