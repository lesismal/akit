//! A node's system and hardware (fit's `model.HWInfo`), as its info carries it.

use akit_config::value::Value;

use crate::json::{self, Obj};

/// One GPU's basic snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuInfo {
    /// Its index.
    pub index: i64,
    /// `nvidia`, `apple`, ...
    pub vendor: String,
    /// Its name.
    pub name: String,
    /// Its memory.
    pub memory_mb: i64,
    /// Its memory used.
    pub memory_used_mb: i64,
    /// Percent.
    pub utilization: f64,
    /// Celsius.
    pub temperature: f64,
    /// Watts.
    pub power_draw: f64,
    /// Watts.
    pub power_limit: f64,
    /// Its driver's version.
    pub driver: String,
}

impl GpuInfo {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new()
            .int("index", self.index)
            .str("vendor", &self.vendor)
            .str("name", &self.name)
            .int("memory_mb", self.memory_mb)
            .int("memory_used_mb", self.memory_used_mb)
            .float("utilization", self.utilization)
            .float("temperature", self.temperature);
        if self.power_draw != 0.0 {
            o = o.float("power_draw", self.power_draw);
        }
        if self.power_limit != 0.0 {
            o = o.float("power_limit", self.power_limit);
        }
        o.str_omit("driver", &self.driver).end()
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> GpuInfo {
        GpuInfo {
            index: json::get_int(v, "index"),
            vendor: json::get_str(v, "vendor"),
            name: json::get_str(v, "name"),
            memory_mb: json::get_int(v, "memory_mb"),
            memory_used_mb: json::get_int(v, "memory_used_mb"),
            utilization: json::get_float(v, "utilization"),
            temperature: json::get_float(v, "temperature"),
            power_draw: json::get_float(v, "power_draw"),
            power_limit: json::get_float(v, "power_limit"),
            driver: json::get_str(v, "driver"),
        }
    }
}

/// One mounted file system.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskInfo {
    /// Where it is mounted.
    pub mount: String,
    /// Its type.
    pub fs_type: String,
    /// Its size.
    pub total_mb: i64,
    /// Its space free.
    pub free_mb: i64,
}

impl DiskInfo {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("mount", &self.mount)
            .str_omit("fs_type", &self.fs_type)
            .int("total_mb", self.total_mb)
            .int("free_mb", self.free_mb)
            .end()
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> DiskInfo {
        DiskInfo {
            mount: json::get_str(v, "mount"),
            fs_type: json::get_str(v, "fs_type"),
            total_mb: json::get_int(v, "total_mb"),
            free_mb: json::get_int(v, "free_mb"),
        }
    }
}

/// A node's system and hardware. A list left out (`None`) is written `null`, as Go writes a nil
/// slice.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HwInfo {
    /// The host's name.
    pub hostname: String,
    /// Go's `GOOS`: `linux`, `darwin`.
    pub os: String,
    /// The distribution or product version.
    pub platform: String,
    /// The kernel's release.
    pub kernel: String,
    /// Go's `GOARCH`: `amd64`, `arm64`.
    pub arch: String,
    /// The CPU's model.
    pub cpu_model: String,
    /// The CPUs.
    pub cpu_cores: i64,
    /// The load average over a minute.
    pub load1: f64,
    /// The memory.
    pub mem_total_mb: i64,
    /// The memory available.
    pub mem_available_mb: i64,
    /// The GPUs.
    pub gpus: Option<Vec<GpuInfo>>,
    /// The file systems.
    pub disks: Option<Vec<DiskInfo>>,
    /// The addresses.
    pub ips: Option<Vec<String>>,
    /// Since the host booted.
    pub uptime_seconds: i64,
    /// When it was read, in seconds since 1970.
    pub collected_at: i64,
}

fn list<T>(v: Option<&Vec<T>>, f: impl Fn(&T) -> String) -> String {
    v.map_or_else(|| "null".to_owned(), |v| json::array(v.iter().map(f)))
}

