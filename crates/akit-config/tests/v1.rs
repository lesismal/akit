//! I79, the v1 freeze: `engine` read as fit reads it, and the old names of the fields renamed in
//! schema version 1 read with a warning at their place, as fit's `renamed` tag has them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akit_config::{Engine, from_conf, from_json, from_yaml};

#[test]
fn engine_read_as_fit_reads_it() {
    // Left out, empty, or every field 0: akio's defaults.
    for src in [
        "{}",
        r#"{"engine": {}}"#,
        r#"{"engine": {"event_loops": 0, "workers": 0, "min_workers": 0, "task_queue": 0,
            "read_buffer_size": 0, "write_watermark": 0, "backlog": 0}}"#,
    ] {
        assert_eq!(from_json(src, None).unwrap().engine, Engine::default());
    }
    let c = from_json(
        r#"{"engine": {"event_loops": 8, "workers": 64, "min_workers": 8, "task_queue": 65536,
            "read_buffer_size": 65536, "write_watermark": 262144, "backlog": 4096}}"#,
        None,
    )
    .unwrap();
    let e = c.engine;
    assert_eq!(
        (e.event_loops, e.workers, e.min_workers, e.task_queue),
        (8, 64, 8, 65536)
    );
    assert_eq!(
        (e.read_buffer_size, e.write_watermark, e.backlog),
        (65536, 262_144, 4096)
    );
    // fit's schema's bounds.
    for (field, value, msg) in [
        (
            "event_loops",
            "1025",
            "1025 is greater than the maximum 1024",
        ),
        (
            "read_buffer_size",
            "67108865",
            "67108865 is greater than the maximum 67108864",
        ),
        ("workers", "-1", "-1 is less than the minimum 0"),
        ("backlog", "\"lots\"", "expected an integer, got a string"),
    ] {
        let e = from_json(&format!(r#"{{"engine": {{"{field}": {value}}}}}"#), None).unwrap_err();
        assert!(!e.only_not_supported_yet(), "{e}");
        assert!(
            e.to_string().ends_with(&format!(": engine.{field}: {msg}")),
            "{e}"
        );
    }
    // fib's event loops' own settings and kernel TLS are refused by name.
    let e = from_json(
        r#"{"engine": {"loop_wait": "block", "poll_batch": 64, "kernel_tls_offload": true}}"#,
        None,
    )
    .unwrap_err();
    assert!(e.only_not_supported_yet(), "{e}");
    let mut paths: Vec<&str> = e.as_slice().iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "engine.kernel_tls_offload",
            "engine.loop_wait",
            "engine.poll_batch"
        ]
    );
    let e = from_json(r#"{"engine": {"threads": 4}}"#, None).unwrap_err();
    assert!(!e.only_not_supported_yet(), "{e}");
}

/// fit-examples' `limits` example, in fit's `.conf`, but for `max_pending_bytes`.
#[test]
fn engine_and_limits_in_conf() {
    let c = from_conf(
        "listener web :8080 http/1.1 h2c\n\
         route hello { handler respond { body \"hello\\n\" } }\n\
         limits {\n\tmax_header_bytes 64KiB\n\tmax_conns 200000\n\tmax_body_bytes 32MiB\n\
         \tmax_write_queue_bytes 8MiB\n\twrite_stall_timeout 30s\n}\n\
         engine {\n\tevent_loops 8\n\tworkers 64\n\tmin_workers 8\n\ttask_queue 65536\n\
         \tread_buffer_size 64KiB\n\twrite_watermark 262144\n\tbacklog 4096\n}\n",
        Some("fit.conf"),
    )
    .unwrap();
    assert_eq!(c.limits.max_header_bytes, 64 << 10);
    assert_eq!(c.listeners[0].max_header_bytes, 64 << 10);
    assert_eq!(c.limits.max_conns, 200_000);
    assert_eq!(c.limits.max_write_queue_bytes, 8 << 20);
    assert_eq!(c.engine.event_loops, 8);
    assert_eq!(c.engine.read_buffer_size, 64 << 10);
}

