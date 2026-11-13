use crate::i18n::{get_translations, Language};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
    CacheFirst, // Maximum KV cache reuse: pins session strictly to account until rate-limited or depleted
    #[default]
    Balanced, // Hybrid: sticky session, but rotates smoothly if other accounts have > threshold more quota
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
    #[serde(default = "default_availability_status")]
    pub availability_status: String, // "available", "disabled", "exhausted", "notice"
    #[serde(default = "default_pct_100")]
    pub quota_percent: i32,
    #[serde(default)]
    pub notice: String,
    #[serde(default)]
    pub available_accounts_count: usize,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl Default for ModelCatalogItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            display_name: String::new(),
            provider: "gemini".to_string(),
            tier: default_tier(),
            is_enabled: true,
            supports_thinking: false,
            availability_status: default_availability_status(),
            quota_percent: 100,
            notice: String::new(),
            available_accounts_count: 0,
            max_tokens: default_max_tokens(),
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

fn default_tier() -> String {
    "Pro".to_string()
}

fn default_availability_status() -> String {
    "available".to_string()
}

fn default_max_tokens() -> u32 {
    1_000_000
}

fn default_max_output_tokens() -> u32 {
    65_536
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountModelStatus {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default = "default_fraction_one")]
    pub remaining_fraction: f64,
    #[serde(default)]
    pub reset_time: Option<String>,
    #[serde(default)]
    pub supports_thinking: bool,
    #[serde(default)]
    pub supports_images: bool,
    #[serde(default)]
    pub is_internal: bool,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub notice: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_modality_chat")]
    pub modality: String, // "chat", "image", "search"
    #[serde(default)]
    pub is_recommended_agent: bool,
    #[serde(default = "default_sort_order")]
    pub sort_order: usize,
}

fn default_modality_chat() -> String {
    "chat".to_string()
}

fn default_sort_order() -> usize {
    usize::MAX
}

