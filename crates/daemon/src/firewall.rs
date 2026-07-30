//! Firewall backend: our own nftables table plus an NFQUEUE worker that
//! decides per-packet verdicts from the rule store.
//!
//! We only ever touch `table inet travelmode` — never any other table.
//! Fail-open everywhere: any error in the decision path means ACCEPT,
//! and if NFQUEUE setup fails the daemon keeps running with
//! `filtering_active = false`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use nfq::{Queue, Verdict};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, error, info, warn};
use travelmode_core::types::Protocol;

use crate::state::DaemonState;

const TABLE: &str = "travelmode";
const FAMILY: &str = "inet";
const CHAIN: &str = "output";

/// nftables state handle (queue rule management). Kept in the state so
/// pause/resume can toggle the queue rule.
pub struct Firewall {
    queue_num: u16,
}

impl Firewall {
    /// Delete any stale table, then create our table fresh with the
    /// queue rule (unless paused).
    pub async fn setup(queue_num: u16, paused: bool) -> std::io::Result<Self> {
        // Ignore the result: the table may not exist yet.
        let _ = nft(&["delete", "table", FAMILY, TABLE]).await;
        let script = table_script(!paused, queue_num);
        nft_stdin(&script).await?;
        info!(queue = queue_num, "nftables table {FAMILY} {TABLE} installed");
        Ok(Self { queue_num })
    }

    /// Pause: remove the queue rule (traffic flows freely).
    pub async fn pause(&self) -> std::io::Result<()> {
        nft(&["flush", "chain", FAMILY, TABLE, CHAIN]).await
    }

    /// Resume: re-add the queue rule.
    pub async fn resume(&self) -> std::io::Result<()> {
        nft(&[
            "add",
            "rule",
            FAMILY,
            TABLE,
            CHAIN,
            "ct",
            "state",
            "new",
            "queue",
            "num",
            &self.queue_num.to_string(),
            "bypass",
        ])
        .await
    }

    /// Remove our table entirely.
    pub async fn teardown(self) {
        if let Err(e) = nft(&["delete", "table", FAMILY, TABLE]).await {
            warn!(error = %e, "failed to remove nftables table on shutdown");
        } else {
            info!("nftables table {FAMILY} {TABLE} removed");
        }
    }
}

fn table_script(with_queue_rule: bool, queue_num: u16) -> String {
    let rule = if with_queue_rule {
        format!("\n        ct state new queue num {queue_num} bypass")
    } else {
        String::new()
    };
    format!(
        "table {FAMILY} {TABLE} {{\n    chain {CHAIN} {{\n        type filter hook output priority 0; policy accept;{rule}\n    }}\n}}\n"
    )
}

async fn nft(args: &[&str]) -> std::io::Result<()> {
    let out = Command::new("nft").args(args).output().await?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "nft {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

async fn nft_stdin(script: &str) -> std::io::Result<()> {
    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await?;
        drop(stdin);
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "nft -f - failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

// ------------------------------------------------------------- NFQUEUE

/// Spawn the blocking NFQUEUE worker. On setup failure (e.g. not root)
/// the daemon keeps running with filtering_active=false.
pub fn spawn_queue_worker(state: Arc<DaemonState>) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = queue_loop(&state) {
            error!(error = %e, "NFQUEUE worker stopped; filtering disabled");
            state.filtering_active.store(false, Ordering::Relaxed);
        }
    });
}

fn queue_loop(state: &Arc<DaemonState>) -> std::io::Result<()> {
    let mut queue = Queue::open()?;
    queue.bind(state.config.queue_num).map_err(|e| {
        if e.raw_os_error() == Some(libc::EPERM) || e.raw_os_error() == Some(libc::EBUSY) {
            std::io::Error::new(
                e.kind(),
                format!(
                    "{e} — cannot bind NFQUEUE {}; is another travelmoded still running?",
                    state.config.queue_num
                ),
            )
        } else {
            e
        }
    })?;
    // Ask the kernel to keep passing traffic if we fall behind.
    let _ = queue.set_fail_open(state.config.queue_num, state.config.fail_open);
    state.filtering_active.store(true, Ordering::Relaxed);
    info!(queue = state.config.queue_num, "NFQUEUE worker bound");

    loop {
        let mut msg = queue.recv()?;
        let verdict = decide(state, msg.get_payload());
        msg.set_verdict(verdict);
        queue.verdict(msg)?;
    }
}

