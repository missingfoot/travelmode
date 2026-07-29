//! Connection tracker: polls /proc/net/nf_conntrack and maintains a map
//! of live flows keyed by 5-tuple, with byte counters from conntrack
//! accounting.
//!
//! Line format (one flow per line, original direction first, reply
//! second):
//!   ipv4 2 tcp 6 431999 ESTABLISHED src=.. dst=.. sport=.. dport=..
//!   packets=.. bytes=.. src=.. dst=.. sport=.. dport=.. packets=..
//!   bytes=.. [ASSURED] mark=0 use=2
//!
//! The daemon runs as root, which this file requires. Byte accounting is
//! enabled at startup via /proc/sys/net/netfilter/nf_conntrack_acct.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tracing::{debug, warn};
use travelmode_core::types::{ConnectionInfo, Event, Protocol};

use crate::state::DaemonState;

const CONNTRACK_PATH: &str = "/proc/net/nf_conntrack";
const ACCT_PATH: &str = "/proc/sys/net/netfilter/nf_conntrack_acct";

/// Poll conntrack forever, keeping `state.connections` current and
/// publishing Connection* events. Designed to be spawned as a task.
pub async fn run(state: Arc<DaemonState>) {
    enable_accounting();
    // Prime the inode cache once before the first poll.
    state.attrib.lock().unwrap().refresh();

    let interval = Duration::from_secs(state.config.conntrack_poll_secs.max(1));
    loop {
        poll_once(&state);
        tokio::time::sleep(interval).await;
    }
}

/// Best-effort: turn on conntrack byte accounting. Without it, byte
/// counters simply stay 0.
fn enable_accounting() {
    if let Err(e) = std::fs::write(ACCT_PATH, "1") {
        warn!(path = ACCT_PATH, error = %e,
            "cannot enable conntrack accounting; byte counters will be 0");
    }
}

fn poll_once(state: &Arc<DaemonState>) {
    let text = match std::fs::read_to_string(CONNTRACK_PATH) {
        Ok(t) => t,
        Err(e) => {
            debug!(path = CONNTRACK_PATH, error = %e, "cannot read conntrack table");
            return;
        }
    };
    let now = Utc::now();
    let mut seen: HashMap<String, ParsedFlow> = HashMap::new();
    for line in text.lines() {
        if let Some((key, proto, local, lport, remote, rport, sent, recv)) = parse_line(line) {
            seen.entry(key.clone())
                .and_modify(|f| {
                    f.6 = f.6.max(sent);
                    f.7 = f.7.max(recv);
                })
                .or_insert((key, proto, local, lport, remote, rport, sent, recv));
        }
    }

    let mut conns = state.connections.write().unwrap();

    // Closed flows: present before, absent now.
    let closed: Vec<String> = conns
        .keys()
        .filter(|k| !seen.contains_key(*k))
        .cloned()
        .collect();
    for key in closed {
        conns.remove(&key);
        state.publish(Event::ConnectionClosed { key });
    }

    // New or updated flows.
    for (key, (_, proto, local, lport, remote, rport, sent, recv)) in seen {
        match conns.get_mut(&key) {
            Some(existing) => {
                if existing.bytes_sent != sent || existing.bytes_recv != recv {
                    existing.bytes_sent = sent;
                    existing.bytes_recv = recv;
                    existing.last_seen = now;
                    state.publish(Event::ConnectionUpdated(existing.clone()));
                }
            }
            None => {
                let (pid, process_name, exe) = state.attribute(proto, lport);
                let info = ConnectionInfo {
                    key: key.clone(),
                    protocol: proto,
                    local_addr: local,
                    local_port: lport,
                    remote_addr: remote,
                    remote_port: rport,
                    bytes_sent: sent,
                    bytes_recv: recv,
                    pid,
                    process_name,
                    exe,
                    started: now,
                    last_seen: now,
                };
                conns.insert(key, info.clone());
                state.publish(Event::ConnectionOpened(info));
            }
        }
    }
}

type ParsedFlow = (String, Protocol, IpAddr, u16, IpAddr, u16, u64, u64);

