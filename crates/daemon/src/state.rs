//! Shared daemon state passed to every module and the IPC dispatcher.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use tokio::sync::broadcast;
use travelmode_core::types::{ConnectionInfo, Event, ProcessInfo};

use crate::attrib::Attributor;
use crate::config::Config;
use crate::firewall::Firewall;
use crate::rules::RuleStore;

/// Broadcast channel capacity for events. Slow subscribers drop events
/// (lagged) rather than blocking the daemon.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

pub struct DaemonState {
    pub config: Config,
    pub started: Instant,
    pub paused: AtomicBool,
    pub filtering_active: AtomicBool,
    pub blocked_count: AtomicU64,
    pub rules: RuleStore,
    pub processes: RwLock<HashMap<u32, ProcessInfo>>,
    pub connections: RwLock<HashMap<String, ConnectionInfo>>,
    pub attrib: Mutex<Attributor>,
    /// Rate limiter for blocked-connection info logging (verdict path).
    pub block_log: Mutex<crate::firewall::LogThrottle>,
    pub events: broadcast::Sender<Event>,
    /// Last network snapshot, cached for GetNetwork.
    pub network: RwLock<Option<travelmode_core::types::NetworkInfo>>,
    /// nftables handle; None when firewall setup failed (e.g. not root).
    pub firewall: tokio::sync::Mutex<Option<Firewall>>,
}

impl DaemonState {
    pub fn new(config: Config) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let rules = RuleStore::load(config.rules_file.clone());
        Self {
            config,
            started: Instant::now(),
            paused: AtomicBool::new(false),
            filtering_active: AtomicBool::new(false),
            blocked_count: AtomicU64::new(0),
            rules,
            processes: RwLock::new(HashMap::new()),
            connections: RwLock::new(HashMap::new()),
            attrib: Mutex::new(Attributor::new()),
            block_log: Mutex::new(crate::firewall::LogThrottle::default()),
            events,
            network: RwLock::new(None),
            firewall: tokio::sync::Mutex::new(None),
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn publish(&self, event: Event) {
        // No subscribers is normal; ignore the send error.
        let _ = self.events.send(event);
    }

    /// Attribute a flow to a process and fill in the process fields.
    pub fn attribute(
        &self,
        proto: travelmode_core::types::Protocol,
        local_port: u16,
    ) -> (Option<u32>, Option<String>, Option<PathBuf>) {
        let pid = self.attrib.lock().unwrap().lookup(proto, local_port);
        let Some(pid) = pid else {
            return (None, None, None);
        };
        if let Some(p) = self.processes.read().unwrap().get(&pid) {
            return (Some(pid), Some(p.name.clone()), p.exe.clone());
        }
        // The periodic scanner hasn't seen this pid yet (short-lived
        // processes can live and die between scans). Fall back to a
        // direct /proc lookup so rule lookups don't fail open.
        match crate::procs::lookup_process(pid) {
            Some(p) => (Some(pid), Some(p.name), p.exe),
            None => (Some(pid), None, None),
        }
    }
}
