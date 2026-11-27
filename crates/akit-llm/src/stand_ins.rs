//! The stand-ins for the vendors' tools this crate's unit tests run, never a real tool: every
//! one written once, before any test of the binary may start a child process ([`setup`] is the
//! first thing each such test calls), so that no child forked while a stand-in is open for
//! writing holds it (ETXTBSY, which fit's tests note). Each case is a directory of its own,
//! which a test makes its thread's lookups use ([`use_case`]).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// nvidia-smi's 21 fields of one GPU.
pub(crate) const NVIDIA_OUT: &str = "0, NVIDIA A10, GPU-a, 550.1, 24564, 100, 5, 40, 20.5, 150.00, 100.00, 150.00, 150.00, 210, 405, 1695, 6251, 30, P8, Enabled, Default\n";
pub(crate) const ROCM_OUT: &str =
    r#"{"card0": {"Card Series": "MI210"}, "system": {"Driver version": "6.7.0"}}"#;
pub(crate) const APPLE_OUT: &str =
    r#"{"SPDisplaysDataType": [{"sppci_model": "Apple M2", "sppci_cores": "10"}]}"#;
const XPU_DISC: &str = r#"{"device_list": [{"device_id": 0, "device_name": "Arc A770", "uuid": "u0"}, {"device_id": 1, "device_name": "Arc A380", "uuid": "u1"}]}"#;
const XPU_STATS: &str = r#"{"device_level": [{"metrics_type": "XPUM_STATS_POWER", "value": 35}]}"#;

/// A script body printing `out` as it is (the shell's own `printf`) and exiting with `code`.
fn printing(out: &str, code: i32) -> String {
    format!("printf '%s' '{}'\nexit {code}", out.replace('\'', r"'\''"))
}

/// Every case: its name and its tools (name, script body).
fn cases() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let xpu = format!(
        "case \"$1 $3\" in\n\"discovery \") {} ;;\n\"stats 0\") {} ;;\n*) exit 1 ;;\nesac",
        printing(XPU_DISC, 0),
        printing(XPU_STATS, 0)
    );
    vec![
        ("none", vec![]),
        ("nvidia", vec![("nvidia-smi", printing(NVIDIA_OUT, 0))]),
        (
            "nvidia-fail",
            vec![("nvidia-smi", printing("NVIDIA-SMI has failed", 9))],
        ),
        ("rocm", vec![("rocm-smi", printing(ROCM_OUT, 0))]),
        ("rocm-exit2", vec![("rocm-smi", printing(ROCM_OUT, 2))]),
        ("rocm-fail", vec![("rocm-smi", printing("no driver", 2))]),
        ("rocm-bad", vec![("rocm-smi", printing("{not json", 0))]),
        ("xpu", vec![("xpu-smi", xpu)]),
        ("xpu-fail", vec![("xpu-smi", printing("no device", 1))]),
        ("xpu-bad", vec![("xpu-smi", printing("[", 0))]),
        ("apple", vec![("system_profiler", printing(APPLE_OUT, 0))]),
        (
            "apple-fail",
            vec![("system_profiler", printing("denied", 1))],
        ),
        (
            "raw",
            vec![
                ("hy-smi", printing("hygon table", 0)),
                ("npu-smi", printing("ascend table", 0)),
                ("mthreads-gmi", printing("mthreads table", 3)),
            ],
        ),
        (
            "collect",
            vec![
                ("nvidia-smi", printing(NVIDIA_OUT, 0)),
                ("nvidia-settings", "exit 0".to_owned()),
                ("rocm-smi", printing("rocm down", 1)),
                ("system_profiler", printing(APPLE_OUT, 0)),
                ("hy-smi", printing("hygon table", 0)),
            ],
        ),
    ]
}

/// Writes every stand-in, once, under the build's directory (`target/<profile>/tmp`), the
/// files closed before it returns; returns where.
pub(crate) fn setup() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        let exe = std::env::current_exe().unwrap();
        let base = exe
            .parent()
            .and_then(Path::parent)
            .map_or_else(std::env::temp_dir, Path::to_path_buf)
            .join("tmp")
            .join(format!("akit-llm-stand-ins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for (case, tools) in cases() {
            let dir = base.join(case);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, body) in tools {
                let p = dir.join(name);
                std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        base
    })
}

/// While held, this thread's tool lookups use case `name`'s stand-ins alone.
pub(crate) struct Case;

impl Drop for Case {
    fn drop(&mut self) {
        crate::exec::tools_on_thread(None);
    }
}

/// Points this thread's tool lookups at case `name`.
pub(crate) fn use_case(name: &str) -> Case {
    let dir = setup().join(name);
    assert!(dir.is_dir(), "no stand-ins {name}");
    crate::exec::tools_on_thread(Some(dir));
    Case
}
