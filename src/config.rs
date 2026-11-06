use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use crate::i18n::{get_translations, Language};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoutingStrategy {
    #[default]
    QuotaAware,
    RoundRobin,
    Failover,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CacheAffinityMode {
    CacheFirst,       // Maximum KV cache reuse: pins session strictly to account until rate-limited or depleted
    #[default]
    Balanced,         // Hybrid: sticky session, but rotates smoothly if other accounts have > threshold more quota
    AggressiveSpread, // Quota leveling: rotates across healthy accounts on every request
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
    pub claude_gpt_weekly_percent: i32,
    #[serde(default = "default_weekly_reset")]
    pub claude_gpt_weekly_reset: String,
    #[serde(default = "default_pct_100")]
    pub gemini_percent: i32,
    #[serde(default = "default_quota_reset")]
    pub gemini_reset: String,
    #[serde(default = "default_pct_100")]
    pub gemini_weekly_percent: i32,
    #[serde(default = "default_weekly_reset")]
    pub gemini_weekly_reset: String,
    #[serde(default = "default_status")]
    pub status: String,      // "ACTIVE", "DISABLED", "ERROR"
    pub is_active: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub added_at: Option<u64>,
    #[serde(default)]
    pub device_profile: Option<DeviceProfile>,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceProfile {
    pub machine_id: String,
    pub mac_machine_id: String,
    pub dev_device_id: String,
    pub sqm_id: String,
}

impl DeviceProfile {
    pub fn generate() -> Self {
        let random_hex_32 = uuid::Uuid::new_v4().simple().to_string();
        let machine_id = format!("auth0|user_{}", random_hex_32);
        let dev_device_id = uuid::Uuid::new_v4().to_string();
        let mac_machine_id = uuid::Uuid::new_v4().to_string();
        let sqm_id = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());

        Self {
            machine_id,
            mac_machine_id,
            dev_device_id,
            sqm_id,
        }
    }
}

impl Default for UpstreamAccount {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            provider: "antigravity".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            fingerprint: "Google Antigravity Client (1.107.0)".to_string(),
            tier: "Pro".to_string(),
            quota_used: 0,
            quota_limit: 100_000,
            quota_reset: "in 4h 00m".to_string(),
            claude_gpt_percent: 100,
            claude_gpt_reset: "in 4h 00m".to_string(),
            claude_gpt_weekly_percent: 100,
            claude_gpt_weekly_reset: "in 7d 00h".to_string(),
            gemini_percent: 100,
            gemini_reset: "in 4h 00m".to_string(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d 00h".to_string(),
            status: "ACTIVE".to_string(),
            is_active: true,
            error_message: None,
            refresh_token: None,
            project_id: None,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        }
    }
}

