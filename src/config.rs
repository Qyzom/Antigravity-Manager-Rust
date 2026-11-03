use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingStrategy {
    QuotaAware,
    RoundRobin,
    Failover,
}

impl Default for RoutingStrategy {
    fn default() -> Self {
        Self::QuotaAware
    }
}

impl RoutingStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::QuotaAware => "Quota-Aware (Smart)",
            Self::RoundRobin => "Round-Robin",
            Self::Failover => "Failover / Priority",
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            0 => Self::QuotaAware,
            1 => Self::RoundRobin,
            _ => Self::Failover,
        }
    }

    pub fn to_index(&self) -> i32 {
        match self {
            Self::QuotaAware => 0,
            Self::RoundRobin => 1,
            Self::Failover => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheAffinityMode {
    CacheFirst,       // Maximum KV cache reuse: pins session strictly to account until rate-limited or depleted
    Balanced,         // Hybrid: sticky session, but rotates smoothly if other accounts have > threshold more quota
    AggressiveSpread, // Quota leveling: rotates across healthy accounts on every request
}

impl Default for CacheAffinityMode {
    fn default() -> Self {
        Self::Balanced
    }
}

impl CacheAffinityMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CacheFirst => "Cache-First (Maximum Speed)",
            Self::Balanced => "Balanced (Smart Hybrid)",
            Self::AggressiveSpread => "Aggressive Spread (Quota Leveling)",
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            0 => Self::CacheFirst,
            1 => Self::Balanced,
            _ => Self::AggressiveSpread,
        }
    }

    pub fn to_index(&self) -> i32 {
        match self {
            Self::CacheFirst => 0,
            Self::Balanced => 1,
            Self::AggressiveSpread => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalogItem {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    #[serde(default = "default_tier")]
    pub tier: String, // "Free", "Pro"
    pub is_enabled: bool,
    pub supports_thinking: bool,
}

fn default_tier() -> String {
    "Pro".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamAccount {
    pub id: String,
    pub name: String,
    pub provider: String, // "antigravity", "openai", "anthropic", "gemini"
    pub base_url: String,
    pub api_key: String,
    pub fingerprint: String, // "Claude Code 2.x", "Cursor Pro", "Antigravity CLI", "Default"
    #[serde(default = "default_account_tier")]
    pub tier: String,        // "Free", "Pro", "Team"
    #[serde(default)]
    pub quota_used: u64,
    #[serde(default = "default_quota_limit")]
    pub quota_limit: u64,
    #[serde(default = "default_quota_reset")]
    pub quota_reset: String, // e.g. "in 3h 12m"
    #[serde(default = "default_pct_100")]
    pub claude_gpt_percent: i32,
    #[serde(default = "default_quota_reset")]
    pub claude_gpt_reset: String,
    #[serde(default = "default_pct_100")]
    pub gemini_percent: i32,
    #[serde(default = "default_quota_reset")]
    pub gemini_reset: String,
    #[serde(default = "default_status")]
    pub status: String,      // "Active", "Cooling Down", "Expired"
    pub is_active: bool,
}

fn default_pct_100() -> i32 {
    100
}

fn default_account_tier() -> String {
    "Pro".to_string()
}

fn default_quota_limit() -> u64 {
    100_000
}

fn default_quota_reset() -> String {
    "in 4h 00m".to_string()
}

fn default_status() -> String {
    "ACTIVE".to_string()
}

fn default_quota_display_mode() -> String {
    "percent".to_string()
}

fn default_language() -> String {
    "en".to_string()
}

fn default_max_log_entries() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub port: u16,
    pub host: String,
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub autostart: bool,
    pub portable_mode: bool,
    pub expose_reasoning_variants: bool, // Exposes (low), (medium), (high) model clones
    #[serde(default = "default_quota_display_mode")]
    pub quota_display_mode: String,     // "percent" or "fraction"
    #[serde(default)]
    pub hide_sidebar_brand: bool,       // Collapses top brand in sidebar
    #[serde(default = "default_language")]
    pub language: String,               // "en", "ru", "zh"
    #[serde(default = "default_true")]
    pub log_enabled: bool,
    #[serde(default = "default_max_log_entries")]
    pub max_log_entries: usize,
    #[serde(default = "default_true")]
    pub group_reasoning_clones: bool,
    #[serde(default)]
    pub routing_strategy: RoutingStrategy,
    #[serde(default)]
    pub cache_affinity_mode: CacheAffinityMode,
    #[serde(default = "default_session_ttl")]
    pub session_sticky_ttl_secs: u64,
    #[serde(default = "default_imbalance_threshold")]
    pub quota_imbalance_threshold: u32,
    #[serde(default = "default_true")]
    pub failover_on_429: bool,
    #[serde(default = "default_refresh_interval")]
    pub quota_refresh_interval_secs: u64,
    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval_secs: u64,
    #[serde(default = "default_fingerprint")]
    pub default_fingerprint: String,
    pub accounts: Vec<UpstreamAccount>,
    pub active_account_id: Option<String>,
    pub models: Vec<ModelCatalogItem>,
}

fn default_session_ttl() -> u64 {
    900 // 15 minutes
}

fn default_imbalance_threshold() -> u32 {
    25 // 25%
}

fn default_true() -> bool {
    true
}

fn default_refresh_interval() -> u64 {
    300
}

fn default_timeout() -> u64 {
    120
}

fn default_heartbeat() -> u64 {
    12
}

fn default_fingerprint() -> String {
    "Claude Code 2.x".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 8045,
            host: "127.0.0.1".to_string(),
            minimize_to_tray: true,
            autostart: false,
            portable_mode: false,
            expose_reasoning_variants: true,
            quota_display_mode: "percent".to_string(),
            hide_sidebar_brand: false,
            language: "en".to_string(),
            log_enabled: true,
            max_log_entries: 100,
            group_reasoning_clones: true,
            routing_strategy: RoutingStrategy::QuotaAware,
            cache_affinity_mode: CacheAffinityMode::Balanced,
            session_sticky_ttl_secs: 900,
            quota_imbalance_threshold: 25,
            failover_on_429: true,
            quota_refresh_interval_secs: 300,
            request_timeout_secs: 120,
            heartbeat_interval_secs: 12,
            default_fingerprint: "Claude Code 2.x".to_string(),
            accounts: Vec::new(),
            active_account_id: None,
            models: vec![
                ModelCatalogItem {
                    id: "claude-3-7-sonnet".to_string(),
                    display_name: "Claude 3.7 Sonnet (Thinking)".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-3-5-sonnet".to_string(),
                    display_name: "Claude 3.5 Sonnet".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: false,
                },
                ModelCatalogItem {
                    id: "claude-sonnet-4-6".to_string(),
                    display_name: "Claude Sonnet 4.6 (Thinking)".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-opus-4-6-thinking".to_string(),
                    display_name: "Claude Opus 4.6 (Thinking)".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-sonnet-5-5-high".to_string(),
                    display_name: "Claude Sonnet 5.5 (High)".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-opus-5-5-high".to_string(),
                    display_name: "Claude Opus 5.5 (High)".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-4-argon".to_string(),
                    display_name: "Gemini 4 Argon".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-3.8-flash-high".to_string(),
                    display_name: "Gemini 3.8 Flash (High)".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-3.7-flash-high".to_string(),
                    display_name: "Gemini 3.7 Flash (High)".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-3.6-flash-high".to_string(),
                    display_name: "Gemini 3.6 Flash (High)".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-3.1-pro-high".to_string(),
                    display_name: "Gemini 3.1 Pro (High)".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-2.5-pro".to_string(),
                    display_name: "Gemini 2.5 Pro".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Pro".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-2.5-flash".to_string(),
                    display_name: "Gemini 2.5 Flash Lite".to_string(),
                    provider: "gemini".to_string(),
                    tier: "Free".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gpt-oss-120b-medium".to_string(),
                    display_name: "GPT-OSS 120B (Medium)".to_string(),
                    provider: "openai".to_string(),
                    tier: "Pro".to_string(),
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
                if let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&content) {
                    // Purge any fake models or obsolete internal models (chat_*, gemini-3.5, gemini-3-flash, o1, etc.)
                    let has_obsolete = cfg.models.iter().any(|m| {
                        m.id.starts_with("chat_")
                            || m.id.contains("gemini-3.5")
                            || m.id.contains("gemini-3-flash")
                            || m.id == "o1"
                            || m.id == "gpt-4o"
                            || m.id == "gpt-4o-mini"
                            || m.id == "o3-mini"
                    });

                    if has_obsolete || cfg.models.is_empty() {
                        let defaults = Self::default();
                        cfg.models = defaults.models;
                        let _ = cfg.save();
                    }

                    // Normalize accounts status to uppercase
                    for acc in &mut cfg.accounts {
                        acc.status = if acc.is_active { "ACTIVE".to_string() } else { "DISABLED".to_string() };
                    }

                    return cfg;
                }
            }
        }
        let default_cfg = Self::default();
        let _ = default_cfg.save();
        default_cfg
    }

    pub fn format_claude_quota(&self) -> String {
        let pct = self.aggregate_claude_gpt_percent();
        if self.quota_display_mode == "fraction" {
            format!("{:.2} Remaining", (pct as f64) / 100.0)
        } else {
            format!("{}% Available", pct)
        }
    }

    pub fn format_gemini_quota(&self) -> String {
        let pct = self.aggregate_gemini_percent();
        if self.quota_display_mode == "fraction" {
            format!("{:.2} Remaining", (pct as f64) / 100.0)
        } else {
            format!("{}% Available", pct)
        }
    }

    pub fn aggregate_claude_gpt_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 100;
        }
        let sum: i32 = active.iter().map(|a| a.claude_gpt_percent).sum();
        sum / active.len() as i32
    }

    pub fn aggregate_gemini_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 100;
        }
        let sum: i32 = active.iter().map(|a| a.gemini_percent).sum();
        sum / active.len() as i32
    }

    pub fn nearest_claude_reset(&self) -> String {
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| a.claude_gpt_reset.clone())
            .next()
            .unwrap_or_else(|| "in 4h 00m".to_string())
    }

    pub fn nearest_gemini_reset(&self) -> String {
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| a.gemini_reset.clone())
            .next()
            .unwrap_or_else(|| "in 4h 00m".to_string())
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)
    }

    pub fn get_active_account(&self) -> Option<&UpstreamAccount> {
        match self.routing_strategy {
            RoutingStrategy::QuotaAware => {
                // Find active account with greatest remaining quota percentage
                self.accounts
                    .iter()
                    .filter(|a| a.is_active && a.status == "Active")
                    .max_by_key(|a| a.claude_gpt_percent.max(a.gemini_percent))
                    .or_else(|| self.accounts.iter().find(|a| a.is_active))
            }
            RoutingStrategy::Failover | RoutingStrategy::RoundRobin => {
                if let Some(id) = &self.active_account_id {
                    self.accounts.iter().find(|a| &a.id == id && a.is_active)
                        .or_else(|| self.accounts.iter().find(|a| a.is_active))
                } else {
                    self.accounts.iter().find(|a| a.is_active)
                }
            }
        }
    }

    pub fn total_quota_used(&self) -> u64 {
        self.accounts.iter().filter(|a| a.is_active).map(|a| a.quota_used).sum()
    }

    pub fn total_quota_limit(&self) -> u64 {
        self.accounts.iter().filter(|a| a.is_active).map(|a| a.quota_limit).sum()
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
                "display_name": m.display_name,
                "tier": m.tier
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
                        "display_name": format!("{} [{}]", m.display_name, label),
                        "tier": m.tier
                    }));
                }
            }
        }
        list
    }
}
