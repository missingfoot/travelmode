//! Pure client-side state: snapshots and daemon events are folded into
//! one ClientState that the pages render. No GTK types here, so the
//! reduction logic is fully unit-testable.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;

use travelmode_core::types::*;

/// Maximum number of live connections kept for the Connections page.
pub const MAX_CONNECTIONS: usize = 500;

/// Per-application aggregated traffic, mirrors the daemon's AppUsage
/// but adds rule linkage and speed sampling.
#[derive(Debug, Clone, Default)]
pub struct AppEntry {
    pub name: String,
    pub exe: Option<PathBuf>,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub connections: u32,
    pub blocked: bool,
    /// Id of the Block rule for this app, if one exists.
    pub block_rule_id: Option<u64>,
    pub speed_up: u64,
    pub speed_down: u64,
    prev_sent: u64,
    prev_recv: u64,
}

impl AppEntry {
    /// Light-switch model for the Apps page: the switch is ON when the
    /// app is allowed to use the network.
    pub fn is_allowed(&self) -> bool {
        !self.blocked
    }
}

/// Everything the UI renders, kept consistent with the daemon.
#[derive(Debug, Default)]
pub struct ClientState {
    pub connected: bool,
    pub status: Option<Status>,
    pub network: Option<NetworkInfo>,
    pub rules: Vec<Rule>,
    pub paused: bool,
    /// Keyed by app_key().
    pub apps: BTreeMap<String, AppEntry>,
    /// Live connections keyed by flow key.
    pub conns: HashMap<String, ConnectionInfo>,
    conn_order: VecDeque<String>,
    /// Session totals (accumulated from events/snapshots).
    pub total_up: u64,
    pub total_down: u64,
    pub speed_up: u64,
    pub speed_down: u64,
    prev_total_up: u64,
    prev_total_down: u64,
}

/// Grouping key for an application: exe path when known, else the
/// process name.
pub fn app_key(exe: &Option<PathBuf>, name: &str) -> String {
    exe.as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| format!("name:{name}"))
}

impl ClientState {
    /// Replace the whole state with the initial fetch result.
    pub fn apply_snapshot(
        &mut self,
        status: Status,
        network: NetworkInfo,
        top: Vec<AppUsage>,
        conns: Vec<ConnectionInfo>,
        rules: Vec<Rule>,
    ) {
        self.connected = true;
        self.paused = status.paused;
        self.status = Some(status);
        self.network = Some(network);
        self.rules = rules;

        self.conns.clear();
        self.conn_order.clear();
        self.total_up = 0;
        self.total_down = 0;
        for conn in conns {
            self.total_up += conn.bytes_sent;
            self.total_down += conn.bytes_recv;
            self.conn_order.push_back(conn.key.clone());
            self.conns.insert(conn.key.clone(), conn);
        }

        self.apps.clear();
        for usage in top {
            let key = app_key(&usage.exe, &usage.name);
            self.apps.insert(
                key,
                AppEntry {
                    name: usage.name,
                    exe: usage.exe,
                    bytes_sent: usage.bytes_sent,
                    bytes_recv: usage.bytes_recv,
                    connections: usage.connections,
                    ..Default::default()
                },
            );
        }
        self.refresh_rule_flags();
        self.prev_total_up = self.total_up;
        self.prev_total_down = self.total_down;
        for app in self.apps.values_mut() {
            app.prev_sent = app.bytes_sent;
            app.prev_recv = app.bytes_recv;
        }
    }

    /// Fold one daemon event into the state.
    pub fn apply_event(&mut self, event: &Event) {
        match event {
            Event::ConnectionOpened(conn) => self.upsert_conn(conn, true),
            Event::ConnectionUpdated(conn) => self.upsert_conn(conn, false),
            Event::ConnectionClosed { key } => self.remove_conn(key),
            Event::RuleAdded(rule) => {
                self.rules.retain(|r| r.id != rule.id);
                self.rules.push(rule.clone());
                self.refresh_rule_flags();
            }
            Event::RuleRemoved { id } => {
                self.rules.retain(|r| r.id != *id);
                self.refresh_rule_flags();
            }
            Event::PausedChanged { paused } => self.paused = *paused,
            Event::NetworkChanged(network) => self.network = Some(network.clone()),
            Event::ProcessStarted(_) | Event::ProcessExited { .. } => {}
        }
    }