/// Decide a verdict for one packet. Fail-open: ACCEPT on any doubt.
fn decide(state: &Arc<DaemonState>, payload: &[u8]) -> Verdict {
    if state.is_paused() {
        return Verdict::Accept;
    }
    let Some((proto, local_port, remote_addr, remote_port)) = parse_packet(payload) else {
        debug!("unparseable or non-TCP/UDP packet; accept");
        return Verdict::Accept; // not TCP/UDP or unparseable: let it through
    };
    let dst = format!("{remote_addr}:{remote_port}");
    let (pid, name, exe) = state.attribute(proto, local_port);
    let (Some(pid), Some(exe)) = (pid, exe) else {
        debug!(?proto, local_port, dst, pid = ?pid, "attribution failed; accept");
        return Verdict::Accept;
    };
    if state.rules.is_blocked(&exe) {
        state.blocked_count.fetch_add(1, Ordering::Relaxed);
        let app = name.as_deref().unwrap_or("?");
        // Full detail on every blocked packet at debug level.
        debug!(app, pid, exe = %exe.display(), dst, "blocked connection");
        // Rate-limited info logging: first block per (exe, dst) logs,
        // repeats within the window are counted, then summarized.
        match state
            .block_log
            .lock()
            .unwrap()
            .should_log((exe.clone(), dst.clone()), std::time::Instant::now())
        {
            LogAction::First => {
                info!(app, pid, exe = %exe.display(), dst, "blocked connection");
            }
            LogAction::Suppress => {}
            LogAction::Summary(n) => {
                info!(app, exe = %exe.display(), dst, "blocked {n} connections (5s window)");
            }
        }
        Verdict::Drop
    } else {
        debug!(
            app = name.as_deref().unwrap_or("?"),
            pid,
            exe = %exe.display(),
            ?proto,
            local_port,
            dst,
            "no block rule; accept"
        );
        Verdict::Accept
    }
}

// ------------------------------------------------------- log throttle

/// What the rate limiter decided for one blocked packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogAction {
    /// First block for this (exe, dst) key: log at info.
    First,
    /// Repeat within the window: counted, no log.
    Suppress,
    /// Window expired: log a summary covering this many blocks since
    /// the last log line (including the current one).
    Summary(u64),
}

/// Key blocks are deduplicated by: executable + destination.
type ThrottleKey = (std::path::PathBuf, String);

struct ThrottleEntry {
    window_start: std::time::Instant,
    suppressed: u64,
}

/// Rate limiter for blocked-connection logging. A blocked browser
/// retries 10-20x/sec; without this every dropped packet is one info
/// line. Pure decision logic (clock injected) so it is unit-testable.
pub struct LogThrottle {
    window: std::time::Duration,
    entries: std::collections::HashMap<ThrottleKey, ThrottleEntry>,
}

impl Default for LogThrottle {
    fn default() -> Self {
        Self::new(std::time::Duration::from_secs(5))
    }
}

impl LogThrottle {
    pub fn new(window: std::time::Duration) -> Self {
        Self {
            window,
            entries: std::collections::HashMap::new(),
        }
    }

    pub fn should_log(&mut self, key: ThrottleKey, now: std::time::Instant) -> LogAction {
        match self.entries.get_mut(&key) {
            None => {
                self.entries.insert(
                    key,
                    ThrottleEntry {
                        window_start: now,
                        suppressed: 0,
                    },
                );
                LogAction::First
            }
            Some(entry) if now.duration_since(entry.window_start) < self.window => {
                entry.suppressed += 1;
                LogAction::Suppress
            }
            Some(entry) => {
                // Window expired: summarize everything since the last
                // log line (suppressed repeats + this block), restart.
                let n = entry.suppressed + 1;
                *entry = ThrottleEntry {
                    window_start: now,
                    suppressed: 0,
                };
                LogAction::Summary(n)
            }
        }
    }
}

// --------------------------------------------------------- packet parser