impl HwInfo {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("hostname", &self.hostname)
            .str("os", &self.os)
            .str_omit("platform", &self.platform)
            .str_omit("kernel", &self.kernel)
            .str("arch", &self.arch)
            .str("cpu_model", &self.cpu_model)
            .int("cpu_cores", self.cpu_cores)
            .float("load1", self.load1)
            .int("mem_total_mb", self.mem_total_mb)
            .int("mem_available_mb", self.mem_available_mb)
            .raw("gpus", &list(self.gpus.as_ref(), GpuInfo::to_json))
            .raw("disks", &list(self.disks.as_ref(), DiskInfo::to_json))
            .raw("ips", &list(self.ips.as_ref(), |s| json::string(s)))
            .int("uptime_seconds", self.uptime_seconds)
            .int("collected_at", self.collected_at)
            .end()
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> HwInfo {
        let arr = |k: &str| match v.get(k) {
            Some(Value::Array(a)) => Some(a.as_slice()),
            _ => None,
        };
        HwInfo {
            hostname: json::get_str(v, "hostname"),
            os: json::get_str(v, "os"),
            platform: json::get_str(v, "platform"),
            kernel: json::get_str(v, "kernel"),
            arch: json::get_str(v, "arch"),
            cpu_model: json::get_str(v, "cpu_model"),
            cpu_cores: json::get_int(v, "cpu_cores"),
            load1: json::get_float(v, "load1"),
            mem_total_mb: json::get_int(v, "mem_total_mb"),
            mem_available_mb: json::get_int(v, "mem_available_mb"),
            gpus: arr("gpus").map(|a| a.iter().map(GpuInfo::from_value).collect()),
            disks: arr("disks").map(|a| a.iter().map(DiskInfo::from_value).collect()),
            ips: arr("ips").map(|a| {
                a.iter()
                    .filter_map(|e| match e {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect()
            }),
            uptime_seconds: json::get_int(v, "uptime_seconds"),
            collected_at: json::get_int(v, "collected_at"),
        }
    }

    /// From JSON text, the zero value when it is none.
    #[must_use]
    pub fn parse(s: &str) -> HwInfo {
        json::parse(s.as_bytes())
            .map(|v| HwInfo::from_value(&v))
            .unwrap_or_default()
    }
}

// ---- Reading this machine's (fit's `llm/node/hwinfo*.go`) ----

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::actions::{goarch, goos};

/// Runs a short command, its standard output trimmed, `""` on any failure.
fn run_cmd(pool: &Arc<akio::Pool>, timeout: Duration, argv: &[&str]) -> String {
    crate::exec::output(pool, argv, timeout, false)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// A snapshot of this machine (fit's `CollectHW`): procfs and common tools (`nvidia-smi`,
/// `sysctl`, `system_profiler`, `df`); what cannot be read is left out. Blocks: on the node's
/// pool.
#[must_use]
pub fn collect(pool: &Arc<akio::Pool>) -> HwInfo {
    let mut h = HwInfo {
        hostname: akit_config::roles::host_name(),
        os: goos().to_owned(),
        arch: goarch().to_owned(),
        cpu_cores: std::thread::available_parallelism()
            .map_or(0, |n| i64::try_from(n.get()).unwrap_or(0)),
        collected_at: akit_roles::linktoken::unix_now(),
        ..HwInfo::default()
    };
    match goos() {
        "linux" => linux_platform(&mut h),
        "darwin" => darwin_platform(pool, &mut h),
        _ => {}
    }
    h.disks = Some(disks(pool));
    let mut gpus = nvidia_gpus(pool);
    if goos() == "darwin" {
        gpus.extend(darwin_gpus(pool, &h));
    }
    h.gpus = Some(gpus);
    h.ips = Some(local_ips());
    h
}

fn nvidia_gpus(pool: &Arc<akio::Pool>) -> Vec<GpuInfo> {
    let out = run_cmd(
        pool,
        Duration::from_secs(5),
        &[
            "nvidia-smi",
            "--query-gpu=index,name,memory.total,memory.used,utilization.gpu,temperature.gpu,driver_version,power.draw,power.limit",
            "--format=csv,noheader,nounits",
        ],
    );
    if out.is_empty() {
        return Vec::new();
    }
    parse_nvidia_smi(&out)
}

/// The CSV of the basic nvidia-smi query.
#[must_use]
pub fn parse_nvidia_smi(out: &str) -> Vec<GpuInfo> {
    let int = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
    let float = |s: &str| {
        s.trim()
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .unwrap_or(0.0)
    };
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 7 {
                return None;
            }
            let mut g = GpuInfo {
                index: int(f.first()?),
                vendor: "nvidia".to_owned(),
                name: (*f.get(1)?).to_owned(),
                memory_mb: int(f.get(2)?),
                memory_used_mb: int(f.get(3)?),
                utilization: float(f.get(4)?),
                temperature: float(f.get(5)?),
                driver: (*f.get(6)?).to_owned(),
                ..GpuInfo::default()
            };
            if f.len() >= 9 {
                g.power_draw = float(f.get(7)?);
                g.power_limit = float(f.get(8)?);
            }
            Some(g)
        })
        .collect()
}

/// The addresses of the interfaces up, but for loopback and link-local ones, sorted.
fn local_ips() -> Vec<String> {
    let mut ips: Vec<String> = akio_sys::interfaces()
        .unwrap_or_default()
        .iter()
        .filter(|i| i.flags.is_up() && !i.flags.is_loopback())
        .flat_map(|i| i.addrs.iter().map(|a| a.ip))
        .filter(|ip| match ip {
            std::net::IpAddr::V4(v4) => !v4.is_link_local(),
            std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
        })
        .map(|ip| ip.to_string())
        .collect();
    ips.sort();
    ips
}

fn linux_platform(h: &mut HwInfo) {
    h.kernel = read_trim("/proc/sys/kernel/osrelease");
    h.platform = read_trim("/etc/os-release")
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_owned())
        .unwrap_or_default();
    h.cpu_model = read_trim("/proc/cpuinfo")
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            matches!(k.trim(), "model name" | "Model" | "Hardware" | "cpu model")
                .then(|| v.trim().to_owned())
        })
        .unwrap_or_default();
    if let Some(f) = read_trim("/proc/loadavg").split_whitespace().next() {
        h.load1 = f.parse().unwrap_or(0.0);
    }
    if let Some(f) = read_trim("/proc/uptime").split_whitespace().next() {
        #[allow(clippy::cast_possible_truncation)]
        let up = f.parse::<f64>().unwrap_or(0.0) as i64;
        h.uptime_seconds = up;
    }
    let mem = read_trim("/proc/meminfo");
    let kb = |key: &str| {
        mem.lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                (k == key).then(|| v.split_whitespace().next()?.parse::<i64>().ok())?
            })
            .unwrap_or(0)
    };
    h.mem_total_mb = kb("MemTotal") / 1024;
    h.mem_available_mb = kb("MemAvailable") / 1024;
}