    /// Called once per second by the UI tick: derives speeds from the
    /// byte counters.
    pub fn tick(&mut self) {
        self.speed_up = self.total_up.saturating_sub(self.prev_total_up);
        self.speed_down = self.total_down.saturating_sub(self.prev_total_down);
        self.prev_total_up = self.total_up;
        self.prev_total_down = self.total_down;
        for app in self.apps.values_mut() {
            app.speed_up = app.bytes_sent.saturating_sub(app.prev_sent);
            app.speed_down = app.bytes_recv.saturating_sub(app.prev_recv);
            app.prev_sent = app.bytes_sent;
            app.prev_recv = app.bytes_recv;
        }
        // Uptime drifts even without a fresh Status from the daemon.
        if let Some(status) = &mut self.status {
            status.uptime_secs += 1;
        }
    }

    pub fn active_app_count(&self) -> usize {
        self.apps.values().filter(|a| a.connections > 0).count()
    }

    pub fn blocked_app_count(&self) -> usize {
        self.apps.values().filter(|a| a.blocked).count()
    }

    fn upsert_conn(&mut self, conn: &ConnectionInfo, is_open: bool) {
        let (old_sent, old_recv, existed) = match self.conns.get(&conn.key) {
            Some(old) => (old.bytes_sent, old.bytes_recv, true),
            None => (0, 0, false),
        };
        // Session totals advance by the delta since we last saw the flow.
        self.total_up += conn.bytes_sent.saturating_sub(old_sent);
        self.total_down += conn.bytes_recv.saturating_sub(old_recv);

        let key = app_key(&conn.exe, conn.process_name.as_deref().unwrap_or("unknown"));
        let rules = self.rules.clone();
        let app = self.apps.entry(key).or_insert_with(|| AppEntry {
            name: conn.process_name.clone().unwrap_or_else(|| "unknown".into()),
            exe: conn.exe.clone(),
            ..Default::default()
        });
        app.bytes_sent += conn.bytes_sent.saturating_sub(old_sent);
        app.bytes_recv += conn.bytes_recv.saturating_sub(old_recv);
        if is_open && !existed {
            app.connections += 1;
        }
        apply_rule_flags(&rules, app);

        if !existed {
            self.conn_order.push_back(conn.key.clone());
        }
        self.conns.insert(conn.key.clone(), conn.clone());
        self.enforce_conn_cap();
    }

    fn remove_conn(&mut self, key: &str) {
        let Some(conn) = self.conns.remove(key) else {
            return;
        };
        self.conn_order.retain(|k| k != key);
        // Decrement the owning app's live-connection count; an app with
        // no connections and no block rule leaves the Apps page.
        let app_key = app_key(&conn.exe, conn.process_name.as_deref().unwrap_or("unknown"));
        if let Some(app) = self.apps.get_mut(&app_key) {
            app.connections = app.connections.saturating_sub(1);
            if app.connections == 0 && !app.blocked {
                self.apps.remove(&app_key);
            }
        }
    }

