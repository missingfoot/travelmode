//! Daemon configuration, loaded from TOML with sane defaults.
//!
//! A missing config file is not an error: the daemon simply runs on
//! defaults. Every filesystem path is overridable so development and
//! testing can point everything at /tmp.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_socket_path() -> PathBuf {
    PathBuf::from("/run/travelmode/daemon.sock")
}

fn default_rules_file() -> PathBuf {
    PathBuf::from("/etc/travelmode/rules.json")
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_fail_open() -> bool {
    true
}

fn default_conntrack_poll_secs() -> u64 {
    1
}

fn default_process_poll_secs() -> u64 {
    2
}

fn default_queue_num() -> u16 {
    0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Unix socket the IPC server listens on.
    #[serde(default = "default_socket_path")]
    pub socket_path: PathBuf,
    /// JSON file rules are persisted to.
    #[serde(default = "default_rules_file")]
    pub rules_file: PathBuf,
    /// Log level filter (overridden by RUST_LOG if set).
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// If true, any failure in the filtering path results in ACCEPT.
    #[serde(default = "default_fail_open")]
    pub fail_open: bool,
    /// Conntrack polling interval in seconds.
    #[serde(default = "default_conntrack_poll_secs")]
    pub conntrack_poll_secs: u64,
    /// Process scan interval in seconds.
    #[serde(default = "default_process_poll_secs")]
    pub process_poll_secs: u64,
    /// NFQUEUE number to bind.
    #[serde(default = "default_queue_num")]
    pub queue_num: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            rules_file: default_rules_file(),
            log_level: default_log_level(),
            fail_open: default_fail_open(),
            conntrack_poll_secs: default_conntrack_poll_secs(),
            process_poll_secs: default_process_poll_secs(),
            queue_num: default_queue_num(),
        }
    }
}

impl Config {
    /// Load config from `path`; a missing file yields defaults.
    pub fn load(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::load_default_location();
        };
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("travelmoded: invalid config {}: {e}; using defaults", path.display());
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("travelmoded: config {} not found; using defaults", path.display());
                Self::default()
            }
            Err(e) => {
                eprintln!("travelmoded: cannot read config {}: {e}; using defaults", path.display());
                Self::default()
            }
        }
    }

    /// Try the default config locations, falling back to defaults.
    fn load_default_location() -> Self {
        for candidate in [Path::new("/etc/travelmode/config.toml")] {
            if candidate.exists() {
                return Self::load(Some(candidate));
            }
        }
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.socket_path, PathBuf::from("/run/travelmode/daemon.sock"));
        assert_eq!(cfg.rules_file, PathBuf::from("/etc/travelmode/rules.json"));
        assert_eq!(cfg.log_level, "info");
        assert!(cfg.fail_open);
        assert_eq!(cfg.conntrack_poll_secs, 1);
    }

    #[test]
    fn parses_partial_toml() {
        let cfg: Config = toml::from_str(
            r#"
            socket_path = "/tmp/tm/daemon.sock"
            rules_file = "/tmp/tm/rules.json"
            log_level = "debug"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.socket_path, PathBuf::from("/tmp/tm/daemon.sock"));
        assert_eq!(cfg.rules_file, PathBuf::from("/tmp/tm/rules.json"));
        assert_eq!(cfg.log_level, "debug");
        // Unset fields fall back to defaults.
        assert!(cfg.fail_open);
        assert_eq!(cfg.conntrack_poll_secs, 1);
    }

    #[test]
    fn missing_file_uses_defaults() {
        let cfg = Config::load(Some(Path::new("/nonexistent/travelmode/config.toml")));
        assert_eq!(cfg.socket_path, PathBuf::from("/run/travelmode/daemon.sock"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // Catches field-name drift between configs and the struct
        // (e.g. `rules_path` vs `rules_file`).
        let result = toml::from_str::<Config>(r#"rules_path = "/tmp/rules.json""#);
        assert!(result.is_err());
    }
}