fn darwin_platform(pool: &Arc<akio::Pool>, h: &mut HwInfo) {
    let s = Duration::from_secs(2);
    h.cpu_model = run_cmd(pool, s, &["sysctl", "-n", "machdep.cpu.brand_string"]);
    h.kernel = run_cmd(pool, s, &["sysctl", "-n", "kern.osrelease"]);
    let v = run_cmd(pool, s, &["sw_vers", "-productVersion"]);
    if !v.is_empty() {
        h.platform = format!("macOS {v}");
    }
    let load = run_cmd(pool, s, &["sysctl", "-n", "vm.loadavg"]);
    if let Some(f) = load
        .trim_matches(|c| c == '{' || c == '}' || c == ' ')
        .split_whitespace()
        .next()
    {
        h.load1 = f.parse().unwrap_or(0.0);
    }
    let boot = run_cmd(pool, s, &["sysctl", "-n", "kern.boottime"]);
    if let Some(sec) = boot
        .split_once("sec = ")
        .and_then(|(_, r)| r.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<i64>().ok())
    {
        h.uptime_seconds = akit_roles::linktoken::unix_now().saturating_sub(sec);
    }
    let total = run_cmd(pool, s, &["sysctl", "-n", "hw.memsize"])
        .parse::<i64>()
        .unwrap_or(0);
    h.mem_total_mb = total >> 20;
    h.mem_available_mb = vm_stat_available(&run_cmd(pool, s, &["vm_stat"])) >> 20;
}

