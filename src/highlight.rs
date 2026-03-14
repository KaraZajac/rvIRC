//! Highlight words persistence. Uses ~/.config/rvIRC/highlight.toml.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Default, Serialize, Deserialize)]
struct HighlightFile {
    #[serde(default)]
    words: Vec<String>,
}

/// Load highlight words from file. Returns empty vec if file missing.
pub fn load_highlights(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    let Ok(s) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(data) = toml::from_str::<HighlightFile>(&s) else {
        return Vec::new();
    };
    data.words
}

/// Save highlight words to file.
pub fn save_highlights(path: &Path, words: &[String]) -> Result<(), String> {
    let data = HighlightFile { words: words.to_vec() };
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
