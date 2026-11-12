//! A snapshot of the machine the engine runs on: CPU, memory and disk. For a
//! cloud computer this is the guest itself, shown in the environment summary.
use serde_json::{Value, json};

/// Measure CPU over a short window and read memory and disk now. Fields this
/// platform can't report are `null`. A computer that is a container (rather
/// than a VM) sees the host in `/proc`; its cgroup limits describe it instead.
pub async fn snapshot(path: &std::path::Path) -> Value {
    let cpu = cpu_percent().await;
    let (memory_used, memory_total) = memory().unzip();
    let (disk_used, disk_total) = disk(path).unzip();
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .ok()
        .map(|n| cgroup_cpus().map_or(n, |limit| n.min(limit.ceil() as usize).max(1)));
    json!({
        "cpuPercent": cpu,
        "cores": cores,
        "memoryUsedBytes": memory_used,
        "memoryTotalBytes": memory_total,
        "diskUsedBytes": disk_used,
        "diskTotalBytes": disk_total,
        "sampledAt": chrono::Utc::now().timestamp_millis(),
    })
}

#[cfg(target_os = "linux")]
fn cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let values: Vec<u64> = line
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    // user nice system idle iowait irq softirq steal
    let idle = values.get(3)? + values.get(4).copied().unwrap_or(0);
    let total = values.iter().take(8).sum();
    Some((idle, total))
}

#[cfg(target_os = "linux")]
const CGROUP: &str = "/sys/fs/cgroup";

/// CPUs this cgroup may use, when `cpu.max` sets a quota.
#[cfg(target_os = "linux")]
fn cgroup_cpus() -> Option<f64> {
    parse_cpu_max(&std::fs::read_to_string(format!("{CGROUP}/cpu.max")).ok()?)
}
#[cfg(not(target_os = "linux"))]
fn cgroup_cpus() -> Option<f64> {
    None
}
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_cpu_max(text: &str) -> Option<f64> {
    let mut parts = text.split_whitespace();
    let quota: f64 = parts.next()?.parse().ok()?; // "max" means no quota
    let period: f64 = parts.next()?.parse().ok()?;
    (quota > 0.0 && period > 0.0).then(|| quota / period)
}

#[cfg(target_os = "linux")]
fn cgroup_usage_usec() -> Option<u64> {
    std::fs::read_to_string(format!("{CGROUP}/cpu.stat"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("usage_usec "))?
        .trim()
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
async fn cpu_percent() -> Option<f64> {
    const WINDOW: std::time::Duration = std::time::Duration::from_millis(250);
    if let (Some(limit), Some(before)) = (cgroup_cpus(), cgroup_usage_usec()) {
        let started = std::time::Instant::now();
        tokio::time::sleep(WINDOW).await;
        let used = cgroup_usage_usec()?.checked_sub(before)? as f64;
        let available = started.elapsed().as_micros() as f64 * limit;
        return (available > 0.0)
            .then(|| (used / available * 1000.0).round().clamp(0.0, 1000.0) / 10.0);
    }
    let (idle_a, total_a) = cpu_times()?;
    tokio::time::sleep(WINDOW).await;
    let (idle_b, total_b) = cpu_times()?;
    let total = total_b.checked_sub(total_a)?;
    let idle = idle_b.checked_sub(idle_a)?;
    (total > 0).then(|| ((total - idle) as f64 / total as f64 * 1000.0).round() / 10.0)
}

#[cfg(not(target_os = "linux"))]
async fn cpu_percent() -> Option<f64> {
    None
}

#[cfg(target_os = "linux")]
fn memory() -> Option<(u64, u64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        info.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map(|kib| kib * 1024)
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    let read = |name: &str| std::fs::read_to_string(format!("{CGROUP}/{name}")).ok();
    if let Some(limited) = cgroup_memory(
        total,
        read("memory.max").as_deref(),
        read("memory.current").as_deref(),
        read("memory.stat").as_deref(),
    ) {
        return Some(limited);
    }
    Some((total.saturating_sub(available), total))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// Used and total bytes from a cgroup memory limit below the machine's RAM.
/// Reclaimable page cache doesn't count as used, matching `MemAvailable`.
fn cgroup_memory(
    machine_total: u64,
    max: Option<&str>,
    current: Option<&str>,
    stat: Option<&str>,
) -> Option<(u64, u64)> {
    let limit: u64 = max?.trim().parse().ok()?; // "max" means unlimited
    if limit == 0 || limit >= machine_total {
        return None;
    }
    let current: u64 = current?.trim().parse().ok()?;
    let inactive_file = stat
        .and_then(|stat| {
            stat.lines()
                .find_map(|line| line.strip_prefix("inactive_file "))
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
        .unwrap_or(0);
    Some((current.saturating_sub(inactive_file).min(limit), limit))
}

#[cfg(not(target_os = "linux"))]
fn memory() -> Option<(u64, u64)> {
    None
}

#[cfg(unix)]
fn disk(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block = stats.f_frsize as u64;
    let total = stats.f_blocks as u64 * block;
    let free = stats.f_bavail as u64 * block;
    Some((total.saturating_sub(free), total))
}

#[cfg(not(unix))]
fn disk(_: &std::path::Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn container_limits_describe_the_computer_instead_of_the_host() {
        // Measured on a Daytona container: 2 GiB and one CPU on a 48-CPU,
        // ~395 GB host.
        let host = 395_596_028 * 1024;
        assert_eq!(
            super::cgroup_memory(
                host,
                Some("2147483648\n"),
                Some("44597248\n"),
                Some("anon 39829504\nfile 28672\ninactive_file 28672\n"),
            ),
            Some((44_597_248 - 28_672, 2_147_483_648))
        );
        assert_eq!(super::parse_cpu_max("100000 100000\n"), Some(1.0));
        assert_eq!(super::parse_cpu_max("150000 100000"), Some(1.5));
        // A VM, or a container without a limit, keeps the machine's figures.
        assert_eq!(super::parse_cpu_max("max 100000"), None);
        assert_eq!(
            super::cgroup_memory(host, Some("max"), Some("1"), None),
            None
        );
        assert_eq!(
            super::cgroup_memory(2 << 30, Some("4294967296"), Some("1"), None),
            None
        );
        assert_eq!(super::cgroup_memory(host, None, None, None), None);
    }

    #[tokio::test]
    async fn snapshot_reports_this_machine() {
        let value = super::snapshot(std::path::Path::new("/")).await;
        assert!(value["sampledAt"].is_i64());
        #[cfg(target_os = "linux")]
        {
            assert!(value["memoryTotalBytes"].as_u64().unwrap() > 0);
            assert!(value["diskTotalBytes"].as_u64().unwrap() > 0);
            let cpu = value["cpuPercent"].as_f64().unwrap();
            assert!((0.0..=100.0).contains(&cpu));
        }
    }
}