/// Parse one nf_conntrack line into (key, proto, local addr/port,
/// remote addr/port, bytes sent, bytes recv). Returns None for
/// non-TCP/UDP flows and malformed lines.
fn parse_line(line: &str) -> Option<ParsedFlow> {
    let mut tokens = line.split_whitespace().peekable();

    // Layer-3 / layer-4 headers: "ipv4 2 tcp 6 <timeout> [state]".
    let _l3 = tokens.next()?;
    let _l3num = tokens.next()?;
    let proto = match tokens.next()? {
        "tcp" => Protocol::Tcp,
        "udp" => Protocol::Udp,
        _ => return None,
    };
    let _l4num = tokens.next()?;
    let _timeout = tokens.next()?;
    // TCP lines carry a connection state token (ESTABLISHED, ...) that
    // is not a key=value pair; UDP lines do not.
    if proto == Protocol::Tcp {
        match tokens.peek() {
            Some(t) if !t.contains('=') => {
                tokens.next();
            }
            _ => {}
        }
    }

    // Key=value pairs. The first src/dst/sport/dport group is the
    // original direction (local side); the second is the reply.
    let mut orig: HashMap<&str, &str> = HashMap::new();
    let mut reply_bytes: Option<u64> = None;
    let mut orig_bytes: Option<u64> = None;
    let mut in_reply = false;
    for tok in tokens {
        let Some((k, v)) = tok.split_once('=') else {
            continue; // [ASSURED], [UNREPLIED], etc.
        };
        if k == "src" && orig.contains_key("src") {
            in_reply = true;
        }
        if in_reply {
            if k == "bytes" && reply_bytes.is_none() {
                reply_bytes = v.parse().ok();
            }
        } else {
            match k {
                "bytes" => {
                    if orig_bytes.is_none() {
                        orig_bytes = v.parse().ok();
                    }
                }
                _ => {
                    orig.entry(k).or_insert(v);
                }
            }
        }
    }

    let local: IpAddr = orig.get("src")?.parse().ok()?;
    let remote: IpAddr = orig.get("dst")?.parse().ok()?;
    let local_port: u16 = orig.get("sport")?.parse().ok()?;
    let remote_port: u16 = orig.get("dport")?.parse().ok()?;

    let proto_name = match proto {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::Other => unreachable!(),
    };
    let key = format!("{proto_name}:{local}:{local_port}-{remote}:{remote_port}");
    Some((
        key,
        proto,
        local,
        local_port,
        remote,
        remote_port,
        orig_bytes.unwrap_or(0),
        reply_bytes.unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn parses_tcp_established_with_bytes() {
        let line = "ipv4     2 tcp      6 431999 ESTABLISHED src=192.168.1.10 dst=142.250.74.46 sport=51234 dport=443 packets=10 bytes=1234 src=142.250.74.46 dst=192.168.1.10 sport=443 dport=51234 packets=8 bytes=5678 [ASSURED] mark=0 use=2";
        let (key, proto, local, lport, remote, rport, sent, recv) = parse_line(line).unwrap();
        assert_eq!(key, "tcp:192.168.1.10:51234-142.250.74.46:443");
        assert_eq!(proto, Protocol::Tcp);
        assert_eq!(local, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
        assert_eq!(lport, 51234);
        assert_eq!(remote, IpAddr::V4(Ipv4Addr::new(142, 250, 74, 46)));
        assert_eq!(rport, 443);
        assert_eq!(sent, 1234);
        assert_eq!(recv, 5678);
    }

    #[test]
    fn parses_udp_without_bytes() {
        let line = "ipv4     2 udp      17 29 src=192.168.1.10 dst=192.168.1.1 sport=53312 dport=53 packets=1 src=192.168.1.1 dst=192.168.1.10 sport=53 dport=53312 packets=1 mark=0 use=2";
        let (key, proto, local, lport, remote, rport, sent, recv) = parse_line(line).unwrap();
        assert_eq!(key, "udp:192.168.1.10:53312-192.168.1.1:53");
        assert_eq!(proto, Protocol::Udp);
        assert_eq!(local, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
        assert_eq!(lport, 53312);
        assert_eq!(remote, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(rport, 53);
        assert_eq!(sent, 0);
        assert_eq!(recv, 0);
    }

    #[test]
    fn parses_tcp_syn_sent_without_reply_bytes() {
        let line = "ipv4     2 tcp      6 59 SYN_SENT src=10.0.0.5 dst=93.184.216.34 sport=40000 dport=80 packets=2 bytes=120 [UNREPLIED] src=93.184.216.34 dst=10.0.0.5 sport=80 dport=40000 packets=0 bytes=0 mark=0 use=2";
        let (key, proto, _, lport, _, rport, sent, recv) = parse_line(line).unwrap();
        assert_eq!(key, "tcp:10.0.0.5:40000-93.184.216.34:80");
        assert_eq!(proto, Protocol::Tcp);
        assert_eq!(lport, 40000);
        assert_eq!(rport, 80);
        assert_eq!(sent, 120);
        assert_eq!(recv, 0);
    }

    #[test]
    fn parses_ipv6_udp() {
        let line = "ipv6     2 udp      17 25 src=fe80::1 dst=ff02::fb sport=5353 dport=5353 packets=3 bytes=300 src=ff02::fb dst=fe80::1 sport=5353 dport=5353 packets=0 bytes=0 mark=0 use=2";
        let (key, proto, _, _, _, _, sent, _) = parse_line(line).unwrap();
        assert_eq!(key, "udp:fe80::1:5353-ff02::fb:5353");
        assert_eq!(proto, Protocol::Udp);
        assert_eq!(sent, 300);
    }

    #[test]
    fn skips_non_tcp_udp() {
        assert!(parse_line(
            "ipv4     2 icmp     1 29 src=192.168.1.10 dst=8.8.8.8 type=8 code=0 id=1234 packets=1 bytes=84 mark=0 use=2"
        )
        .is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("garbage line").is_none());
    }
}
