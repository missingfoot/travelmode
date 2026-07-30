//! Terminate existing flows when a Block rule comes into effect.
//!
//! The firewall only queues `ct state new` packets, so flows that were
//! already established when an app gets blocked would keep flowing.
//! This module kills them two ways:
//!
//! - TCP: destroy the local socket with `ss -K` (SOCK_DESTROY via
//!   iproute2, always present). The app gets a RST; its reconnect hits
//!   the block verdict in the NFQUEUE worker.
//! - All protocols: delete the conntrack entry with `conntrack -D` so
//!   later packets re-enter conntrack as NEW and are queued/dropped.
//!   conntrack-tools is optional: its absence is detected once and UDP
//!   flows then simply linger until timeout.
//!
//! Killing is strictly best-effort: a block operation never fails
//! because a kill failed.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use tracing::{debug, info, warn};
use travelmode_core::types::{ConnectionInfo, Protocol, RuleAction};

use crate::state::DaemonState;

/// Whether the `conntrack` CLI exists (probed once).
static CONNTRACK_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Kill existing flows for every Block rule. Runs once at startup,
/// after the first conntrack poll has populated the flow map, so
/// persisted blocks also terminate flows that pre-date the daemon.
pub fn kill_flows_for_block_rules(state: &Arc<DaemonState>) {
    for rule in state.rules.list() {
        if rule.action == RuleAction::Block {
            kill_flows_for_exe(state, &rule.exe_path);
        }
    }
}

/// Terminate all tracked flows attributed to `exe_path` (exact path
/// match, same as rule matching).
pub fn kill_flows_for_exe(state: &Arc<DaemonState>, exe_path: &Path) {
    let victims = matching_flows(&state.connections.read().unwrap(), exe_path);
    if victims.is_empty() {
        return;
    }
    let mut killed = 0;
    for conn in &victims {
        let mut acted = false;
        if delete_conntrack_entry(conn) {
            acted = true;
        }
        if conn.protocol == Protocol::Tcp && kill_tcp_socket(conn) {
            acted = true;
        }
        if acted {
            killed += 1;
            debug!(flow = %conn.key, exe = %exe_path.display(), "killed existing flow");
        }
    }
    info!(
        killed,
        total = victims.len(),
        exe = %exe_path.display(),
        "killed existing flows for blocked app"
    );
}

/// Tracked flows owned by `exe_path` (exact match).
fn matching_flows(
    conns: &HashMap<String, ConnectionInfo>,
    exe_path: &Path,
) -> Vec<ConnectionInfo> {
    conns
        .values()
        .filter(|c| c.exe.as_deref() == Some(exe_path))
        .cloned()
        .collect()
}

fn conntrack_available() -> bool {
    *CONNTRACK_AVAILABLE.get_or_init(|| {
        let available = Command::new("conntrack")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !available {
            info!("conntrack-tools not installed, UDP flows will linger until timeout");
        }
        available
    })
}

/// `conntrack -D -p tcp --orig-src .. --orig-dst .. --sport .. --dport ..`
fn conntrack_delete_args(conn: &ConnectionInfo) -> Vec<String> {
    let proto = match conn.protocol {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::Other => return Vec::new(),
    };
    vec![
        "-D".into(),
        "-p".into(),
        proto.into(),
        "--orig-src".into(),
        conn.local_addr.to_string(),
        "--orig-dst".into(),
        conn.remote_addr.to_string(),
        "--sport".into(),
        conn.local_port.to_string(),
        "--dport".into(),
        conn.remote_port.to_string(),
    ]
}

/// `ss -K src <local> dst <remote> sport = <lport> dport = <rport>`
fn ss_kill_args(conn: &ConnectionInfo) -> Vec<String> {
    vec![
        "-K".into(),
        "src".into(),
        conn.local_addr.to_string(),
        "dst".into(),
        conn.remote_addr.to_string(),
        "sport".into(),
        "=".into(),
        conn.local_port.to_string(),
        "dport".into(),
        "=".into(),
        conn.remote_port.to_string(),
    ]
}

fn delete_conntrack_entry(conn: &ConnectionInfo) -> bool {
    let args = conntrack_delete_args(conn);
    if args.is_empty() || !conntrack_available() {
        return false;
    }
    match Command::new("conntrack").args(&args).output() {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            debug!(
                flow = %conn.key,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "conntrack -D failed"
            );
            false
        }
        Err(e) => {
            warn!(error = %e, "cannot run conntrack");
            false
        }
    }
}

fn kill_tcp_socket(conn: &ConnectionInfo) -> bool {
    match Command::new("ss").args(ss_kill_args(conn)).output() {
        // ss exits 0 even when nothing matched; best-effort either way.
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            debug!(
                flow = %conn.key,
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "ss -K failed"
            );
            false
        }
        Err(e) => {
            warn!(error = %e, "cannot run ss");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    fn conn(key: &str, proto: Protocol, exe: Option<&str>) -> ConnectionInfo {
        ConnectionInfo {
            key: key.to_string(),
            protocol: proto,
            local_addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            local_port: 51234,
            remote_addr: IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            remote_port: 443,
            bytes_sent: 100,
            bytes_recv: 900,
            pid: Some(1234),
            process_name: Some("curl".into()),
            exe: exe.map(PathBuf::from),
            started: Utc::now(),
            last_seen: Utc::now(),
        }
    }

    #[test]
    fn ss_kill_command_is_specific() {
        let c = conn("tcp:a", Protocol::Tcp, Some("/usr/bin/curl"));
        assert_eq!(
            ss_kill_args(&c),
            vec![
                "-K",
                "src",
                "192.168.1.10",
                "dst",
                "93.184.216.34",
                "sport",
                "=",
                "51234",
                "dport",
                "=",
                "443",
            ]
        );
    }

    #[test]
    fn conntrack_delete_command_matches_tuple() {
        let c = conn("tcp:a", Protocol::Tcp, Some("/usr/bin/curl"));
        assert_eq!(
            conntrack_delete_args(&c),
            vec![
                "-D",
                "-p",
                "tcp",
                "--orig-src",
                "192.168.1.10",
                "--orig-dst",
                "93.184.216.34",
                "--sport",
                "51234",
                "--dport",
                "443",
            ]
        );
        let u = conn("udp:b", Protocol::Udp, Some("/usr/bin/curl"));
        assert_eq!(conntrack_delete_args(&u)[2], "udp");
        // Non-TCP/UDP flows produce no command.
        let o = conn("other:c", Protocol::Other, Some("/usr/bin/curl"));
        assert!(conntrack_delete_args(&o).is_empty());
    }

    #[test]
    fn victim_matching_is_exact_exe_path() {
        let mut conns = HashMap::new();
        conns.insert("a".into(), conn("a", Protocol::Tcp, Some("/usr/bin/curl")));
        conns.insert("b".into(), conn("b", Protocol::Tcp, Some("/usr/bin/curl2")));
        conns.insert("c".into(), conn("c", Protocol::Tcp, None));
        conns.insert("d".into(), conn("d", Protocol::Tcp, Some("/usr/bin/wget")));

        let victims = matching_flows(&conns, Path::new("/usr/bin/curl"));
        assert_eq!(victims.len(), 1);
        assert_eq!(victims[0].key, "a");
    }
}