fn default_weekly_reset() -> String {
    "in 7d 00h".to_string()
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

fn default_quota_view_mode() -> String {
    "both".to_string()
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
    #[serde(default = "default_quota_view_mode")]
    pub quota_view_mode: String,        // "both", "5hour", "weekly"
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
    #[serde(default = "default_protection_threshold")]
    pub quota_protection_threshold: u8,
    #[serde(default = "default_true")]
    pub spoof_go_sdk: bool,
    #[serde(default = "default_true")]
    pub inject_ide_metadata: bool,
    #[serde(default = "default_true")]
    pub lock_production_endpoints: bool,
    #[serde(default = "default_true")]
    pub cooldown_on_429: bool,
    #[serde(default = "default_cooldown_duration")]
    pub cooldown_duration_secs: u64,
    #[serde(default = "default_jitter_percent")]
    pub cooldown_jitter_percent: u8,
    #[serde(default = "default_true")]
    pub request_jitter: bool,
    #[serde(default = "default_active_refresh_mins")]
    pub active_account_refresh_mins: u32,
    #[serde(default = "default_pool_refresh_mins")]
    pub pool_account_refresh_mins: u32,
    #[serde(default = "default_jitter_percent")]
    pub cadence_jitter_percent: u8,
    #[serde(default = "default_true")]
    pub show_logs_tab: bool,
    #[serde(default = "default_true")]
    pub show_stats_tab: bool,
    #[serde(default = "default_true")]
    pub show_about_tab: bool,
    #[serde(default = "default_true")]
    pub enable_billing_emulation: bool,
    #[serde(default = "default_true")]
    pub check_updates_startup: bool,
    #[serde(default)]
    pub clear_logs_on_exit: bool,
    #[serde(default = "default_master_api_key")]
    pub master_api_key: String,
    pub accounts: Vec<UpstreamAccount>,
    pub active_account_id: Option<String>,
    pub models: Vec<ModelCatalogItem>,
}

fn default_master_api_key() -> String {
    format!("sk-amr-{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
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

fn default_cooldown_duration() -> u64 {
    300 // 5 minutes default
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
    "Google Antigravity Client (1.107.0)".to_string()
}

fn default_protection_threshold() -> u8 {
    10
}

fn default_active_refresh_mins() -> u32 {
    2
}

fn default_pool_refresh_mins() -> u32 {
    15
}

fn default_jitter_percent() -> u8 {
    15
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
            quota_view_mode: "both".to_string(),
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
            quota_protection_threshold: 10,
            spoof_go_sdk: true,
            inject_ide_metadata: true,
            lock_production_endpoints: true,
            cooldown_on_429: true,
            cooldown_duration_secs: 300,
            cooldown_jitter_percent: 15,
            request_jitter: true,
            active_account_refresh_mins: 2,
            pool_account_refresh_mins: 15,
            cadence_jitter_percent: 15,
            show_logs_tab: true,
            show_stats_tab: true,
            show_about_tab: true,
            enable_billing_emulation: true,
            check_updates_startup: true,
            clear_logs_on_exit: false,
            master_api_key: "sk-amr-v1-master-key".to_string(),
            accounts: Vec::new(),
            active_account_id: None,
            models: vec![
                ModelCatalogItem {
                    id: "claude-sonnet-5-5".to_string(),
                    display_name: "Claude Sonnet 5.5".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "PRO".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-opus-5-5".to_string(),
                    display_name: "Claude Opus 5.5".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "PRO".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-sonnet-4-6".to_string(),
                    display_name: "Claude Sonnet 4.6".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "PRO".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "claude-opus-4-6".to_string(),
                    display_name: "Claude Opus 4.6".to_string(),
                    provider: "anthropic".to_string(),
                    tier: "PRO".to_string(),
                    is_enabled: true,
                    supports_thinking: false,
                },
                ModelCatalogItem {
                    id: "gemini-4-argon".to_string(),
                    display_name: "Gemini 4 Argon".to_string(),
                    provider: "gemini".to_string(),
                    tier: "PRO".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
                ModelCatalogItem {
                    id: "gemini-3.8-flash".to_string(),
                    display_name: "Gemini 3.8 Flash".to_string(),
                    provider: "gemini".to_string(),
                    tier: "PRO".to_string(),
                    is_enabled: true,
                    supports_thinking: true,
                },
            ],
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let local_cfg = Path::new("amr.json");
        if local_cfg.exists() {
            return local_cfg.to_path_buf();
        }
        let legacy_local_cfg = Path::new("aroxy.json");
        if legacy_local_cfg.exists() {
            return legacy_local_cfg.to_path_buf();
        }

        if let Some(cfg_dir) = dirs::config_dir() {
            let app_dir = cfg_dir.join("amr");
            if !app_dir.exists() {
                let legacy_dir = cfg_dir.join("aroxy");
                if legacy_dir.exists() {
                    return legacy_dir.join("config.json");
                }
            }
            let _ = fs::create_dir_all(&app_dir);
            app_dir.join("config.json")
        } else {
            local_cfg.to_path_buf()
        }
    }

    pub fn sanitize_models(&mut self) {
        self.models.retain(|m| {
            !m.id.starts_with("chat_")
                && !m.id.starts_with("tab_")
                && !m.id.contains("gemini-3.5")
                && !m.id.contains("gemini-3-flash")
                && !m.id.contains("gemini-3.7")
                && !m.id.contains("gemini-3.6")
                && !m.id.contains("gemini-3.1")
                && !m.id.contains("flash-lite")
                && !m.id.contains("lite")
                && !m.id.contains("preview")
                && !m.id.contains("gpt-oss")
                && !m.id.contains("tiered")
                && m.id != "gemini-3.9-flash"
                && m.id != "o1"
                && m.id != "gpt-4o"
                && m.id != "gpt-4o-mini"
                && m.id != "o3-mini"
                && m.id != "claude-3-5-sonnet"
                && m.id != "claude-3-7-sonnet"
                && m.id != "gemini-2.5-flash"
                && m.id != "gemini-2.5-flash-thinking"
                && m.id != "gemini-2.5-pro"
                && !m.id.ends_with("-low")
                && !m.id.ends_with("-medium")
                && !m.id.ends_with("-high")
                && !m.id.ends_with("-no-thinking")
                && !m.id.ends_with(" (low)")
                && !m.id.ends_with(" (medium)")
                && !m.id.ends_with(" (high)")
                && !m.id.ends_with(" (no-thinking)")
        });

        for m in &mut self.models {
            let (base_id, _) = crate::proxy::router::parse_model_and_reasoning(&m.id, None);
            m.id = base_id;

            if m.id == "claude-sonnet-5-5-high" || m.id == "claude-sonnet-5-5" || m.id == "claude-3-7-sonnet" {
                m.id = "claude-sonnet-5-5".to_string();
                m.display_name = "Claude Sonnet 5.5".to_string();
                m.provider = "anthropic".to_string();
                m.supports_thinking = true;
            } else if m.id == "claude-opus-5-5-high" || m.id == "claude-opus-5-5" {
                m.id = "claude-opus-5-5".to_string();
                m.display_name = "Claude Opus 5.5".to_string();
                m.provider = "anthropic".to_string();
                m.supports_thinking = true;
            } else if m.id == "claude-sonnet-4-6" {
                m.display_name = "Claude Sonnet 4.6".to_string();
                m.provider = "anthropic".to_string();
                m.supports_thinking = true;
            } else if m.id == "claude-opus-4-6-thinking" || m.id == "claude-opus-4-6" {
                m.id = "claude-opus-4-6".to_string();
                m.display_name = "Claude Opus 4.6".to_string();
                m.provider = "anthropic".to_string();
                m.supports_thinking = false;
            } else if m.id == "gemini-3.8-flash-high" || m.id == "gemini-3.8-flash-medium" || m.id == "gemini-3.8-flash-low" || m.id == "gemini-3.8-flash-tiered" || m.id == "gemini-2.5-flash-thinking" || m.id == "gemini-2.5-flash" || m.id == "gemini-3.8-flash" {
                m.id = "gemini-3.8-flash".to_string();
                m.display_name = "Gemini 3.8 Flash".to_string();
                m.provider = "gemini".to_string();
                m.supports_thinking = true;
            } else if m.id == "gemini-2.5-pro" || m.id == "gemini-4-argon" {
                m.id = "gemini-4-argon".to_string();
                m.display_name = "Gemini 4 Argon".to_string();
                m.provider = "gemini".to_string();
                m.supports_thinking = true;
            }
        }

        // Always ensure all 6 flagship models are present
        let defaults = Self::default().models;
        for def in defaults {
            if !self.models.iter().any(|m| m.id == def.id) {
                self.models.push(def);
            }
        }

        // Deduplicate models by ID
        let mut seen_ids = std::collections::HashSet::new();
        self.models.retain(|m| seen_ids.insert(m.id.clone()));
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&content) {
                    cfg.sanitize_models();

                    // Auto-recover accounts and ensure timestamp & tier consistency
                    let now_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    for acc in &mut cfg.accounts {
                        if acc.added_at.is_none() || acc.added_at.unwrap_or(0) < now_ts.saturating_sub(30 * 86400) || acc.added_at.unwrap_or(0) < 1740000000 {
                            acc.added_at = Some(now_ts);
                        }
                        if acc.claude_gpt_percent > 0 || acc.tier.to_lowercase() == "pro" {
                            acc.tier = "PRO".to_string();
                        }
                        if acc.device_profile.is_none() {
                            acc.device_profile = Some(DeviceProfile::generate());
                        }
                    }

                    // Ensure required core models exist
                    for def_m in Self::default().models {
                        if !cfg.models.iter().any(|m| m.id == def_m.id) {
                            cfg.models.push(def_m);
                        }
                    }

                    if cfg.models.is_empty() {
                        let defaults = Self::default();
                        cfg.models = defaults.models;
                        let _ = cfg.save();
                    }

                    // Normalize accounts status to uppercase, preserving ERROR state
                    for acc in &mut cfg.accounts {
                        if acc.status != "ERROR" {
                            acc.status = if acc.is_active { "ACTIVE".to_string() } else { "DISABLED".to_string() };
                        }
                    }

                    return cfg;
                }
            }
        }
        let default_cfg = Self::default();
        let _ = default_cfg.save();
        default_cfg
    }

    pub fn config_folder_info() -> (PathBuf, u64) {
        let path = Self::config_path();
        let folder = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let mut total_size = 0u64;
        if let Ok(entries) = fs::read_dir(&folder) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        total_size += meta.len();
                    }
                }
            }
        }
        (folder, total_size)
    }

    pub fn calculate_jittered_secs(base_mins: u32, jitter_percent: u8) -> u64 {
        let base_secs = (base_mins.max(1) as u64) * 60;
        if jitter_percent == 0 {
            return base_secs;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let mut x = nanos ^ (nanos << 13) ^ (nanos >> 17) ^ (nanos << 5);
        if x == 0 { x = 1234567; }
        let range = (jitter_percent as u32 * 2) + 1;
        let pct_offset = ((x % range) as i32) - (jitter_percent as i32);
        let factor = 1.0 + (pct_offset as f64 / 100.0);
        let jittered = ((base_secs as f64) * factor).round() as u64;
        jittered.max(20)
    }

    pub fn random_delay_ms(min_ms: u64, max_ms: u64) -> u64 {
        if min_ms >= max_ms {
            return min_ms;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let mut x = nanos ^ (nanos << 7) ^ (nanos >> 11) ^ (nanos << 3);
        if x == 0 { x = 7654321; }
        min_ms + ((x as u64) % (max_ms - min_ms + 1))
    }

    pub fn localize_reset_str(raw: &str, is_ru: bool, is_zh: bool) -> String {
        let clean = raw.trim()
            .trim_start_matches("Reset: in ")
            .trim_start_matches("Reset: ")
            .trim_start_matches("in ")
            .trim_start_matches("через ")
            .trim_start_matches("约 ")
            .trim();
        if is_ru {
            format!("через {}", clean.replace('d', "д").replace('h', "ч").replace('m', "м"))
        } else if is_zh {
            clean.replace('d', "天").replace('h', "小时").replace('m', "分")
        } else {
            format!("in {}", clean)
        }
    }

    pub fn format_quota_label(&self, pct: i32) -> String {
        let lang = Language::from_str_code(&self.language);
        let t = get_translations(lang);
        if self.quota_display_mode == "fraction" {
            let frac = (pct as f64) / 100.0;
            format!("{:.2} {}", frac, t.lbl_remaining)
        } else {
            format!("{}% {}", pct, t.lbl_available)
        }
    }

    pub fn format_claude_quota(&self) -> String {
        self.format_quota_label(self.aggregate_claude_gpt_percent())
    }

    pub fn format_gemini_quota(&self) -> String {
        self.format_quota_label(self.aggregate_gemini_percent())
    }

    pub fn format_claude_weekly_quota(&self) -> String {
        self.format_quota_label(self.aggregate_claude_gpt_weekly_percent())
    }

    pub fn format_gemini_weekly_quota(&self) -> String {
        self.format_quota_label(self.aggregate_gemini_weekly_percent())
    }

    pub fn aggregate_claude_gpt_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 100;
        }
        let sum: i32 = active.iter().map(|a| a.claude_gpt_percent).sum();
        sum / active.len() as i32
    }

    pub fn aggregate_claude_gpt_weekly_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 100;
        }
        let sum: i32 = active.iter().map(|a| a.claude_gpt_weekly_percent).sum();
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

    pub fn aggregate_gemini_weekly_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 100;
        }
        let sum: i32 = active.iter().map(|a| a.gemini_weekly_percent).sum();
        sum / active.len() as i32
    }

    pub fn nearest_claude_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.claude_gpt_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| {
                if is_ru {
                    "4ч 00м".to_string()
                } else if is_zh {
                    "4小时00分".to_string()
                } else {
                    "4h 00m".to_string()
                }
            })
    }

    pub fn nearest_claude_weekly_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.claude_gpt_weekly_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| {
                if is_ru {
                    "5д 17ч".to_string()
                } else if is_zh {
                    "5天17小时".to_string()
                } else {
                    "5d 17h".to_string()
                }
            })
    }

    pub fn nearest_gemini_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.gemini_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| {
                if is_ru {
                    "4ч 00м".to_string()
                } else if is_zh {
                    "4小时00分".to_string()
                } else {
                    "4h 00m".to_string()
                }
            })
    }

    pub fn nearest_gemini_weekly_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.gemini_weekly_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| {
                if is_ru {
                    "5д 17ч".to_string()
                } else if is_zh {
                    "5天17小时".to_string()
                } else {
                    "5d 17h".to_string()
                }
            })
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
        let mut seen_ids = std::collections::HashSet::new();

        for m in &self.models {
            if !m.is_enabled {
                continue;
            }
            let (base_id, _) = crate::proxy::router::parse_model_and_reasoning(&m.id, None);
            if !seen_ids.insert(base_id.clone()) {
                continue;
            }

            // Base model enriched with reasoning and capability metadata
            let mut model_json = serde_json::json!({
                "id": base_id,
                "object": "model",
                "created": 1700000000,
                "owned_by": m.provider,
                "display_name": m.display_name,
                "tier": m.tier,
                "context_window": 1000000,
                "max_tokens": 65536,
                "input_modalities": ["text", "image"],
                "capabilities": {
                    "chat_completions": true,
                    "reasoning": m.supports_thinking,
                    "thinking": m.supports_thinking,
                    "images": true
                },
                "supported_parameters": [
                    "reasoning_effort",
                    "temperature",
                    "max_tokens",
                    "max_completion_tokens",
                    "thinking",
                    "tools"
                ]
            });

            if m.supports_thinking {
                model_json["reasoning"] = serde_json::json!({
                    "mandatory": false,
                    "default_enabled": true,
                    "default_effort": "medium",
                    "supported_efforts": ["high", "medium", "low", "none"]
                });
            }

            list.push(model_json);

            // If reasoning clones are NOT grouped into base models, expose clones
            if !self.group_reasoning_clones && self.expose_reasoning_variants && m.supports_thinking {
                for (suffix, label) in [
                    (" (low)", "Low Effort"),
                    (" (medium)", "Medium Effort"),
                    (" (high)", "High Effort"),
                    (" (no-thinking)", "Thinking Off"),
                ] {
                    let clone_id = format!("{}{}", base_id, suffix);
                    if seen_ids.insert(clone_id.clone()) {
                        list.push(serde_json::json!({
                            "id": clone_id,
                            "object": "model",
                            "created": 1700000000,
                            "owned_by": m.provider,
                            "display_name": format!("{} [{}]", m.display_name, label),
                            "tier": m.tier
                        }));
                    }
                }
            }
        }
        list
    }
}
