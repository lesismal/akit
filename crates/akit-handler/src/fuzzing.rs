//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. fit's `FuzzReader`
//! (`handler/fastcgi/fuzz_test.go`): what a FastCGI application sends back; and the paths a
//! request's target turns into, for FastCGI scripts and static files, and a CONNECT's target.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::time::Duration;

use akit_config::{Dotfiles, FastCgi, FastCgiAddress, Scan};

use crate::fastcgi::exchange::{header_end, parse_head};
use crate::fastcgi::params::{clean, join};
use crate::fastcgi::record::{Reader, STDOUT};
use crate::fastcgi::{candidate, origin_form, unescape};
use crate::{forward_proxy, static_files};

/// The records `data` holds, read in pieces of `step` bytes: each record's type and content, and
/// whether the stream broke FastCGI's framing.
fn records(data: &[u8], step: usize) -> (Vec<(u8, Vec<u8>)>, bool) {
    let mut r = Reader::default();
    let mut out = Vec::new();
    for chunk in data.chunks(step.max(1)) {
        let fed = r.feed(chunk, |rec| {
            assert!(rec.content.len() <= 65535);
            out.push((rec.t, rec.content.to_vec()));
            true
        });
        if fed.is_err() {
            return (out, true);
        }
        // fit's bound: one record's header, content and padding.
        assert!(r.kept() <= 8 + 65535 + 255, "{} bytes kept", r.kept());
    }
    (out, false)
}

/// `fastcgi_response`: what a FastCGI application sends. `data[0]` splits the reads; the rest is
/// the records. Read in pieces or whole, the same records come out (until the framing breaks);
/// the CGI header their standard output starts with parses to a status between 100 and 999 and
/// fields that are tokens with values that break no line, without the hop-by-hop ones.
pub fn fastcgi_response(data: &[u8]) {
    let Some((&split, wire)) = data.split_first() else {
        return;
    };
    let (whole, broke_whole) = records(wire, wire.len());
    let (pieces, broke_pieces) = records(wire, usize::from(split));
    if broke_whole == broke_pieces {
        assert_eq!(whole, pieces, "read in pieces of {split}");
    } else {
        // The whole stream breaks at the first bad header; read in pieces, the records before
        // it come out first. Either way the records read are a prefix of the other's.
        let n = whole.len().min(pieces.len());
        assert_eq!(whole[..n], pieces[..n]);
    }
    let mut stdout = Vec::new();
    for (t, content) in &whole {
        if *t == STDOUT {
            stdout.extend_from_slice(content);
        }
    }
    let Some((end, sep)) = header_end(&stdout) else {
        return;
    };
    assert!(sep == 2 || sep == 4);
    assert!(end + sep <= stdout.len());
    let Ok((status, headers, declared)) = parse_head(&stdout[..end]) else {
        return;
    };
    assert!((100..=999).contains(&status), "status {status}");
    for (name, value) in headers.iter() {
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)),
            "field name {name:?}"
        );
        assert!(!value.contains(['\r', '\n', '\0']), "{name}: {value:?}");
        for hop in ["connection", "transfer-encoding", "keep-alive", "upgrade"] {
            assert!(!name.eq_ignore_ascii_case(hop), "{name} was kept");
        }
        assert!(!name.eq_ignore_ascii_case("status"));
    }
    assert_eq!(declared, headers.get("content-length").is_some());
}

/// The FastCGI handler of a site at `/srv/www`, its `try_files` `templates`.
fn handler(templates: &[&str], dotfiles: Dotfiles) -> FastCgi {
    FastCgi {
        address: FastCgiAddress::Unix("/run/php.sock".into()),
        address_text: "unix:/run/php.sock".into(),
        root: "/srv/www".into(),
        split_path: vec![".php".into()],
        index: "index.php".into(),
        try_files: templates.iter().map(|t| (*t).to_owned()).collect(),
        dotfiles,
        env: Vec::new(),
        max_conns: 1,
        max_idle_conns: 1,
        max_pending: 1,
        connect_timeout: Duration::from_secs(1),
        response_header_timeout: Duration::from_secs(1),
        response_idle_timeout: Duration::from_secs(1),
        timeout: Duration::from_secs(1),
        idle_timeout: Duration::from_secs(1),
        max_header_bytes: 1 << 16,
    }
}