/// The memory available, from `vm_stat` (free, inactive, speculative and purgeable pages), in
/// bytes.
#[must_use]
pub fn vm_stat_available(out: &str) -> i64 {
    if out.is_empty() {
        return 0;
    }
    let page = out
        .split_once("page size of ")
        .and_then(|(_, r)| r.split_whitespace().next())
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(4096);
    let pages: i64 = out
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            matches!(
                k.trim(),
                "Pages free" | "Pages inactive" | "Pages speculative" | "Pages purgeable"
            )
            .then(|| v.trim().trim_end_matches('.').parse::<i64>().unwrap_or(0))
        })
        .sum();
    pages.saturating_mul(page)
}

/// The GPUs `system_profiler` lists, read once (it is slow); Apple silicon's share the memory,
/// reported as the machine's.
fn darwin_gpus(pool: &Arc<akio::Pool>, h: &HwInfo) -> Vec<GpuInfo> {
    static GPUS: OnceLock<Vec<GpuInfo>> = OnceLock::new();
    let gpus = GPUS.get_or_init(|| {
        let out = run_cmd(
            pool,
            Duration::from_secs(15),
            &["system_profiler", "SPDisplaysDataType", "-json"],
        );
        parse_displays(&out)
    });
    gpus.iter()
        .cloned()
        .map(|mut g| {
            if g.memory_mb == 0 && h.cpu_model.starts_with("Apple") {
                "apple".clone_into(&mut g.vendor);
                g.memory_mb = h.mem_total_mb;
            }
            g
        })
        .collect()
}

/// `system_profiler SPDisplaysDataType -json`'s GPUs.
#[must_use]
pub fn parse_displays(out: &str) -> Vec<GpuInfo> {
    let Ok(v) = json::parse(out.as_bytes()) else {
        return Vec::new();
    };
    json::get_arr(&v, "SPDisplaysDataType")
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let vendor = json::get_str(d, "spdisplays_vendor");
            let mut g = GpuInfo {
                index: i64::try_from(i).unwrap_or(0),
                name: json::get_str(d, "sppci_model"),
                vendor: vendor
                    .strip_prefix("sppci_vendor_")
                    .unwrap_or(&vendor)
                    .to_owned(),
                ..GpuInfo::default()
            };
            let cores = json::get_str(d, "sppci_cores");
            if !cores.is_empty() {
                g.name = format!("{} ({cores} cores)", g.name);
            }
            let vram = json::get_str(d, "spdisplays_vram");
            let mut f = vram.split_whitespace();
            if let Some(Ok(mut n)) = f.next().map(str::parse::<i64>) {
                if f.next().is_some_and(|u| u.eq_ignore_ascii_case("GB")) {
                    n <<= 10;
                }
                g.memory_mb = n;
            }
            g
        })
        .collect()
}

/// The file systems of the kinds a node reports, Linux's from `/proc/mounts`.
const LINUX_FS: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "vfat", "exfat",
    "ntfs", "ntfs3", "nfs", "nfs4", "cifs", "fuseblk",
];

/// The mounted file systems with their space, from `df -k -P` (a stopgap until akio's A249, a
/// file system's space from akio-sys): on Linux those of `/proc/mounts` of the kinds a node
/// reports, each device once; on macOS the local ones, each APFS container once.
fn disks(pool: &Arc<akio::Pool>) -> Vec<DiskInfo> {
    match goos() {
        "linux" => {
            let mut mounts: Vec<(String, String)> = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            for line in read_trim("/proc/mounts").lines() {
                let f: Vec<&str> = line.split_whitespace().collect();
                let (Some(dev), Some(mnt), Some(fs)) = (f.first(), f.get(1), f.get(2)) else {
                    continue;
                };
                if !LINUX_FS.contains(fs) || seen.iter().any(|d| d == dev) {
                    continue;
                }
                seen.push((*dev).to_owned());
                mounts.push((mnt.replace("\\040", " "), (*fs).to_owned()));
            }
            if mounts.is_empty() {
                return Vec::new();
            }
            let mut argv = vec!["df", "-k", "-P", "--"];
            argv.extend(mounts.iter().map(|(m, _)| m.as_str()));
            let out = crate::exec::output(pool, &argv, Duration::from_secs(5), false)
                .unwrap_or_else(|(o, _)| o);
            parse_df(&out)
                .into_iter()
                .filter_map(|(mount, total, free)| {
                    let fs = mounts.iter().find(|(m, _)| *m == mount)?.1.clone();
                    (total > 0).then_some(DiskInfo {
                        mount,
                        fs_type: fs,
                        total_mb: total >> 10,
                        free_mb: free >> 10,
                    })
                })
                .collect()
        }
        "darwin" => {
            let out = run_cmd(pool, Duration::from_secs(5), &["df", "-k", "-P", "-l"]);
            darwin_disks(&out)
        }
        _ => Vec::new(),
    }
}

