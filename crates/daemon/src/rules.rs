//! Rule store: in-memory Vec<Rule> behind a lock, with JSON persistence
//! and TTL-based expiry reaping.
//!
//! Default policy is ALLOW-ALL: only rules with action=Block affect
//! filtering. Allow rules are stored for the future/UI.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use travelmode_core::types::{Rule, RuleAction, RuleInput};

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    next_id: u64,
    rules: Vec<Rule>,
}

#[derive(Debug)]
struct Inner {
    next_id: u64,
    rules: Vec<Rule>,
    path: PathBuf,
}

/// Persistent store of firewall rules.
pub struct RuleStore {
    inner: Mutex<Inner>,
}

impl RuleStore {
    /// Load rules from `path` (missing file = empty store).
    pub fn load(path: PathBuf) -> Self {
        let persisted = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Persisted>(&text) {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "cannot parse rules file; starting empty");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "cannot read rules file; starting empty");
                None
            }
        };
        let (next_id, mut rules) = persisted
            .map(|p| (p.next_id, p.rules))
            .unwrap_or((1, Vec::new()));
        // Drop rules that expired while the daemon was down.
        let now = Utc::now();
        rules.retain(|r| !is_expired(r, now));
        let max_id = rules.iter().map(|r| r.id).max().unwrap_or(0);
        info!(path = %path.display(), count = rules.len(), "loaded rules");
        Self {
            inner: Mutex::new(Inner {
                next_id: next_id.max(max_id + 1),
                rules,
                path,
            }),
        }
    }

    pub fn list(&self) -> Vec<Rule> {
        self.inner.lock().unwrap().rules.clone()
    }

    pub fn count(&self) -> usize {
        self.inner.lock().unwrap().rules.len()
    }

    /// Add a rule from user input; returns the stored rule.
    pub fn add(&self, input: RuleInput) -> Rule {
        let now = Utc::now();
        let rule = Rule {
            id: 0, // assigned below
            name: input.name,
            exe_path: input.exe_path,
            action: input.action,
            persistent: input.persistent,
            expires_at: input
                .ttl_secs
                .and_then(|ttl| now.checked_add_signed(Duration::seconds(ttl as i64))),
            profile: "default".to_string(),
            updated_at: now,
        };
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        let rule = Rule { id, ..rule };
        // Replace any existing rule for the same exe path + action.
        inner
            .rules
            .retain(|r| !(r.exe_path == rule.exe_path && r.action == rule.action));
        inner.rules.push(rule.clone());
        save_locked(&inner);
        rule
    }

    /// Remove a rule by id; returns it if it existed.
    pub fn remove(&self, id: u64) -> Option<Rule> {
        let mut inner = self.inner.lock().unwrap();
        let pos = inner.rules.iter().position(|r| r.id == id)?;
        let rule = inner.rules.remove(pos);
        save_locked(&inner);
        Some(rule)
    }

    /// Drop expired temporary rules; returns the removed ids.
    pub fn reap_expired(&self) -> Vec<u64> {
        let now = Utc::now();
        let mut inner = self.inner.lock().unwrap();
        let before = inner.rules.len();
        let removed: Vec<u64> = inner
            .rules
            .iter()
            .filter(|r| is_expired(r, now))
            .map(|r| r.id)
            .collect();
        inner.rules.retain(|r| !is_expired(r, now));
        if inner.rules.len() != before {
            save_locked(&inner);
        }
        removed
    }

    /// Whether a non-expired Block rule exists for this executable path.
    pub fn is_blocked(&self, exe_path: &Path) -> bool {
        let now = Utc::now();
        self.inner.lock().unwrap().rules.iter().any(|r| {
            r.action == RuleAction::Block && r.exe_path == exe_path && !is_expired(r, now)
        })
    }

    /// Persist current state (used on shutdown; saves happen on every
    /// mutation too, so this is mostly a no-op safety net).
    pub fn save(&self) {
        let inner = self.inner.lock().unwrap();
        save_locked(&inner);
    }
}

fn is_expired(rule: &Rule, now: DateTime<Utc>) -> bool {
    rule.expires_at.is_some_and(|exp| exp <= now)
}

/// Serialize the store to JSON, atomically (write tmp + rename).
fn save_locked(inner: &Inner) {
    let persisted = Persisted {
        next_id: inner.next_id,
        rules: inner.rules.clone(),
    };
    let text = match serde_json::to_string_pretty(&persisted) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "cannot serialize rules");
            return;
        }
    };
    if let Some(parent) = inner.path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            error!(path = %parent.display(), error = %e, "cannot create rules directory");
            return;
        }
    }
    let tmp = inner.path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, text) {
        error!(path = %tmp.display(), error = %e, "cannot write rules file");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &inner.path) {
        error!(path = %inner.path.display(), error = %e, "cannot replace rules file");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, exe: &str, action: RuleAction, ttl: Option<u64>) -> RuleInput {
        RuleInput {
            name: name.to_string(),
            exe_path: PathBuf::from(exe),
            action,
            persistent: ttl.is_none(),
            ttl_secs: ttl,
        }
    }

    #[test]
    fn add_list_remove_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tm-rules-test-{}", std::process::id()));
        let path = dir.join("rules.json");
        let store = RuleStore::load(path.clone());
        assert_eq!(store.count(), 0);

        let r1 = store.add(input("curl", "/usr/bin/curl", RuleAction::Block, None));
        let r2 = store.add(input("wget", "/usr/bin/wget", RuleAction::Allow, None));
        assert_eq!(store.count(), 2);
        assert_ne!(r1.id, r2.id);
        assert!(store.is_blocked(Path::new("/usr/bin/curl")));
        assert!(!store.is_blocked(Path::new("/usr/bin/wget")));

        // Reload from disk: rules survive.
        let store2 = RuleStore::load(path.clone());
        assert_eq!(store2.count(), 2);
        assert!(store2.is_blocked(Path::new("/usr/bin/curl")));
        // Ids continue from where the previous store left off.
        let r3 = store2.add(input("ftp", "/usr/bin/ftp", RuleAction::Block, None));
        assert!(r3.id > r2.id);

        assert!(store2.remove(r1.id).is_some());
        assert!(!store2.is_blocked(Path::new("/usr/bin/curl")));
        assert!(store2.remove(9999).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expired_rules_are_reaped() {
        let dir = std::env::temp_dir().join(format!("tm-rules-ttl-{}", std::process::id()));
        let path = dir.join("rules.json");
        let store = RuleStore::load(path);

        let r = store.add(input("tmp", "/usr/bin/tmp", RuleAction::Block, Some(0)));
        // ttl of 0 seconds expires immediately.
        let removed = store.reap_expired();
        assert_eq!(removed, vec![r.id]);
        assert_eq!(store.count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
