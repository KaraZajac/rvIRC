//! Load and parse ~/.config/rvIRC/config.toml

use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RvConfig {
    pub username: Option<String>,
    pub nickname: Option<String>,
    /// Alternate nickname if primary is in use (used on 433).
    #[serde(default)]
    pub alt_nick: Option<String>,
    pub real_name: Option<String>,
    /// Default directory for received file transfers. Expands ~ to home dir.
    #[serde(default)]
    pub download_dir: Option<String>,
    /// Whether to fetch and display inline images for image URLs (default: true).
    #[serde(default = "default_render_images")]
    pub render_images: bool,
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
}

fn default_render_images() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
    /// Server connection password (sent on connect).
    pub password: Option<String>,
    /// Password to identify with NickServ after connecting (e.g. PRIVMSG NickServ :IDENTIFY <this>).
    #[serde(default)]
    pub identify_password: Option<String>,
    /// Auto-connect to this server on startup: "yes" or "no".
    #[serde(default)]
    pub auto_connect: Option<String>,
    /// Comma-separated channels to join after connecting (e.g. "#chan1, #chan2").
    #[serde(default)]
    pub auto_join: Option<String>,
}

impl ServerEntry {
    pub fn is_auto_connect(&self) -> bool {
        self.auto_connect
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("yes"))
            .unwrap_or(false)
    }

    /// Parse auto_join string into a list of channel names (trimmed, non-empty). Adds # if missing.
    pub fn auto_join_channels(&self) -> Vec<String> {
        self.auto_join
            .as_deref()
            .map(|s| {
                s.split(',')
                    .map(|c| {
                        let c = c.trim().to_string();
                        if c.is_empty() {
                            c
                        } else if c.starts_with('#') || c.starts_with('&') {
                            c
                        } else {
                            format!("#{}", c)
                        }
                    })
                    .filter(|c| !c.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl RvConfig {
    /// Config directory: ~/.config/rvIRC (or $XDG_CONFIG_HOME/rvIRC if set)
    pub fn config_dir() -> Option<PathBuf> {
        let base = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                directories::BaseDirs::new().map(|d| d.home_dir().join(".config"))
            })?;
        Some(base.join("rvIRC"))
    }

    /// Config file path: ~/.config/rvIRC/config.toml
    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("config.toml"))
    }

    /// Load config from disk. Creates config dir and a default config if file is missing.
    pub fn load() -> Result<Self, String> {
        let path = Self::config_path().ok_or("Could not determine config directory")?;
        let dir = path.parent().ok_or("Invalid config path")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

        if !path.exists() {
            let default = Self::default_config();
            let toml = toml::to_string_pretty(&default).map_err(|e| e.to_string())?;
            std::fs::write(&path, toml).map_err(|e| e.to_string())?;
            return Ok(default);
        }
        let s = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        toml::from_str(&s).map_err(|e| e.to_string())
    }

    /// Resolve download_dir, expanding ~ to home directory. Returns None if not set.
    pub fn resolved_download_dir(&self) -> Option<PathBuf> {
        self.download_dir.as_ref().map(|d| {
            if d.starts_with('~') {
                if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
                    home.join(d.strip_prefix("~/").unwrap_or(&d[1..]))
                } else {
                    PathBuf::from(d)
                }
            } else {
                PathBuf::from(d)
            }
        })
    }

    fn default_config() -> RvConfig {
        RvConfig {
            username: Some("user".to_string()),
            nickname: Some("rvirc_user".to_string()),
            alt_nick: None,
            real_name: Some("rvIRC User".to_string()),
            download_dir: None,
            render_images: true,
            servers: vec![
                ServerEntry {
                    name: "Libera".to_string(),
                    host: "irc.libera.chat".to_string(),
                    port: 6697,
                    tls: true,
                    password: None,
                    identify_password: None,
                    auto_connect: None,
                    auto_join: None,
                },
                ServerEntry {
                    name: "Hackint".to_string(),
                    host: "irc.hackint.org".to_string(),
                    port: 6697,
                    tls: true,
                    password: None,
                    identify_password: None,
                    auto_connect: None,
                    auto_join: None,
                },
                ServerEntry {
                    name: "Local".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 6667,
                    tls: false,
                    password: None,
                    identify_password: None,
                    auto_connect: None,
                    auto_join: None,
                },
            ],
        }
    }

    /// Find server by name (case-insensitive).
    pub fn server_by_name(&self, name: &str) -> Option<&ServerEntry> {
        self.servers
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
}
