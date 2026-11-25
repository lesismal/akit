//! A request changed by a middleware before its handler: its fields and its path, changed in
//! place on akio's request ([`Context::req_mut`], akio ADR-126), so the handlers after it and what
//! akio itself reads of the request (the `static` handler's conditional and range fields, the
//! response filters) see the change.

use akio_http::{Context, Headers};

/// Makes `fields` the request's header fields. Nothing once the request is shared (a middleware
/// that detached it), which no middleware does before calling its handler.
pub fn set_request_headers(ctx: &mut Context, fields: Headers) {
    if let Some(r) = ctx.req_mut() {
        r.headers = fields;
    }
}

/// Makes `path`, escaped, the path of the request's target, its query kept. Nothing once the
/// request is shared, as [`set_request_headers`].
pub fn set_request_path(ctx: &mut Context, path: &str) {
    if let Some(r) = ctx.req_mut() {
        r.target = with_path(&r.target, path);
    }
}

/// `target` with `path` in place of its path, its query kept.
#[must_use]
pub fn with_path(target: &str, path: &str) -> String {
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let mut t = String::with_capacity(path.len().saturating_add(query.len()).saturating_add(1));
    t.push_str(path);
    if target.contains('?') {
        t.push('?');
        t.push_str(query);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_replaced_queries_kept() {
        assert_eq!(with_path("/a/../b", "/b"), "/b");
        assert_eq!(with_path("/a/./b?x=1&y=/z", "/a/b"), "/a/b?x=1&y=/z");
        assert_eq!(with_path("/a?", "/c"), "/c?");
        assert_eq!(with_path("", "/"), "/");
    }
}
