//! STS (Strict Transport Security) policy persistence.
//! Loads policies before connect; run_stream saves policies when server advertises on secure connection.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StsPolicyEntry {
    pub port: u16,
    pub expiry_ts: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StsPolicies {
    #[serde(flatten)]
    policies: HashMap<String, StsPolicyEntry>,
}

impl StsPolicies {
    pub fn load(path: &Path) -> Self {
        let s = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        let raw: HashMap<String, StsPolicyEntry> = toml::from_str(&s).unwrap_or_default();
        let mut policies = HashMap::new();
        for (host, entry) in raw {
            policies.insert(host.to_lowercase(), entry);
        }
        Self { policies }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let s = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, s).map_err(|e| e.to_string())
    }

    /// Returns (port, remaining_secs) if we have a valid (non-expired) policy for host.
    pub fn get_valid(&self, host: &str) -> Option<(u16, u64)> {
        let entry = self.policies.get(&host.to_lowercase())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();
        if now < entry.expiry_ts {
            Some((entry.port, entry.expiry_ts.saturating_sub(now)))
        } else {
            None
        }
    }

    /// Set/update policy for host. duration_secs = seconds from now until expiry.
    pub fn set(&mut self, host: &str, port: u16, duration_secs: u64) {
        let expiry_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs()
            .saturating_add(duration_secs);
        self.policies.insert(host.to_lowercase(), StsPolicyEntry { port, expiry_ts });
    }
}

/// Parse sts=port=6697,duration=31536000 from CAP value.
/// Returns (port, duration_secs) if both present.
pub fn parse_sts_cap_value(value: &str) -> Option<(u16, u64)> {
    let mut port = None;
    let mut duration = None;
    for token in value.split(',') {
        let token = token.trim();
        if let Some(rest) = token.strip_prefix("port=") {
            port = rest.parse().ok();
        } else if let Some(rest) = token.strip_prefix("duration=") {
            duration = rest.parse().ok();
        }
    }
    match (port, duration) {
        (Some(p), Some(d)) if d > 0 => Some((p, d)),
        _ => None,
    }
}

/// Find sts=... in CAP LS/LIST/NEW params (space-separated or single value).
/// Returns (port, duration_secs) if both present (for persistence on secure connection).
pub fn find_sts_in_cap_list(list: &str) -> Option<(u16, u64)> {
    for item in list.split_whitespace() {
        if let Some(value) = item.strip_prefix("sts=") {
            if let Some((port, dur)) = parse_sts_cap_value(value) {
                return Some((port, dur));
            }
        }
    }
    None
}

/// For insecure connections: extract port= from sts= for upgrade. Returns Some(port) if port= present.
pub fn find_sts_upgrade_port(list: &str) -> Option<u16> {
    for item in list.split_whitespace() {
        if let Some(value) = item.strip_prefix("sts=") {
            for token in value.split(',') {
                let token = token.trim();
                if let Some(rest) = token.strip_prefix("port=") {
                    if let Ok(p) = rest.parse::<u16>() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}
