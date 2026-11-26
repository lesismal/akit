//! `log` (I82): akit's own log, as fit's `config.Log`: its fields, what a logger takes from the
//! log, its files, and fit's checks (`checkLog`, `checkLogFileSharing`).
#![allow(clippy::panic, clippy::unwrap_used, clippy::indexing_slicing)]

use akit_config::{Log, LogFormat, LogLevel, Logger, Rotate, from_conf, from_json};

const BASE: &str = r#""listeners": [{"name": "web", "address": ":8080"}],
  "routes": [{"handler": {"module": "respond"}}]"#;

fn log(fields: &str) -> Result<Log, Vec<String>> {
    from_json(&format!("{{{BASE}, \"log\": {{{fields}}}}}"), None)
        .map(|c| c.log.unwrap())
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

fn config(rest: &str) -> Result<akit_config::Config, Vec<String>> {
    from_json(&format!("{{{BASE}, {rest}}}"), None)
        .map_err(|e| e.as_slice().iter().map(ToString::to_string).collect())
}

#[test]
fn defaults_and_what_loggers_take() {
    let l = log("").unwrap();
    assert_eq!(l, Log::default());
    assert_eq!(l.level_of("tls"), LogLevel::Info);
    assert_eq!(l.output_of("tls"), "");
    assert_eq!(l.format_of("tls"), LogFormat::Line);
    assert!(l.file_outputs().is_empty());
    let l = log(
        r#""level": "warn", "levels": {"tls": "debug"}, "format": "json",
           "output": "/var/log/akit.log", "rotate": {"max_files": 3}, "time_zone": "utc",
           "loggers": {"engine": {"output": "/var/log/engine.log", "format": "line",
                                  "rotate": {"interval": "1h"}},
                       "admin": {"level": "error"}}"#,
    )
    .unwrap();
    assert_eq!(l.level_of("tls"), LogLevel::Debug);
    assert_eq!(l.level_of("admin"), LogLevel::Error);
    assert_eq!(l.level_of("server"), LogLevel::Warn);
    assert_eq!(l.format_of("engine"), LogFormat::Line);
    assert_eq!(l.format_of("server"), LogFormat::Json);
    assert_eq!(l.output_of("engine"), "/var/log/engine.log");
    assert_eq!(l.output_of("admin"), "/var/log/akit.log");
    assert_eq!(
        l.rotate_of("engine").unwrap().interval,
        std::time::Duration::from_secs(3600)
    );
    assert_eq!(l.rotate_of("admin").unwrap().max_files, Some(3));
    let outs = l.file_outputs();
    assert_eq!(outs.len(), 2);
    assert_eq!(outs[0].path, "/var/log/akit.log");
    assert!(outs[0].loggers.contains(&"server") && !outs[0].loggers.contains(&"engine"));
    assert_eq!(outs[1].loggers, ["engine"]);
    assert_eq!(
        l.loggers[1],
        (
            "admin".to_owned(),
            Logger {
                level: Some(LogLevel::Error),
                ..Logger::default()
            }
        )
    );
}

#[test]
fn fit_checks() {
    for (fields, want) in [
        (
            r#""level": "trace""#,
            r#"log.level: "trace" is not one of "debug", "info", "warn", "error""#,
        ),
        (
            r#""format": "xml""#,
            r#"log.format: "xml" is not one of "line", "text", "json""#,
        ),
        (
            r#""levels": {"http": "info"}"#,
            r#"log.levels["http"]: no logger is named "http": the loggers are server, admin, tls"#,
        ),
        (
            r#""levels": {"tls": "loud"}"#,
            r#"log.levels["tls"]: "loud" is not a level: debug, info, warn or error"#,
        ),
        (
            r#""loggers": {"http": {}}"#,
            r#"log.loggers["http"]: no logger is named "http""#,
        ),
        (
            r#""levels": {"tls": "debug"}, "loggers": {"tls": {"level": "info"}}"#,
            r#"log.loggers["tls"].level: logger tls has a level in levels too: set it in one place"#,
        ),
        (
            r#""rotate": {"interval": "100ms"}"#,
            "log.rotate.interval: 100ms is too short",
        ),
        (
            r#""loggers": {"engine": {"rotate": {"max_bytes": -1}}}"#,
            r#"log.loggers["engine"].rotate.max_bytes: -1 is less than the minimum 0"#,
        ),
        (
            r#""output": "/l/a.log", "rotate": {}, "loggers": {"tls": {"output": "/l/./a.log", "rotate": {"max_files": 1}}}"#,
            r#"log.loggers["tls"].output: logger tls writes the log's other loggers' file "/l/./a.log" with other rotation settings: a file has one rotate"#,
        ),
        (
            r#""loggers": {"tls": {"output": "/l/b.log"}, "admin": {"output": "/l/b.log", "rotate": {}}}"#,
            r#"log.loggers["tls"].output: logger tls writes the log's other loggers' file "/l/b.log""#,
        ),
        (
            r#""loggers": {"tls": {"colour": 1}}"#,
            r#"log.loggers["tls"].colour: unknown field"#,
        ),
        (r#""rotation": {}"#, "log.rotation: unknown field"),
    ] {
        let e = log(fields).unwrap_err();
        assert_eq!(e.len(), 1, "{fields}: {e:?}");
        assert!(e[0].contains(want), "{fields}: {e:?}");
    }
    // One file, one policy: the same settings are fine.
    log(r#""output": "/l/a.log", "rotate": {}, "loggers": {"tls": {"output": "/l/a.log", "rotate": {"max_bytes": 1073741824}}}"#)
        .unwrap();
}

#[test]
fn a_file_shared_with_the_access_log() {
    let ok = config(
        r#""log": {"output": "/l/all.log", "rotate": {}, "time_zone": "UTC"},
           "access_log": {"output": "/l/all.log", "rotate": {"compress": true}, "time_zone": "utc"}"#,
    );
    assert!(ok.is_ok(), "{ok:?}");
    let e = config(
        r#""log": {"loggers": {"engine": {"output": "/l/all.log"}}, "time_zone": "Asia/Shanghai"},
           "access_log": {"output": "/l/all.log", "rotate": {}}"#,
    )
    .unwrap_err();
    assert_eq!(e.len(), 2, "{e:?}");
    assert!(
        e[0].contains(r#"access_log.rotate: the access log writes the log's file "/l/all.log" with other rotation settings"#),
        "{e:?}"
    );
    assert!(
        e[1].contains(r#"access_log.time_zone: the access log writes the log's file "/l/all.log" in Local, not the log's Asia/Shanghai"#),
        "{e:?}"
    );
    // Different files: no problem.
    config(r#""log": {"output": "/l/a.log"}, "access_log": {"output": "/l/b.log", "rotate": {}}"#)
        .unwrap();
}

#[test]
fn fit_logging_example() {
    // fit-examples' logging/fit.conf, as the gate reads it.
    let c = from_conf(
        "listener web :8080\nroute hello { handler respond { body \"hello\\n\" } }\nlog {\n\tlevel info\n\tlevels { tls debug }\n\tformat json\n\tloggers {\n\t\tengine { output /var/log/fit/engine.log; format line }\n\t}\n}\naccess_log {\n\toutput stdout\n\tformat json\n\tflush_interval 1s\n\tbuffer_bytes 1MiB\n}\n",
        None,
    )
    .unwrap();
    let l = c.log.unwrap();
    assert_eq!(l.level_of("tls"), LogLevel::Debug);
    assert_eq!(l.output_of("engine"), "/var/log/fit/engine.log");
    assert_eq!(l.format_of("engine"), LogFormat::Line);
    assert_eq!(l.format_of("server"), LogFormat::Json);
    assert_eq!(l.rotate_of("engine"), None::<&Rotate>);
}
