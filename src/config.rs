use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalogItem {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub is_enabled: bool,
    pub supports_thinking: bool,
}

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
    pub expose_reasoning_variants: bool, // Exposes (low), (medium), (high) model clones
    pub accounts: Vec<UpstreamAccount>,
    pub active_account_id: Option<String>,
    pub models: Vec<ModelCatalogItem>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 8045,
            host: "127.0.0.1".to_string(),
            minimize_to_tray: true,
            portable_mode: false,
            expose_reasoning_variants: true,
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
            models: vec![
                ModelCatalogItem {
                    id: "claude-3-7-sonnet".to_string(),
                    display_name: "Claude 3.7 Sonnet (Hybrid)".to_string(),
                    provider: "anthropic".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-3-5-sonnet".to_string(),
                    display_name: "Claude 3.5 Sonnet".to_string(),
                    provider: "anthropic".to_string(),
                    is_enabled: true,
                    supports_thinking: false,
                },
                ModelCatalogItem {
                    id: "claude-3-5-haiku".to_string(),
                    display_name: "Claude 3.5 Haiku".to_string(),
                    provider: "anthropic".to_string(),
                    is_enabled: true,
                    supports_thinking: false,
                },
                ModelCatalogItem {
                    id: "gpt-4o".to_string(),
                    display_name: "GPT-4o (Omni)".to_string(),
                    provider: "openai".to_string(),
                    is_enabled: true,
                    supports_thinking: false,
                },
                ModelCatalogItem {
                    id: "gpt-4o-mini".to_string(),
                    display_name: "GPT-4o Mini".to_string(),
                    provider: "openai".to_string(),
                    is_enabled: true,
                    supports_thinking: false,
                },
                ModelCatalogItem {
                    id: "o3-mini".to_string(),
                    display_name: "o3-mini (Reasoning)".to_string(),
                    provider: "openai".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "o1".to_string(),
                    display_name: "o1 Preview".to_string(),
                    provider: "openai".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-2.5-flash".to_string(),
                    display_name: "Gemini 2.5 Flash (Thinking)".to_string(),
                    provider: "gemini".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-2.5-pro".to_string(),
                    display_name: "Gemini 2.5 Pro".to_string(),
                    provider: "gemini".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
            ],
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
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

    pub fn get_broadcast_models(&self) -> Vec<serde_json::Value> {
        let mut list = Vec::new();
        for m in &self.models {
            if !m.is_enabled {
                continue;
            }
            // Base model
            list.push(serde_json::json!({
                "id": m.id,
                "object": "model",
                "owned_by": m.provider,
                "display_name": m.display_name
            }));

            // If thinking variants are enabled, expose clones
            if self.expose_reasoning_variants && m.supports_thinking {
                for (suffix, label) in [
                    (" (low)", "Low Effort"),
                    (" (medium)", "Medium Effort"),
                    (" (high)", "High Effort"),
                    (" (no-thinking)", "Thinking Off"),
                ] {
                    list.push(serde_json::json!({
                        "id": format!("{}{}", m.id, suffix),
                        "object": "model",
                        "owned_by": m.provider,
                        "display_name": format!("{} [{}]", m.display_name, label)
                    }));
                }
            }
        }
        list
    }
}
