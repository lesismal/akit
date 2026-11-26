//! The files the console serves (I91): none yet.

use akio_http::{Method, Response};

/// The answer to `method` on `path`, a path outside `/api/`: the pages are only read.
pub(crate) fn serve(method: &Method, path: &str) -> Response {
    let _ = path;
    if !matches!(method, Method::Get | Method::Head) {
        return crate::server::error(405, "the pages are only read").header("Allow", "GET, HEAD");
    }
    Response::with_body(
        404,
        "text/plain; charset=utf-8",
        b"404 page not found\n".to_vec(),
    )
}