    fn enforce_conn_cap(&mut self) {
        while self.conns.len() > MAX_CONNECTIONS {
            if let Some(oldest) = self.conn_order.pop_front() {
                self.conns.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Recompute blocked flags + rule ids for every app from the rules.
    fn refresh_rule_flags(&mut self) {
        // Blocked apps without traffic still appear on the Apps page.
        for rule in self.rules.clone() {
            if rule.action == RuleAction::Block {
                let key = app_key(&Some(rule.exe_path.clone()), &rule.name);
                self.apps.entry(key).or_insert_with(|| AppEntry {
                    name: rule.name.clone(),
                    exe: Some(rule.exe_path.clone()),
                    ..Default::default()
                });
            }
        }
        let rules = self.rules.clone();
        let mut empty_apps = Vec::new();
        for (key, app) in self.apps.iter_mut() {
            apply_rule_flags(&rules, app);
            if app.connections == 0 && !app.blocked {
                empty_apps.push(key.clone());
            }
        }
        for key in empty_apps {
            self.apps.remove(&key);
        }
    }
}

/// Set blocked + block_rule_id on an app from the current rule set.
fn apply_rule_flags(rules: &[Rule], app: &mut AppEntry) {
    let rule = app.exe.as_ref().and_then(|exe| {
        rules
            .iter()
            .find(|r| r.action == RuleAction::Block && &r.exe_path == exe)
    });
    app.blocked = rule.is_some();
    app.block_rule_id = rule.map(|r| r.id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::net::{IpAddr, Ipv4Addr};

    fn conn(key: &str, exe: &str, name: &str, sent: u64, recv: u64) -> ConnectionInfo {
        ConnectionInfo {
            key: key.to_string(),
            protocol: Protocol::Tcp,
            local_addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            local_port: 51234,
            remote_addr: IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            remote_port: 443,
            bytes_sent: sent,
            bytes_recv: recv,
            pid: Some(1234),
            process_name: Some(name.to_string()),
            exe: Some(PathBuf::from(exe)),
            started: Utc::now(),
            last_seen: Utc::now(),
        }
    }

    fn block_rule(id: u64, exe: &str, name: &str) -> Rule {
        Rule {
            id,
            name: name.to_string(),
            exe_path: PathBuf::from(exe),
            action: RuleAction::Block,
            persistent: true,
            expires_at: None,
            profile: "default".into(),
            updated_at: Utc::now(),
        }
    }

    fn empty_status() -> Status {
        Status {
            version: "0.1.0".into(),
            uptime_secs: 0,
            paused: false,
            filtering_active: true,
            rules_count: 0,
            tracked_connections: 0,
            tracked_processes: 0,
        }
    }

    fn empty_network() -> NetworkInfo {
        NetworkInfo {
            interfaces: vec![],
            gateway: None,
            dns_servers: vec![],
            ssid: None,
            metered: None,
            primary_interface: None,
        }
    }

    #[test]
    fn snapshot_populates_state() {
        let mut s = ClientState::default();
        s.apply_snapshot(
            empty_status(),
            empty_network(),
            vec![AppUsage {
                name: "curl".into(),
                exe: Some(PathBuf::from("/usr/bin/curl")),
                bytes_sent: 100,
                bytes_recv: 900,
                connections: 1,
                blocked: false,
            }],
            vec![conn("tcp:a", "/usr/bin/curl", "curl", 100, 900)],
            vec![block_rule(1, "/usr/bin/firefox", "firefox")],
        );
        assert!(s.connected);
        assert_eq!(s.total_up, 100);
        assert_eq!(s.total_down, 900);
        // Blocked app with no traffic appears on the Apps page.
        let ff = &s.apps["/usr/bin/firefox"];
        assert!(ff.blocked);
        assert_eq!(ff.block_rule_id, Some(1));
        assert_eq!(s.blocked_app_count(), 1);
        assert_eq!(s.active_app_count(), 1);
    }

    #[test]
    fn opened_updated_closed_lifecycle() {
        let mut s = ClientState::default();
        let c1 = conn("tcp:a", "/usr/bin/curl", "curl", 100, 900);
        s.apply_event(&Event::ConnectionOpened(c1.clone()));
        assert_eq!(s.total_up, 100);
        assert_eq!(s.conns.len(), 1);
        assert_eq!(s.apps["/usr/bin/curl"].connections, 1);

        // Duplicate open (subscribe/snapshot overlap) is idempotent.
        s.apply_event(&Event::ConnectionOpened(c1.clone()));
        assert_eq!(s.total_up, 100);
        assert_eq!(s.apps["/usr/bin/curl"].connections, 1);

        // Update advances totals by delta only.
        let mut c2 = c1.clone();
        c2.bytes_sent = 150;
        c2.bytes_recv = 1200;
        s.apply_event(&Event::ConnectionUpdated(c2));
        assert_eq!(s.total_up, 150);
        assert_eq!(s.total_down, 1200);
        assert_eq!(s.apps["/usr/bin/curl"].bytes_recv, 1200);

        s.apply_event(&Event::ConnectionClosed { key: "tcp:a".into() });
        assert!(s.conns.is_empty());
        // Unblocked app with no connections disappears from the Apps page.
        assert!(!s.apps.contains_key("/usr/bin/curl"));
        // Session totals survive the close.
        assert_eq!(s.total_up, 150);
        assert_eq!(s.total_down, 1200);
    }

    #[test]
    fn rule_events_toggle_blocked_apps() {
        let mut s = ClientState::default();
        s.apply_event(&Event::ConnectionOpened(conn(
            "tcp:a", "/usr/bin/curl", "curl", 10, 10,
        )));
        assert!(!s.apps["/usr/bin/curl"].blocked);

        s.apply_event(&Event::RuleAdded(block_rule(7, "/usr/bin/curl", "curl")));
        assert!(s.apps["/usr/bin/curl"].blocked);
        assert_eq!(s.apps["/usr/bin/curl"].block_rule_id, Some(7));

        // Closing the flow keeps the blocked app visible.
        s.apply_event(&Event::ConnectionClosed { key: "tcp:a".into() });
        assert!(s.apps["/usr/bin/curl"].blocked);

        s.apply_event(&Event::RuleRemoved { id: 7 });
        // No traffic, no rule: the app leaves the page.
        assert!(!s.apps.contains_key("/usr/bin/curl"));
    }

    #[test]
    fn tick_computes_speeds() {
        let mut s = ClientState::default();
        s.apply_event(&Event::ConnectionOpened(conn(
            "tcp:a", "/usr/bin/curl", "curl", 1000, 2000,
        )));
        s.tick();
        assert_eq!(s.speed_up, 1000);
        assert_eq!(s.speed_down, 2000);
        assert_eq!(s.apps["/usr/bin/curl"].speed_up, 1000);

        // No new data: speed decays to zero.
        s.tick();
        assert_eq!(s.speed_up, 0);
        assert_eq!(s.speed_down, 0);

        let mut c = conn("tcp:a", "/usr/bin/curl", "curl", 1500, 2000);
        c.bytes_sent = 1500;
        s.apply_event(&Event::ConnectionUpdated(c));
        s.tick();
        assert_eq!(s.speed_up, 500);
    }

    #[test]
    fn connection_cap_evicts_oldest() {
        let mut s = ClientState::default();
        for i in 0..(MAX_CONNECTIONS + 10) {
            s.apply_event(&Event::ConnectionOpened(conn(
                &format!("tcp:{i}"),
                "/usr/bin/curl",
                "curl",
                1,
                1,
            )));
        }
        assert_eq!(s.conns.len(), MAX_CONNECTIONS);
        assert!(!s.conns.contains_key("tcp:0"));
        assert!(s.conns.contains_key(&format!("tcp:{}", MAX_CONNECTIONS + 9)));
    }

    #[test]
    fn allowed_is_the_inverse_of_blocked() {
        let mut app = AppEntry::default();
        assert!(app.is_allowed());
        app.blocked = true;
        assert!(!app.is_allowed());
    }

    #[test]
    fn paused_and_network_events() {
        let mut s = ClientState::default();
        s.apply_event(&Event::PausedChanged { paused: true });
        assert!(s.paused);
        let mut net = empty_network();
        net.ssid = Some("home-wifi".into());
        s.apply_event(&Event::NetworkChanged(net));
        assert_eq!(s.network.unwrap().ssid.as_deref(), Some("home-wifi"));
    }
}
