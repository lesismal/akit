//! `respond`: answers every request with the same fixed response, as fit's.
//!
//! akit-config checks the params when the config is read (the status is a final one, header
//! names are tokens and values hold no line break or NUL, a 204 or 304 has no body), so
//! provisioning only builds the response, once.

use std::sync::Arc;

use akio_http::{Context, Response};
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

/// The response, built once; each request sends a copy.
struct Respond {
    response: Response,
}

impl akio_http::Service for Respond {
    #[inline]
    fn call(&self, ctx: &mut Context) {
        ctx.respond(self.response.clone());
    }
}

impl Handler for Respond {}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Respond(r) = p.params();
    Ok(Provisioned::new(Arc::new(Respond {
        response: response(r),
    }) as Arc<dyn Handler>))
}

fn response(r: &akit_config::Respond) -> Response {
    let mut resp = Response::new(r.status);
    for (name, value) in &r.headers {
        resp.headers.set(name, value);
    }
    if !r.body.is_empty() && resp.headers.get("content-type").is_none() {
        resp.headers
            .set("Content-Type", "text/plain; charset=utf-8");
    }
    resp.body = r.body.as_bytes().to_vec();
    resp
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use akit_config::Respond;

    fn respond(status: u16, headers: &[(&str, &str)], body: &str) -> Respond {
        Respond {
            status,
            headers: headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn a_body_defaults_its_content_type() {
        let r = super::response(&respond(200, &[], "hi\n"));
        assert_eq!(r.status, 200);
        assert_eq!(
            r.headers.get("Content-Type"),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(r.body, b"hi\n");
    }

    #[test]
    fn headers_are_kept_and_win() {
        let r = super::response(&respond(
            404,
            &[("content-type", "application/json"), ("X-A", "1")],
            "{}",
        ));
        assert_eq!(r.status, 404);
        assert_eq!(r.headers.get("Content-Type"), Some("application/json"));
        assert_eq!(r.headers.get("x-a"), Some("1"));
    }

    #[test]
    fn no_body_no_content_type() {
        let r = super::response(&respond(204, &[], ""));
        assert_eq!(r.headers.get("Content-Type"), None);
        assert!(r.body.is_empty());
    }
}
