//! Core domain types shared by daemon, CLI and (later) the GUI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

/// A running process that holds (or held) network sockets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
    pub user: Option<String>,
    pub ppid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Tcp,
    Udp,
    Other,
}

/// A single tracked network flow (conntrack entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Stable key for the flow (5-tuple + protocol rendered as a string).
    pub key: String,
    pub protocol: Protocol,
    pub local_addr: IpAddr,
    pub local_port: u16,
    pub remote_addr: IpAddr,
    pub remote_port: u16,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    /// Owning process, if attribution succeeded.
    pub pid: Option<u32>,
    pub process_name: Option<String>,
    pub exe: Option<PathBuf>,
    pub started: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Per-application aggregated usage (for `top`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUsage {
    pub name: String,
    pub exe: Option<PathBuf>,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub connections: u32,
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterfaceKind {
    Ethernet,
    Wifi,
    Vpn,
    Loopback,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub kind: InterfaceKind,
    pub mac: Option<String>,
    pub addrs: Vec<IpAddr>,
    pub is_up: bool,
}

/// Snapshot of the current network environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub interfaces: Vec<InterfaceInfo>,
    pub gateway: Option<IpAddr>,
    pub dns_servers: Vec<IpAddr>,
    /// Wi-Fi SSID if connected via wireless (from NetworkManager).
    pub ssid: Option<String>,
    /// Whether NetworkManager marks the connection as metered.
    pub metered: Option<bool>,
    /// Name of the interface carrying the default route.
    pub primary_interface: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleAction {
    Allow,
    Block,
}

/// A firewall rule keyed by executable path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: u64,
    /// Display name (usually the binary name).
    pub name: String,
    /// Absolute path of the executable this rule applies to.
    pub exe_path: PathBuf,
    pub action: RuleAction,
    /// If true the rule survives daemon restarts; otherwise it expires
    /// at `expires_at` (or when the daemon stops, if `expires_at` is None).
    pub persistent: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// Profile the rule belongs to ("default" until Phase 4).
    pub profile: String,
    pub updated_at: DateTime<Utc>,
}

/// Input for creating a rule (id/timestamps assigned by the daemon).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleInput {
    pub name: String,
    pub exe_path: PathBuf,
    pub action: RuleAction,
    pub persistent: bool,
    /// Seconds from now after which a temporary rule expires.
    pub ttl_secs: Option<u64>,
}

/// Daemon-wide status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub uptime_secs: u64,
    pub paused: bool,
    pub filtering_active: bool,
    pub rules_count: usize,
    pub tracked_connections: usize,
    pub tracked_processes: usize,
}

/// Live events pushed to `Subscribe` clients (the GUI in Phase 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    ConnectionOpened(ConnectionInfo),
    ConnectionUpdated(ConnectionInfo),
    ConnectionClosed { key: String },
    ProcessStarted(ProcessInfo),
    ProcessExited { pid: u32 },
    RuleAdded(Rule),
    RuleRemoved { id: u64 },
    PausedChanged { paused: bool },
    NetworkChanged(NetworkInfo),
}
