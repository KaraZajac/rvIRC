//! Read position persistence per target. Uses ~/.config/rvIRC/read_markers.toml.
//! Key: server + target. Value: message_scroll_offset (rows from bottom).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Serialize, Deserialize)]
struct ReadMarkersFile {
    #[serde(default)]
    targets: HashMap<String, u64>,
}

fn key(server: Option<&str>, target: &str) -> String {
    match server {
        Some(s) if !s.is_empty() => format!("{}:{}", s, target),
        _ => target.to_string(),
    }
}

/// Load scroll offset for a target. Returns None if not found.
pub fn load_read_offset(path: &Path, server: Option<&str>, target: &str) -> Option<usize> {
    if !path.exists() || target.is_empty() {
        return None;
    }
    let s = std::fs::read_to_string(path).ok()?;
    let data: ReadMarkersFile = toml::from_str(&s).ok()?;
    let k = key(server, target);
    data.targets.get(&k).copied().map(|v| v as usize)
}

/// Save scroll offset for a target.
pub fn save_read_offset(
    path: &Path,
    server: Option<&str>,
    target: &str,
    offset: usize,
) -> Result<(), String> {
    if target.is_empty() {
        return Ok(());
    }
    let mut data = if path.exists() {
        let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        toml::from_str(&s).unwrap_or_default()
    } else {
        ReadMarkersFile::default()
    };
    let k = key(server, target);
    data.targets.insert(k, offset as u64);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let toml_str = toml::to_string_pretty(&data).map_err(|e| e.to_string())?;
    std::fs::write(path, &toml_str).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
    Ok(())
}
