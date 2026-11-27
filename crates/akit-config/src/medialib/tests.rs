//! fit's `config/medialibrary_test.go`, ported (I140). The uploads' fields wait for I149 and are
//! refused as not supported yet.

use super::*;
use crate::from_json;

const HEAD: &str = r#"{"listeners": [{"name": "web", "address": ":8080"}], "media_library": "#;

fn load(doc: &str) -> Result<crate::Config, String> {
    from_json(&format!("{HEAD}{doc}}}"), None).map_err(|e| e.to_string())
}

/// fit's `TestMediaLibrary`.
#[test]
fn media_library() {
    let c = load(
        r#"[{"name": "films", "roots": ["/srv/films", "/srv/shows"], "files": ["*.mp4", "films/**/*.m4a"],
		"exclude": ["**/extras/**"], "follow_symlinks": true, "scan_interval": "1h", "max_items": 10, "max_table_bytes": 1000}]"#,
    )
    .unwrap();
    let m = c.media_library.iter().find(|m| m.name == "films").unwrap();
    assert!(!c.media_library.iter().any(|m| m.name == "none"));
    assert_eq!(m.interval(), Duration::from_secs(3600));
    assert_eq!((m.items(), m.table_bytes()), (10, 1000));
    assert_eq!(m.files_bound(), DEFAULT_MAX_FILES);
    assert_eq!(m.catalog_bytes(), DEFAULT_MAX_CATALOG_BYTES);
    assert_eq!(m.probes(), DEFAULT_MAX_PROBES);
    assert_eq!(m.sidecar_bytes(), DEFAULT_MAX_SIDECAR_BYTES);
    assert_eq!(m.subtitle_bytes(), DEFAULT_MAX_SUBTITLE_BYTES);
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    assert_eq!(m.workers(), 16.max(4 * cpus));
    assert_eq!(
        (m.queue(), m.timeout()),
        (DEFAULT_FILE_QUEUE, DEFAULT_FILE_TIMEOUT)
    );
    assert!(m.follow_symlinks && m.files.len() == 2 && m.exclude.len() == 1);
    assert!(!m.encryptable());
    let with = |f: fn(&mut MediaLibrary)| {
        let mut m = MediaLibrary::default();
        f(&mut m);
        m
    };
    assert!(with(|m| m.secret_file = "/s".into()).encryptable());
    assert!(with(|m| m.secret = "s".into()).encryptable());
    assert_eq!(MediaLibrary::default().interval(), DEFAULT_SCAN_INTERVAL);
    assert_eq!(with(|m| m.file_workers = 3).workers(), 3);
    let abs = std::env::current_dir().unwrap().join("x");
    let inside = format!(
        r#"[{{"name": "a", "roots": ["{}/y", "x"]}}]"#,
        abs.display()
    );
    for (doc, want) in [
        (r#"[{"name": "-x", "roots": ["/a"]}]"#, "not a valid name"),
        (
            r#"[{"name": "a", "roots": ["/a"]}, {"name": "a", "roots": ["/b"]}]"#,
            r#"another media_library is already named "a""#,
        ),
        (r#"[{"name": "a", "roots": []}]"#, "at least one root"),
        (r#"[{"name": "a"}]"#, "roots"),
        (
            r#"[{"name": "a", "roots": [" ", "/a"]}]"#,
            "a root may not be empty",
        ),
        (r#"[{"name": "a", "roots": ["/a", "/a/"]}]"#, "listed twice"),
        (
            r#"[{"name": "a", "roots": ["/a", "/a/b"]}]"#,
            "lie one inside the other",
        ),
        (&inside, "lie one inside the other"),
        (
            r#"[{"name": "a", "roots": ["/a"], "files": ["["]}]"#,
            "is not a pattern",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "exclude": ["/abs"]}]"#,
            "is not a pattern",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "exclude": ["a//b"]}]"#,
            "is not a pattern",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "exclude": [""]}]"#,
            "is not a pattern",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "scan_interval": "1s"}]"#,
            "at least 10s",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "max_sidecar_bytes": 2000000}]"#,
            "greater than the maximum",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "secret": "0123456789abcdef", "secret_file": "/s"}]"#,
            "one of the two",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "secret": "short"}]"#,
            "use at least 16",
        ),
        // fit's checks of the uploads (I139) come with them, in I149.
        (
            r#"[{"name": "a", "roots": ["/a"], "upload_dir": "/a/up"}]"#,
            "not supported yet",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "max_uploads": 2}]"#,
            "not supported yet",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "upload_expiry": "1h"}]"#,
            "not supported yet",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "max_upload_bytes": 1}]"#,
            "not supported yet",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "min_free_bytes": 1}]"#,
            "not supported yet",
        ),
        (
            r#"[{"name": "a", "roots": ["/a"], "nope": 1}]"#,
            "unknown field",
        ),
    ] {
        match load(doc) {
            Ok(_) => panic!("{doc}: loaded, want {want:?}"),
            Err(e) => assert!(e.contains(want), "{doc}: {e}, want {want:?}"),
        }
    }
    for p in ["*.mp4", "**", "a/**/b", "[a-z]?.mp4"] {
        assert!(valid_path_pattern(p), "{p}");
    }
    let (a, ab, a_b) = (Path::new("/a"), Path::new("/ab"), Path::new("/a/b"));
    assert!(!within(a, a) && !within(a, ab) && within(a, a_b) && !within(a_b, a));
    assert_eq!(clean(Path::new("/b/x/../up/./")), Path::new("/b/up"));
    assert_eq!(clean(Path::new("/../a")), Path::new("/a"));
    assert_eq!(clean(Path::new("../../a")), Path::new("../../a"));
    assert_eq!(clean(Path::new("a/../../b")), Path::new("../b"));
    assert_eq!(clean(Path::new("a/..")), Path::new("."));
}

