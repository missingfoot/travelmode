//! Process scanner: keeps a map of pid → ProcessInfo for processes that
//! hold at least one socket fd, refreshed on an interval, publishing
//! ProcessStarted/ProcessExited events on diff.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use procfs::process::{FDTarget, Process};
use tracing::debug;
use travelmode_core::types::{Event, ProcessInfo};

use crate::state::DaemonState;

/// Scan forever; designed to be spawned as a task.
pub async fn run(state: Arc<DaemonState>) {
    let interval = Duration::from_secs(state.config.process_poll_secs.max(1));
    let mut users = UserCache::new();
    loop {
        scan_once(&state, &mut users);
        tokio::time::sleep(interval).await;
    }
}

fn scan_once(state: &Arc<DaemonState>, users: &mut UserCache) {
    let mut current: HashMap<u32, ProcessInfo> = HashMap::new();
    let processes = match procfs::process::all_processes() {
        Ok(p) => p,
        Err(e) => {
            debug!(error = %e, "cannot iterate /proc");
            return;
        }
    };
    for proc in processes.flatten() {
        if let Some(info) = inspect_process(&proc, users) {
            current.insert(info.pid, info);
        }
    }

    let mut shared = state.processes.write().unwrap();

    for (pid, info) in &current {
        if !shared.contains_key(pid) {
            state.publish(Event::ProcessStarted(info.clone()));
        }
    }
    for pid in shared.keys() {
        if !current.contains_key(pid) {
            state.publish(Event::ProcessExited { pid: *pid });
        }
    }
    *shared = current;
}

/// Build a ProcessInfo if the process holds at least one socket.
fn inspect_process(proc: &Process, users: &mut UserCache) -> Option<ProcessInfo> {
    if !has_socket(proc) {
        return None;
    }
    let stat = proc.stat().ok()?;
    let pid = stat.pid as u32;
    let uid = proc.uid().ok();
    Some(ProcessInfo {
        pid,
        name: stat.comm,
        exe: proc.exe().ok(),
        user: uid.map(|u| users.name(u)),
        ppid: stat.ppid as u32,
    })
}

fn has_socket(proc: &Process) -> bool {
    let Ok(fds) = proc.fd() else {
        return false;
    };
    fds.flatten()
        .any(|fd| matches!(fd.target, FDTarget::Socket(_)))
}

/// Direct /proc lookup for a single pid. Used when the periodic scanner
/// hasn't seen the process yet — short-lived processes (e.g. curl) can
/// live and die between scans, and verdicts must not fail open for them.
pub fn lookup_process(pid: u32) -> Option<ProcessInfo> {
    let proc = Process::new(pid as i32).ok()?;
    let stat = proc.stat().ok()?;
    Some(ProcessInfo {
        pid,
        name: stat.comm,
        exe: proc.exe().ok(),
        user: None, // cheap path; user is not needed for verdicts
        ppid: stat.ppid as u32,
    })
}

/// Best-effort uid → username resolution, cached from /etc/passwd.
struct UserCache {
    map: HashMap<u32, String>,
}

impl UserCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn name(&mut self, uid: u32) -> String {
        if let Some(name) = self.map.get(&uid) {
            return name.clone();
        }
        let name = lookup_uid(uid).unwrap_or_else(|| uid.to_string());
        self.map.insert(uid, name.clone());
        name
    }
}

/// Resolve a uid by scanning /etc/passwd. Returns None if not found.
fn lookup_uid(uid: u32) -> Option<String> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    find_user_in_passwd(&text, uid)
}

fn find_user_in_passwd(text: &str, uid: u32) -> Option<String> {
    for line in text.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        let line_uid: u32 = fields.next()?.parse().ok()?;
        if line_uid == uid {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_users_in_passwd_fixture() {
        let fixture = "root:x:0:0:root:/root:/bin/bash\n\
                       daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin\n\
                       james:x:1000:1000:James:/home/james:/bin/zsh\n";
        assert_eq!(find_user_in_passwd(fixture, 0), Some("root".to_string()));
        assert_eq!(find_user_in_passwd(fixture, 1000), Some("james".to_string()));
        assert_eq!(find_user_in_passwd(fixture, 65534), None);
    }

    #[test]
    fn current_process_is_inspectable() {
        // The test process itself always exists in /proc.
        let proc = Process::myself().unwrap();
        let mut users = UserCache::new();
        // May or may not hold sockets depending on the test harness, so
        // just check the pieces that must work.
        let stat = proc.stat().unwrap();
        assert_eq!(stat.pid, std::process::id() as i32);
        assert!(proc.uid().is_ok());
        let _ = inspect_process(&proc, &mut users);
    }

    #[test]
    fn lookup_process_finds_self() {
        let info = lookup_process(std::process::id()).unwrap();
        assert_eq!(info.pid, std::process::id());
        assert!(info.name.starts_with("travelmoded"));
        let exe = info.exe.unwrap();
        assert!(exe.is_absolute());
        assert!(exe.display().to_string().contains("travelmoded"));
        // A pid that does not exist resolves to None, not a panic.
        assert!(lookup_process(u32::MAX).is_none());
    }
}