/// The fields renamed for 1.0 (fit's I77): each old name read as the new one, with fit's warning
/// at its file, line, column and path; both names given is an error, as in fit.
#[test]
fn renamed_fields_warn() {
    let cases = [
        ("cache", r#""max_size": 1048576"#, "max_size", "max_bytes"),
        (
            "cache",
            r#""max_entry_size": 4096"#,
            "max_entry_size",
            "max_entry_bytes",
        ),
        (
            "cache",
            r#""disk": {"path": "/tmp/akit-cache", "max_size": 1048576}"#,
            "disk.max_size",
            "max_bytes",
        ),
        ("compress", r#""min_length": 10"#, "min_length", "min_bytes"),
        (
            "jwt",
            r#""jwks_url": "https://example.com/jwks", "jwks_refresh": "5m""#,
            "jwks_refresh",
            "jwks_refresh_interval",
        ),
        (
            "wasm",
            r#""path": "p.wasm", "max_memory": 1048576"#,
            "max_memory",
            "max_memory_bytes",
        ),
    ];
    for (module, params, old, new) in cases {
        let src = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":1"}}], "routes": [{{"middleware": [{{"module": "{module}", "params": {{{params}}}}}],
                "handler": {{"module": "respond"}}}}]}}"#
        );
        let c = from_json(&src, Some("fit.json")).unwrap_or_else(|e| panic!("{module}: {e}"));
        let w = &c.warnings.0;
        assert_eq!(w.len(), 1, "{module}: {w:?}");
        // At the old name's value, as the decoder reports a field.
        assert_eq!(w[0].file.as_deref(), Some("fit.json"));
        assert_eq!(w[0].line, 1, "{module}: {}", w[0]);
        assert!(w[0].col > 0);
        assert_eq!(w[0].path, format!("routes[0].middleware[0].params.{old}"));
        assert_eq!(
            w[0].msg,
            format!(
                "deprecated: renamed {new:?}, which takes the same value; the old name goes in \
                 config version 2"
            )
        );
    }
    // Read as the new name.
    let c = from_json(
        r#"{"listeners": [{"name": "web", "address": ":1"}], "routes": [{"middleware": [{"module": "compress", "params": {"min_length": 10}}],
            "handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    let akit_config::Handler::Compress(compress) = &c.routes[0].middleware[0] else {
        panic!("{:?}", c.routes[0].middleware[0]);
    };
    assert_eq!(compress.min_bytes, 10);
    // Both names: fit's error.
    for (module, params, msg) in [
        (
            "cache",
            r#""max_size": 1, "max_bytes": 1"#,
            r#""max_size" is the old name of "max_bytes"; give only "max_bytes""#,
        ),
        (
            "compress",
            r#""min_length": 1, "min_bytes": 1"#,
            r#""min_length" is the old name of "min_bytes"; give only "min_bytes""#,
        ),
    ] {
        let src = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":1"}}], "routes": [{{"middleware": [{{"module": "{module}", "params": {{{params}}}}}],
                "handler": {{"module": "respond"}}}}]}}"#
        );
        let e = from_json(&src, None).unwrap_err();
        assert_eq!(e.as_slice().len(), 1, "{e}");
        assert!(e.to_string().ends_with(msg), "{e}");
    }
    // The new names say nothing.
    let c = from_json(
        r#"{"listeners": [{"name": "web", "address": ":1"}], "routes": [{"middleware": [{"module": "compress", "params": {"min_bytes": 10}}],
            "handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    assert!(c.warnings.0.is_empty());
}

/// A warning points into the file as written: YAML's line and column, not JSON's.
#[test]
fn warnings_in_yaml_and_conf() {
    let c = from_yaml(
        "listeners:\n  - name: web\n    address: \":1\"\nroutes:\n  - middleware:\n      - module: compress\n        params:\n          min_length: 10\n    handler:\n      module: respond\n",
        Some("fit.yaml"),
    )
    .unwrap();
    assert_eq!(c.warnings.0.len(), 1);
    let w = c.warnings.0[0].to_string();
    assert!(
        w.starts_with("fit.yaml:8:11: routes[0].middleware[0].params.min_length: deprecated: "),
        "{w}"
    );
    let c = from_conf(
        "listener web :1\nroute { middleware compress { min_length 10 }\n handler respond }\n",
        Some("fit.conf"),
    )
    .unwrap();
    assert_eq!(c.warnings.0.len(), 1, "{:?}", c.warnings);
    assert!(
        c.warnings.0[0].to_string().starts_with("fit.conf:2:"),
        "{}",
        c.warnings.0[0]
    );
    // Configs equal but for their warnings are equal.
    let a = from_json(
        r#"{"listeners": [{"name": "web", "address": ":1"}], "routes": [{"middleware": [{"module": "compress", "params": {"min_length": 10}}],
            "handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    let b = from_json(
        r#"{"listeners": [{"name": "web", "address": ":1"}], "routes": [{"middleware": [{"module": "compress", "params": {"min_bytes": 10}}],
            "handler": {"module": "respond"}}]}"#,
        None,
    )
    .unwrap();
    assert_eq!(a, b);
}