/// Go's `path.Match`, as Go's own tests have it (`path/match_test.go`).
#[test]
fn go_path_match() {
    let ok = |p: &str, s: &str, want: bool| {
        assert_eq!(path_match(p, s), Ok(want), "Match({p:?}, {s:?})");
    };
    let bad = |p: &str, s: &str| {
        assert_eq!(path_match(p, s), Err(BadPattern), "Match({p:?}, {s:?})");
    };
    ok("abc", "abc", true);
    ok("*", "abc", true);
    ok("*c", "abc", true);
    ok("a*", "a", true);
    ok("a*", "abc", true);
    ok("a*", "ab/c", false);
    ok("a*/b", "abc/b", true);
    ok("a*/b", "a/c/b", false);
    ok("a*b*c*d*e*/f", "axbxcxdxe/f", true);
    ok("a*b*c*d*e*/f", "axbxcxdxexxx/f", true);
    ok("a*b*c*d*e*/f", "axbxcxdxe/xxx/f", false);
    ok("a*b*c*d*e*/f", "axbxcxdxexxx/fff", false);
    ok("a*b?c*x", "abxbbxdbxebxczzx", true);
    ok("a*b?c*x", "abxbbxdbxebxczzy", false);
    ok("ab[c]", "abc", true);
    ok("ab[b-d]", "abc", true);
    ok("ab[e-g]", "abc", false);
    ok("ab[^c]", "abc", false);
    ok("ab[^b-d]", "abc", false);
    ok("ab[^e-g]", "abc", true);
    ok("a\\*b", "a*b", true);
    ok("a\\*b", "ab", false);
    ok("a?b", "a☺b", true);
    ok("a[^a]b", "a☺b", true);
    ok("a???b", "a☺b", false);
    ok("a[^a][^a][^a]b", "a☺b", false);
    ok("[a-ζ]*", "α", true);
    ok("*[a-ζ]", "A", false);
    ok("a?b", "a/b", false);
    ok("a*b", "a/b", false);
    ok("[\\]a]", "]", true);
    ok("[\\-]", "-", true);
    ok("[x\\-]", "x", true);
    ok("[x\\-]", "-", true);
    ok("[x\\-]", "z", false);
    ok("[\\-x]", "x", true);
    ok("[\\-x]", "-", true);
    ok("[\\-x]", "a", false);
    bad("[]a]", "]");
    bad("[-]", "-");
    bad("[x-]", "x");
    bad("[x-]", "-");
    bad("[x-]", "z");
    bad("[-x]", "x");
    bad("[-x]", "-");
    bad("[-x]", "a");
    bad("\\", "a");
    bad("[a-b-c]", "a");
    bad("[", "a");
    bad("[^", "a");
    bad("[^bc", "a");
    bad("a[", "a");
    bad("a[", "ab");
    bad("a[", "x");
    bad("a/b[", "x");
    ok("*x", "xxx", true);
}
