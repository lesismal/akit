//! The dashboard (I63): fit's read-only web pages of the Admin API's GET endpoints (`/status`,
//! `/config/effective`, `/versions`), served under `/dashboard/` unless `admin.disable_dashboard`.
//! Plain HTML, CSS and JavaScript built into akit, which fetch nothing from elsewhere and send
//! nothing but GETs; with the headers fit sends them with, a Content-Security-Policy that keeps
//! them to their own files and the API's origin among them.
//!
//! The files hold no data, so they are served without the token; the page asks for it and sends
//! it with its requests. Those carry the API's own origin, which [`crate::server`]'s guard lets
//! read and not write.

use akio_http::Response;

/// The dashboard's Content-Security-Policy, fit's `dashboardCSP`.
pub const DASHBOARD_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; \
     frame-ancestors 'none'";

/// The dashboard's files: name, content type, content.
pub(crate) const FILES: &[(&str, &str, &str)] = &[
    (
        "index.html",
        "text/html; charset=utf-8",
        include_str!("dashboard/index.html"),
    ),
    (
        "app.js",
        "text/javascript; charset=utf-8",
        include_str!("dashboard/app.js"),
    ),
    (
        "app.css",
        "text/css; charset=utf-8",
        include_str!("dashboard/app.css"),
    ),
];

/// Whether a request for `path` with a GET or HEAD asks for one of the dashboard's files, as
/// fit's `isDashboard`.
pub(crate) fn is_dashboard(path: &str) -> bool {
    path == "/dashboard" || path.starts_with("/dashboard/")
}

/// The answer to a GET or HEAD of `path`, one of the dashboard's ([`is_dashboard`]), as fit's
/// `http.FileServer` under its headers answers it: `/dashboard` and `/dashboard/index.html`
/// redirected to `/dashboard/`, a file, or 404.
pub(crate) fn serve(path: &str) -> Response {
    let name = match path {
        "/dashboard" | "/dashboard/index.html" => {
            return headers(Response::new(301).header("Location", "/dashboard/"));
        }
        "/dashboard/" => "index.html",
        p => p.get("/dashboard/".len()..).unwrap_or(""),
    };
    match FILES.iter().find(|(n, _, _)| *n == name) {
        Some((_, ty, data)) => headers(Response::with_body(200, ty, data.as_bytes().to_vec())),
        None => headers(Response::with_body(
            404,
            "text/plain; charset=utf-8",
            b"404 page not found\n".to_vec(),
        )),
    }
}

/// fit's `dashboardHandler` headers.
fn headers(resp: Response) -> Response {
    resp.header("Content-Security-Policy", DASHBOARD_CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("X-Frame-Options", "DENY")
        .header("Referrer-Policy", "no-referrer")
        .header("Cache-Control", "no-cache")
}
