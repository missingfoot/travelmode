//! Human-readable byte and speed formatting. Pure functions, unit-tested.

/// Format a byte count: B below 1 KiB, then KiB/MiB/GiB/TiB with one
/// decimal. Examples: `512 B`, `2.0 KiB`, `3.5 MiB`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Format a per-second rate: `0 B/s`, `1.5 MiB/s`.
pub fn human_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", human_bytes(bytes_per_sec))
}

/// Format seconds of uptime as `2d 4h`, `3h 12m`, `45m 1s`, `12s`.
pub fn human_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 + 512 * 1024), "3.5 MiB");
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn formats_speeds() {
        assert_eq!(human_speed(0), "0 B/s");
        assert_eq!(human_speed(1536 * 1024), "1.5 MiB/s");
    }

    #[test]
    fn formats_uptime() {
        assert_eq!(human_uptime(12), "12s");
        assert_eq!(human_uptime(60), "1m 0s");
        assert_eq!(human_uptime(45 * 60 + 1), "45m 1s");
        assert_eq!(human_uptime(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(human_uptime(2 * 86400 + 4 * 3600), "2d 4h");
    }
}
