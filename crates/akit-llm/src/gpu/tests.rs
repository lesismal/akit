//! fit's `llm/internal/gpu` tests (`gpu_test.go`, `fake_test.go`), ported: stand-ins for the
//! vendors' tools on a directory of their own, never a real tool.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;

use crate::stand_ins::{NVIDIA_OUT, setup, use_case};

fn q(id: &str) -> (Vec<Device>, String, Option<String>) {
    query(vendor(id).unwrap())
}

#[test]
fn parse_nvidia() {
    let out = "0, NVIDIA GeForce RTX 4090, GPU-abc, 550.54.15, 24564, 1024, 37, 45, 30.52, 450.00, 150.00, 600.00, 450.00, 210, 405, 3120, 10501, 30, P8, Enabled, Default\n\
               1, NVIDIA A100, GPU-def, 550.54.15, 81920, [N/A], 0, 30, [N/A], 400.00, 100.00, 400.00, 400.00, 1410, 1593, 1410, 1593, [N/A], P0, Disabled, Default\n";
    let devs = parse_nvidia_smi(out);
    assert_eq!(devs.len(), 2);
    let d = &devs[0];
    assert_eq!(d.name, "NVIDIA GeForce RTX 4090");
    assert!((d.power_limit - 450.0).abs() < 1e-9 && (d.power_max - 600.0).abs() < 1e-9);
    assert!((d.clock_gfx_max - 3120.0).abs() < 1e-9);
    assert_eq!(d.persistence, "Enabled");
    assert!((devs[1].mem_used_mb + 1.0).abs() < 1e-9 && (devs[1].fan_speed + 1.0).abs() < 1e-9);
    assert_eq!(devs[1].index, 1);
}

#[test]
fn parse_rocm() {
    let out = "WARNING: something\n{\"card1\": {\"Card Series\": \"Instinct MI210\", \"Temperature (Sensor edge) (C)\": \"36.0\", \"Average Graphics Package Power (W)\": \"41.0\",\n \"Max Graphics Package Power (W)\": \"300.0\", \"GPU use (%)\": \"12\", \"VRAM Total Memory (B)\": \"68702699520\", \"VRAM Total Used Memory (B)\": \"1073741824\",\n \"sclk clock speed:\": \"(800Mhz)\", \"mclk clock speed:\": \"(1600Mhz)\", \"Fan speed (%)\": \"20\", \"Performance Level\": \"auto\"},\n \"card0\": {\"Card Series\": \"Radeon RX 7900 XTX\"}, \"system\": {\"Driver version\": \"6.7.0\"}}";
    let devs = parse_rocm_smi(out).unwrap();
    assert_eq!(devs.len(), 2);
    let d = &devs[1];
    assert_eq!((d.index, d.name.as_str()), (1, "Instinct MI210"));
    assert!((d.power_limit - 300.0).abs() < 1e-9 && (d.mem_used_mb - 1024.0).abs() < 1e-9);
    assert!((d.clock_gfx - 800.0).abs() < 1e-9);
    assert_eq!(
        (d.perf_level.as_str(), d.driver.as_str()),
        ("auto", "6.7.0")
    );
}

