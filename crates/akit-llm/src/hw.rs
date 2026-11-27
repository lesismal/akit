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