/// `df -k -P`'s lines as mount, KiB in all and KiB available, `/dev/` devices only.
fn parse_df(out: &str) -> Vec<(String, i64, i64)> {
    out.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 6 {
                return None;
            }
            let mount = f.get(5..)?.join(" ");
            let total = f.get(1)?.parse::<i64>().ok()?;
            let free = f.get(3)?.parse::<i64>().ok()?;
            Some((mount, total, free))
        })
        .collect()
}

/// macOS's `df -k -P -l`: the `/dev/` volumes, the APFS system's helper volumes left out, the
/// volumes of one container (the same space) once.
#[must_use]
pub fn darwin_disks(out: &str) -> Vec<DiskInfo> {
    let mut seen: Vec<(i64, i64)> = Vec::new();
    let mut disks = Vec::new();
    for (i, line) in out.lines().enumerate() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if i == 0 || f.len() < 6 || !f.first().is_some_and(|d| d.starts_with("/dev/")) {
            continue;
        }
        let mount = f.get(5..).map(|m| m.join(" ")).unwrap_or_default();
        if mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data" {
            continue;
        }
        let total = f.get(1).and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
        let free = f.get(3).and_then(|n| n.parse::<i64>().ok()).unwrap_or(0);
        if seen.contains(&(total, free)) {
            continue;
        }
        seen.push((total, free));
        disks.push(DiskInfo {
            mount,
            fs_type: "apfs".to_owned(),
            total_mb: total >> 10,
            free_mb: free >> 10,
        });
    }
    disks
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    /// fit's `TestCollectHW`: this machine read, on macOS and Linux alike, without root.
    #[test]
    fn collect_this_machine() {
        crate::stand_ins::setup();
        let pool = crate::jobs::pool("test-hw", 2, 16);
        let h = collect(&pool);
        assert_eq!(h.os, goos());
        assert!(h.cpu_cores > 0);
        assert!(h.gpus.is_some() && h.disks.is_some() && h.ips.is_some());
        assert!(h.mem_total_mb > 0, "{h:?}");
        assert!(!h.disks.as_ref().unwrap().is_empty(), "{h:?}");
        let j = h.to_json();
        assert_eq!(HwInfo::parse(&j), h);
    }

    #[test]
    fn parsers() {
        crate::stand_ins::setup();
        let g = parse_nvidia_smi("0, NVIDIA A10, 24564, 100, 5, 40, 550.1, 20.5, 150.00\n1, x\n");
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].memory_mb, g[0].driver.as_str()), (24564, "550.1"));
        assert!((g[0].power_limit - 150.0).abs() < 1e-9);
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:  10.\nPages inactive: 5.\nPages active: 99.\n";
        assert_eq!(vm_stat_available(vm), 15 * 16384);
        let d = parse_displays(
            r#"{"SPDisplaysDataType":[{"sppci_model":"Apple M2","sppci_cores":"10","spdisplays_vendor":"sppci_vendor_Apple"},{"sppci_model":"Radeon","spdisplays_vram":"8 GB"}]}"#,
        );
        assert_eq!(d[0].name, "Apple M2 (10 cores)");
        assert_eq!(d[0].vendor, "Apple");
        assert_eq!(d[1].memory_mb, 8 << 10);
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s1s1 971350180 10000 500000000 3% /\n/dev/disk3s6 971350180 20000 500000000 3% /System/Volumes/VM\n/dev/disk3s5 971350180 30000 500000000 3% /System/Volumes/Data\nmap auto_home 0 0 0 100% /System/Volumes/Data/home\n";
        let ds = darwin_disks(df);
        assert_eq!(ds.len(), 1, "one container once: {ds:?}");
        assert_eq!(ds[0].mount, "/");
        assert_eq!(read_trim("/nonexistent/akit-llm-node"), "");
    }
}
