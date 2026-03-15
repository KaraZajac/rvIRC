//! Friends list persistence (per-server). Uses ~/.config/rvIRC/friends.toml.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Serialize, Deserialize)]
struct FriendsFile {
    #[serde(default)]
    servers: HashMap<String, Vec<String>>,
}

/// Load friends for all servers from file. Returns empty map if file missing.
pub fn load_all_friends(path: &Path) -> HashMap<String, Vec<String>> {
    if !path.exists() {
        return HashMap::new();
    }
    let Ok(s) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(data) = toml::from_str::<FriendsFile>(&s) else {
        return HashMap::new();
    };
    data.servers
}

/// Load friends list for the given server. Returns empty vec if file missing or server not found.
pub fn load_friends(path: &Path, server: Option<&str>) -> Vec<String> {
    let server = match server {
        Some(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };
    if !path.exists() {
        return Vec::new();
    }
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(data) = toml::from_str::<FriendsFile>(&s) else {
        return Vec::new();
    };
    data.servers
        .get(server)
        .cloned()
        .unwrap_or_default()
}

/// Save friends list for the given server. Merges with existing data for other servers.
pub fn save_friends(
    path: &Path,
    server: Option<&str>,
    friends: &[String],
) -> Result<(), String> {
    let server = match server {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(()),
    };
    let mut data = if path.exists() {
        let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        toml::from_str::<FriendsFile>(&s).unwrap_or_default()
    } else {
        FriendsFile::default()
    };
    data.servers.insert(server, friends.to_vec());
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
