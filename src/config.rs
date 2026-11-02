use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamAccount {
    pub id: String,
    pub name: String,
    pub provider: String, // "openai", "anthropic", "gemini", "custom"
    pub base_url: String,
    pub api_key: String,
    pub fingerprint: String, // "Claude Code 2.x", "Cursor Pro", "Antigravity CLI", "Default"
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub port: u16,
    pub host: String,
    pub minimize_to_tray: bool,
    pub portable_mode: bool,
    pub accounts: Vec<UpstreamAccount>,
    pub active_account_id: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 8045,
            host: "127.0.0.1".to_string(),
            minimize_to_tray: true,
            portable_mode: false,
            accounts: vec![
                UpstreamAccount {
                    id: "default-anthropic".to_string(),
                    name: "Anthropic Direct".to_string(),
                    provider: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    api_key: "".to_string(),
                    fingerprint: "Claude Code 2.x".to_string(),
                    is_active: true,
                },
                UpstreamAccount {
                    id: "default-openai".to_string(),
                    name: "OpenAI Direct".to_string(),
                    provider: "openai".to_string(),
                    base_url: "https://api.openai.com".to_string(),
                    api_key: "".to_string(),
                    fingerprint: "Cursor Pro".to_string(),
                    is_active: false,
                },
            ],
            active_account_id: Some("default-anthropic".to_string()),
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        // Check for portable config in current directory first
        let local_cfg = Path::new("aroxy.json");
        if local_cfg.exists() {
            return local_cfg.to_path_buf();
        }

        if let Some(cfg_dir) = dirs::config_dir() {
            let app_dir = cfg_dir.join("aroxy");
            let _ = fs::create_dir_all(&app_dir);
            app_dir.join("config.json")
        } else {
            local_cfg.to_path_buf()
        }
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
                    return cfg;
                }
            }
        }
        let default_cfg = Self::default();
        let _ = default_cfg.save();
        default_cfg
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)
    }

    pub fn get_active_account(&self) -> Option<&UpstreamAccount> {
        if let Some(id) = &self.active_account_id {
            self.accounts.iter().find(|a| &a.id == id)
        } else {
            self.accounts.iter().find(|a| a.is_active)
        }
    }
}