#[test]
fn command_validation() {
    let cmd = command("nvidia", 1, "power_limit", "250").unwrap();
    assert_eq!(cmd.join(" "), "nvidia-smi -i 1 -pl 250");
    let cmd = command("nvidia", 0, "gpu_clock_lock", "210, 1800").unwrap();
    assert_eq!(cmd.join(" "), "nvidia-smi -i 0 -lgc 210,1800");
    for (key, value) in [
        ("power_limit", "250; rm -rf /"),
        ("power_limit", "$(id)"),
        ("power_limit", "1\n2"),
        ("power_limit", "-pl"),
        ("power_limit", "999999"),
        ("gpu_clock_lock", "1800,210"),
        ("gpu_clock_lock", "1,2;id"),
        ("compute_mode", "BOGUS"),
        ("compute_mode", "DEFAULT; id"),
        ("nope", "1"),
    ] {
        assert!(command("nvidia", 0, key, value).is_err(), "{key}={value:?}");
    }
    assert!(command("nvidia", -1, "power_limit", "100").is_err());
    assert!(command("nvidia", 1025, "power_limit", "100").is_err());
    assert!(command("nope", 0, "power_limit", "1").is_err());
    let cmd = command("amd", 2, "fan_auto", "ignored").unwrap();
    assert!(cmd.join(" ").contains("-d 2 --resetfans"));
    let cmd = command("nvidia", 0, "compute_mode", " DEFAULT ").unwrap();
    assert!(cmd.contains(&"DEFAULT".to_owned()));
    // The display, $DISPLAY or :0 (the variable is not set by the tests: it is read as it is).
    let cmd = command("nvidia", 3, "fan_speed", "60").unwrap();
    assert_eq!(cmd[0], "nvidia-settings");
    assert_eq!(
        cmd[3..],
        [
            "-a",
            "[gpu:3]/GPUFanControlState=1",
            "-a",
            "[fan:3]/GPUTargetFanSpeed=60"
        ]
    );
    assert_eq!(Options::default().wrap(vec!["x".into()]), ["x"]);
}

#[test]
fn wrap_sudo() {
    setup();
    let got = Options { use_sudo: true }.wrap(vec!["nvidia-smi".into(), "-pm".into(), "1".into()]);
    if is_root() {
        assert_eq!(got, ["nvidia-smi", "-pm", "1"]);
    } else {
        assert_eq!(
            got,
            [
                "sudo",
                "-n",
                "--preserve-env=DISPLAY,XAUTHORITY,AKIT_LLM_GPU_DATA_DIR",
                "nvidia-smi",
                "-pm",
                "1"
            ]
        );
    }
}

