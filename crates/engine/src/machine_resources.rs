//! A snapshot of the machine the engine runs on: CPU, memory and disk. For a
//! cloud computer this is the guest itself, shown in the environment summary.
use serde_json::{Value, json};

/// Measure CPU over a short window and read memory and disk now. Fields this
/// platform can't report are `null`.
pub async fn snapshot(path: &std::path::Path) -> Value {
    let cpu = cpu_percent().await;
    let (memory_used, memory_total) = memory().unzip();
    let (disk_used, disk_total) = disk(path).unzip();
    json!({
        "cpuPercent": cpu,
        "cores": std::thread::available_parallelism().map(|n| n.get()).ok(),
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
async fn cpu_percent() -> Option<f64> {
    let (idle_a, total_a) = cpu_times()?;
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
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
            .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kib| kib * 1024)
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    Some((total.saturating_sub(available), total))
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
