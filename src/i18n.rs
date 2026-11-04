use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Ru,
    Zh,
    En,
}

impl Language {
    pub fn from_str_code(code: &str) -> Self {
        match code.to_lowercase().as_str() {
            "ru" => Language::Ru,
            "zh" => Language::Zh,
            _ => Language::En,
        }
    }

    pub fn to_str_code(self) -> &'static str {
        match self {
            Language::Ru => "ru",
            Language::Zh => "zh",
            Language::En => "en",
        }
    }

    pub fn to_code(self) -> &'static str {
        self.to_str_code()
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Language::Ru,
            2 => Language::Zh,
            _ => Language::En,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Language::En => 0,
            Language::Ru => 1,
            Language::Zh => 2,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct TranslationBundle {
    pub tab_dashboard: String,
    pub tab_accounts: String,
    pub tab_models: String,
    pub tab_proxy: String,
    pub tab_logs: String,
    pub tab_stats: String,
    pub tab_settings: String,
    pub tab_about: String,
    pub btn_stop_proxy: String,
    pub btn_start_proxy: String,
    pub dash_title: String,
    pub dash_subtitle: String,
    pub stat_requests: String,
    pub stat_tokens: String,
    pub stat_latency: String,
    pub stat_accounts: String,
    pub stat_app_ram: String,
    pub stat_proxy_engine: String,
    pub quota_claude_title: String,
    pub quota_claude_desc: String,
    pub quota_gemini_title: String,
    pub quota_gemini_desc: String,
    pub acc_title: String,
    pub acc_subtitle: String,
    pub btn_login_oauth: String,
    pub btn_add_token: String,
    pub acc_placeholder: String,
    pub acc_pool_header: String,
    pub btn_update: String,
    pub btn_info: String,
    pub btn_delete: String,
    pub btn_enable: String,
    pub btn_disable: String,
    pub mod_title: String,
    pub mod_subtitle: String,
    pub group_reasoning_title: String,
    pub group_reasoning_desc: String,
    pub btn_test_dialog: String,
    pub test_dialog_title: String,
    pub test_prompt_label: String,
    pub test_timeout_label: String,
    pub test_max_tokens_label: String,
    pub test_stream_toggle: String,
    pub btn_send_test: String,
    pub btn_cancel_test: String,
    pub btn_close: String,
    pub proxy_title: String,
    pub proxy_subtitle: String,
    pub sec_network: String,
    pub sec_routing: String,
    pub sec_cache: String,
    pub sec_timeouts: String,
    pub lbl_port: String,
    pub lbl_host: String,
    pub lbl_timeout: String,
    pub lbl_heartbeat: String,
    pub lbl_failover: String,
    pub logs_title: String,
    pub logs_subtitle: String,
    pub btn_clear_logs: String,
    pub lbl_enable_logging: String,
    pub lbl_max_logs: String,
    pub stats_title: String,
    pub stats_subtitle: String,
    pub sett_title: String,
    pub sett_subtitle: String,
    pub sec_visual: String,
    pub lbl_language: String,
    pub lbl_quota_mode: String,
    pub opt_quota_percent: String,
    pub opt_quota_fraction: String,
    pub lbl_hide_brand: String,
    pub sec_system: String,
    pub lbl_tray: String,
    pub lbl_autostart: String,
    pub lbl_portable: String,
    pub btn_save_settings: String,
    pub lbl_show_logs: String,
    pub lbl_show_stats: String,
    pub lbl_hide_about: String,
    pub lbl_hide_about_desc: String,
    pub lbl_added_at: String,
    pub lbl_check_updates: String,
    pub lbl_clear_logs_exit: String,
    pub lbl_quota_protection: String,
    pub lbl_active_refresh: String,
    pub lbl_pool_refresh: String,
    pub info_error_label: String,
    pub about_title: String,
    pub about_subtitle: String,
    pub lbl_app_folder: String,
    pub btn_open_folder: String,
    pub lbl_quota_view_mode: String,
    pub opt_quota_both: String,
    pub opt_quota_5h: String,
    pub opt_quota_weekly: String,
    pub lbl_endpoints_title: String,
    pub lbl_endpoints_protocol: String,
    pub lbl_endpoints_path: String,
    pub lbl_endpoints_models: String,
    pub lbl_master_key_title: String,
    pub lbl_master_key_desc: String,
    pub btn_copy_key: String,
    pub btn_regenerate_key: String,
    pub lbl_quota_protection_desc: String,
    pub lbl_failover_desc: String,
    pub opt_cache_first: String,
    pub opt_cache_balanced: String,
    pub opt_cache_spread: String,
    pub lbl_status_active: String,
    pub lbl_status_error: String,
    pub lbl_status_disabled: String,
    pub lbl_modal_claude_quota: String,
    pub lbl_modal_gemini_quota: String,
    pub lbl_modal_5h: String,
    pub lbl_modal_week: String,
    pub lbl_modal_fingerprint: String,
    pub lbl_modal_endpoint: String,
    pub lbl_modal_models: String,
    pub lbl_modal_error_details: String,
    pub lbl_updating: String,
    pub lbl_updated: String,
    pub lbl_update_failed: String,
    pub lbl_active_refresh_desc: String,
    pub lbl_pool_refresh_desc: String,
    pub lbl_modal_machine_id: String,
    pub lbl_modal_device_id: String,
    pub lbl_modal_proxy_url: String,
    pub lbl_modal_proxy_placeholder: String,
    pub btn_save_proxy: String,
    pub btn_regenerate_fp: String,
    pub lbl_direct_proxy: String,
}

pub fn get_translations(lang: Language) -> TranslationBundle {
    let json_str = match lang {
        Language::Ru => include_str!("../i18n/ru.json"),
        Language::Zh => include_str!("../i18n/zh.json"),
        Language::En => include_str!("../i18n/en.json"),
    };
    let clean = json_str.trim_start_matches('\u{feff}');
    serde_json::from_str(clean).unwrap_or_default()
}