#[test]
fn catalog() {
    setup();
    let mut seen = Vec::new();
    let known = ["{index}", "{value}", "{min}", "{max}", "{display}"];
    for v in VENDORS {
        assert!(!seen.contains(&v.id) && !v.tool.is_empty() && !v.install.is_empty());
        seen.push(v.id);
        for s in v.settings {
            assert!(
                !s.command.is_empty()
                    && !s.kind.is_empty()
                    && !s.label.is_empty()
                    && !s.label_zh.is_empty()
            );
            // Templates are argv, never shell: no shell syntax, only known placeholders.
            assert!(
                !s.command
                    .contains(|c: char| "$`;|&<>()\\'\"*?~\n".contains(c)),
                "{}/{}: shell syntax",
                v.id,
                s.key
            );
            let mut rest = s.command;
            while let Some(i) = rest.find('{') {
                let tail = &rest[i..];
                let end = tail.find('}').unwrap();
                let p = &tail[..=end];
                if p[1..p.len() - 1].bytes().all(|b| b.is_ascii_lowercase()) {
                    assert!(known.contains(&p), "{}/{}: {p}", v.id, s.key);
                }
                rest = &tail[end + 1..];
            }
            for o in s.options {
                assert!(o.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            }
        }
        // The scripts name akit's variable, never fit's.
        for m in v.install {
            assert!(!m.script.contains("FIT_"), "{}", v.id);
        }
    }
    if let Ok((_, argv)) = install_command("nvidia") {
        assert_eq!((argv.len(), argv[0].as_str()), (3, "sh"));
    }
    assert!(install_command("nope").is_err());
    let cat = catalog_json();
    assert!(cat.starts_with(r#"[{"id":"nvidia","name":"NVIDIA","pci_vendors":["10de"],"tool":"nvidia-smi","extra_tools":["nvidia-settings"],"query_cmd":"#));
    assert!(cat.contains(r#""id":"apple","name":"Apple Silicon","pci_vendors":null"#));
    assert!(cat.contains(r#""settings":null,"description":"mthreads-gmi"#));
    // Collect does not fail on a machine without any GPU tool.
    let _f = use_case("none");
    let st = collect(Options::default());
    assert_eq!(st.vendors.len(), VENDORS.len());
    assert!(st.vendors.iter().all(|v| !v.installed));
}

#[test]
fn query_fake_tools() {
    setup();
    {
        let _f = use_case("nvidia");
        let (devs, raw, err) = q("nvidia");
        assert!(err.is_none() && raw.is_empty() && devs.len() == 1 && devs[0].name == "NVIDIA A10");
    }
    {
        let _f = use_case("nvidia-fail");
        let (devs, raw, err) = q("nvidia");
        assert!(
            err.is_some() && devs.is_empty() && raw == "NVIDIA-SMI has failed",
            "{raw:?} {err:?}"
        );
    }
    {
        let _f = use_case("rocm");
        let (devs, _, err) = q("amd");
        assert!(err.is_none() && devs.len() == 1 && devs[0].driver == "6.7.0");
    }
    {
        // rocm-smi exits non-zero when one of its many values is missing, yet prints them.
        let _f = use_case("rocm-exit2");
        let (devs, _, err) = q("amd");
        assert!(err.is_none() && devs.len() == 1);
    }
    {
        let _f = use_case("rocm-fail");
        let (_, raw, err) = q("amd");
        assert!(err.is_some() && raw == "no driver");
    }
    {
        let _f = use_case("rocm-bad");
        let (_, raw, err) = q("amd");
        assert!(err.is_some() && raw == "{not json");
    }
    {
        // Device 1's stats fail: it is listed with unknown values.
        let _f = use_case("xpu");
        let (devs, raw, err) = q("intel");
        assert!(
            err.is_none() && raw.is_empty() && devs.len() == 2,
            "{err:?}"
        );
        assert!(
            (devs[0].power_draw - 35.0).abs() < 1e-9
                && devs[0].name == "Arc A770"
                && devs[0].uuid == "u0"
        );
        assert!((devs[1].power_draw + 1.0).abs() < 1e-9);
    }
    {
        let _f = use_case("xpu-fail");
        let (_, raw, err) = q("intel");
        assert!(err.is_some() && raw == "no device");
    }
    {
        let _f = use_case("xpu-bad");
        let (_, raw, err) = q("intel");
        assert!(err.unwrap().contains("discovery") && raw == "[");
    }
    {
        let _f = use_case("apple");
        let (devs, _, err) = q("apple");
        assert!(err.is_none() && devs.len() == 1 && devs[0].name == "Apple M2 (10 cores)");
    }
    {
        let _f = use_case("apple-fail");
        let (_, raw, err) = q("apple");
        assert!(err.is_some() && raw == "denied");
    }
    {
        let _f = use_case("raw");
        for (id, want) in [
            ("hygon", "hygon table"),
            ("ascend", "ascend table"),
            ("mthreads", "mthreads table"),
        ] {
            let (devs, raw, err) = q(id);
            assert!(
                devs.is_empty() && raw == want && err.is_some() == (id == "mthreads"),
                "{id}"
            );
        }
    }
}

#[test]
fn collect_fake_tools() {
    setup();
    let _f = use_case("collect");
    let st = collect(Options::default());
    let by = |id: &str| st.vendors.iter().find(|v| v.vendor.id == id).unwrap();
    let nv = by("nvidia");
    assert!(nv.installed && !nv.tool_path.is_empty());
    assert_eq!(nv.extra_found, [("nvidia-settings".to_owned(), true)]);
    let amd = by("amd");
    assert!(amd.installed && amd.extra_found == [("amd-smi".to_owned(), false)]);
    assert!(!by("intel").installed);
    let get = |m: &[(String, String)], k: &str| {
        m.iter()
            .find(|(mk, _)| mk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    assert!(!get(&st.errors, "amd").is_empty());
    assert_eq!(get(&st.raw, "amd"), "rocm down");
    assert_eq!(get(&st.raw, "hygon"), "hygon table");
    let vendors: Vec<&str> = st.devices.iter().map(|d| d.vendor.as_str()).collect();
    assert_eq!(vendors, ["nvidia", "apple"]);
    let j = st.to_json();
    assert!(
        j.contains(r#""extra_found":{"nvidia-settings":true}"#),
        "{j}"
    );
    assert!(
        j.contains(r#""raw":{"amd":"rocm down","hygon":"hygon table"}"#),
        "{j}"
    );
}

#[test]
fn device_indices_expand() {
    let nv = vendor("nvidia").unwrap();
    let intel = vendor("intel").unwrap();
    let vs = |v: &'static Vendor, n: i64| VendorStatus {
        vendor: v,
        detected: n,
        tool_path: String::new(),
        installed: false,
        extra_found: Vec::new(),
        install_for: String::new(),
    };
    let st = Status {
        os: String::new(),
        root: false,
        use_sudo: false,
        devices: vec![
            Device::unknown("nvidia", 2),
            Device::unknown("amd", 0),
            Device::unknown("nvidia", 5),
        ],
        vendors: vec![vs(intel, 3), vs(nv, 8)],
        raw: Vec::new(),
        errors: Vec::new(),
        collected_at: 0,
    };
    assert_eq!(device_indices(&st, "nvidia"), [2, 5]);
    // No device read: every GPU found on the bus.
    assert_eq!(device_indices(&st, "intel"), [0, 1, 2]);
    assert!(device_indices(&st, "ascend").is_empty());
}

#[test]
fn install_methods() {
    let v = vendor("ascend").unwrap(); // only "any"
    for (family, want) in [
        ("debian", true),
        ("rhel", true),
        ("linux", true),
        ("darwin", false),
        ("windows", false),
    ] {
        assert_eq!(install_for(v, family).is_some(), want, "{family}");
    }
    let nv = vendor("nvidia").unwrap();
    assert_eq!(
        install_for(nv, "debian").unwrap().title,
        "Ubuntu / Debian: driver + nvidia-smi"
    );
    assert!(install_for(nv, "linux").is_none());
    // A vendor with no method for this system: apple but on darwin, nvidia on darwin.
    let id = if os_family() == "darwin" {
        "nvidia"
    } else {
        "apple"
    };
    assert!(
        install_command(id)
            .unwrap_err()
            .contains("no install method")
    );
    assert_eq!(family_of("ID=ubuntu\nID_LIKE=debian\n"), "debian");
    assert_eq!(
        family_of("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"),
        "rhel"
    );
    assert_eq!(family_of("ID=alpine\n"), "linux");
}

#[test]
fn parsers_bad_input() {
    assert!(
        parse_nvidia_smi("0, \"a\n").is_empty(),
        "an unterminated quote"
    );
    let bad_index = NVIDIA_OUT.replacen("0,", "x,", 1);
    assert!(parse_nvidia_smi(&format!("0, short\n{bad_index}")).is_empty());
    assert!(parse_rocm_smi("garbage").is_err());
    let devs = parse_rocm_smi(
        r#"{"card2": {"Device Name": "Navi", "sclk clock speed:": "n/a"}, "cardx": {}}"#,
    )
    .unwrap();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].name, "Navi");
    assert!((devs[0].clock_gfx + 1.0).abs() < 1e-9 && devs[0].driver.is_empty());
    assert!(parse_apple_displays("nope").is_empty());
    let devs = parse_apple_displays(r#"{"SPDisplaysDataType": [{"_name": "kHW_AppleM1Item"}]}"#);
    assert_eq!(devs[0].name, "kHW_AppleM1Item");
}

#[test]
fn xpu_stats() {
    let mut d = Device::unknown("intel", 0);
    apply_xpu_stats(&mut d, "not json");
    assert!((d.power_draw + 1.0).abs() < 1e-9);
    apply_xpu_stats(
        &mut d,
        r#"{"device_level": [
        {"metrics_type": "XPUM_STATS_POWER", "value": 1}, {"metrics_type": "XPUM_STATS_GPU_FREQUENCY", "value": 2},
        {"metrics_type": "XPUM_STATS_GPU_CORE_TEMPERATURE", "value": 3}, {"metrics_type": "XPUM_STATS_MEMORY_USED", "value": 4},
        {"metrics_type": "XPUM_STATS_GPU_UTILIZATION", "value": 5}, {"metrics_type": "XPUM_STATS_MEMORY_FREQUENCY", "value": 6},
        {"metrics_type": "XPUM_STATS_OTHER", "value": 7}]}"#,
    );
    let got = [
        d.power_draw,
        d.clock_gfx,
        d.temperature,
        d.mem_used_mb,
        d.utilization,
        d.clock_mem,
    ];
    assert_eq!(got, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
}
