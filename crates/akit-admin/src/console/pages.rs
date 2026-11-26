//! The files the console serves (I91): fit's workbench, `app/workbench/web` at the fit commit the
//! compatibility gate pins, copied unchanged by `tools/console-pages.py` and built into akit
//! ([`files`]), served from memory as fit embeds them; and fit's config schema, the gate's
//! `fit.schema.json`, which the pages' forms are made from ([decision 036](../../../../docs/decisions/036-console.md)).

use akio_http::{Method, Response};

#[path = "files.rs"]
mod files;

pub(crate) use files::FIT_COMMIT;

/// fit's config schema at the gate's pin, as `GET /api/m/admin/schema` answers it.
pub(crate) const SCHEMA: &[u8] =
    include_bytes!("../../../../tests/compat/fixtures/fit/schema/fit.schema.json");

/// The content type of a file, by its extension, as Go's `mime.TypeByExtension` gives it.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, e)| e) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

/// The file at `path`, `index.html` for a directory, if there is one.
pub(crate) fn file(path: &str) -> Option<(&'static str, &'static [u8])> {
    let index;
    let path = if path.ends_with('/') {
        index = format!("{path}index.html");
        index.as_str()
    } else {
        path
    };
    files::FILES
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(p, data)| (content_type(p), *data))
}

/// The answer to `method` on `path`, a path outside `/api/`: the file, or 404; the pages are only
/// read.
pub(crate) fn serve(method: &Method, path: &str) -> Response {
    if !matches!(method, Method::Get | Method::Head) {
        return crate::server::error(405, "the pages are only read").header("Allow", "GET, HEAD");
    }
    match file(path) {
        Some((ct, data)) => Response::with_body(200, ct, data.to_vec()),
        None => Response::with_body(
            404,
            "text/plain; charset=utf-8",
            b"404 page not found\n".to_vec(),
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// The files built in are those in the directory, byte for byte.
    #[test]
    fn files_are_the_copied_ones() {
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("console/web");
        let mut on_disk = Vec::new();
        let mut stack = vec![web.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p
                        .strip_prefix(&web)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    on_disk.push((format!("/{rel}"), std::fs::read(&p).unwrap()));
                }
            }
        }
        on_disk.sort();
        let built: Vec<(String, Vec<u8>)> = files::FILES
            .iter()
            .map(|(p, d)| ((*p).to_owned(), d.to_vec()))
            .collect();
        assert_eq!(built.len(), on_disk.len());
        assert!(
            built == on_disk,
            "files.rs is stale: run tools/console-pages.py"
        );
        assert_eq!(FIT_COMMIT.len(), 40);
    }

    #[test]
    fn served_with_their_types() {
        let (ct, body) = file("/").unwrap();
        assert_eq!(ct, "text/html; charset=utf-8");
        assert!(body.starts_with(b"<!doctype html>"));
        assert_eq!(
            file("/core/ui.js").unwrap().0,
            "text/javascript; charset=utf-8"
        );
        assert_eq!(file("/core/ui.css").unwrap().0, "text/css; charset=utf-8");
        assert!(file("/nothing.js").is_none());
        assert!(file("/../Cargo.toml").is_none());
        assert_eq!(serve(&Method::Post, "/").status, 405);
        assert!(SCHEMA.starts_with(b"{"));
    }
}