fn default_fraction_one() -> f64 {
    1.0
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
    pub tier: String, // "Free", "Pro", "Team"
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
    pub status: String, // "ACTIVE", "DISABLED", "ERROR"
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
    #[serde(default)]
    pub available_models: Vec<AccountModelStatus>,
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
        let machine_id = format!("{}{}", random_hex_32, uuid::Uuid::new_v4().simple());
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
            available_models: Vec::new(),
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

pub const VERSION_AMR_RELEASE: &str = "2.23.1";
pub const VERSION_ANTIGRAVITY_LATEST: &str = "2.29.1";

fn default_spoof_platform() -> String {
    "AUTO".to_string()
}

fn default_spoof_ide_version() -> String {
    VERSION_ANTIGRAVITY_LATEST.to_string()
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
    pub quota_display_mode: String, // "percent" or "fraction"
    #[serde(default = "default_quota_view_mode")]
    pub quota_view_mode: String, // "both", "5hour", "weekly"
    #[serde(default)]
    pub hide_sidebar_brand: bool, // Collapses top brand in sidebar
    #[serde(default = "default_language")]
    pub language: String, // "en", "ru", "zh"
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
    #[serde(default = "default_spoof_platform")]
    pub spoof_platform: String,
    #[serde(default = "default_spoof_ide_version")]
    pub spoof_ide_version: String,
    #[serde(default = "default_spoof_arch")]
    pub spoof_arch: String,
    #[serde(default = "default_spoof_channel")]
    pub spoof_channel: String,
    #[serde(default = "default_true")]
    pub strip_telemetry_traces: bool,
    #[serde(default = "default_true")]
    pub auto_rotate_fp_on_limit: bool,
    #[serde(default = "default_true")]
    pub normalize_headers: bool,
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
    #[serde(default)]
    pub default_agent_model: Option<String>,
    #[serde(default)]
    pub image_generation_models: Vec<String>,
    #[serde(default)]
    pub web_search_models: Vec<String>,
}

fn default_master_api_key() -> String {
    format!(
        "sk-amr-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
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

fn default_spoof_arch() -> String {
    "AUTO".to_string()
}

fn default_spoof_channel() -> String {
    "stable".to_string()
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
            spoof_platform: "AUTO".to_string(),
            spoof_ide_version: VERSION_ANTIGRAVITY_LATEST.to_string(),
            spoof_arch: "AUTO".to_string(),
            spoof_channel: "stable".to_string(),
            strip_telemetry_traces: true,
            auto_rotate_fp_on_limit: true,
            normalize_headers: true,
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
            models: Vec::new(),
            default_agent_model: None,
            image_generation_models: Vec::new(),
            web_search_models: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn resolved_platform(&self) -> &'static str {
        match self.spoof_platform.to_uppercase().as_str() {
            "LINUX" => "LINUX",
            "WINDOWS" => "WINDOWS",
            "MACOS" | "DARWIN" => "MACOS",
            _ => match std::env::consts::OS {
                "windows" => "WINDOWS",
                "macos" => "MACOS",
                _ => "LINUX",
            },
        }
    }
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
        let has_account_models = self
            .accounts
            .iter()
            .any(|a| a.is_active && !a.available_models.is_empty());

        if has_account_models {
            self.rebuild_models_from_accounts();
            return;
        }

        // Remove only purely internal IDE models
        self.models.retain(|m| {
            !m.id.starts_with("chat_") && !m.id.starts_with("tab_")
        });

        let mut seen_ids = std::collections::HashSet::new();
        self.models.retain(|m| seen_ids.insert(m.id.clone()));
    }

    /// Strip known reasoning/effort suffixes algorithmically without hardcoding model names.
    pub fn strip_effort_suffix(raw_id: &str) -> (&str, Option<&'static str>) {
        for (suffix, eff) in [
            ("-thinking", "thinking"),
            ("-minimal", "minimal"),
            ("-low", "low"),
            ("-medium", "medium"),
            ("-high", "high"),
            ("-none", "none"),
            ("-off", "off"),
        ] {
            if let Some(base) = raw_id.strip_suffix(suffix) {
                if !base.is_empty() {
                    return (base, Some(eff));
                }
            }
        }
        (raw_id, None)
    }

    /// Normalize an upstream model ID into a clean base ID when grouping is enabled.
    pub fn canonical_catalog_id(raw_id: &str) -> String {
        Self::strip_effort_suffix(raw_id).0.to_string()
    }

    /// Rebuild `self.models` dynamically from real per-account `available_models`
    /// while preserving user `is_enabled` toggles and respecting `group_reasoning_clones`.
    pub fn rebuild_models_from_accounts(&mut self) {
        let prev_enabled: std::collections::HashMap<String, bool> = self
            .models
            .iter()
            .map(|m| (m.id.clone(), m.is_enabled))
            .collect();

        let active_accounts: Vec<&UpstreamAccount> =
            self.accounts.iter().filter(|a| a.is_active).collect();

        if !active_accounts
            .iter()
            .any(|a| !a.available_models.is_empty())
        {
            return;
        }

        // Check whether upstream provided curated user-facing lists (agentModelSorts / imageGenerationModelIds)
        let has_curated = active_accounts.iter().any(|a| {
            a.available_models
                .iter()
                .any(|m| m.is_recommended_agent || m.modality == "image")
        });

        // Count how many distinct upstream IDs map to each stripped base (so singletons like gpt-oss-120b-medium keep their real ID)
        let mut base_variants: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        for acc in &active_accounts {
            for am in &acc.available_models {
                if am.is_internal || am.id.starts_with("chat_") || am.id.starts_with("tab_") {
                    continue;
                }
                if has_curated && !am.is_recommended_agent && am.modality != "image" {
                    continue;
                }
                let (base, _) = Self::strip_effort_suffix(&am.id);
                base_variants
                    .entry(base.to_string())
                    .or_default()
                    .insert(am.id.clone());
            }
        }

        struct AggModel {
            id: String,
            display_name: String,
            provider: String,
            tier: String,
            supports_thinking: bool,
            any_enabled: bool,
            any_quota: bool,
            best_pct: i32,
            accounts_ok: std::collections::HashSet<String>,
            notices: Vec<String>,
            max_tokens: u32,
            max_output_tokens: u32,
            sort_order: usize,
            modality: String,
        }

        let mut agg: std::collections::BTreeMap<String, AggModel> = std::collections::BTreeMap::new();

        for acc in &active_accounts {
            for am in &acc.available_models {
                if am.is_internal || am.id.starts_with("chat_") || am.id.starts_with("tab_") {
                    continue;
                }
                if has_curated && !am.is_recommended_agent && am.modality != "image" {
                    continue;
                }

                let (base, eff) = Self::strip_effort_suffix(&am.id);
                let should_group = self.group_reasoning_clones
                    && (eff == Some("thinking")
                        || base_variants.get(base).is_some_and(|s| s.len() > 1));

                let cid = if should_group {
                    base.to_string()
                } else {
                    am.id.clone()
                };

                let clean_display = if should_group {
                    let mut d = if am.display_name.is_empty() {
                        cid.clone()
                    } else {
                        am.display_name.clone()
                    };
                    for tag in [" (High)", " (Medium)", " (Low)", " (Minimal)", " (Thinking)"] {
                        if let Some(stripped) = d.strip_suffix(tag) {
                            d = stripped.to_string();
                            break;
                        }
                    }
                    d
                } else if am.display_name.is_empty() {
                    cid.clone()
                } else {
                    am.display_name.clone()
                };

                let pct = (am.remaining_fraction * 100.0).round().clamp(0.0, 100.0) as i32;
                let ok_here = !am.disabled && am.remaining_fraction > 0.0;

                let entry = agg.entry(cid.clone()).or_insert_with(|| AggModel {
                    id: cid.clone(),
                    display_name: clean_display,
                    provider: am.provider.clone(),
                    tier: acc.tier.clone(),
                    supports_thinking: am.supports_thinking,
                    any_enabled: false,
                    any_quota: false,
                    best_pct: 0,
                    accounts_ok: std::collections::HashSet::new(),
                    notices: Vec::new(),
                    max_tokens: am.max_tokens,
                    max_output_tokens: am.max_output_tokens,
                    sort_order: am.sort_order,
                    modality: am.modality.clone(),
                });

                entry.sort_order = entry.sort_order.min(am.sort_order);
                entry.supports_thinking |= am.supports_thinking;
                entry.max_tokens = entry.max_tokens.max(am.max_tokens);
                entry.max_output_tokens = entry.max_output_tokens.max(am.max_output_tokens);
                if !am.disabled {
                    entry.any_enabled = true;
                    entry.best_pct = entry.best_pct.max(pct);
                }
                if ok_here {
                    entry.any_quota = true;
                    entry.accounts_ok.insert(acc.id.clone());
                }
                if let Some(n) = &am.notice {
                    if !n.is_empty() && !entry.notices.contains(n) {
                        entry.notices.push(n.clone());
                    }
                }
            }
        }

        let mut rebuilt: Vec<(usize, ModelCatalogItem)> = agg
            .into_values()
            .map(|a| {
                let notice = a.notices.join(" | ");
                let availability_status = if !a.any_enabled {
                    "disabled".to_string()
                } else if !a.any_quota {
                    "exhausted".to_string()
                } else if !notice.is_empty() {
                    "notice".to_string()
                } else {
                    "available".to_string()
                };
                let is_enabled = prev_enabled.get(&a.id).copied().unwrap_or(true);
                let order = if a.modality == "image" {
                    usize::MAX - 10
                } else {
                    a.sort_order
                };
                (
                    order,
                    ModelCatalogItem {
                        id: a.id,
                        display_name: a.display_name,
                        provider: a.provider,
                        tier: a.tier,
                        is_enabled,
                        supports_thinking: a.supports_thinking,
                        availability_status,
                        quota_percent: if a.any_enabled { a.best_pct } else { 0 },
                        notice,
                        available_accounts_count: a.accounts_ok.len(),
                        max_tokens: a.max_tokens,
                        max_output_tokens: a.max_output_tokens,
                    },
                )
            })
            .collect();

        // Append AMR virtual utility endpoints backed by active accounts (Audio STT & Audio TTS)
        let active_count = active_accounts.len();
        for (idx, (vid, vname, vprov)) in [
            ("whisper-1", "Whisper 1 (Audio STT)", "openai"),
            ("tts-1", "TTS 1 (Audio Speech)", "openai"),
        ]
        .into_iter()
        .enumerate()
        {
            let is_enabled = prev_enabled.get(vid).copied().unwrap_or(true);
            rebuilt.push((
                usize::MAX - 5 + idx,
                ModelCatalogItem {
                    id: vid.to_string(),
                    display_name: vname.to_string(),
                    provider: vprov.to_string(),
                    tier: "PRO".to_string(),
                    is_enabled,
                    supports_thinking: false,
                    availability_status: "available".to_string(),
                    quota_percent: 100,
                    notice: String::new(),
                    available_accounts_count: active_count,
                    max_tokens: 1_048_576,
                    max_output_tokens: 65_536,
                },
            ));
        }

        rebuilt.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
        self.models = rebuilt.into_iter().map(|(_, m)| m).collect();
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
                        if acc.added_at.is_none()
                            || acc.added_at.unwrap_or(0) < now_ts.saturating_sub(30 * 86400)
                            || acc.added_at.unwrap_or(0) < 1740000000
                        {
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
                            acc.status = if acc.is_active {
                                "ACTIVE".to_string()
                            } else {
                                "DISABLED".to_string()
                            };
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
        let folder = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
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
        if x == 0 {
            x = 1234567;
        }
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
        if x == 0 {
            x = 7654321;
        }
        min_ms + ((x as u64) % (max_ms - min_ms + 1))
    }

    pub fn localize_reset_str(raw: &str, is_ru: bool, is_zh: bool) -> String {
        let clean = raw
            .trim()
            .trim_start_matches("Reset: in ")
            .trim_start_matches("Reset: ")
            .trim_start_matches("in ")
            .trim_start_matches("через ")
            .trim_start_matches("约 ")
            .trim();
        if is_ru {
            format!(
                "через {}",
                clean.replace('d', "д").replace('h', "ч").replace('m', "м")
            )
        } else if is_zh {
            clean
                .replace('d', "天")
                .replace('h', "小时")
                .replace('m', "分")
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
            return 0;
        }
        let sum: i32 = active.iter().map(|a| a.claude_gpt_percent).sum();
        sum / active.len() as i32
    }

    pub fn aggregate_claude_gpt_weekly_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 0;
        }
        let sum: i32 = active.iter().map(|a| a.claude_gpt_weekly_percent).sum();
        sum / active.len() as i32
    }

    pub fn aggregate_gemini_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 0;
        }
        let sum: i32 = active.iter().map(|a| a.gemini_percent).sum();
        sum / active.len() as i32
    }

    pub fn aggregate_gemini_weekly_percent(&self) -> i32 {
        let active: Vec<_> = self.accounts.iter().filter(|a| a.is_active).collect();
        if active.is_empty() {
            return 0;
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
            .unwrap_or_else(|| "—".to_string())
    }

    pub fn nearest_claude_weekly_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.claude_gpt_weekly_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| "—".to_string())
    }

    pub fn nearest_gemini_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.gemini_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| "—".to_string())
    }

    pub fn nearest_gemini_weekly_reset(&self) -> String {
        let is_ru = self.language.eq_ignore_ascii_case("ru");
        let is_zh = self.language.eq_ignore_ascii_case("zh");
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| Self::localize_reset_str(&a.gemini_weekly_reset, is_ru, is_zh))
            .next()
            .unwrap_or_else(|| "—".to_string())
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = if self.portable_mode {
            PathBuf::from("amr.json")
        } else {
            Self::config_path()
        };
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
                    self.accounts
                        .iter()
                        .find(|a| &a.id == id && a.is_active)
                        .or_else(|| self.accounts.iter().find(|a| a.is_active))
                } else {
                    self.accounts.iter().find(|a| a.is_active)
                }
            }
        }
    }

    pub fn total_quota_used(&self) -> u64 {
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| a.quota_used)
            .sum()
    }

    pub fn total_quota_limit(&self) -> u64 {
        self.accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| a.quota_limit)
            .sum()
    }

    pub fn get_broadcast_models(&self) -> Vec<serde_json::Value> {
        let mut list = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        for m in &self.models {
            if !m.is_enabled || m.availability_status == "disabled" {
                continue;
            }
            let (base_id, _) = if self.group_reasoning_clones {
                crate::proxy::router::parse_model_and_reasoning(&m.id, None)
            } else {
                (m.id.clone(), None)
            };
            if !seen_ids.insert(base_id.clone()) {
                continue;
            }

            let owned_by = if m.provider == "gemini" {
                "google"
            } else {
                m.provider.as_str()
            };

            // Base model enriched with modality, reasoning and capability metadata
            let lower_id = base_id.to_lowercase();
            let is_image = lower_id.contains("image") || lower_id.contains("dall-e");
            let is_stt = lower_id.contains("whisper") || lower_id.contains("transcrib");
            let is_tts = lower_id.contains("tts");
            let is_search = lower_id.contains("web-search") || lower_id.contains("google-search");

            let (input_modalities, output_modalities, capabilities, supported_parameters) =
                if is_image {
                    (
                        serde_json::json!(["text", "image"]),
                        serde_json::json!(["image", "text"]),
                        serde_json::json!({
                            "chat_completions": true,
                            "image_generation": true,
                            "images": true,
                            "reasoning": false,
                            "thinking": false,
                            "reasoning_effort": false,
                            "reasoningEffort": false,
                            "thinking_budget": false,
                            "max_thinking_budget": false
                        }),
                        serde_json::json!(["prompt", "aspect_ratio", "size", "n", "response_format", "temperature"]),
                    )
                } else if is_stt {
                    (
                        serde_json::json!(["audio"]),
                        serde_json::json!(["text"]),
                        serde_json::json!({
                            "chat_completions": true,
                            "audio_transcription": true,
                            "speech_to_text": true,
                            "reasoning": false,
                            "thinking": false,
                            "reasoning_effort": false,
                            "reasoningEffort": false,
                            "thinking_budget": false,
                            "max_thinking_budget": false,
                            "images": false
                        }),
                        serde_json::json!(["file", "model", "prompt", "language", "response_format"]),
                    )
                } else if is_tts {
                    (
                        serde_json::json!(["text"]),
                        serde_json::json!(["audio"]),
                        serde_json::json!({
                            "chat_completions": true,
                            "audio_speech": true,
                            "text_to_speech": true,
                            "reasoning": false,
                            "thinking": false,
                            "reasoning_effort": false,
                            "reasoningEffort": false,
                            "thinking_budget": false,
                            "max_thinking_budget": false,
                            "images": false
                        }),
                        serde_json::json!(["input", "model", "voice", "response_format", "speed"]),
                    )
                } else if is_search {
                    (
                        serde_json::json!(["text"]),
                        serde_json::json!(["text"]),
                        serde_json::json!({
                            "chat_completions": true,
                            "web_search": true,
                            "google_search": true,
                            "grounding": true,
                            "reasoning": false,
                            "thinking": false,
                            "reasoning_effort": false,
                            "reasoningEffort": false,
                            "thinking_budget": false,
                            "max_thinking_budget": false,
                            "images": false
                        }),
                        serde_json::json!(["query", "domain", "temperature", "max_tokens", "tools"]),
                    )
                } else {
                    (
                        serde_json::json!(["text", "image"]),
                        serde_json::json!(["text"]),
                        serde_json::json!({
                            "chat_completions": true,
                            "web_search": true,
                            "reasoning": m.supports_thinking,
                            "thinking": m.supports_thinking,
                            "reasoning_effort": m.supports_thinking,
                            "reasoningEffort": m.supports_thinking,
                            "thinking_budget": m.supports_thinking,
                            "max_thinking_budget": m.supports_thinking,
                            "images": true
                        }),
                        serde_json::json!([
                            "reasoning_effort",
                            "reasoningEffort",
                            "temperature",
                            "max_tokens",
                            "max_completion_tokens",
                            "thinking",
                            "thinking_budget",
                            "budget_tokens",
                            "max_thinking_budget",
                            "reasoning_budget",
                            "tools"
                        ]),
                    )
                };

            let mut model_json = serde_json::json!({
                "id": base_id,
                "object": "model",
                "created": 1700000000,
                "owned_by": owned_by,
                "display_name": m.display_name,
                "tier": m.tier,
                "context_window": m.max_tokens,
                "max_tokens": m.max_output_tokens,
                "availability_status": m.availability_status,
                "quota_percent": m.quota_percent,
                "available_accounts": m.available_accounts_count,
                "input_modalities": input_modalities,
                "output_modalities": output_modalities,
                "capabilities": capabilities,
                "supported_parameters": supported_parameters
            });

            if m.supports_thinking {
                model_json["reasoning_effort"] = serde_json::json!("medium");
                model_json["supported_reasoning_efforts"] =
                    serde_json::json!(["low", "medium", "high", "none"]);
                model_json["reasoning_efforts"] =
                    serde_json::json!(["low", "medium", "high", "none"]);
                model_json["reasoningEfforts"] = serde_json::json!({
                    "low": "low",
                    "medium": "medium",
                    "high": "high",
                    "none": "none"
                });
                model_json["max_thinking_budget"] = serde_json::json!(32768);
                model_json["min_thinking_budget"] = serde_json::json!(0);
                model_json["default_thinking_budget"] = serde_json::json!(4000);
                model_json["thinking_budget"] = serde_json::json!(4000);
                model_json["thinking"] = serde_json::json!({
                    "supported": true,
                    "type": "enabled",
                    "budget_tokens": 4000,
                    "min_budget": 0,
                    "max_budget": 32768,
                    "default_budget": 4000
                });
                model_json["reasoning"] = serde_json::json!({
                    "mandatory": false,
                    "default_enabled": true,
                    "default_effort": "medium",
                    "supported_efforts": ["high", "medium", "low", "none"],
                    "supported_reasoning_efforts": ["high", "medium", "low", "none"],
                    "reasoning_efforts": ["high", "medium", "low", "none"],
                    "min_budget": 0,
                    "max_budget": 32768,
                    "default_budget": 4000
                });
            }

            list.push(model_json);

            // If reasoning clones are NOT grouped into base models, expose clones
            if !self.group_reasoning_clones && self.expose_reasoning_variants && m.supports_thinking
            {
                for (suffix, label) in [
                    (" (none)", "none"),
                    (" (low)", "low"),
                    (" (medium)", "medium"),
                    (" (high)", "high"),
                    (" (no-thinking)", "no-thinking"),
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
