//! The memory akit may use, for a share of it (`limits.max_pending_bytes`, I79b): the machine's,
//! or the container's memory limit where that is lower, as fit's `internal/sysmem` reads it.
//! Linux only, from `/proc/meminfo` and the cgroup's limit files; elsewhere `None`, and a share
//! falls back as fit's does where it reads no memory. Read once, when the listeners' engine is
//! built (never on an I/O thread).

use std::sync::OnceLock;

/// The cgroup v2 and v1 memory limit files fit reads.
#[cfg(target_os = "linux")]
const CGROUP_LIMITS: [&str; 2] = [
    "/sys/fs/cgroup/memory.max",
    "/sys/fs/cgroup/memory/memory.limit_in_bytes",
];

/// The memory in bytes, `None` where it cannot be read.
pub(crate) fn total() -> Option<u64> {
    static TOTAL: OnceLock<Option<u64>> = OnceLock::new();
    *TOTAL.get_or_init(read)
}

#[cfg(target_os = "linux")]
fn read() -> Option<u64> {
    let ram = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| mem_total(&m));
    let limit = CGROUP_LIMITS
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .min();
    lower(ram, limit)
}

#[cfg(not(target_os = "linux"))]
fn read() -> Option<u64> {
    None
}

/// `MemTotal` of `/proc/meminfo`, in bytes (the kernel writes kB, which are KiB).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mem_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib = line
        .trim_start_matches("MemTotal:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024).filter(|n| *n > 0)
}

/// The machine's memory, or a container's limit where lower (a limit of "max", unset, is none).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn lower(ram: Option<u64>, limit: Option<u64>) -> Option<u64> {
    match (ram, limit) {
        (Some(r), Some(l)) => Some(r.min(l)),
        (r, l) => r.or(l),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_meminfo_and_takes_the_lower_limit() {
        let m = "MemTotal:       65536000 kB\nMemFree:        1000 kB\n";
        assert_eq!(mem_total(m), Some(65_536_000 * 1024));
        assert_eq!(mem_total("MemFree: 1 kB\n"), None);
        assert_eq!(mem_total("MemTotal: lots kB\n"), None);
        assert_eq!(lower(Some(8), Some(4)), Some(4));
        assert_eq!(lower(Some(4), Some(8)), Some(4));
        assert_eq!(lower(Some(4), None), Some(4));
        assert_eq!(lower(None, Some(4)), Some(4));
        assert_eq!(lower(None, None), None);
        #[cfg(target_os = "linux")]
        assert!(total().is_some_and(|n| n > 0));
        #[cfg(not(target_os = "linux"))]
        assert_eq!(total(), None);
    }
}
