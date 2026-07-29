//! Flow → process attribution.
//!
//! Given a protocol and local port, find the socket inode from
//! /proc/net/{tcp,tcp6,udp,udp6}, then map inode → pid by scanning
//! /proc/<pid>/fd symlinks for `socket:[inode]`.
//!
//! The inode → pid map is rebuilt on a full refresh and updated
//! incrementally on cache misses. Processes dying mid-scan, permission
//! errors and missing files are all tolerated silently.

use std::collections::HashMap;
use std::path::Path;

use travelmode_core::types::Protocol;

/// Shared attributor; guard with a Mutex from the caller.
pub struct Attributor {
    inode_to_pid: HashMap<u64, u32>,
}

impl Attributor {
    pub fn new() -> Self {
        Self {
            inode_to_pid: HashMap::new(),
        }
    }

    /// Full rescan of /proc/<pid>/fd. Rebuilds the map from scratch.
    pub fn refresh(&mut self) {
        self.inode_to_pid = scan_proc_fds(Path::new("/proc"));
    }

    /// Look up the pid owning the socket bound to `local_port`/`proto`.
    /// On a cache miss, tries an incremental rescan once before giving up.
    pub fn lookup(&mut self, proto: Protocol, local_port: u16) -> Option<u32> {
        let inode = find_socket_inode(proto, local_port)?;
        if let Some(&pid) = self.inode_to_pid.get(&inode) {
            // Verify the pid still owns the inode (cheap single-dir check).
            if pid_owns_inode(pid, inode) {
                return Some(pid);
            }
        }
        // Cache miss or stale: rescan once.
        self.refresh();
        self.inode_to_pid.get(&inode).copied()
    }
}

/// Parse one /proc/net/{tcp,udp,tcp6,udp6} line; returns
/// (local_port, remote_port, inode) for socket rows.
fn parse_proc_net_line(line: &str) -> Option<(u16, u16, u64)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    // sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode
    if fields.len() < 10 || !fields[0].ends_with(':') {
        return None;
    }
    let local = fields[1];
    let remote = fields[2];
    let local_port = u16::from_str_radix(local.rsplit(':').next()?, 16).ok()?;
    let remote_port = u16::from_str_radix(remote.rsplit(':').next()?, 16).ok()?;
    let inode: u64 = fields[9].parse().ok()?;
    if inode == 0 {
        return None;
    }
    Some((local_port, remote_port, inode))
}

/// Find the inode of a socket with the given local port and protocol.
fn find_socket_inode(proto: Protocol, local_port: u16) -> Option<u64> {
    let files: &[&str] = match proto {
        Protocol::Tcp => &["/proc/net/tcp", "/proc/net/tcp6"],
        Protocol::Udp => &["/proc/net/udp", "/proc/net/udp6"],
        Protocol::Other => return None,
    };
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines() {
            if let Some((port, _, inode)) = parse_proc_net_line(line) {
                if port == local_port {
                    return Some(inode);
                }
            }
        }
    }
    None
}

/// Build the full inode → pid map by scanning /proc/<pid>/fd.
fn scan_proc_fds(proc_root: &Path) -> HashMap<u64, u32> {
    let mut map = HashMap::new();
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return map;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else { continue };
        collect_pid_sockets(pid, &entry.path().join("fd"), &mut map);
    }
    map
}

fn collect_pid_sockets(pid: u32, fd_dir: &Path, map: &mut HashMap<u64, u32>) {
    let Ok(fds) = std::fs::read_dir(fd_dir) else {
        return; // process died or permission denied
    };
    for fd in fds.flatten() {
        let Ok(target) = std::fs::read_link(fd.path()) else {
            continue;
        };
        if let Some(inode) = socket_inode_of(&target) {
            map.insert(inode, pid);
        }
    }
}

/// Extract the inode from a `socket:[12345]` symlink target.
fn socket_inode_of(target: &Path) -> Option<u64> {
    let s = target.to_str()?;
    let inner = s.strip_prefix("socket:[")?.strip_suffix(']')?;
    inner.parse().ok()
}

/// Cheap check that `pid` still owns `inode` (avoids stale cache hits).
fn pid_owns_inode(pid: u32, inode: u64) -> bool {
    let fd_dir = Path::new("/proc").join(pid.to_string()).join("fd");
    let Ok(fds) = std::fs::read_dir(fd_dir) else {
        return false;
    };
    for fd in fds.flatten() {
        if let Ok(target) = std::fs::read_link(fd.path()) {
            if socket_inode_of(&target) == Some(inode) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcp_line() {
        let line = "   1: 0100007F:0277 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 123456 1 0000000000000000 100 0 0 10 0";
        let (local, remote, inode) = parse_proc_net_line(line).unwrap();
        assert_eq!(local, 0x0277);
        assert_eq!(remote, 0);
        assert_eq!(inode, 123456);
    }

    #[test]
    fn skips_header_and_garbage() {
        assert!(parse_proc_net_line(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode"
        )
        .is_none());
        assert!(parse_proc_net_line("").is_none());
        assert!(parse_proc_net_line("not a row").is_none());
    }

    #[test]
    fn skips_zero_inode() {
        let line = "   1: 0100007F:0277 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 0 1 0000000000000000 100 0 0 10 0";
        assert!(parse_proc_net_line(line).is_none());
    }

    #[test]
    fn parses_socket_symlink() {
        assert_eq!(socket_inode_of(Path::new("socket:[12345]")), Some(12345));
        assert_eq!(socket_inode_of(Path::new("/dev/null")), None);
        assert_eq!(socket_inode_of(Path::new("pipe:[99]")), None);
    }
}