/// Parse an IPv4/IPv6 packet, returning (protocol, local port, remote
/// addr, remote port) for TCP/UDP. Returns None otherwise.
fn parse_packet(
    payload: &[u8],
) -> Option<(Protocol, u16, std::net::IpAddr, u16)> {
    let version = payload.first()? >> 4;
    let (proto_byte, l4_offset, src, dst) = match version {
        4 => {
            let ihl = (payload.first()? & 0x0f) as usize * 4;
            if payload.len() < ihl + 4 || ihl < 20 {
                return None;
            }
            let proto = payload[9];
            let src = std::net::Ipv4Addr::new(
                payload[12], payload[13], payload[14], payload[15],
            );
            let dst = std::net::Ipv4Addr::new(
                payload[16], payload[17], payload[18], payload[19],
            );
            (proto, ihl, std::net::IpAddr::V4(src), std::net::IpAddr::V4(dst))
        }
        6 => {
            if payload.len() < 40 + 4 {
                return None;
            }
            let proto = payload[6];
            let mut src = [0u8; 16];
            src.copy_from_slice(&payload[8..24]);
            let mut dst = [0u8; 16];
            dst.copy_from_slice(&payload[24..40]);
            (
                proto,
                40,
                std::net::IpAddr::V6(std::net::Ipv6Addr::from(src)),
                std::net::IpAddr::V6(std::net::Ipv6Addr::from(dst)),
            )
        }
        _ => return None,
    };
    let proto = match proto_byte {
        6 => Protocol::Tcp,
        17 => Protocol::Udp,
        _ => return None,
    };
    if payload.len() < l4_offset + 4 {
        return None;
    }
    // Outbound hook: source port is the local port.
    let local_port = u16::from_be_bytes([payload[l4_offset], payload[l4_offset + 1]]);
    let remote_port = u16::from_be_bytes([payload[l4_offset + 2], payload[l4_offset + 3]]);
    let _ = src;
    Some((proto, local_port, dst, remote_port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn ipv4_tcp_packet(sport: u16, dport: u16) -> Vec<u8> {
        let mut p = vec![0u8; 40];
        p[0] = 0x45; // version 4, IHL 5
        p[9] = 6; // TCP
        p[12..16].copy_from_slice(&[192, 168, 1, 10]);
        p[16..20].copy_from_slice(&[93, 184, 216, 34]);
        p[20..22].copy_from_slice(&sport.to_be_bytes());
        p[22..24].copy_from_slice(&dport.to_be_bytes());
        p
    }

    #[test]
    fn parses_ipv4_tcp() {
        let p = ipv4_tcp_packet(51234, 443);
        let (proto, local, remote, rport) = parse_packet(&p).unwrap();
        assert_eq!(proto, Protocol::Tcp);
        assert_eq!(local, 51234);
        assert_eq!(
            remote,
            IpAddr::V4(std::net::Ipv4Addr::new(93, 184, 216, 34))
        );
        assert_eq!(rport, 443);
    }

    #[test]
    fn parses_ipv6_udp() {
        let mut p = vec![0u8; 44];
        p[0] = 0x60; // version 6
        p[6] = 17; // UDP
        p[8] = 0xfe;
        p[9] = 0x80;
        p[40..42].copy_from_slice(&53312u16.to_be_bytes());
        p[42..44].copy_from_slice(&53u16.to_be_bytes());
        let (proto, local, _, rport) = parse_packet(&p).unwrap();
        assert_eq!(proto, Protocol::Udp);
        assert_eq!(local, 53312);
        assert_eq!(rport, 53);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_packet(&[]).is_none());
        assert!(parse_packet(&[0x45]).is_none());
        assert!(parse_packet(&[0x99; 60]).is_none()); // version 9
        let mut icmp = ipv4_tcp_packet(1, 2);
        icmp[9] = 1; // ICMP
        assert!(parse_packet(&icmp).is_none());
    }

    #[test]
    fn log_throttle_window_transitions() {
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let key = || (PathBuf::from("/usr/bin/curl"), "93.184.216.34:443".to_string());
        let other_key = || (PathBuf::from("/usr/bin/wget"), "1.1.1.1:443".to_string());
        let mut throttle = LogThrottle::new(Duration::from_secs(5));
        let t0 = Instant::now();

        // First block for a key logs.
        assert_eq!(throttle.should_log(key(), t0), LogAction::First);
        // Repeats inside the window are suppressed.
        assert_eq!(
            throttle.should_log(key(), t0 + Duration::from_secs(1)),
            LogAction::Suppress
        );
        assert_eq!(
            throttle.should_log(key(), t0 + Duration::from_secs(2)),
            LogAction::Suppress
        );
        // A different key is independent.
        assert_eq!(
            throttle.should_log(other_key(), t0 + Duration::from_secs(3)),
            LogAction::First
        );
        // After the window: one summary covering the 2 suppressed
        // repeats + this block.
        assert_eq!(
            throttle.should_log(key(), t0 + Duration::from_secs(6)),
            LogAction::Summary(3)
        );
        // New window starts clean.
        assert_eq!(
            throttle.should_log(key(), t0 + Duration::from_secs(7)),
            LogAction::Suppress
        );
        // Expiry summarizes the suppressed repeat + this block.
        assert_eq!(
            throttle.should_log(key(), t0 + Duration::from_secs(13)),
            LogAction::Summary(2)
        );
        // Exactly at the boundary the window has expired, and with no
        // repeats in between the summary covers just this block.
        assert_eq!(
            throttle.should_log(key(), t0 + Duration::from_secs(18)),
            LogAction::Summary(1)
        );
    }

    #[test]
    fn table_script_contains_hook_and_queue() {
        let s = table_script(true, 0);
        assert!(s.contains("table inet travelmode"));
        assert!(s.contains("type filter hook output priority 0; policy accept;"));
        assert!(s.contains("ct state new queue num 0 bypass"));
        let s = table_script(false, 0);
        assert!(!s.contains("queue num"));
    }
}