/// Whether `file` is `root` or under it.
fn under(root: &str, file: &str) -> bool {
    file == root || file.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// `fastcgi_paths`: the script a request's path runs, with and without `try_files`, and its path
/// info: whatever the path, the files named to the application (`SCRIPT_FILENAME`,
/// `PATH_TRANSLATED`) are under the root, and a script with a `..` segment, a NUL or a backslash
/// is refused. `data[0]` picks `dotfiles`; the rest is the request's target.
pub fn fastcgi_paths(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let Ok(target) = std::str::from_utf8(rest) else {
        return;
    };
    let origin = origin_form(target);
    assert!(
        origin.starts_with('/') || *origin == *target,
        "{target:?}: {origin:?}"
    );
    let raw = origin.split('?').next().unwrap_or("");
    let Some(path) = unescape(raw) else {
        return;
    };
    let dotfiles = if flags & 1 != 0 {
        Dotfiles::Allow
    } else {
        Dotfiles::Deny
    };
    let root = "/srv/www";
    let check = |script: &str, info: &str| {
        let name = clean(script);
        assert!(name.starts_with('/'), "SCRIPT_NAME {name:?}");
        assert_eq!(clean(&name), name, "clean is not idempotent on {script:?}");
        let file = join(root, &name);
        assert!(under(root, &file), "{path:?}: SCRIPT_FILENAME {file:?}");
        // The listener hands the handler a canonical path (`akit_router::canonicalize`, fuzzed
        // by `router_path`), with no `..` segment left in it.
        if !info.is_empty() && !info.split('/').any(|s| s == "..") {
            let t = join(root, info);
            assert!(under(root, &t), "{path:?}: PATH_TRANSLATED {t:?}");
        }
    };
    let h = handler(&[], dotfiles);
    let (script, info) = h.split(&path);
    match h.check(&script) {
        0 => {
            assert_ne!(akit_config::scan(&script), Scan::Unsafe);
            check(&script, info);
        }
        400 => assert_eq!(akit_config::scan(&script), Scan::Unsafe),
        404 => assert_eq!(dotfiles, Dotfiles::Deny),
        s => panic!("status {s}"),
    }
    let templates = [
        "{path}",
        "{path}/index.php",
        "/index.php?{path}",
        "app/{path}.php",
    ];
    let h = handler(&templates, dotfiles);
    for t in templates {
        match candidate(&h, t, &path) {
            Ok(Some((script, info))) => check(&script, &info),
            Ok(None) => {}
            Err(s) => assert_eq!(s, 400),
        }
    }
}

/// `static_path`: the static handler's look at a request path as sent, decoding as it goes,
/// says what a look at the decoded path says; and a CONNECT's `host:port` target splits into a
/// host and a port, or is refused.
pub fn static_path(data: &[u8]) {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    let s = static_files::scan(raw);
    match unescape(raw) {
        Some(decoded) => assert_eq!(
            s,
            static_files::scan_decoded(&decoded),
            "{raw:?}, decoded {decoded:?}"
        ),
        None => {
            // A broken escape is unsafe; one that decodes to what is not UTF-8 is looked at as
            // bytes, which the decoded look cannot take.
            if !raw.contains('%') {
                panic!("{raw:?} does not decode");
            }
        }
    }
    if let Some((host, port)) = forward_proxy::target(raw) {
        assert!(!host.is_empty());
        let (_, p) = raw.rsplit_once(':').unwrap();
        assert_eq!(p.parse::<u16>().ok(), Some(port));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_fastcgi_response() {
        replay("fastcgi_response", fastcgi_response);
    }

    #[test]
    fn corpus_fastcgi_paths() {
        replay("fastcgi_paths", fastcgi_paths);
    }

    #[test]
    fn corpus_static_path() {
        replay("static_path", static_path);
    }
}
