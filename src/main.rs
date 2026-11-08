#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

use amr::autostart::set_autostart;
use amr::cli::CliArgs;
use amr::config::{AppConfig, RoutingStrategy, UpstreamAccount};
use amr::i18n::{get_translations, Language};
use amr::proxy::antigravity;
use amr::proxy::server::{create_proxy_router, AppState};
use amr::proxy::stats::ProxyStats;
use amr::sys_metrics::get_process_memory_string;
use amr::tray::{SystemTray, TrayAction};
use clap::Parser;
use parking_lot::RwLock;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

fn open_in_system(target: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = std::ffi::OsStr::new(target).encode_wide().collect();
        wide.push(0);
        let mut operation: Vec<u16> = std::ffi::OsStr::new("open").encode_wide().collect();
        operation.push(0);
        let res = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
            )
        };
        if res as usize <= 32 {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", target])
                .spawn();
        }
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
}

pub fn show_and_focus_window(w: &MainWindow) {
    w.window().set_minimized(false);
    let _ = w.show();
    w.window().request_redraw();
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
        };
        let title: Vec<u16> = "AMR — Antigravity Manager Rust\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            BringWindowToTop(hwnd);
        }
    }
}

pub fn hide_window_to_tray(w: &MainWindow) {
    w.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
    let _ = w.hide();
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        EmptyWorkingSet(GetCurrentProcess());
    }
}

fn format_elapsed_time(added_at: u64, lang: Language) -> String {
    if added_at == 0 {
        return match lang {
            Language::Ru => "только что".to_string(),
            Language::Zh => "刚刚".to_string(),
            Language::En => "just now".to_string(),
        };
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(added_at);
    let diff = now.saturating_sub(added_at);
    if diff < 60 {
        match lang {
            Language::Ru => "только что".to_string(),
            Language::Zh => "刚刚".to_string(),
            Language::En => "just now".to_string(),
        }
    } else if diff < 3600 {
        let mins = diff / 60;
        match lang {
            Language::Ru => format!("{} мин. назад", mins),
            Language::Zh => format!("{} 分钟前", mins),
            Language::En => format!("{}m ago", mins),
        }
    } else if diff < 86400 {
        let hours = diff / 3600;
        match lang {
            Language::Ru => format!("{} ч. назад", hours),
            Language::Zh => format!("{} 小时前", hours),
            Language::En => format!("{}h ago", hours),
        }
    } else {
        let days = diff / 86400;
        match lang {
            Language::Ru => format!("{} дн. назад", days),
            Language::Zh => format!("{} 天前", days),
            Language::En => format!("{}d ago", days),
        }
    }
}

fn arc_svg_path(cx: f64, cy: f64, r: f64, start_deg: f64, end_deg: f64) -> String {
    let sweep = end_deg - start_deg;
    if sweep <= 0.5 {
        return String::new();
    }
    let sweep = sweep.clamp(0.5, 359.8);
    let s_rad = (start_deg - 90.0).to_radians();
    let e_rad = ((start_deg + sweep) - 90.0).to_radians();
    let x1 = cx + r * s_rad.cos();
    let y1 = cy + r * s_rad.sin();
    let x2 = cx + r * e_rad.cos();
    let y2 = cy + r * e_rad.sin();
    let large_arc = if sweep > 180.0 { 1 } else { 0 };
    format!(
        "M {:.1} {:.1} A {:.1} {:.1} 0 {} 1 {:.1} {:.1}",
        x1, y1, r, r, large_arc, x2, y2
    )
}

fn full_circle_path(cx: f64, cy: f64, r: f64) -> String {
    format!(
        "M {:.1} {:.1} A {:.1} {:.1} 0 1 1 {:.1} {:.1} A {:.1} {:.1} 0 1 1 {:.1} {:.1}",
        cx,
        cy - r,
        r,
        r,
        cx,
        cy + r,
        r,
        r,
        cx,
        cy - r
    )
}

fn get_display_models(cfg: &AppConfig) -> Vec<ModelItemData> {
    let mut result = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for m in &cfg.models {
        let (base_id, _) = amr::proxy::router::parse_model_and_reasoning(&m.id, None);

        if !seen_ids.insert(base_id.clone()) {
            continue;
        }

        let disp_name = match base_id.as_str() {
            "claude-sonnet-5-5" => "Claude Sonnet 5.5",
            "claude-opus-5-5" => "Claude Opus 5.5",
            "claude-sonnet-4-6" => "Claude Sonnet 4.6",
            "claude-opus-4-6" => "Claude Opus 4.6",
            "gemini-4-argon" => "Gemini 4 Argon",
            "gemini-3.8-flash" => "Gemini 3.8 Flash",
            _ => &m.display_name,
        };

        result.push(ModelItemData {
            id: base_id.clone().into(),
            display_name: disp_name.to_string().into(),
            provider: m.provider.clone().into(),
            tier: m.tier.clone().into(),
            is_enabled: m.is_enabled,
            supports_thinking: m.supports_thinking,
        });

        if !cfg.group_reasoning_clones && m.supports_thinking {
            for (suffix, label) in [
                ("-none", "none"),
                ("-low", "low"),
                ("-medium", "medium"),
                ("-high", "high"),
            ] {
                let clone_id = format!("{}{}", base_id, suffix);
                result.push(ModelItemData {
                    id: clone_id.into(),
                    display_name: format!("{} ({})", disp_name, label).into(),
                    provider: m.provider.clone().into(),
                    tier: m.tier.clone().into(),
                    is_enabled: m.is_enabled,
                    supports_thinking: suffix != "-none",
                });
            }
        }
    }
    result
}

fn apply_translations(ui: &MainWindow, lang: Language) {
    let t = get_translations(lang);
    let i18n_struct = I18nData {
        tab_dashboard: t.tab_dashboard.into(),
        tab_accounts: t.tab_accounts.into(),
        tab_models: t.tab_models.into(),
        tab_proxy: t.tab_proxy.into(),
        tab_spoofing: t.tab_spoofing.into(),
        tab_logs: t.tab_logs.into(),
        tab_stats: t.tab_stats.into(),
        tab_settings: t.tab_settings.into(),
        tab_about: t.tab_about.into(),

        btn_stop_proxy: t.btn_stop_proxy.into(),
        btn_start_proxy: t.btn_start_proxy.into(),

        dash_title: t.dash_title.into(),
        dash_subtitle: t.dash_subtitle.into(),
        stat_requests: t.stat_requests.into(),
        stat_tokens: t.stat_tokens.into(),
        stat_latency: t.stat_latency.into(),
        stat_accounts: t.stat_accounts.into(),
        stat_app_ram: t.stat_app_ram.into(),
        stat_proxy_engine: t.stat_proxy_engine.into(),
        quota_claude_title: t.quota_claude_title.into(),
        quota_claude_desc: t.quota_claude_desc.into(),
        quota_gemini_title: t.quota_gemini_title.into(),
        quota_gemini_desc: t.quota_gemini_desc.into(),

        acc_title: t.acc_title.into(),
        acc_subtitle: t.acc_subtitle.into(),
        btn_login_oauth: t.btn_login_oauth.into(),
        btn_add_token: t.btn_add_token.into(),
        acc_placeholder: t.acc_placeholder.into(),
        acc_pool_header: t.acc_pool_header.into(),
        btn_update: t.btn_update.into(),
        btn_info: t.btn_info.into(),
        btn_delete: t.btn_delete.into(),
        btn_enable: t.btn_enable.into(),
        btn_disable: t.btn_disable.into(),

        mod_title: t.mod_title.into(),
        mod_subtitle: t.mod_subtitle.into(),
        group_reasoning_title: t.group_reasoning_title.into(),
        group_reasoning_desc: t.group_reasoning_desc.into(),
        btn_test_dialog: t.btn_test_dialog.into(),

        test_dialog_title: t.test_dialog_title.into(),
        test_prompt_label: t.test_prompt_label.into(),
        test_timeout_label: t.test_timeout_label.into(),
        test_max_tokens_label: t.test_max_tokens_label.into(),
        test_stream_toggle: t.test_stream_toggle.into(),
        btn_send_test: t.btn_send_test.into(),
        btn_cancel_test: t.btn_cancel_test.into(),
        btn_close: t.btn_close.into(),

        proxy_title: t.proxy_title.into(),
        proxy_subtitle: t.proxy_subtitle.into(),
        sec_network: t.sec_network.into(),
        sec_routing: t.sec_routing.into(),
        sec_cache: t.sec_cache.into(),
        sec_timeouts: t.sec_timeouts.into(),
        lbl_port: t.lbl_port.into(),
        lbl_host: t.lbl_host.into(),
        lbl_timeout: t.lbl_timeout.into(),
        lbl_heartbeat: t.lbl_heartbeat.into(),
        lbl_failover: t.lbl_failover.into(),

        logs_title: t.logs_title.into(),
        logs_subtitle: t.logs_subtitle.into(),
        btn_clear_logs: t.btn_clear_logs.into(),
        lbl_enable_logging: t.lbl_enable_logging.into(),
        lbl_max_logs: t.lbl_max_logs.into(),

        stats_title: t.stats_title.into(),
        stats_subtitle: t.stats_subtitle.into(),

        sett_title: t.sett_title.into(),
        sett_subtitle: t.sett_subtitle.into(),
        sec_visual: t.sec_visual.into(),
        lbl_language: t.lbl_language.into(),
        lbl_quota_mode: t.lbl_quota_mode.into(),
        opt_quota_percent: t.opt_quota_percent.into(),
        opt_quota_fraction: t.opt_quota_fraction.into(),
        lbl_hide_brand: t.lbl_hide_brand.into(),
        sec_system: t.sec_system.into(),
        lbl_tray: t.lbl_tray.into(),
        lbl_autostart: t.lbl_autostart.into(),
        lbl_portable: t.lbl_portable.into(),
        btn_save_settings: t.btn_save_settings.into(),
        lbl_show_logs: t.lbl_show_logs.into(),
        lbl_show_stats: t.lbl_show_stats.into(),
        lbl_hide_about: t.lbl_hide_about.into(),
        lbl_hide_about_desc: t.lbl_hide_about_desc.into(),
        lbl_billing_emulation: t.lbl_billing_emulation.into(),
        lbl_billing_emulation_desc: t.lbl_billing_emulation_desc.into(),
        lbl_reasoning_effort: t.lbl_reasoning_effort.into(),
        lbl_reasoning_default: t.lbl_reasoning_default.into(),
        lbl_reasoning_off: t.lbl_reasoning_off.into(),
        lbl_reasoning_low: t.lbl_reasoning_low.into(),
        lbl_reasoning_medium: t.lbl_reasoning_medium.into(),
        lbl_reasoning_high: t.lbl_reasoning_high.into(),
        lbl_jitter_percent: t.lbl_jitter_percent.into(),
        lbl_inspector_prompt_tok: t.lbl_inspector_prompt_tok.into(),
        lbl_inspector_comp_tok: t.lbl_inspector_comp_tok.into(),
        lbl_added_at: t.lbl_added_at.into(),
        lbl_check_updates: t.lbl_check_updates.into(),
        lbl_clear_logs_exit: t.lbl_clear_logs_exit.into(),
        lbl_quota_protection: t.lbl_quota_protection.into(),
        lbl_active_refresh: t.lbl_active_refresh.into(),
        lbl_pool_refresh: t.lbl_pool_refresh.into(),
        info_error_label: t.info_error_label.into(),
        about_title: t.about_title.into(),
        about_subtitle: t.about_subtitle.into(),
        lbl_app_folder: t.lbl_app_folder.into(),
        btn_open_folder: t.btn_open_folder.into(),
        lbl_quota_view_mode: t.lbl_quota_view_mode.into(),
        opt_quota_both: t.opt_quota_both.into(),
        opt_quota_5h: t.opt_quota_5h.into(),
        opt_quota_weekly: t.opt_quota_weekly.into(),
        lbl_endpoints_title: t.lbl_endpoints_title.into(),
        lbl_endpoints_protocol: t.lbl_endpoints_protocol.into(),
        lbl_endpoints_path: t.lbl_endpoints_path.into(),
        lbl_endpoints_models: t.lbl_endpoints_models.into(),
        lbl_master_key_title: t.lbl_master_key_title.into(),
        lbl_master_key_desc: t.lbl_master_key_desc.into(),
        btn_copy_key: t.btn_copy_key.into(),
        btn_regenerate_key: t.btn_regenerate_key.into(),
        lbl_quota_protection_desc: t.lbl_quota_protection_desc.into(),
        lbl_failover_desc: t.lbl_failover_desc.into(),
        opt_cache_first: t.opt_cache_first.into(),
        opt_cache_balanced: t.opt_cache_balanced.into(),
        opt_cache_spread: t.opt_cache_spread.into(),
        lbl_status_active: t.lbl_status_active.into(),
        lbl_status_error: t.lbl_status_error.into(),
        lbl_status_disabled: t.lbl_status_disabled.into(),
        lbl_modal_claude_quota: t.lbl_modal_claude_quota.into(),
        lbl_modal_gemini_quota: t.lbl_modal_gemini_quota.into(),
        lbl_modal_5h: t.lbl_modal_5h.into(),
        lbl_modal_week: t.lbl_modal_week.into(),
        lbl_modal_fingerprint: t.lbl_modal_fingerprint.into(),
        lbl_modal_endpoint: t.lbl_modal_endpoint.into(),
        lbl_modal_models: t.lbl_modal_models.into(),
        lbl_modal_error_details: t.lbl_modal_error_details.into(),
        lbl_updating: t.lbl_updating.into(),
        lbl_updated: t.lbl_updated.into(),
        lbl_update_failed: t.lbl_update_failed.into(),
        lbl_active_refresh_desc: t.lbl_active_refresh_desc.into(),
        lbl_pool_refresh_desc: t.lbl_pool_refresh_desc.into(),
        lbl_modal_machine_id: t.lbl_modal_machine_id.into(),
        lbl_modal_device_id: t.lbl_modal_device_id.into(),
        lbl_modal_proxy_url: t.lbl_modal_proxy_url.into(),
        lbl_modal_proxy_placeholder: t.lbl_modal_proxy_placeholder.into(),
        btn_save_proxy: t.btn_save_proxy.into(),
        btn_regenerate_fp: t.btn_regenerate_fp.into(),
        lbl_direct_proxy: t.lbl_direct_proxy.into(),
        lbl_language_desc: t.lbl_language_desc.into(),
        lbl_quota_mode_desc: t.lbl_quota_mode_desc.into(),
        lbl_quota_view_mode_desc: t.lbl_quota_view_mode_desc.into(),
        lbl_hide_brand_desc: t.lbl_hide_brand_desc.into(),
        lbl_show_logs_desc: t.lbl_show_logs_desc.into(),
        lbl_show_stats_desc: t.lbl_show_stats_desc.into(),
        lbl_tray_desc: t.lbl_tray_desc.into(),
        lbl_autostart_desc: t.lbl_autostart_desc.into(),
        lbl_portable_desc: t.lbl_portable_desc.into(),
        lbl_check_updates_desc: t.lbl_check_updates_desc.into(),
        lbl_clear_logs_exit_desc: t.lbl_clear_logs_exit_desc.into(),
        lbl_logs_folder: t.lbl_logs_folder.into(),
        btn_open_logs_folder: t.btn_open_logs_folder.into(),
        btn_reset_stats: t.btn_reset_stats.into(),
        lbl_time_range: t.lbl_time_range.into(),
        lbl_stat_composition: t.lbl_stat_composition.into(),
        lbl_stat_reliability: t.lbl_stat_reliability.into(),
        lbl_stat_model_share: t.lbl_stat_model_share.into(),
        lbl_inspector_title: t.lbl_inspector_title.into(),
        lbl_inspector_req_body: t.lbl_inspector_req_body.into(),
        lbl_inspector_resp_body: t.lbl_inspector_resp_body.into(),
        lbl_inspector_headers: t.lbl_inspector_headers.into(),
        btn_copy: t.btn_copy.into(),
        lbl_copied: t.lbl_copied.into(),
        lbl_inbound_reqs: t.lbl_inbound_reqs.into(),
        lbl_process_ram: t.lbl_process_ram.into(),
        strat_quota_aware: t.strat_quota_aware.into(),
        strat_round_robin: t.strat_round_robin.into(),
        strat_failover: t.strat_failover.into(),
        strat_quota_aware_desc: t.strat_quota_aware_desc.into(),
        strat_round_robin_desc: t.strat_round_robin_desc.into(),
        strat_failover_desc: t.strat_failover_desc.into(),
        btn_cache_priority: t.btn_cache_priority.into(),
        btn_balanced: t.btn_balanced.into(),
        btn_scatter: t.btn_scatter.into(),
        btn_off: t.btn_off.into(),
        btn_default: t.btn_default.into(),
        opt_percent: t.opt_percent.into(),
        opt_fraction: t.opt_fraction.into(),
        about_app_desc: t.about_app_desc.into(),
        about_spec_title: t.about_spec_title.into(),
        about_spec_runtime: t.about_spec_runtime.into(),
        about_spec_transport: t.about_spec_transport.into(),
        about_spec_gui: t.about_spec_gui.into(),
        about_spec_license: t.about_spec_license.into(),
        btn_check_updates: t.btn_check_updates.clone().into(),
        btn_github_releases: t.btn_github_releases.into(),
        btn_config_folder: t.btn_config_folder.into(),
        lbl_reset_5h_prefix: t.lbl_reset_5h_prefix.into(),
        lbl_reset_week_prefix: t.lbl_reset_week_prefix.into(),
        lbl_remaining: t.lbl_remaining.into(),
        lbl_available: t.lbl_available.into(),
        lbl_resets_prefix: t.lbl_resets_prefix.into(),
        lbl_proxy_prefix: t.lbl_proxy_prefix.into(),
        btn_time_today: t.btn_time_today.into(),
        btn_time_7d: t.btn_time_7d.into(),
        btn_time_30d: t.btn_time_30d.into(),
        btn_time_all: t.btn_time_all.into(),
        stat_card_prompt_tok: t.stat_card_prompt_tok.into(),
        stat_sub_prompt: t.stat_sub_prompt.into(),
        stat_card_output_tok: t.stat_card_output_tok.into(),
        stat_sub_output: t.stat_sub_output.into(),
        stat_card_thinking_tok: t.stat_card_thinking_tok.into(),
        stat_sub_thinking: t.stat_sub_thinking.into(),
        stat_card_total_tok: t.stat_card_total_tok.into(),
        stat_sub_total_tok: t.stat_sub_total_tok.into(),
        stat_card_avg_lat: t.stat_card_avg_lat.into(),
        stat_sub_avg_lat: t.stat_sub_avg_lat.into(),
        stat_card_success_rate: t.stat_card_success_rate.into(),
        stat_sub_success_rate: t.stat_sub_success_rate.into(),
        stat_card_failovers: t.stat_card_failovers.into(),
        stat_sub_failovers: t.stat_sub_failovers.into(),
        stat_card_peak_lat: t.stat_card_peak_lat.into(),
        stat_sub_peak_lat: t.stat_sub_peak_lat.into(),
        stat_card_cache_hit: t.stat_card_cache_hit.into(),
        stat_sub_cache_match: t.stat_sub_cache_match.into(),
        stat_card_p50: t.stat_card_p50.into(),
        stat_sub_median_lat: t.stat_sub_median_lat.into(),
        stat_card_p95: t.stat_card_p95.into(),
        stat_sub_stream_lat: t.stat_sub_stream_lat.into(),
        stat_card_p99: t.stat_card_p99.into(),
        stat_sub_max_regular_lat: t.stat_sub_max_regular_lat.into(),
        lbl_donut_prompt: t.lbl_donut_prompt.into(),
        lbl_donut_output: t.lbl_donut_output.into(),
        lbl_donut_thinking: t.lbl_donut_thinking.into(),
        lbl_col_time: t.lbl_col_time.into(),
        lbl_col_method: t.lbl_col_method.into(),
        lbl_col_endpoint: t.lbl_col_endpoint.into(),
        lbl_col_model: t.lbl_col_model.into(),
        lbl_col_status: t.lbl_col_status.into(),
        lbl_col_tokens: t.lbl_col_tokens.into(),
        lbl_col_latency: t.lbl_col_latency.into(),
        log_detail_account: t.log_detail_account.into(),
        log_detail_timestamp: t.log_detail_timestamp.into(),
        log_detail_model: t.log_detail_model.into(),
        log_detail_tokens: t.log_detail_tokens.into(),
        log_detail_latency: t.log_detail_latency.into(),
        log_detail_error: t.log_detail_error.into(),
        log_detail_headers: t.log_detail_headers.into(),
        log_detail_payload: t.log_detail_payload.into(),
        log_detail_response: t.log_detail_response.into(),
        btn_updating: t.btn_updating.into(),
        lbl_status_prefix: t.lbl_status_prefix.into(),
        test_temp_label: t.test_temp_label.into(),
        test_prompt_placeholder: t.test_prompt_placeholder.into(),
        lbl_catalog_models_header: t.lbl_catalog_models_header.into(),
        lbl_rolling_5h: t.lbl_rolling_5h.into(),
        lbl_rolling_weekly: t.lbl_rolling_weekly.into(),
        lbl_proxy_engine_active: t.lbl_proxy_engine_active.into(),
        lbl_proxy_engine_standby: t.lbl_proxy_engine_standby.into(),
        lbl_stat_success: t.lbl_stat_success.into(),
        lbl_stat_error: t.lbl_stat_error.into(),
        lbl_checking_updates: t.lbl_checking_updates.into(),
        lbl_update_available: t.lbl_update_available.into(),
        lbl_latest_version: t.lbl_latest_version.into(),
        lbl_sending: t.lbl_sending.into(),
        lbl_tokens_upper: t.lbl_tokens_upper.into(),
        lbl_active_upper: t.lbl_active_upper.into(),
        lbl_antifraud_badge: t.lbl_antifraud_badge.into(),
        lbl_antifraud_desc: t.lbl_antifraud_desc.into(),
        lbl_total: t.lbl_total.into(),
        lbl_proxy_engine_online: t.lbl_proxy_engine_online.into(),
        lbl_auto_selected: t.lbl_auto_selected.into(),
        lbl_just_now: t.lbl_just_now.into(),
        lbl_auto_device_id: t.lbl_auto_device_id.into(),
        lbl_upstream_models_suffix: t.lbl_upstream_models_suffix.into(),
        lbl_empty_response: t.lbl_empty_response.into(),
        lbl_test_default_prompt: t.lbl_test_default_prompt.clone().into(),
        lbl_test_response_placeholder: t.lbl_test_response_placeholder.into(),
        test_status_ready: t.test_status_ready.clone().into(),
        test_status_dispatching: t.test_status_dispatching.into(),
        test_status_waiting_token: t.test_status_waiting_token.into(),
        test_status_cancelled: t.test_status_cancelled.into(),
        test_status_error: t.test_status_error.into(),
        test_status_success: t.test_status_success.into(),
        test_err_request_prefix: t.test_err_request_prefix.into(),
        lbl_acc_claude_5h: t.lbl_acc_claude_5h.into(),
        lbl_acc_claude_week: t.lbl_acc_claude_week.into(),
        lbl_acc_gemini_5h: t.lbl_acc_gemini_5h.into(),
        lbl_acc_gemini_week: t.lbl_acc_gemini_week.into(),
        lbl_tray_show: t.lbl_tray_show.into(),
        lbl_tray_exit: t.lbl_tray_exit.into(),
        lbl_tray_tooltip: t.lbl_tray_tooltip.into(),
        lbl_config_files_hint: t.lbl_config_files_hint.into(),
        sec_stealth: t.sec_stealth.into(),
        spoofing_title: t.spoofing_title.into(),
        spoofing_subtitle: t.spoofing_subtitle.into(),
        lbl_spoof_platform: t.lbl_spoof_platform.into(),
        lbl_spoof_platform_desc: t.lbl_spoof_platform_desc.into(),
        lbl_spoof_ide_version: t.lbl_spoof_ide_version.into(),
        lbl_spoof_ide_version_desc: t.lbl_spoof_ide_version_desc.into(),
        lbl_cooldown_jitter: t.lbl_cooldown_jitter.into(),
        lbl_cooldown_jitter_desc: t.lbl_cooldown_jitter_desc.into(),
        lbl_ladder_backoff_info: t.lbl_ladder_backoff_info.into(),
        lbl_platform_auto: t.lbl_platform_auto.into(),
        lbl_spoof_go_sdk: t.lbl_spoof_go_sdk.into(),
        lbl_spoof_go_sdk_desc: t.lbl_spoof_go_sdk_desc.into(),
        lbl_inject_ide_metadata: t.lbl_inject_ide_metadata.into(),
        lbl_inject_ide_metadata_desc: t.lbl_inject_ide_metadata_desc.into(),
        lbl_lock_prod_endpoints: t.lbl_lock_prod_endpoints.into(),
        lbl_lock_prod_endpoints_desc: t.lbl_lock_prod_endpoints_desc.into(),
        lbl_cooldown_429: t.lbl_cooldown_429.into(),
        lbl_cooldown_429_desc: t.lbl_cooldown_429_desc.into(),
        lbl_cooldown_duration: t.lbl_cooldown_duration.into(),
        lbl_request_jitter: t.lbl_request_jitter.into(),
        lbl_request_jitter_desc: t.lbl_request_jitter_desc.into(),
        lbl_stealth_sec_protocol: t.lbl_stealth_sec_protocol.into(),
        lbl_stealth_sec_identity: t.lbl_stealth_sec_identity.into(),
        lbl_stealth_sec_backoff: t.lbl_stealth_sec_backoff.into(),
        lbl_tag_latest: t.lbl_tag_latest.into(),
        lbl_tag_legacy: t.lbl_tag_legacy.into(),
        lbl_tag_default: t.lbl_tag_default.into(),
        lbl_modal_effort_prefix: t.lbl_modal_effort_prefix.into(),
        about_spec_runtime_val: t.about_spec_runtime_val.into(),
        about_spec_transport_val: t.about_spec_transport_val.into(),
        about_spec_gui_val: t.about_spec_gui_val.into(),
        about_spec_license_val: t.about_spec_license_val.into(),
        lbl_endpoints_openai: t.lbl_endpoints_openai.into(),
        lbl_endpoints_claude: t.lbl_endpoints_claude.into(),
        lbl_tag_stable: t.lbl_tag_stable.into(),
        lbl_test_image_prompt: t.lbl_test_image_prompt.into(),
        lbl_test_image_prompt_placeholder: t.lbl_test_image_prompt_placeholder.into(),
        opt_cache_first_desc: t.opt_cache_first_desc.into(),
        opt_cache_balanced_desc: t.opt_cache_balanced_desc.into(),
        opt_cache_spread_desc: t.opt_cache_spread_desc.into(),
        lbl_badge_thinking: t.lbl_badge_thinking.into(),
        lbl_badge_no_thinking: t.lbl_badge_no_thinking.into(),
        lbl_spoof_arch: t.lbl_spoof_arch.into(),
        lbl_spoof_arch_desc: t.lbl_spoof_arch_desc.into(),
        lbl_spoof_channel: t.lbl_spoof_channel.into(),
        lbl_spoof_channel_desc: t.lbl_spoof_channel_desc.into(),
        lbl_strip_telemetry: t.lbl_strip_telemetry.into(),
        lbl_strip_telemetry_desc: t.lbl_strip_telemetry_desc.into(),
        lbl_auto_rotate_fp: t.lbl_auto_rotate_fp.into(),
        lbl_auto_rotate_fp_desc: t.lbl_auto_rotate_fp_desc.into(),
        lbl_normalize_headers: t.lbl_normalize_headers.into(),
        lbl_normalize_headers_desc: t.lbl_normalize_headers_desc.into(),
        lbl_version_amr_release: t.lbl_version_amr_release.into(),
        lbl_version_antigravity_latest: t.lbl_version_antigravity_latest.into(),
        lbl_modal_health_title: t.lbl_modal_health_title.into(),
        lbl_modal_quarantine_stage: t.lbl_modal_quarantine_stage.into(),
        lbl_modal_quarantine_remaining: t.lbl_modal_quarantine_remaining.into(),
        lbl_modal_quarantine_reason: t.lbl_modal_quarantine_reason.into(),
        lbl_modal_quarantine_active: t.lbl_modal_quarantine_active.into(),
        lbl_modal_quarantine_no_error: t.lbl_modal_quarantine_no_error.into(),
        btn_modal_reset_quarantine: t.btn_modal_reset_quarantine.into(),
        lbl_stage_healthy: t.lbl_stage_healthy.into(),
        lbl_stage_soft_pause: t.lbl_stage_soft_pause.into(),
        lbl_stage_backoff: t.lbl_stage_backoff.into(),
        lbl_stage_cooldown: t.lbl_stage_cooldown.into(),
        lbl_stage_quarantine: t.lbl_stage_quarantine.into(),
        lbl_stage_probing: t.lbl_stage_probing.into(),
    };
    ui.set_github_update_status(t.btn_check_updates.clone().into());
    ui.set_test_prompt(t.lbl_test_default_prompt.clone().into());
    ui.set_test_status(t.test_status_ready.clone().into());
    ui.set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    ui.set_i18n(i18n_struct);
    ui.set_current_language(lang.to_index());
}

#[cfg(target_os = "linux")]
fn setup_linux_desktop_integration() {
    if let Some(home) = dirs::home_dir() {
        let app_dir = home.join(".local/share/applications");
        let icons_base = home.join(".local/share/icons/hicolor");
        let pixmaps_dir = home.join(".local/share/pixmaps");
        let fonts_dir = home.join(".local/share/fonts/amr");

        let _ = std::fs::create_dir_all(&app_dir);
        let _ = std::fs::create_dir_all(&pixmaps_dir);
        let _ = std::fs::create_dir_all(&fonts_dir);

        let icon_sizes: [(u32, &[u8]); 7] = [
            (16, include_bytes!("../assets/icons/logo_16.png").as_slice()),
            (32, include_bytes!("../assets/icons/logo_32.png").as_slice()),
            (48, include_bytes!("../assets/icons/logo_48.png").as_slice()),
            (64, include_bytes!("../assets/icons/logo_64.png").as_slice()),
            (
                128,
                include_bytes!("../assets/icons/logo_128.png").as_slice(),
            ),
            (
                256,
                include_bytes!("../assets/icons/logo_256.png").as_slice(),
            ),
            (
                512,
                include_bytes!("../assets/icons/logo_512.png").as_slice(),
            ),
        ];

        for (sz, bytes) in icon_sizes {
            let sz_dir = icons_base.join(format!("{}x{}/apps", sz, sz));
            let _ = std::fs::create_dir_all(&sz_dir);
            let _ = std::fs::write(sz_dir.join("amr.png"), bytes);
        }

        let _ = std::fs::write(
            pixmaps_dir.join("amr.png"),
            include_bytes!("../assets/icons/logo_256.png"),
        );
        let _ = std::fs::write(
            home.join(".local/share/icons").join("amr.png"),
            include_bytes!("../assets/icons/logo_256.png"),
        );
        let _ = std::fs::write(
            fonts_dir.join("Unbounded.ttf"),
            include_bytes!("../ui/fonts/Unbounded.ttf"),
        );

        let exec_path = if let Ok(appimage) = std::env::var("APPIMAGE") {
            appimage
        } else if let Ok(exe) = std::env::current_exe() {
            exe.to_string_lossy().to_string()
        } else {
            "amr".to_string()
        };

        let desktop_content = format!(
            "[Desktop Entry]\n\
            Name=AMR\n\
            Comment=AMR (Antigravity Manager Rust) — Ultra-lightweight AI Protocol Proxy\n\
            Exec=\"{}\"\n\
            Icon=amr\n\
            Terminal=false\n\
            Type=Application\n\
            Categories=Development;Network;Utility;\n\
            StartupNotify=false\n\
            StartupWMClass=amr\n\
            Keywords=ai;proxy;openai;claude;gemini;antigravity;amr;\n",
            exec_path
        );

        let has_external_desktop = if let Ok(entries) = std::fs::read_dir(&app_dir) {
            entries.filter_map(|e| e.ok()).any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name != "amr.desktop"
                    && (name.starts_with("amr-")
                        || name.starts_with("io.github.amr")
                        || name.starts_with("appimagekit_amr"))
                    && name.ends_with(".desktop")
            })
        } else {
            false
        };

        let desktop_file = app_dir.join("amr.desktop");
        if has_external_desktop {
            if desktop_file.exists() {
                let _ = std::fs::remove_file(&desktop_file);
            }
        } else {
            let _ = std::fs::write(&desktop_file, desktop_content);
        }

        let _ = std::process::Command::new("update-desktop-database")
            .arg(&app_dir)
            .output();
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .arg("-f")
            .arg("-t")
            .arg(&icons_base)
            .output();
        let _ = std::process::Command::new("fc-cache")
            .arg("-f")
            .arg(&fonts_dir)
            .output();
    }
}

fn capture_and_save_screenshots(target_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let main_window = MainWindow::new()?;
    apply_translations(&main_window, Language::En);

    main_window.set_proxy_running(true);
    main_window.set_proxy_port(8045);
    main_window.set_proxy_host("127.0.0.1".into());
    main_window.set_total_requests("1,428".into());
    main_window.set_total_tokens("384.2k".into());
    main_window.set_avg_latency("412 ms".into());
    main_window.set_app_memory("22.4 MB".into());
    main_window.set_healthy_accounts_count(2);

    main_window.set_global_claude_percent(100);
    main_window.set_global_claude_label("100% Available".into());
    main_window.set_global_claude_reset("in 2h 44m".into());
    main_window.set_global_claude_weekly_percent(81);
    main_window.set_global_claude_weekly_label("81% Available".into());
    main_window.set_global_claude_weekly_reset("in 3d 11h".into());

    main_window.set_global_gemini_percent(91);
    main_window.set_global_gemini_label("91% Available".into());
    main_window.set_global_gemini_reset("in 4h 08m".into());
    main_window.set_global_gemini_weekly_percent(28);
    main_window.set_global_gemini_weekly_label("28% Available".into());
    main_window.set_global_gemini_weekly_reset("in 3d 11h".into());

    let accounts = vec![
        AccountData {
            id: "acc_1".into(),
            name: "t0***l@gmail.com".into(),
            provider: "Google Antigravity".into(),
            base_url: "https://cloudcode-pa.googleapis.com".into(),
            api_key_masked: "ya29.a0AehfTO...".into(),
            fingerprint: "Google Antigravity Client (1.107.0)".into(),
            proxy_url: "".into(),
            tier: "PRO".into(),
            quota_used: 17510,
            quota_limit: 100000,
            quota_percent: 82,
            quota_reset: "in 4h 08m".into(),
            claude_gpt_percent: 100,
            claude_gpt_reset: "in 2h 44m".into(),
            claude_gpt_weekly_percent: 100,
            claude_gpt_weekly_reset: "in 4d 21h".into(),
            gemini_percent: 82,
            gemini_reset: "in 4h 08m".into(),
            gemini_weekly_percent: 48,
            gemini_weekly_reset: "in 4d 21h".into(),
            status: "ACTIVE".into(),
            is_active: true,
            error_message: "".into(),
            is_updating: false,
            update_status_text: "".into(),
            added_at_text: "2 days ago".into(),
        },
        AccountData {
            id: "acc_2".into(),
            name: "ag***t@gmail.com".into(),
            provider: "Google Antigravity".into(),
            base_url: "https://cloudcode-pa.googleapis.com".into(),
            api_key_masked: "ya29.a0AehfTO...".into(),
            fingerprint: "Google Antigravity Client (1.107.0)".into(),
            proxy_url: "".into(),
            tier: "PRO".into(),
            quota_used: 91882,
            quota_limit: 100000,
            quota_percent: 8,
            quota_reset: "in 49h 50m".into(),
            claude_gpt_percent: 100,
            claude_gpt_reset: "in 4h 00m".into(),
            claude_gpt_weekly_percent: 62,
            claude_gpt_weekly_reset: "in 1d 02h".into(),
            gemini_percent: 100,
            gemini_reset: "in 4h 00m".into(),
            gemini_weekly_percent: 8,
            gemini_weekly_reset: "in 2d 01h".into(),
            status: "ACTIVE".into(),
            is_active: true,
            error_message: "".into(),
            is_updating: false,
            update_status_text: "".into(),
            added_at_text: "2 days ago".into(),
        },
    ];
    main_window.set_accounts(Rc::new(VecModel::from(accounts)).into());

    let models = vec![
        ModelItemData {
            id: "claude-sonnet-5-5".into(),
            display_name: "Claude Sonnet 5.5".into(),
            provider: "Anthropic".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
        },
        ModelItemData {
            id: "claude-opus-5-5".into(),
            display_name: "Claude Opus 5.5".into(),
            provider: "Anthropic".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
        },
        ModelItemData {
            id: "claude-sonnet-4-6".into(),
            display_name: "Claude Sonnet 4.6".into(),
            provider: "Anthropic".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
        },
        ModelItemData {
            id: "claude-opus-4-6".into(),
            display_name: "Claude Opus 4.6".into(),
            provider: "Anthropic".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
        },
        ModelItemData {
            id: "gemini-4-argon".into(),
            display_name: "Gemini 4 Argon".into(),
            provider: "Google".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
        },
        ModelItemData {
            id: "gemini-3.8-flash".into(),
            display_name: "Gemini 3.8 Flash".into(),
            provider: "Google".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
        },
    ];
    main_window.set_models(Rc::new(VecModel::from(models)).into());

    let logs = vec![
        LogEntryData {
            id: "req_01".into(),
            time: "19:02:14".into(),
            method: "POST".into(),
            path: "/v1/chat/completions (stream)".into(),
            model: "gemini-3.8-flash".into(),
            status: 200,
            tokens: 520,
            latency_ms: 1420,
        },
        LogEntryData {
            id: "req_02".into(),
            time: "19:01:50".into(),
            method: "POST".into(),
            path: "/v1/messages (stream)".into(),
            model: "claude-sonnet-5-5".into(),
            status: 200,
            tokens: 1850,
            latency_ms: 3210,
        },
        LogEntryData {
            id: "req_03".into(),
            time: "19:00:22".into(),
            method: "POST".into(),
            path: "/v1/chat/completions (stream)".into(),
            model: "claude-sonnet-4-6".into(),
            status: 200,
            tokens: 890,
            latency_ms: 1940,
        },
        LogEntryData {
            id: "req_04".into(),
            time: "18:59:05".into(),
            method: "GET".into(),
            path: "/v1/models".into(),
            model: "system".into(),
            status: 200,
            tokens: 0,
            latency_ms: 2,
        },
    ];
    main_window.set_log_entries(Rc::new(VecModel::from(logs)).into());

    // Stats tab metrics
    main_window.set_stat_prompt_tokens("184,200".into());
    main_window.set_stat_completion_tokens("142,500".into());
    main_window.set_stat_thinking_tokens("57,500".into());
    main_window.set_stat_prompt_pct(48);
    main_window.set_stat_comp_pct(37);
    main_window.set_stat_think_pct(15);
    main_window.set_stat_success_rate("100%".into());
    main_window.set_stat_success_pct(100);
    main_window.set_stat_error_count("0".into());
    main_window.set_stat_error_pct(0);
    main_window.set_stat_max_latency("1,240 ms".into());
    main_window.set_stat_claude_pct(60);
    main_window.set_stat_gemini_pct(40);

    let out_dir = std::path::PathBuf::from(target_dir);
    std::fs::create_dir_all(&out_dir)?;

    let tabs: Vec<(i32, &'static str)> = vec![
        (0, "dashboard.png"),
        (1, "accounts.png"),
        (2, "models.png"),
        (3, "balancer.png"),
        (4, "spoofing.png"),
        (5, "logs.png"),
        (6, "stats.png"),
        (7, "settings.png"),
        (8, "about.png"),
    ];

    main_window.show()?;
    let ui_weak = main_window.as_weak();
    let step = Rc::new(std::cell::Cell::new(0usize));
    let timer = Rc::new(slint::Timer::default());
    let timer_clone = Rc::clone(&timer);

    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(250),
        move || {
            let cur = step.get();
            if cur >= tabs.len() * 2 {
                timer_clone.stop();
                println!("All screenshots captured successfully in English!");
                slint::quit_event_loop().ok();
                return;
            }

            let tab_idx = cur / 2;
            let is_capture_tick = (cur % 2) == 1;

            if let Some(ui) = ui_weak.upgrade() {
                if !is_capture_tick {
                    let (tid, _) = tabs[tab_idx];
                    ui.set_active_tab(tid);
                    ui.window().request_redraw();
                } else {
                    let (_, filename) = tabs[tab_idx];
                    let target_path = out_dir.join(filename);
                    match ui.window().take_snapshot() {
                        Ok(snapshot) => {
                            let w = snapshot.width();
                            let h = snapshot.height();
                            let slice = snapshot.as_slice();
                            let mut raw = Vec::with_capacity((w * h * 4) as usize);
                            for p in slice {
                                raw.push(p.r);
                                raw.push(p.g);
                                raw.push(p.b);
                                raw.push(p.a);
                            }
                            if let Some(img) = image::RgbaImage::from_raw(w, h, raw) {
                                if let Err(e) = img.save(&target_path) {
                                    eprintln!("Failed to save screenshot {}: {}", filename, e);
                                } else {
                                    println!("Saved screenshot: {}", target_path.display());
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to take snapshot for {}: {:?}", filename, e);
                        }
                    }
                }
            }
            step.set(cur + 1);
        },
    );

    slint::run_event_loop()?;
    std::process::exit(0);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    #[cfg(target_os = "linux")]
    setup_linux_desktop_integration();

    let args = CliArgs::parse();
    if let Some(target_dir) = args.take_screenshots.as_ref() {
        return capture_and_save_screenshots(target_dir);
    }
    let mut config = AppConfig::load();
    if let Some(port) = args.port {
        config.port = port;
    }

    let config_arc = Arc::new(RwLock::new(config));
    let stats_arc = Arc::new(ProxyStats::new());
    let balancer_arc = Arc::new(amr::proxy::balancer::LoadBalancer::new());
    let cfg_version = Arc::new(AtomicU64::new(1));
    let (stop_sender, stop_receiver) = watch::channel(false);
    let (proxy_enable_tx, mut proxy_enable_rx) = watch::channel(true);

    let proxy_running = Arc::new(AtomicBool::new(false));

    // Spawn proxy server in background
    let s_cfg = Arc::clone(&config_arc);
    let s_stats = Arc::clone(&stats_arc);
    let s_balancer = Arc::clone(&balancer_arc);
    let s_ver = Arc::clone(&cfg_version);
    let mut s_stop_rx = stop_receiver.clone();
    let p_running_srv = Arc::clone(&proxy_running);

    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .pool_max_idle_per_host(32)
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        let state = AppState {
            config: s_cfg.clone(),
            stats: s_stats,
            balancer: s_balancer,
            http_client: client,
            cfg_version: s_ver,
            proxy_clients: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        };

        loop {
            if *s_stop_rx.borrow() {
                break;
            }

            if !*proxy_enable_rx.borrow_and_update() {
                p_running_srv.store(false, Ordering::SeqCst);
                tokio::select! {
                    _ = proxy_enable_rx.changed() => {
                        continue;
                    }
                    _ = s_stop_rx.changed() => {
                        break;
                    }
                }
            }

            let (host, port) = {
                let cfg = s_cfg.read();
                (cfg.host.clone(), cfg.port)
            };
            let addr = format!("{}:{}", host, port);
            log::info!("Starting AMR core server on http://{}", addr);

            // Resilient bind: retry with backoff if port is temporarily in TIME_WAIT / in use
            let mut listener_opt = None;
            for attempt in 1..=10 {
                match tokio::net::TcpListener::bind(&addr).await {
                    Ok(l) => {
                        listener_opt = Some(l);
                        break;
                    }
                    Err(e) => {
                        log::warn!(
                            "Bind attempt {} failed for {}: {}. Retrying in 300ms...",
                            attempt,
                            addr,
                            e
                        );
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                }
            }

            let listener = match listener_opt {
                Some(l) => {
                    log::info!(
                        "AMR core server successfully bound and listening on http://{}",
                        addr
                    );
                    p_running_srv.store(true, Ordering::SeqCst);
                    l
                }
                None => {
                    log::error!("AMR failed to bind to {} after 10 attempts", addr);
                    p_running_srv.store(false, Ordering::SeqCst);
                    tokio::select! {
                        _ = proxy_enable_rx.changed() => continue,
                        _ = s_stop_rx.changed() => break,
                    }
                }
            };

            let app = create_proxy_router(state.clone());
            let server = axum::serve(listener, app);

            tokio::select! {
                res = server => {
                    if let Err(e) = res {
                        log::error!("AMR server error: {}", e);
                    }
                }
                _ = proxy_enable_rx.changed() => {
                    log::info!("AMR server shutting down listener per user toggle.");
                }
                _ = s_stop_rx.changed() => {
                    log::info!("AMR server received application shutdown signal.");
                    break;
                }
            }

            p_running_srv.store(false, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    // Background dual-cadence account refresh task
    let bg_cfg = Arc::clone(&config_arc);
    let bg_ver = Arc::clone(&cfg_version);
    let mut bg_stop_rx = stop_receiver.clone();

    tokio::spawn(async move {
        let (init_active_mins, init_pool_mins, init_jitter) = {
            let cfg = bg_cfg.read();
            (
                cfg.active_account_refresh_mins.clamp(1, 10),
                cfg.pool_account_refresh_mins.clamp(1, 60),
                cfg.cadence_jitter_percent.clamp(0, 50),
            )
        };
        let mut next_active_check = tokio::time::Instant::now()
            + Duration::from_secs(AppConfig::calculate_jittered_secs(
                init_active_mins,
                init_jitter,
            ));
        // Offset pool check so active and pool refreshes never collide
        let mut next_pool_check = tokio::time::Instant::now()
            + Duration::from_secs(
                AppConfig::calculate_jittered_secs(init_pool_mins, init_jitter) + 45,
            );

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = bg_stop_rx.changed() => {
                    break;
                }
            }

            let (active_id, active_mins, pool_mins, jitter_pct) = {
                let cfg = bg_cfg.read();
                (
                    cfg.active_account_id.clone(),
                    cfg.active_account_refresh_mins.clamp(1, 10),
                    cfg.pool_account_refresh_mins.clamp(1, 60),
                    cfg.cadence_jitter_percent.clamp(0, 50),
                )
            };

            let now = tokio::time::Instant::now();
            let check_active = now >= next_active_check;
            let check_pool = now >= next_pool_check;

            // 1. Process active account check if due
            if check_active {
                next_active_check = tokio::time::Instant::now()
                    + Duration::from_secs(AppConfig::calculate_jittered_secs(
                        active_mins,
                        jitter_pct,
                    ));
                // Prevent overlap with upcoming pool check
                if next_pool_check > next_active_check
                    && (next_pool_check - next_active_check) < Duration::from_secs(30)
                {
                    next_pool_check += Duration::from_secs(45);
                }

                let active_account_opt = {
                    let cfg = bg_cfg.read();
                    cfg.accounts
                        .iter()
                        .find(|a| active_id.as_deref() == Some(&a.id) && a.is_active)
                        .map(|a| (a.id.clone(), a.api_key.clone(), a.refresh_token.clone()))
                };

                if let Some((id, mut token, refresh_tok)) = active_account_opt {
                    let mut quota_res =
                        antigravity::fetch_models_and_quota(&client, &token, None).await;
                    if quota_res.is_err() {
                        if let Some(ref rtok) = refresh_tok {
                            if let Ok(ref_res) = antigravity::refresh_token(&client, rtok).await {
                                token = ref_res.access_token.clone();
                                {
                                    let mut cfg = bg_cfg.write();
                                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id)
                                    {
                                        acc.api_key = token.clone();
                                    }
                                    let _ = cfg.save();
                                }
                                quota_res =
                                    antigravity::fetch_models_and_quota(&client, &token, None)
                                        .await;
                            }
                        }
                    }

                    match quota_res {
                        Ok((models, used, limit, reset, detailed_quota)) => {
                            {
                                let mut cfg = bg_cfg.write();
                                if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id) {
                                    acc.quota_used = used;
                                    acc.quota_limit = limit;
                                    acc.quota_reset = reset;
                                    acc.claude_gpt_percent = detailed_quota.claude_gpt_percent;
                                    acc.claude_gpt_reset = detailed_quota.claude_gpt_reset;
                                    acc.claude_gpt_weekly_percent =
                                        detailed_quota.claude_gpt_weekly_percent;
                                    acc.claude_gpt_weekly_reset =
                                        detailed_quota.claude_gpt_weekly_reset;
                                    acc.gemini_percent = detailed_quota.gemini_percent;
                                    acc.gemini_reset = detailed_quota.gemini_reset;
                                    acc.gemini_weekly_percent =
                                        detailed_quota.gemini_weekly_percent;
                                    acc.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
                                    acc.error_message = None;
                                    acc.status = "ACTIVE".to_string();
                                    if detailed_quota.claude_gpt_percent > 0 {
                                        acc.tier = "PRO".to_string();
                                    }
                                }
                                let clean: Vec<_> = models
                                    .into_iter()
                                    .filter(|x| {
                                        !x.id.starts_with("chat_") && !x.id.contains("gemini-3.5")
                                    })
                                    .collect();
                                if !clean.is_empty() {
                                    cfg.models = clean;
                                }
                                let _ = cfg.save();
                            }
                            bg_ver.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("401")
                                || err_str.contains("403")
                                || err_str.contains("invalid_grant")
                            {
                                {
                                    let mut cfg = bg_cfg.write();
                                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id)
                                    {
                                        acc.status = "ERROR".to_string();
                                        acc.error_message =
                                            Some(format!("Authentication failed: {}", err_str));
                                    }
                                    let _ = cfg.save();
                                }
                                bg_ver.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                }
            }

            // 2. Process pool accounts check if due (staggered and non-simultaneous)
            if check_pool {
                next_pool_check = tokio::time::Instant::now()
                    + Duration::from_secs(AppConfig::calculate_jittered_secs(
                        pool_mins, jitter_pct,
                    ));
                // Prevent collision with next active check
                if next_active_check > next_pool_check
                    && (next_active_check - next_pool_check) < Duration::from_secs(30)
                {
                    next_active_check += Duration::from_secs(45);
                }

                let pool_accounts: Vec<(String, String, Option<String>)> = {
                    let cfg = bg_cfg.read();
                    cfg.accounts
                        .iter()
                        .filter(|a| {
                            a.is_active
                                && active_id.as_deref() != Some(&a.id)
                                && !(a.status == "ERROR" && a.refresh_token.is_none())
                        })
                        .map(|a| (a.id.clone(), a.api_key.clone(), a.refresh_token.clone()))
                        .collect()
                };

                for (id, mut token, refresh_tok) in pool_accounts {
                    // Stagger pool account checks with randomized 3 to 7 second pauses to avoid bot-like bursting
                    tokio::time::sleep(Duration::from_millis(AppConfig::random_delay_ms(
                        3000, 7000,
                    )))
                    .await;

                    let mut quota_res =
                        antigravity::fetch_models_and_quota(&client, &token, None).await;
                    if quota_res.is_err() {
                        if let Some(ref rtok) = refresh_tok {
                            if let Ok(ref_res) = antigravity::refresh_token(&client, rtok).await {
                                token = ref_res.access_token.clone();
                                {
                                    let mut cfg = bg_cfg.write();
                                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id)
                                    {
                                        acc.api_key = token.clone();
                                    }
                                    let _ = cfg.save();
                                }
                                quota_res =
                                    antigravity::fetch_models_and_quota(&client, &token, None)
                                        .await;
                            }
                        }
                    }

                    match quota_res {
                        Ok((_, used, limit, reset, detailed_quota)) => {
                            {
                                let mut cfg = bg_cfg.write();
                                if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id) {
                                    acc.quota_used = used;
                                    acc.quota_limit = limit;
                                    acc.quota_reset = reset;
                                    acc.claude_gpt_percent = detailed_quota.claude_gpt_percent;
                                    acc.claude_gpt_reset = detailed_quota.claude_gpt_reset;
                                    acc.claude_gpt_weekly_percent =
                                        detailed_quota.claude_gpt_weekly_percent;
                                    acc.claude_gpt_weekly_reset =
                                        detailed_quota.claude_gpt_weekly_reset;
                                    acc.gemini_percent = detailed_quota.gemini_percent;
                                    acc.gemini_reset = detailed_quota.gemini_reset;
                                    acc.gemini_weekly_percent =
                                        detailed_quota.gemini_weekly_percent;
                                    acc.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
                                    acc.error_message = None;
                                    acc.status = "ACTIVE".to_string();
                                    if detailed_quota.claude_gpt_percent > 0 {
                                        acc.tier = "PRO".to_string();
                                    }
                                }
                                let _ = cfg.save();
                            }
                            bg_ver.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("401")
                                || err_str.contains("403")
                                || err_str.contains("invalid_grant")
                            {
                                {
                                    let mut cfg = bg_cfg.write();
                                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id)
                                    {
                                        acc.status = "ERROR".to_string();
                                        acc.error_message =
                                            Some(format!("Authentication failed: {}", err_str));
                                    }
                                    let _ = cfg.save();
                                }
                                bg_ver.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                }
            }
        }
    });

    // Startup dynamic catalog & quota sync
    let startup_cfg = Arc::clone(&config_arc);
    let startup_ver = Arc::clone(&cfg_version);
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();

        let (acc_id, token, refresh_tok, project_id) = {
            let cfg = startup_cfg.read();
            if let Some(acc) = cfg
                .accounts
                .iter()
                .find(|a| a.is_active && !a.api_key.is_empty())
            {
                (
                    acc.id.clone(),
                    acc.api_key.clone(),
                    acc.refresh_token.clone(),
                    acc.project_id.clone(),
                )
            } else {
                return;
            }
        };

        let mut active_pid = project_id;
        if active_pid.is_none() {
            let (pid_opt, tier) = antigravity::fetch_project_and_tier(&client, &token).await;
            if pid_opt.is_some() {
                active_pid = pid_opt.clone();
                let mut cfg = startup_cfg.write();
                if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                    acc.project_id = pid_opt;
                    acc.tier = tier;
                }
                let _ = cfg.save();
            }
        }

        log::info!("Synchronizing active upstream models on proxy launch...");
        let mut res =
            antigravity::fetch_models_and_quota(&client, &token, active_pid.as_deref()).await;

        if res.is_err() {
            if let Some(ref rtok) = refresh_tok {
                if let Ok(ref_res) = antigravity::refresh_token(&client, rtok).await {
                    let new_tok = ref_res.access_token;
                    {
                        let mut cfg = startup_cfg.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                            acc.api_key = new_tok.clone();
                            if ref_res.refresh_token.is_some() {
                                acc.refresh_token = ref_res.refresh_token;
                            }
                        }
                        let _ = cfg.save();
                    }
                    res = antigravity::fetch_models_and_quota(
                        &client,
                        &new_tok,
                        active_pid.as_deref(),
                    )
                    .await;
                }
            }
        }

        if let Ok((models, used, limit, reset, detailed_quota)) = res {
            log::info!(
                "Successfully synchronized {} models from Google Antigravity on startup",
                models.len()
            );
            {
                let mut cfg = startup_cfg.write();
                if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                    acc.quota_used = used;
                    acc.quota_limit = limit;
                    acc.quota_reset = reset;
                    acc.claude_gpt_percent = detailed_quota.claude_gpt_percent;
                    acc.claude_gpt_reset = detailed_quota.claude_gpt_reset;
                    acc.claude_gpt_weekly_percent = detailed_quota.claude_gpt_weekly_percent;
                    acc.claude_gpt_weekly_reset = detailed_quota.claude_gpt_weekly_reset;
                    acc.gemini_percent = detailed_quota.gemini_percent;
                    acc.gemini_reset = detailed_quota.gemini_reset;
                    acc.gemini_weekly_percent = detailed_quota.gemini_weekly_percent;
                    acc.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
                }
                if !models.is_empty() {
                    cfg.models = models;
                }
                let _ = cfg.save();
            }
            startup_ver.fetch_add(1, Ordering::SeqCst);
        }
    });

    if args.headless {
        log::info!("AMR running in headless daemon mode. Press Ctrl+C to stop.");
        tokio::signal::ctrl_c().await?;
        let _ = stop_sender.send(true);
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        // Enforce Winit backend with FemtoVG renderer for hardware-accelerated OpenGL and embedded fonts
        std::env::set_var("SLINT_BACKEND", "winit");
        // Ensure Wayland / X11 window manager associates window with amr.desktop / GNOME dock icon
        let mut backend_builder = i_slint_backend_winit::Backend::builder();
        backend_builder = backend_builder.with_window_attributes_hook(|attrs| {
            #[cfg(target_os = "linux")]
            {
                use i_slint_backend_winit::winit::platform::wayland::WindowAttributesExtWayland;
                use i_slint_backend_winit::winit::platform::x11::WindowAttributesExtX11;
                let attrs = WindowAttributesExtWayland::with_name(attrs, "amr", "amr");
                let attrs = WindowAttributesExtX11::with_name(attrs, "amr", "amr");
                let icon_bytes = include_bytes!("../assets/icons/tray_32x32.rgba");
                if let Ok(icon) = i_slint_backend_winit::winit::window::Icon::from_rgba(
                    icon_bytes.to_vec(),
                    32,
                    32,
                ) {
                    attrs.with_window_icon(Some(icon))
                } else {
                    attrs
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                attrs
            }
        });
        if let Ok(backend) = backend_builder.build() {
            let _ = slint::platform::set_platform(Box::new(backend));
        }
        let _ = slint::set_xdg_app_id("amr");
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = gtk::init() {
            log::warn!("Failed to initialize GTK for system tray: {:?}", e);
        }
    }

    // Launch Slint GUI
    let main_window = MainWindow::new()?;
    let window_handle = main_window.as_weak();

    let initial_visible = !args.start_minimized;
    let is_window_visible = Arc::new(AtomicBool::new(initial_visible));

    // System Tray Instance Holder
    let tray_holder: Rc<std::cell::RefCell<Option<SystemTray>>> =
        Rc::new(std::cell::RefCell::new(None));
    {
        let initial_lang = Language::from_str_code(&config_arc.read().language);
        let t_init = get_translations(initial_lang);
        let w_tray = window_handle.clone();
        let vis_tray = Arc::clone(&is_window_visible);
        if let Ok(tray) = SystemTray::new(
            &t_init.lbl_tray_show,
            &t_init.lbl_tray_exit,
            &t_init.lbl_tray_tooltip,
            move |action| {
                let win_weak = w_tray.clone();
                let vis = Arc::clone(&vis_tray);
                let _ = slint::invoke_from_event_loop(move || match action {
                    TrayAction::Show => {
                        if let Some(w) = win_weak.upgrade() {
                            show_and_focus_window(&w);
                            vis.store(true, Ordering::SeqCst);
                        }
                    }
                    TrayAction::Toggle => {
                        if let Some(w) = win_weak.upgrade() {
                            let is_showing = vis.load(Ordering::SeqCst);
                            if is_showing {
                                hide_window_to_tray(&w);
                                vis.store(false, Ordering::SeqCst);
                            } else {
                                show_and_focus_window(&w);
                                vis.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                    TrayAction::Exit => {
                        let _ = slint::quit_event_loop();
                    }
                });
            },
        ) {
            *tray_holder.borrow_mut() = Some(tray);
        }
    }

    #[cfg(target_os = "linux")]
    let _gtk_timer = {
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                while gtk::events_pending() {
                    gtk::main_iteration_do(false);
                }
            },
        );
        timer
    };

    // Accounts model
    let accounts_model = Rc::new(VecModel::<AccountData>::default());
    main_window.set_accounts(ModelRc::from(accounts_model.clone()));

    // Models catalog model
    let models_model = Rc::new(VecModel::<ModelItemData>::default());
    main_window.set_models(ModelRc::from(models_model.clone()));

    // Traffic logs model
    let logs_model = Rc::new(VecModel::<LogEntryData>::default());
    main_window.set_log_entries(ModelRc::from(logs_model.clone()));

    // Initial state populate from config
    {
        let cfg = config_arc.read();
        let lang = Language::from_str_code(&cfg.language);
        apply_translations(&main_window, lang);

        main_window.set_proxy_running(true);
        main_window.set_proxy_port(cfg.port as i32);
        main_window.set_proxy_host(cfg.host.clone().into());
        main_window.set_proxy_port_input(cfg.port.to_string().into());
        main_window.set_proxy_host_input(cfg.host.clone().into());
        main_window.set_setting_timeout_str(cfg.request_timeout_secs.to_string().into());
        main_window.set_setting_heartbeat_str(cfg.heartbeat_interval_secs.to_string().into());
        main_window.set_expose_reasoning_variants(cfg.expose_reasoning_variants);
        main_window.set_group_reasoning_clones(cfg.group_reasoning_clones);
        main_window.set_quota_display_mode(cfg.quota_display_mode.clone().into());
        main_window.set_quota_view_mode(cfg.quota_view_mode.clone().into());
        main_window.set_hide_sidebar_brand(cfg.hide_sidebar_brand);
        main_window.set_show_logs_tab(cfg.show_logs_tab);
        main_window.set_show_stats_tab(cfg.show_stats_tab);
        main_window.set_show_about_tab(cfg.show_about_tab);
        main_window.set_enable_billing_emulation(cfg.enable_billing_emulation);
        main_window.set_cadence_jitter_percent(cfg.cadence_jitter_percent as i32);
        main_window.set_setting_check_updates(cfg.check_updates_startup);
        main_window.set_setting_clear_logs_exit(cfg.clear_logs_on_exit);
        main_window.set_quota_protection_threshold(cfg.quota_protection_threshold as i32);
        main_window.set_stealth_spoof_go(cfg.spoof_go_sdk);
        main_window.set_stealth_inject_ide(cfg.inject_ide_metadata);
        main_window.set_stealth_lock_prod(cfg.lock_production_endpoints);
        main_window.set_stealth_cooldown_429(cfg.cooldown_on_429);
        main_window.set_stealth_cooldown_duration(cfg.cooldown_duration_secs as i32);
        main_window.set_stealth_request_jitter(cfg.request_jitter);
        main_window.set_stealth_spoof_platform(cfg.spoof_platform.clone().into());
        main_window.set_stealth_spoof_ide_version(cfg.spoof_ide_version.clone().into());
        main_window.set_stealth_spoof_arch(cfg.spoof_arch.clone().into());
        main_window.set_stealth_spoof_channel(cfg.spoof_channel.clone().into());
        main_window.set_stealth_strip_telemetry(cfg.strip_telemetry_traces);
        main_window.set_stealth_auto_rotate_fp(cfg.auto_rotate_fp_on_limit);
        main_window.set_stealth_normalize_headers(cfg.normalize_headers);
        main_window.set_stealth_cooldown_jitter_pct(cfg.cooldown_jitter_percent as i32);
        main_window.set_active_account_refresh_mins(cfg.active_account_refresh_mins as i32);
        main_window.set_pool_account_refresh_mins(cfg.pool_account_refresh_mins as i32);
        main_window.set_setting_timeout_secs(cfg.request_timeout_secs as i32);
        main_window.set_setting_heartbeat_secs(cfg.heartbeat_interval_secs as i32);
        main_window.set_setting_minimize_tray(cfg.minimize_to_tray);
        main_window.set_setting_autostart(cfg.autostart);
        main_window.set_setting_portable_mode(cfg.portable_mode);
        main_window.set_routing_strategy_index(cfg.routing_strategy.to_index());
        main_window.set_cache_affinity_mode_index(cfg.cache_affinity_mode.to_index());
        main_window.set_session_sticky_ttl_secs(cfg.session_sticky_ttl_secs as i32);
        main_window.set_quota_imbalance_threshold(cfg.quota_imbalance_threshold as i32);
        main_window.set_failover_on_429(cfg.failover_on_429);
        main_window.set_logging_enabled(cfg.log_enabled);
        main_window.set_max_log_capacity(cfg.max_log_entries as i32);
        main_window.set_max_log_capacity_input(cfg.max_log_entries.to_string().into());
        main_window.set_master_api_key(cfg.master_api_key.clone().into());

        let (f_path, f_size) = AppConfig::config_folder_info();
        main_window.set_config_folder_path(f_path.to_string_lossy().to_string().into());
        let size_mb = (f_size as f64) / (1024.0 * 1024.0);
        let size_display = if size_mb < 0.1 {
            format!("{} KB", f_size.div_ceil(1024))
        } else {
            format!("{:.1} MB", size_mb)
        };
        main_window.set_config_folder_size(size_display.into());

        let is_ru = cfg.language.eq_ignore_ascii_case("ru");
        let is_zh = cfg.language.eq_ignore_ascii_case("zh");

        for acc in &cfg.accounts {
            accounts_model.push(AccountData {
                id: acc.id.clone().into(),
                name: acc.name.clone().into(),
                provider: acc.provider.clone().into(),
                base_url: acc.base_url.clone().into(),
                api_key_masked: if acc.api_key.is_empty() {
                    "No key".into()
                } else {
                    "••••••••".into()
                },
                fingerprint: acc.fingerprint.clone().into(),
                tier: acc.tier.clone().into(),
                quota_used: acc.quota_used as i32,
                quota_limit: acc.quota_limit as i32,
                quota_percent: acc.claude_gpt_percent,
                quota_reset: AppConfig::localize_reset_str(&acc.claude_gpt_reset, is_ru, is_zh)
                    .into(),
                claude_gpt_percent: acc.claude_gpt_percent,
                claude_gpt_reset: AppConfig::localize_reset_str(
                    &acc.claude_gpt_reset,
                    is_ru,
                    is_zh,
                )
                .into(),
                claude_gpt_weekly_percent: acc.claude_gpt_weekly_percent,
                claude_gpt_weekly_reset: AppConfig::localize_reset_str(
                    &acc.claude_gpt_weekly_reset,
                    is_ru,
                    is_zh,
                )
                .into(),
                gemini_percent: acc.gemini_percent,
                gemini_reset: AppConfig::localize_reset_str(&acc.gemini_reset, is_ru, is_zh).into(),
                gemini_weekly_percent: acc.gemini_weekly_percent,
                gemini_weekly_reset: AppConfig::localize_reset_str(
                    &acc.gemini_weekly_reset,
                    is_ru,
                    is_zh,
                )
                .into(),
                status: acc.status.clone().into(),
                is_active: acc.is_active,
                error_message: acc.error_message.clone().unwrap_or_default().into(),
                is_updating: false,
                update_status_text: "".into(),
                added_at_text: format_elapsed_time(
                    acc.added_at.unwrap_or(0),
                    Language::from_str_code(&cfg.language),
                )
                .into(),
                proxy_url: acc.proxy_url.clone().unwrap_or_default().into(),
            });
        }

        for m in get_display_models(&cfg) {
            models_model.push(m);
        }

        main_window.set_global_claude_percent(cfg.aggregate_claude_gpt_percent());
        main_window.set_global_claude_label(cfg.format_claude_quota().into());
        main_window.set_global_claude_reset(cfg.nearest_claude_reset().into());
        main_window.set_global_claude_weekly_percent(cfg.aggregate_claude_gpt_weekly_percent());
        main_window.set_global_claude_weekly_label(cfg.format_claude_weekly_quota().into());
        main_window.set_global_claude_weekly_reset(cfg.nearest_claude_weekly_reset().into());
        main_window.set_global_gemini_percent(cfg.aggregate_gemini_percent());
        main_window.set_global_gemini_label(cfg.format_gemini_quota().into());
        main_window.set_global_gemini_reset(cfg.nearest_gemini_reset().into());
        main_window.set_global_gemini_weekly_percent(cfg.aggregate_gemini_weekly_percent());
        main_window.set_global_gemini_weekly_label(cfg.format_gemini_weekly_quota().into());
        main_window.set_global_gemini_weekly_reset(cfg.nearest_gemini_weekly_reset().into());
        main_window
            .set_healthy_accounts_count(cfg.accounts.iter().filter(|a| a.is_active).count() as i32);
        main_window.set_app_memory(get_process_memory_string().into());
    }

    // Toggle model callback
    let c_mod = Arc::clone(&config_arc);
    let v_mod = Arc::clone(&cfg_version);
    main_window.on_toggle_model(move |id_slint| {
        let id_str = id_slint.to_string();
        {
            let mut cfg = c_mod.write();
            if let Some(item) = cfg.models.iter_mut().find(|m| m.id == id_str) {
                item.is_enabled = !item.is_enabled;
            }
            let _ = cfg.save();
        }
        v_mod.fetch_add(1, Ordering::SeqCst);
    });

    // Toggle reasoning clones grouping callback
    let c_grp = Arc::clone(&config_arc);
    let v_grp = Arc::clone(&cfg_version);
    let h_grp = window_handle.clone();
    let m_grp = models_model.clone();
    main_window.on_toggle_reasoning_clones_grouping(move || {
        let next_val = {
            let mut cfg = c_grp.write();
            cfg.group_reasoning_clones = !cfg.group_reasoning_clones;
            let _ = cfg.save();
            cfg.group_reasoning_clones
        };
        if let Some(h) = h_grp.upgrade() {
            h.set_group_reasoning_clones(next_val);
            let cfg = c_grp.read();
            m_grp.set_vec(get_display_models(&cfg));
        }
        v_grp.fetch_add(1, Ordering::SeqCst);
    });

    // Toggle proxy server state
    let p_running = Arc::clone(&proxy_running);
    let p_enable_tx = proxy_enable_tx.clone();
    let h_weak = window_handle.clone();
    main_window.on_toggle_proxy(move || {
        let current = p_running.load(Ordering::SeqCst);
        let next = !current;
        let _ = p_enable_tx.send(next);
        p_running.store(next, Ordering::SeqCst);
        if let Some(h) = h_weak.upgrade() {
            h.set_proxy_running(next);
        }
    });

    // Logging controls
    let s_clear = Arc::clone(&stats_arc);
    main_window.on_clear_logs(move || {
        s_clear.clear_logs();
    });

    let s_log_toggle = Arc::clone(&stats_arc);
    let c_log_toggle = Arc::clone(&config_arc);
    let h_log_toggle = window_handle.clone();
    main_window.on_toggle_logging(move || {
        let cur = s_log_toggle.logging_enabled.load(Ordering::Relaxed);
        let next = !cur;
        s_log_toggle.logging_enabled.store(next, Ordering::Relaxed);
        {
            let mut cfg = c_log_toggle.write();
            cfg.log_enabled = next;
            let _ = cfg.save();
        }
        if let Some(h) = h_log_toggle.upgrade() {
            h.set_logging_enabled(next);
        }
    });

    let s_max_logs = Arc::clone(&stats_arc);
    let c_max_logs = Arc::clone(&config_arc);
    let h_max_logs = window_handle.clone();
    main_window.on_set_max_logs(move |count| {
        let cnt = count as usize;
        s_max_logs.max_log_capacity.store(cnt, Ordering::Relaxed);
        {
            let mut cfg = c_max_logs.write();
            cfg.max_log_entries = cnt;
            let _ = cfg.save();
        }
        if let Some(h) = h_max_logs.upgrade() {
            h.set_max_log_capacity(count);
        }
    });

    // Add account manually callback
    let c_add = Arc::clone(&config_arc);
    let v_add = Arc::clone(&cfg_version);
    let h_add = window_handle.clone();
    main_window.on_add_account(move || {
        if let Some(h) = h_add.upgrade() {
            let token = h.get_new_acc_token().to_string().trim().to_string();
            if token.is_empty() {
                return;
            }
            let proxy_input = h.get_new_acc_proxy().to_string().trim().to_string();
            let proxy_opt = if proxy_input.is_empty() {
                None
            } else {
                Some(proxy_input)
            };

            h.set_new_acc_token("".into());
            h.set_new_acc_proxy("".into());

            let c_clone = Arc::clone(&c_add);
            let v_clone = Arc::clone(&v_add);

            tokio::spawn(async move {
                let http_client = reqwest::Client::new();
                let mut access_token = token.clone();
                let mut refresh_token_opt = None;

                if token.starts_with("1//") {
                    refresh_token_opt = Some(token.clone());
                    if let Ok(tok_res) = antigravity::refresh_token(&http_client, &token).await {
                        access_token = tok_res.access_token;
                        if tok_res.refresh_token.is_some() {
                            refresh_token_opt = tok_res.refresh_token;
                        }
                    }
                }

                let email = match antigravity::fetch_user_info(&http_client, &access_token).await {
                    Ok(info) => info.email,
                    Err(_) => {
                        if access_token.starts_with("sk-ant") {
                            "anthropic.direct@api".to_string()
                        } else if access_token.starts_with("sk-") {
                            "openai.direct@api".to_string()
                        } else {
                            format!(
                                "account-{}@antigravity",
                                &uuid::Uuid::new_v4().simple().to_string()[..6]
                            )
                        }
                    }
                };

                let (project_id, tier) =
                    antigravity::fetch_project_and_tier(&http_client, &access_token).await;

                let (fetched_models, quota_used, quota_limit, quota_reset, detailed_quota) =
                    match antigravity::fetch_models_and_quota(
                        &http_client,
                        &access_token,
                        project_id.as_deref(),
                    )
                    .await
                    {
                        Ok(res) => (Some(res.0), res.1, res.2, res.3, res.4),
                        Err(_) => {
                            let det =
                                antigravity::fetch_user_quota_summary(&http_client, &access_token)
                                    .await;
                            (None, 0, 100_000, "in 4h 00m".to_string(), det)
                        }
                    };

                let new_id = format!("acc-{}", uuid::Uuid::new_v4().simple());
                let new_acc = UpstreamAccount {
                    id: new_id,
                    name: email,
                    provider: "antigravity".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    api_key: access_token,
                    fingerprint: "Google Antigravity Client (1.107.0)".to_string(),
                    tier,
                    quota_used,
                    quota_limit,
                    quota_reset,
                    claude_gpt_percent: detailed_quota.claude_gpt_percent,
                    claude_gpt_reset: detailed_quota.claude_gpt_reset,
                    claude_gpt_weekly_percent: detailed_quota.claude_gpt_weekly_percent,
                    claude_gpt_weekly_reset: detailed_quota.claude_gpt_weekly_reset,
                    gemini_percent: detailed_quota.gemini_percent,
                    gemini_reset: detailed_quota.gemini_reset,
                    gemini_weekly_percent: detailed_quota.gemini_weekly_percent,
                    gemini_weekly_reset: detailed_quota.gemini_weekly_reset,
                    status: "ACTIVE".to_string(),
                    error_message: None,
                    refresh_token: refresh_token_opt,
                    project_id,
                    is_active: true,
                    added_at: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    ),
                    device_profile: Some(amr::config::DeviceProfile::generate()),
                    proxy_url: proxy_opt,
                };

                {
                    let mut cfg = c_clone.write();
                    cfg.accounts.push(new_acc);
                    if let Some(m) = fetched_models {
                        // Filter out any uncurated internal junk models
                        let clean_models: Vec<_> = m
                            .into_iter()
                            .filter(|x| !x.id.starts_with("chat_") && !x.id.contains("gemini-3.5"))
                            .collect();
                        if !clean_models.is_empty() {
                            cfg.models = clean_models;
                        }
                    }
                    let _ = cfg.save();
                }

                v_clone.fetch_add(1, Ordering::SeqCst);
            });
        }
    });

    // Add account via Browser OAuth
    let port_oauth = config_arc.read().port;
    main_window.on_add_account_browser_oauth(move || {
        let auth_url = antigravity::get_oauth_authorization_url(port_oauth);
        log::info!("Opening Google OAuth in browser: {}", auth_url);
        open_in_system(&auth_url);
    });

    // Shared update states for account interactive feedback: id -> (is_updating, status_text)
    let account_update_states = Arc::new(parking_lot::RwLock::new(std::collections::HashMap::<
        String,
        (bool, String),
    >::new()));

    // Toggle account active state callback
    let c_tog = Arc::clone(&config_arc);
    let v_tog = Arc::clone(&cfg_version);
    let acc_model_tog = accounts_model.clone();
    main_window.on_toggle_account_active(move |id_slint| {
        let id_str = id_slint.to_string();
        {
            let mut cfg = c_tog.write();
            if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                acc.is_active = !acc.is_active;
                acc.status = if acc.is_active {
                    "ACTIVE".to_string()
                } else {
                    "DISABLED".to_string()
                };
            }
            let _ = cfg.save();
        }
        for i in 0..acc_model_tog.row_count() {
            if let Some(mut row) = acc_model_tog.row_data(i) {
                if row.id == id_slint {
                    row.is_active = !row.is_active;
                    row.status = if row.is_active {
                        "ACTIVE".into()
                    } else {
                        "DISABLED".into()
                    };
                    acc_model_tog.set_row_data(i, row);
                    break;
                }
            }
        }
        v_tog.fetch_add(1, Ordering::SeqCst);
    });

    // Refresh quota callback with interactive feedback
    let c_ref = Arc::clone(&config_arc);
    let v_ref = Arc::clone(&cfg_version);
    let states_ref = Arc::clone(&account_update_states);
    main_window.on_refresh_account_quota(move |id_slint| {
        let id_str = id_slint.to_string();
        let c_clone = Arc::clone(&c_ref);
        let v_clone = Arc::clone(&v_ref);
        let states_clone = Arc::clone(&states_ref);

        let lang = Language::from_str_code(&c_ref.read().language);
        let updating_lbl = match lang {
            Language::Ru => "Обновление...",
            Language::Zh => "正在刷新...",
            Language::En => "Updating...",
        };
        states_ref.write().insert(id_str.clone(), (true, updating_lbl.to_string()));
        v_ref.fetch_add(1, Ordering::SeqCst);

        tokio::spawn(async move {
            let (mut token, project_id, refresh_token_opt) = {
                let cfg = c_clone.read();
                if let Some(acc) = cfg.accounts.iter().find(|a| a.id == id_str) {
                    (acc.api_key.clone(), acc.project_id.clone(), acc.refresh_token.clone())
                } else {
                    return;
                }
            };

            let client = reqwest::Client::new();
            let mut resolved_pid = project_id;
            if resolved_pid.is_none() {
                let (pid_opt, tier) = antigravity::fetch_project_and_tier(&client, &token).await;
                if pid_opt.is_some() {
                    resolved_pid = pid_opt.clone();
                    let mut cfg = c_clone.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                        acc.project_id = pid_opt;
                        acc.tier = tier;
                    }
                    let _ = cfg.save();
                }
            }

            let mut res = antigravity::fetch_models_and_quota(&client, &token, resolved_pid.as_deref()).await;

            // If token expired, try refreshing via refresh token
            if res.is_err() {
                if let Some(ref rtok) = refresh_token_opt {
                    if let Ok(tok_res) = antigravity::refresh_token(&client, rtok).await {
                        token = tok_res.access_token.clone();
                        {
                            let mut cfg = c_clone.write();
                            if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                                acc.api_key = token.clone();
                                if tok_res.refresh_token.is_some() {
                                    acc.refresh_token = tok_res.refresh_token;
                                }
                            }
                            let _ = cfg.save();
                        }
                        res = antigravity::fetch_models_and_quota(&client, &token, resolved_pid.as_deref()).await;
                    }
                }
            }

            let success = match res {
                Ok((models, used, limit, reset, detailed_quota)) => {
                    {
                        let mut cfg = c_clone.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                            acc.status = "ACTIVE".to_string();
                            acc.error_message = None;
                            acc.is_active = true;
                            acc.quota_used = used;
                            acc.quota_limit = limit;
                            acc.quota_reset = reset;
                            acc.claude_gpt_percent = detailed_quota.claude_gpt_percent;
                            acc.claude_gpt_reset = detailed_quota.claude_gpt_reset;
                            acc.claude_gpt_weekly_percent = detailed_quota.claude_gpt_weekly_percent;
                            acc.claude_gpt_weekly_reset = detailed_quota.claude_gpt_weekly_reset;
                            acc.gemini_percent = detailed_quota.gemini_percent;
                            acc.gemini_reset = detailed_quota.gemini_reset;
                            acc.gemini_weekly_percent = detailed_quota.gemini_weekly_percent;
                            acc.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
                            if detailed_quota.claude_gpt_percent > 0 {
                                acc.tier = "PRO".to_string();
                            }
                        }
                        for m in models {
                            if !cfg.models.iter().any(|existing| existing.id == m.id) {
                                cfg.models.push(m);
                            }
                        }
                        cfg.sanitize_models();
                        let _ = cfg.save();
                    }
                    true
                }
                Err(_) => {
                    let mut cfg = c_clone.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                        acc.status = "ERROR".to_string();
                        if acc.refresh_token.is_none() {
                            acc.error_message = Some("Google OAuth access token (ya29...) has expired. Please Login via Browser to obtain a persistent session.".to_string());
                        }
                    }
                    let _ = cfg.save();
                    false
                }
            };

            let current_lang = Language::from_str_code(&c_clone.read().language);
            let result_str = if success {
                match current_lang {
                    Language::Ru => "Обновлено ✓",
                    Language::Zh => "已刷新 ✓",
                    Language::En => "Updated ✓",
                }
            } else {
                match current_lang {
                    Language::Ru => "Ошибка ✕",
                    Language::Zh => "失败 ✕",
                    Language::En => "Failed ✕",
                }
            };

            states_clone.write().insert(id_str.clone(), (false, result_str.to_string()));
            v_clone.fetch_add(1, Ordering::SeqCst);

            tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;
            states_clone.write().remove(&id_str);
            v_clone.fetch_add(1, Ordering::SeqCst);
        });
    });

    // Open account details dialog callback
    let c_info = Arc::clone(&config_arc);
    let h_info = window_handle.clone();
    let b_info = Arc::clone(&balancer_arc);
    main_window.on_open_account_info(move |id_slint| {
        let id_str = id_slint.to_string();
        if let Some(h) = h_info.upgrade() {
            let cfg = c_info.read();
            if let Some(acc) = cfg.accounts.iter().find(|a| a.id == id_str) {
                h.set_info_account_id(acc.id.clone().into());
                let (dev_id, mach_id) = if let Some(ref dp) = acc.device_profile {
                    (dp.dev_device_id.clone(), dp.machine_id.clone())
                } else {
                    (String::new(), String::new())
                };
                h.set_info_account_device_id(dev_id.into());
                h.set_info_account_machine_id(mach_id.into());
                h.set_info_account_proxy_url(acc.proxy_url.clone().unwrap_or_default().into());
                h.set_info_account_name(acc.name.clone().into());
                h.set_info_account_tier(acc.tier.clone().into());
                h.set_info_account_status(acc.status.clone().into());
                h.set_info_account_claude_5h_pct(acc.claude_gpt_percent);
                h.set_info_account_claude_5h_reset(acc.claude_gpt_reset.clone().into());
                h.set_info_account_claude_week_pct(acc.claude_gpt_weekly_percent);
                h.set_info_account_claude_week_reset(acc.claude_gpt_weekly_reset.clone().into());
                h.set_info_account_gemini_5h_pct(acc.gemini_percent);
                h.set_info_account_gemini_5h_reset(acc.gemini_reset.clone().into());
                h.set_info_account_gemini_week_pct(acc.gemini_weekly_percent);
                h.set_info_account_gemini_week_reset(acc.gemini_weekly_reset.clone().into());
                h.set_info_account_fingerprint(acc.fingerprint.clone().into());
                let lang = Language::from_str_code(&cfg.language);
                let t = get_translations(lang);
                h.set_info_account_models(
                    format!("{} {}", cfg.models.len(), t.lbl_upstream_models_suffix).into(),
                );
                h.set_info_account_added_at(
                    format_elapsed_time(acc.added_at.unwrap_or(0), lang).into(),
                );
                h.set_info_account_error(acc.error_message.clone().unwrap_or_default().into());

                // Health & Quarantine diagnosis
                let health = b_info.get_detailed_health(&acc.id);
                let (stage_text, is_cooling) = match health.stage {
                    amr::proxy::balancer::HealthStage::Healthy => {
                        (t.lbl_stage_healthy.clone(), false)
                    }
                    amr::proxy::balancer::HealthStage::SoftPause => {
                        (t.lbl_stage_soft_pause.clone(), true)
                    }
                    amr::proxy::balancer::HealthStage::Backoff => {
                        (t.lbl_stage_backoff.clone(), true)
                    }
                    amr::proxy::balancer::HealthStage::Cooldown => {
                        (t.lbl_stage_cooldown.clone(), true)
                    }
                    amr::proxy::balancer::HealthStage::Quarantine => {
                        (t.lbl_stage_quarantine.clone(), true)
                    }
                    amr::proxy::balancer::HealthStage::Probing => {
                        (t.lbl_stage_probing.clone(), false)
                    }
                };
                h.set_info_account_health_stage(stage_text.into());
                h.set_info_account_health_level(health.stage_level as i32);
                h.set_info_account_is_cooling(is_cooling);
                if let Some(rem) = health.remaining_secs {
                    h.set_info_account_health_remaining(format!("{}s", rem).into());
                } else {
                    h.set_info_account_health_remaining(
                        t.lbl_modal_quarantine_active.clone().into(),
                    );
                }
                if let Some(ref reason) = health.last_failure_reason {
                    h.set_info_account_health_reason(reason.clone().into());
                } else {
                    h.set_info_account_health_reason(
                        t.lbl_modal_quarantine_no_error.clone().into(),
                    );
                }

                h.set_show_account_info(true);
            }
        }
    });

    // Reset account quarantine callback
    let b_reset = Arc::clone(&balancer_arc);
    let h_reset = window_handle.clone();
    let c_reset = Arc::clone(&config_arc);
    main_window.on_reset_account_quarantine(move |id_slint| {
        let id_str = id_slint.to_string();
        b_reset.reset_health(&id_str);
        if let Some(h) = h_reset.upgrade() {
            let cfg = c_reset.read();
            let lang = Language::from_str_code(&cfg.language);
            let t = get_translations(lang);
            h.set_info_account_health_stage(t.lbl_stage_healthy.into());
            h.set_info_account_health_level(0);
            h.set_info_account_is_cooling(false);
            h.set_info_account_health_remaining(t.lbl_modal_quarantine_active.into());
            h.set_info_account_health_reason(t.lbl_modal_quarantine_no_error.into());
        }
    });

    // Save individual account proxy
    let c_save_proxy = Arc::clone(&config_arc);
    let v_save_proxy = Arc::clone(&cfg_version);
    main_window.on_save_account_proxy(move |id_slint, proxy_slint| {
        let id_str = id_slint.to_string();
        let p_str = proxy_slint.to_string().trim().to_string();
        let p_opt = if p_str.is_empty() { None } else { Some(p_str) };
        {
            let mut cfg = c_save_proxy.write();
            if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                acc.proxy_url = p_opt;
            }
            let _ = cfg.save();
        }
        v_save_proxy.fetch_add(1, Ordering::SeqCst);
    });

    // Regenerate account device fingerprint (isolation on par with Antigravity-Manager)
    let c_regen_fp = Arc::clone(&config_arc);
    let v_regen_fp = Arc::clone(&cfg_version);
    let h_regen_fp = window_handle.clone();
    main_window.on_regenerate_account_device_fingerprint(move |id_slint| {
        let id_str = id_slint.to_string();
        let new_dp = amr::config::DeviceProfile::generate();
        {
            let mut cfg = c_regen_fp.write();
            if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                acc.device_profile = Some(new_dp.clone());
            }
            let _ = cfg.save();
        }
        if let Some(h) = h_regen_fp.upgrade() {
            h.set_info_account_device_id(new_dp.dev_device_id.into());
            h.set_info_account_machine_id(new_dp.machine_id.into());
        }
        v_regen_fp.fetch_add(1, Ordering::SeqCst);
    });

    // Master API Key Callbacks
    let c_mkey = Arc::clone(&config_arc);
    let h_mkey = window_handle.clone();
    main_window.on_copy_master_key(move || {
        let (key, lang) = {
            let cfg = c_mkey.read();
            (cfg.master_api_key.clone(), cfg.language.clone())
        };
        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(key));
        if let Some(h) = h_mkey.upgrade() {
            let t = amr::i18n::get_translations(amr::i18n::Language::from_str_code(&lang));
            h.set_master_key_status_text(t.lbl_copied.into());
        }
        let h_timer = h_mkey.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_timer.upgrade() {
                    h.set_master_key_status_text("".into());
                }
            });
        });
    });

    let c_regen = Arc::clone(&config_arc);
    let h_regen = window_handle.clone();
    main_window.on_regenerate_master_key(move || {
        let new_key = format!(
            "sk-amr-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..16]
        );
        let lang = {
            let mut cfg = c_regen.write();
            cfg.master_api_key = new_key.clone();
            let _ = cfg.save();
            cfg.language.clone()
        };
        if let Some(h) = h_regen.upgrade() {
            let t = amr::i18n::get_translations(amr::i18n::Language::from_str_code(&lang));
            h.set_master_api_key(new_key.into());
            h.set_master_key_status_text(t.lbl_updated.into());
        }
        let h_timer = h_regen.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_timer.upgrade() {
                    h.set_master_key_status_text("".into());
                }
            });
        });
    });

    // GitHub Update Check Callback
    let h_gh = window_handle.clone();
    let c_gh_lang = Arc::clone(&config_arc);
    main_window.on_check_github_updates(move || {
        let lang_code = c_gh_lang.read().language.clone();
        let t_init = amr::i18n::get_translations(amr::i18n::Language::from_str_code(&lang_code));
        if let Some(h) = h_gh.upgrade() {
            h.set_github_checking_update(true);
            h.set_github_update_status(t_init.lbl_checking_updates.into());
        }
        let h_res = h_gh.clone();
        let h_gh_reset = h_gh.clone();
        let lang_for_async = lang_code.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default();
            let res = client
                .get("https://api.github.com/repos/Qyzom/Antigravity-Manager-Rust/releases/latest")
                .header("User-Agent", "AMR-Client")
                .send()
                .await;

            let t =
                amr::i18n::get_translations(amr::i18n::Language::from_str_code(&lang_for_async));
            let current_ver = concat!("v", env!("CARGO_PKG_VERSION"));
            let status_str = match res {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                            if tag == current_ver
                                || tag.trim_start_matches('v') == env!("CARGO_PKG_VERSION")
                            {
                                format!("{} ({})", current_ver, t.lbl_latest_version)
                            } else {
                                format!("{} ({})", t.lbl_update_available, tag)
                            }
                        } else {
                            format!("{} ({})", current_ver, t.lbl_latest_version)
                        }
                    } else {
                        format!("{} ({})", current_ver, t.lbl_latest_version)
                    }
                }
                _ => format!("{} ({})", current_ver, t.lbl_latest_version),
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_res.upgrade() {
                    h.set_github_checking_update(false);
                    h.set_github_update_status(status_str.into());
                }
            });

            // Automatically revert button text after 3 seconds
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_gh_reset.upgrade() {
                    let t_rev = amr::i18n::get_translations(amr::i18n::Language::from_str_code(
                        &lang_for_async,
                    ));
                    h.set_github_update_status(t_rev.btn_check_updates.into());
                }
            });
        });
    });

    // Delete account callback
    let c_del = Arc::clone(&config_arc);
    let v_del = Arc::clone(&cfg_version);
    main_window.on_delete_account(move |id_slint| {
        let id_str = id_slint.to_string();
        {
            let mut cfg = c_del.write();
            cfg.accounts.retain(|a| a.id != id_str);
            let _ = cfg.save();
        }
        v_del.fetch_add(1, Ordering::SeqCst);
    });

    // Open Test Model Dialog callback
    let h_test_dlg = window_handle.clone();
    let c_test_dlg = Arc::clone(&config_arc);
    main_window.on_open_test_dialog(move |model_id_slint| {
        if let Some(h) = h_test_dlg.upgrade() {
            let is_image =
                model_id_slint == "Image generator" || model_id_slint == "image-generator";
            let lang = Language::from_str_code(&c_test_dlg.read().language);
            let t = get_translations(lang);
            h.set_test_model_id(model_id_slint);
            h.set_test_status(t.test_status_ready.into());
            h.set_test_latency("0 ms".into());
            h.set_test_tokens("0".into());
            h.set_test_response("".into());
            h.set_test_reasoning_effort_index(0);
            if is_image {
                h.set_test_stream(false);
                h.set_test_prompt(t.lbl_test_image_prompt.into());
            } else if h.get_test_prompt().is_empty()
                || h.get_test_prompt() == t.lbl_test_image_prompt
            {
                h.set_test_prompt(t.lbl_test_default_prompt.into());
            }
            h.set_show_test_dialog(true);
        }
    });

    // Test execution cancellation flag
    let test_cancelled = Arc::new(AtomicBool::new(false));

    // Cancel test callback
    let cancel_flag = Arc::clone(&test_cancelled);
    let h_cancel = window_handle.clone();
    let c_cancel = Arc::clone(&config_arc);
    main_window.on_cancel_test_request(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Some(h) = h_cancel.upgrade() {
            let lang = Language::from_str_code(&c_cancel.read().language);
            let t = get_translations(lang);
            h.set_test_status(t.test_status_cancelled.into());
            h.set_test_running(false);
        }
    });

    // Execute test request callback
    let h_test_exec = window_handle.clone();
    let port_test = config_arc.read().port;
    let c_test_exec = Arc::clone(&config_arc);
    let cancel_exec = Arc::clone(&test_cancelled);
    main_window.on_execute_test_request(move || {
        if let Some(h) = h_test_exec.upgrade() {
            let lang = Language::from_str_code(&c_test_exec.read().language);
            let t = get_translations(lang);
            cancel_exec.store(false, Ordering::SeqCst);
            h.set_test_running(true);
            h.set_test_status(t.test_status_dispatching.into());
            h.set_test_response(t.test_status_waiting_token.into());

            let prompt = h.get_test_prompt().to_string();
            let model = h.get_test_model_id().to_string();
            let timeout_val: u64 = h.get_test_timeout_secs().to_string().parse().unwrap_or(60);
            let max_tok: u64 = h.get_test_max_tokens().to_string().parse().unwrap_or(2048);
            let temp: f64 = h.get_test_temperature().to_string().parse().unwrap_or(0.7);
            let is_stream = h.get_test_stream();
            let effort_idx = h.get_test_reasoning_effort_index();
            let reasoning_effort = match effort_idx {
                1 => Some("none"),
                2 => Some("low"),
                3 => Some("medium"),
                4 => Some("high"),
                _ => None,
            };

            let h_async = h_test_exec.clone();
            let cancel_ref = Arc::clone(&cancel_exec);
            let c_async_err = Arc::clone(&c_test_exec);

            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let client = reqwest::Client::builder()
                    .tcp_nodelay(true)
                    .timeout(Duration::from_secs(timeout_val))
                    .build()
                    .unwrap_or_default();

                let test_url = format!("http://127.0.0.1:{}/v1/chat/completions", port_test);
                let mut req_payload = serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": temp,
                    "max_tokens": max_tok,
                    "stream": is_stream
                });
                if let Some(effort) = reasoning_effort {
                    req_payload["reasoning_effort"] = serde_json::json!(effort);
                }

                if cancel_ref.load(Ordering::SeqCst) {
                    return;
                }

                match client.post(&test_url).json(&req_payload).send().await {
                    Ok(resp) => {
                        let status_code = resp.status();
                        let lat = start.elapsed().as_millis();

                        if cancel_ref.load(Ordering::SeqCst) {
                            return;
                        }

                        if !status_code.is_success() {
                            let body_text = resp.text().await.unwrap_or_default();
                            let clean_err = antigravity::extract_clean_error_message(&body_text);
                            let err_msg =
                                format!("Error ({}): {}", status_code.as_u16(), clean_err);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = h_async.upgrade() {
                                    let reason = status_code.canonical_reason().unwrap_or("Error");
                                    ui.set_test_status(
                                        format!("{} {}", status_code.as_u16(), reason).into(),
                                    );
                                    ui.set_test_latency(format!("{} ms", lat).into());
                                    ui.set_test_tokens("0".into());
                                    ui.set_test_response(err_msg.into());
                                    ui.set_test_running(false);
                                }
                            });
                            return;
                        }

                        if is_stream {
                            use futures_util::StreamExt;
                            let mut stream = resp.bytes_stream();
                            let mut full_text = String::new();
                            let mut buffer = String::new();
                            let mut total_tok: u64 = 0;

                            while let Some(chunk_res) = stream.next().await {
                                if cancel_ref.load(Ordering::SeqCst) {
                                    return;
                                }
                                if let Ok(chunk_bytes) = chunk_res {
                                    if let Ok(s) = std::str::from_utf8(&chunk_bytes) {
                                        buffer.push_str(s);
                                        while let Some(pos) = buffer.find("\n\n") {
                                            let event = buffer[..pos].to_string();
                                            buffer = buffer[pos + 2..].to_string();
                                            for line in event.lines() {
                                                if let Some(data) = line.strip_prefix("data:") {
                                                    let trimmed = data.trim();
                                                    if trimmed == "[DONE]" {
                                                        break;
                                                    }
                                                    if let Ok(val) =
                                                        serde_json::from_str::<serde_json::Value>(
                                                            trimmed,
                                                        )
                                                    {
                                                        if let Some(delta) = val
                                                            .pointer("/choices/0/delta/content")
                                                            .and_then(|c| c.as_str())
                                                        {
                                                            full_text.push_str(delta);
                                                            let current_text = full_text.clone();
                                                            let h_delta = h_async.clone();
                                                            let _ = slint::invoke_from_event_loop(
                                                                move || {
                                                                    if let Some(ui) =
                                                                        h_delta.upgrade()
                                                                    {
                                                                        ui.set_test_response(
                                                                            current_text.into(),
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                        }
                                                        if let Some(usage_tok) = val
                                                            .pointer("/usage/total_tokens")
                                                            .and_then(|t| t.as_u64())
                                                        {
                                                            total_tok = usage_tok;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if total_tok == 0 {
                                total_tok = (full_text.split_whitespace().count() as u64).max(1);
                            }
                            let final_lat = start.elapsed().as_millis();
                            let final_text = if full_text.is_empty() {
                                "(stream completed with no text delta)".to_string()
                            } else {
                                full_text
                            };

                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = h_async.upgrade() {
                                    ui.set_test_status(
                                        format!("{} OK", status_code.as_u16()).into(),
                                    );
                                    ui.set_test_latency(format!("{} ms", final_lat).into());
                                    ui.set_test_tokens(total_tok.to_string().into());
                                    ui.set_test_response(final_text.into());
                                    ui.set_test_running(false);
                                }
                            });
                        } else {
                            let body_text = resp.text().await.unwrap_or_default();
                            let final_lat = start.elapsed().as_millis();
                            let (content_text, tokens) = if let Ok(parsed) =
                                serde_json::from_str::<serde_json::Value>(&body_text)
                            {
                                let text = parsed["choices"][0]["message"]["content"]
                                    .as_str()
                                    .unwrap_or(&body_text)
                                    .to_string();
                                let tok = parsed["usage"]["total_tokens"].as_u64().unwrap_or(45);
                                (text, tok)
                            } else {
                                (body_text, 0)
                            };

                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = h_async.upgrade() {
                                    ui.set_test_status(
                                        format!("{} OK", status_code.as_u16()).into(),
                                    );
                                    ui.set_test_latency(format!("{} ms", final_lat).into());
                                    ui.set_test_tokens(tokens.to_string().into());
                                    ui.set_test_response(content_text.into());
                                    ui.set_test_running(false);
                                }
                            });
                        }
                    }
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = h_async.upgrade() {
                                let lang = Language::from_str_code(&c_async_err.read().language);
                                let t = get_translations(lang);
                                ui.set_test_status(t.test_status_error.into());
                                ui.set_test_response(
                                    format!("{}{}", t.test_err_request_prefix, e).into(),
                                );
                                ui.set_test_running(false);
                            }
                        });
                    }
                }
            });
        }
    });

    // Open Log Details Modal callback
    let s_log_modal = Arc::clone(&stats_arc);
    let h_log_modal = window_handle.clone();
    main_window.on_open_log_details(move |id_slint| {
        let id_str = id_slint.to_string();
        if let Some(h) = h_log_modal.upgrade() {
            h.set_modal_log_copied(false);
            let logs = s_log_modal.recent_logs.read();
            if let Some(item) = logs.iter().find(|l| l.id == id_str) {
                h.set_modal_log_time(item.time.clone().into());
                h.set_modal_log_method(item.method.clone().into());
                h.set_modal_log_path(item.path.clone().into());
                h.set_modal_log_model(item.model.clone().into());
                h.set_modal_log_effort(item.reasoning_effort.clone().into());
                h.set_modal_log_status(if item.status == 200 {
                    "200 OK".into()
                } else {
                    format!("{} Error", item.status).into()
                });
                let tok_str = if item.prompt_tokens > 0 || item.completion_tokens > 0 {
                    format!(
                        "{} (in: {}, out: {})",
                        item.tokens, item.prompt_tokens, item.completion_tokens
                    )
                } else {
                    item.tokens.to_string()
                };
                h.set_modal_log_tokens(tok_str.into());
                h.set_modal_log_latency(format!("{} ms", item.latency_ms).into());
                h.set_modal_log_account(item.account.clone().into());
                h.set_modal_log_headers(item.request_headers.clone().into());
                h.set_modal_log_req_body(if item.request_body.is_empty() {
                    "{}".into()
                } else {
                    item.request_body.clone().into()
                });
                h.set_modal_log_resp_body(if item.response_body.is_empty() {
                    if let Some(ref err) = item.error_msg {
                        err.clone().into()
                    } else {
                        "(empty or streaming response)".into()
                    }
                } else {
                    item.response_body.clone().into()
                });
                h.set_modal_log_error(item.error_msg.clone().unwrap_or_default().into());
                h.set_modal_inspector_tab(0);
                h.set_show_log_modal(true);
            }
        }
    });

    // Language switcher callback
    let c_lang = Arc::clone(&config_arc);
    let h_lang = window_handle.clone();
    let tray_lang = Rc::clone(&tray_holder);
    main_window.on_set_language(move |idx| {
        let lang = Language::from_index(idx);
        {
            let mut cfg = c_lang.write();
            cfg.language = lang.to_code().to_string();
            let _ = cfg.save();
        }
        let t = get_translations(lang);
        if let Some(tray) = tray_lang.borrow().as_ref() {
            tray.update_labels(&t.lbl_tray_show, &t.lbl_tray_exit, &t.lbl_tray_tooltip);
        }
        if let Some(h) = h_lang.upgrade() {
            apply_translations(&h, lang);
        }
    });

    // Quota display mode callback
    let c_qdm = Arc::clone(&config_arc);
    let h_qdm = window_handle.clone();
    let v_qdm = Arc::clone(&cfg_version);
    main_window.on_set_quota_display_mode(move |mode_slint| {
        let mode_str = mode_slint.to_string();
        {
            let mut cfg = c_qdm.write();
            cfg.quota_display_mode = mode_str.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_qdm.upgrade() {
            h.set_quota_display_mode(mode_str.into());
        }
        v_qdm.fetch_add(1, Ordering::SeqCst);
    });

    // Toggle hide brand callback
    let c_hb = Arc::clone(&config_arc);
    let h_hb = window_handle.clone();
    main_window.on_toggle_hide_brand(move || {
        let next_val = {
            let mut cfg = c_hb.write();
            cfg.hide_sidebar_brand = !cfg.hide_sidebar_brand;
            let _ = cfg.save();
            cfg.hide_sidebar_brand
        };
        if let Some(h) = h_hb.upgrade() {
            h.set_hide_sidebar_brand(next_val);
        }
    });

    // Set routing strategy callback
    let c_strat = Arc::clone(&config_arc);
    let h_strat = window_handle.clone();
    main_window.on_set_routing_strategy(move |index| {
        let strat = RoutingStrategy::from_index(index);
        {
            let mut cfg = c_strat.write();
            cfg.routing_strategy = strat;
            let _ = cfg.save();
        }
        if let Some(h) = h_strat.upgrade() {
            h.set_routing_strategy_index(index);
        }
    });

    // Set cache affinity mode callback
    let c_cam = Arc::clone(&config_arc);
    let h_cam = window_handle.clone();
    main_window.on_set_cache_affinity_mode(move |index| {
        let mode = amr::config::CacheAffinityMode::from_index(index);
        {
            let mut cfg = c_cam.write();
            cfg.cache_affinity_mode = mode;
            let _ = cfg.save();
        }
        if let Some(h) = h_cam.upgrade() {
            h.set_cache_affinity_mode_index(index);
        }
    });

    // Set session sticky TTL callback
    let c_ttl = Arc::clone(&config_arc);
    let h_ttl = window_handle.clone();
    main_window.on_set_session_ttl(move |ttl| {
        {
            let mut cfg = c_ttl.write();
            cfg.session_sticky_ttl_secs = ttl as u64;
            let _ = cfg.save();
        }
        if let Some(h) = h_ttl.upgrade() {
            h.set_session_sticky_ttl_secs(ttl);
        }
    });

    // Set imbalance threshold callback
    let c_imb = Arc::clone(&config_arc);
    let h_imb = window_handle.clone();
    main_window.on_set_imbalance_threshold(move |thresh| {
        {
            let mut cfg = c_imb.write();
            cfg.quota_imbalance_threshold = thresh as u32;
            let _ = cfg.save();
        }
        if let Some(h) = h_imb.upgrade() {
            h.set_quota_imbalance_threshold(thresh);
        }
    });

    // Toggle 429 failover callback
    let c_fo = Arc::clone(&config_arc);
    let h_fo = window_handle.clone();
    main_window.on_toggle_failover_429(move || {
        let next_val = {
            let mut cfg = c_fo.write();
            cfg.failover_on_429 = !cfg.failover_on_429;
            let _ = cfg.save();
            cfg.failover_on_429
        };
        if let Some(h) = h_fo.upgrade() {
            h.set_failover_on_429(next_val);
        }
    });

    // Instant Auto-Save Preferences Callbacks
    let c_tray = Arc::clone(&config_arc);
    let h_tray = window_handle.clone();
    main_window.on_toggle_minimize_tray(move || {
        let next_val = {
            let mut cfg = c_tray.write();
            cfg.minimize_to_tray = !cfg.minimize_to_tray;
            let _ = cfg.save();
            cfg.minimize_to_tray
        };
        if let Some(h) = h_tray.upgrade() {
            h.set_setting_minimize_tray(next_val);
        }
    });

    let c_auto = Arc::clone(&config_arc);
    let h_auto = window_handle.clone();
    main_window.on_toggle_autostart(move || {
        let next_val = {
            let mut cfg = c_auto.write();
            cfg.autostart = !cfg.autostart;
            let _ = cfg.save();
            cfg.autostart
        };
        let _ = set_autostart(next_val);
        if let Some(h) = h_auto.upgrade() {
            h.set_setting_autostart(next_val);
        }
    });

    let c_port = Arc::clone(&config_arc);
    let h_port = window_handle.clone();
    main_window.on_toggle_portable_mode(move || {
        let next_val = {
            let mut cfg = c_port.write();
            cfg.portable_mode = !cfg.portable_mode;
            let _ = cfg.save();
            cfg.portable_mode
        };
        if let Some(h) = h_port.upgrade() {
            h.set_setting_portable_mode(next_val);
        }
    });

    let c_upd = Arc::clone(&config_arc);
    let h_upd = window_handle.clone();
    main_window.on_toggle_check_updates(move || {
        let next_val = {
            let mut cfg = c_upd.write();
            cfg.check_updates_startup = !cfg.check_updates_startup;
            let _ = cfg.save();
            cfg.check_updates_startup
        };
        if let Some(h) = h_upd.upgrade() {
            h.set_setting_check_updates(next_val);
        }
    });

    let c_clx = Arc::clone(&config_arc);
    let h_clx = window_handle.clone();
    main_window.on_toggle_clear_logs_exit(move || {
        let next_val = {
            let mut cfg = c_clx.write();
            cfg.clear_logs_on_exit = !cfg.clear_logs_on_exit;
            let _ = cfg.save();
            cfg.clear_logs_on_exit
        };
        if let Some(h) = h_clx.upgrade() {
            h.set_setting_clear_logs_exit(next_val);
        }
    });

    let c_slog = Arc::clone(&config_arc);
    let h_slog = window_handle.clone();
    main_window.on_toggle_show_logs(move || {
        let next_val = {
            let mut cfg = c_slog.write();
            cfg.show_logs_tab = !cfg.show_logs_tab;
            let _ = cfg.save();
            cfg.show_logs_tab
        };
        if let Some(h) = h_slog.upgrade() {
            h.set_show_logs_tab(next_val);
        }
    });

    let c_sstat = Arc::clone(&config_arc);
    let h_sstat = window_handle.clone();
    main_window.on_toggle_show_stats(move || {
        let next_val = {
            let mut cfg = c_sstat.write();
            cfg.show_stats_tab = !cfg.show_stats_tab;
            let _ = cfg.save();
            cfg.show_stats_tab
        };
        if let Some(h) = h_sstat.upgrade() {
            h.set_show_stats_tab(next_val);
        }
    });

    let c_abt = Arc::clone(&config_arc);
    let h_abt = window_handle.clone();
    main_window.on_toggle_show_about_tab(move || {
        let next_val = {
            let mut cfg = c_abt.write();
            cfg.show_about_tab = !cfg.show_about_tab;
            let _ = cfg.save();
            cfg.show_about_tab
        };
        if let Some(h) = h_abt.upgrade() {
            h.set_show_about_tab(next_val);
        }
    });

    let c_bill = Arc::clone(&config_arc);
    let h_bill = window_handle.clone();
    main_window.on_toggle_billing_emulation(move || {
        let next_val = {
            let mut cfg = c_bill.write();
            cfg.enable_billing_emulation = !cfg.enable_billing_emulation;
            let _ = cfg.save();
            cfg.enable_billing_emulation
        };
        if let Some(h) = h_bill.upgrade() {
            h.set_enable_billing_emulation(next_val);
        }
    });

    let c_jitter = Arc::clone(&config_arc);
    let h_jitter = window_handle.clone();
    main_window.on_set_cadence_jitter_percent(move |val| {
        {
            let mut cfg = c_jitter.write();
            cfg.cadence_jitter_percent = val.clamp(5, 50) as u8;
            let _ = cfg.save();
        }
        if let Some(h) = h_jitter.upgrade() {
            h.set_cadence_jitter_percent(val);
        }
    });

    let c_qvm = Arc::clone(&config_arc);
    let h_qvm = window_handle.clone();
    main_window.on_set_quota_view_mode(move |mode_slint| {
        let mode_str = mode_slint.to_string();
        {
            let mut cfg = c_qvm.write();
            cfg.quota_view_mode = mode_str.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_qvm.upgrade() {
            h.set_quota_view_mode(mode_str.into());
        }
    });

    main_window.on_open_config_folder(move || {
        let (p, _) = AppConfig::config_folder_info();
        open_in_system(&p.to_string_lossy());
    });

    main_window.on_open_logs_folder(move || {
        let p = amr::proxy::stats::ProxyStats::logs_folder_path();
        open_in_system(&p.to_string_lossy());
    });

    let s_reset = Arc::clone(&stats_arc);
    let h_reset = window_handle.clone();
    main_window.on_reset_stats(move || {
        s_reset.reset_stats();
        if let Some(h) = h_reset.upgrade() {
            h.set_stat_prompt_tokens("0".into());
            h.set_stat_completion_tokens("0".into());
            h.set_stat_thinking_tokens("0".into());
            h.set_total_tokens("0".into());
            h.set_total_requests("0".into());
            h.set_avg_latency("0 ms".into());
            h.set_stat_success_rate("100%".into());
            h.set_stat_success_pct(100);
            h.set_stat_error_count("0".into());
            h.set_stat_error_pct(0);
            h.set_stat_max_latency("0 ms".into());
            h.set_stat_prompt_pct(0);
            h.set_stat_comp_pct(0);
            h.set_stat_think_pct(0);
            h.set_stat_claude_pct(0);
            h.set_stat_gemini_pct(0);
            h.set_stat_chart1_path1("".into());
            h.set_stat_chart1_path2("".into());
            h.set_stat_chart1_path3("".into());
            h.set_stat_chart2_path_succ("".into());
            h.set_stat_chart2_path_err("".into());
        }
    });

    main_window.on_copy_text_to_clipboard(move |text_slint| {
        let text = text_slint.to_string();
        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text));
    });

    let h_copy_model = window_handle.clone();
    main_window.on_copy_model_id(move |model_id_slint| {
        let model_id = model_id_slint.to_string();
        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(model_id));
        if let Some(ui) = h_copy_model.upgrade() {
            ui.set_copied_model_id(model_id_slint);
            let ui_weak = h_copy_model.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_copied_model_id("".into());
                    }
                });
            });
        }
    });

    main_window.on_open_github_repo(move || {
        open_in_system("https://github.com/Qyzom/Antigravity-Manager-Rust");
    });

    main_window.on_open_github_releases(move || {
        open_in_system("https://github.com/Qyzom/Antigravity-Manager-Rust/releases");
    });

    let c_pport = Arc::clone(&config_arc);
    let h_pport = window_handle.clone();
    main_window.on_set_proxy_port_str(move |s| {
        if let Ok(p) = s.to_string().trim().parse::<u16>() {
            let mut cfg = c_pport.write();
            cfg.port = p;
            let _ = cfg.save();
            if let Some(h) = h_pport.upgrade() {
                h.set_proxy_port(p as i32);
            }
        }
    });

    let c_phost = Arc::clone(&config_arc);
    let h_phost = window_handle.clone();
    main_window.on_set_proxy_host_str(move |s| {
        let host_str = s.to_string().trim().to_string();
        if !host_str.is_empty() {
            let mut cfg = c_phost.write();
            cfg.host = host_str.clone();
            let _ = cfg.save();
            if let Some(h) = h_phost.upgrade() {
                h.set_proxy_host(host_str.into());
            }
        }
    });

    let c_ptim = Arc::clone(&config_arc);
    let h_ptim = window_handle.clone();
    main_window.on_set_proxy_timeout_str(move |s| {
        if let Ok(t) = s.to_string().trim().parse::<u64>() {
            let mut cfg = c_ptim.write();
            cfg.request_timeout_secs = t;
            let _ = cfg.save();
            if let Some(h) = h_ptim.upgrade() {
                h.set_setting_timeout_secs(t as i32);
            }
        }
    });

    let c_phb = Arc::clone(&config_arc);
    let h_phb = window_handle.clone();
    main_window.on_set_proxy_heartbeat_str(move |s| {
        if let Ok(hb) = s.to_string().trim().parse::<u64>() {
            let mut cfg = c_phb.write();
            cfg.heartbeat_interval_secs = hb;
            let _ = cfg.save();
            if let Some(h) = h_phb.upgrade() {
                h.set_setting_heartbeat_secs(hb as i32);
            }
        }
    });

    let c_qpt = Arc::clone(&config_arc);
    let h_qpt = window_handle.clone();
    main_window.on_set_quota_protection_threshold(move |val| {
        {
            let mut cfg = c_qpt.write();
            cfg.quota_protection_threshold = val as u8;
            let _ = cfg.save();
        }
        if let Some(h) = h_qpt.upgrade() {
            h.set_quota_protection_threshold(val);
        }
    });

    let c_sgo = Arc::clone(&config_arc);
    let h_sgo = window_handle.clone();
    main_window.on_toggle_stealth_spoof_go(move || {
        let new_val = {
            let mut cfg = c_sgo.write();
            cfg.spoof_go_sdk = !cfg.spoof_go_sdk;
            let _ = cfg.save();
            cfg.spoof_go_sdk
        };
        if let Some(h) = h_sgo.upgrade() {
            h.set_stealth_spoof_go(new_val);
        }
    });

    let c_side = Arc::clone(&config_arc);
    let h_side = window_handle.clone();
    main_window.on_toggle_stealth_inject_ide(move || {
        let new_val = {
            let mut cfg = c_side.write();
            cfg.inject_ide_metadata = !cfg.inject_ide_metadata;
            let _ = cfg.save();
            cfg.inject_ide_metadata
        };
        if let Some(h) = h_side.upgrade() {
            h.set_stealth_inject_ide(new_val);
        }
    });

    let c_slock = Arc::clone(&config_arc);
    let h_slock = window_handle.clone();
    main_window.on_toggle_stealth_lock_prod(move || {
        let new_val = {
            let mut cfg = c_slock.write();
            cfg.lock_production_endpoints = !cfg.lock_production_endpoints;
            let _ = cfg.save();
            cfg.lock_production_endpoints
        };
        if let Some(h) = h_slock.upgrade() {
            h.set_stealth_lock_prod(new_val);
        }
    });

    let c_scd = Arc::clone(&config_arc);
    let h_scd = window_handle.clone();
    main_window.on_toggle_stealth_cooldown_429(move || {
        let new_val = {
            let mut cfg = c_scd.write();
            cfg.cooldown_on_429 = !cfg.cooldown_on_429;
            let _ = cfg.save();
            cfg.cooldown_on_429
        };
        if let Some(h) = h_scd.upgrade() {
            h.set_stealth_cooldown_429(new_val);
        }
    });

    let c_scdd = Arc::clone(&config_arc);
    let h_scdd = window_handle.clone();
    main_window.on_set_stealth_cooldown_duration(move |dur| {
        {
            let mut cfg = c_scdd.write();
            cfg.cooldown_duration_secs = dur as u64;
            let _ = cfg.save();
        }
        if let Some(h) = h_scdd.upgrade() {
            h.set_stealth_cooldown_duration(dur);
        }
    });

    let c_sjit = Arc::clone(&config_arc);
    let h_sjit = window_handle.clone();
    main_window.on_toggle_stealth_request_jitter(move || {
        let new_val = {
            let mut cfg = c_sjit.write();
            cfg.request_jitter = !cfg.request_jitter;
            let _ = cfg.save();
            cfg.request_jitter
        };
        if let Some(h) = h_sjit.upgrade() {
            h.set_stealth_request_jitter(new_val);
        }
    });

    let c_splat = Arc::clone(&config_arc);
    let h_splat = window_handle.clone();
    main_window.on_set_stealth_platform(move |plat| {
        let plat_str = plat.to_string();
        {
            let mut cfg = c_splat.write();
            cfg.spoof_platform = plat_str.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_splat.upgrade() {
            h.set_stealth_spoof_platform(plat_str.into());
        }
    });

    let c_side = Arc::clone(&config_arc);
    let h_side = window_handle.clone();
    main_window.on_set_stealth_ide_version(move |ver| {
        let ver_str = ver.to_string();
        {
            let mut cfg = c_side.write();
            cfg.spoof_ide_version = ver_str.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_side.upgrade() {
            h.set_stealth_spoof_ide_version(ver_str.into());
        }
    });

    let c_sarch = Arc::clone(&config_arc);
    let h_sarch = window_handle.clone();
    main_window.on_set_stealth_arch(move |arch| {
        let arch_str = arch.to_string();
        {
            let mut cfg = c_sarch.write();
            cfg.spoof_arch = arch_str.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_sarch.upgrade() {
            h.set_stealth_spoof_arch(arch_str.into());
        }
    });

    let c_schan = Arc::clone(&config_arc);
    let h_schan = window_handle.clone();
    main_window.on_set_stealth_channel(move |chan| {
        let chan_str = chan.to_string();
        {
            let mut cfg = c_schan.write();
            cfg.spoof_channel = chan_str.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_schan.upgrade() {
            h.set_stealth_spoof_channel(chan_str.into());
        }
    });

    let c_stt = Arc::clone(&config_arc);
    let h_stt = window_handle.clone();
    main_window.on_toggle_stealth_strip_telemetry(move || {
        let new_val = {
            let mut cfg = c_stt.write();
            cfg.strip_telemetry_traces = !cfg.strip_telemetry_traces;
            let _ = cfg.save();
            cfg.strip_telemetry_traces
        };
        if let Some(h) = h_stt.upgrade() {
            h.set_stealth_strip_telemetry(new_val);
        }
    });

    let c_sarfp = Arc::clone(&config_arc);
    let h_sarfp = window_handle.clone();
    main_window.on_toggle_stealth_auto_rotate_fp(move || {
        let new_val = {
            let mut cfg = c_sarfp.write();
            cfg.auto_rotate_fp_on_limit = !cfg.auto_rotate_fp_on_limit;
            let _ = cfg.save();
            cfg.auto_rotate_fp_on_limit
        };
        if let Some(h) = h_sarfp.upgrade() {
            h.set_stealth_auto_rotate_fp(new_val);
        }
    });

    let c_snh = Arc::clone(&config_arc);
    let h_snh = window_handle.clone();
    main_window.on_toggle_stealth_normalize_headers(move || {
        let new_val = {
            let mut cfg = c_snh.write();
            cfg.normalize_headers = !cfg.normalize_headers;
            let _ = cfg.save();
            cfg.normalize_headers
        };
        if let Some(h) = h_snh.upgrade() {
            h.set_stealth_normalize_headers(new_val);
        }
    });

    let c_scjit = Arc::clone(&config_arc);
    let h_scjit = window_handle.clone();
    main_window.on_set_stealth_cooldown_jitter(move |pct| {
        {
            let mut cfg = c_scjit.write();
            cfg.cooldown_jitter_percent = (pct as u8).clamp(0, 50);
            cfg.request_jitter = pct > 0;
            let _ = cfg.save();
        }
        if let Some(h) = h_scjit.upgrade() {
            h.set_stealth_cooldown_jitter_pct(pct);
        }
    });

    let c_arc = Arc::clone(&config_arc);
    let h_arc = window_handle.clone();
    main_window.on_set_active_refresh_cadence(move |mins| {
        {
            let mut cfg = c_arc.write();
            cfg.active_account_refresh_mins = mins as u32;
            let _ = cfg.save();
        }
        if let Some(h) = h_arc.upgrade() {
            h.set_active_account_refresh_mins(mins);
        }
    });

    let c_prc = Arc::clone(&config_arc);
    let h_prc = window_handle.clone();
    main_window.on_set_pool_refresh_cadence(move |mins| {
        {
            let mut cfg = c_prc.write();
            cfg.pool_account_refresh_mins = mins as u32;
            let _ = cfg.save();
        }
        if let Some(h) = h_prc.upgrade() {
            h.set_pool_account_refresh_mins(mins);
        }
    });

    let s_mlog = Arc::clone(&stats_arc);
    let c_mlog = Arc::clone(&config_arc);
    let h_mlog = window_handle.clone();
    main_window.on_set_max_log_capacity_str(move |val_slint| {
        if let Ok(num) = val_slint.to_string().trim().parse::<usize>() {
            let clamped = num.clamp(10, 5000);
            s_mlog.max_log_capacity.store(clamped, Ordering::Relaxed);
            {
                let mut cfg = c_mlog.write();
                cfg.max_log_entries = clamped;
                let _ = cfg.save();
            }
            if let Some(h) = h_mlog.upgrade() {
                h.set_max_log_capacity(clamped as i32);
                h.set_max_log_capacity_input(clamped.to_string().into());
            }
        }
    });

    // Periodic UI stats and state updater timer
    let timer = slint::Timer::default();
    let s_timer = Arc::clone(&stats_arc);
    let c_timer = Arc::clone(&config_arc);
    let b_timer = Arc::clone(&balancer_arc);
    let v_timer = Arc::clone(&cfg_version);
    let states_timer = Arc::clone(&account_update_states);
    let p_running_timer = Arc::clone(&proxy_running);
    let mut last_seen_version: u64 = 0;
    let w_timer = window_handle.clone();

    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            if let Some(w) = w_timer.upgrade() {
                w.set_proxy_running(p_running_timer.load(Ordering::SeqCst));
                let (reqs, tokens, avg_lat) = s_timer.get_metrics();
                w.set_total_requests(reqs.to_string().into());
                w.set_total_tokens(tokens.to_string().into());
                w.set_avg_latency(format!("{} ms", avg_lat).into());
                w.set_app_memory(get_process_memory_string().into());

                // Detailed stats
                let (_, succ, errs, prompt_tok, comp_tok, think_tok, _, max_lat) =
                    s_timer.get_detailed_stats();
                w.set_stat_prompt_tokens(prompt_tok.to_string().into());
                w.set_stat_completion_tokens(comp_tok.to_string().into());
                w.set_stat_thinking_tokens(think_tok.to_string().into());

                let total_tok = prompt_tok + comp_tok + think_tok;
                let (p_pct, c_pct, t_pct) = if total_tok > 0 {
                    let p = ((prompt_tok as f64 / total_tok as f64) * 100.0).round() as i32;
                    let c = ((comp_tok as f64 / total_tok as f64) * 100.0).round() as i32;
                    let t = 100 - p - c;
                    (p.max(1), c.max(1), t.max(1))
                } else {
                    (50, 40, 10)
                };
                w.set_stat_prompt_pct(p_pct);
                w.set_stat_comp_pct(c_pct);
                w.set_stat_think_pct(t_pct);

                let (succ_pct, err_pct, succ_rate) = if reqs > 0 {
                    let s = (succ as f64 / reqs as f64) * 100.0;
                    let e = (errs as f64 / reqs as f64) * 100.0;
                    (s.round() as i32, e.round() as i32, format!("{:.1}%", s))
                } else {
                    (100, 0, "100%".to_string())
                };
                w.set_stat_success_rate(succ_rate.into());
                w.set_stat_success_pct(succ_pct);
                w.set_stat_error_count(errs.to_string().into());
                w.set_stat_error_pct(err_pct);
                w.set_stat_max_latency(format!("{} ms", max_lat).into());

                // Circular Vector Charts
                let cx = 48.0;
                let cy = 48.0;
                let r = 38.0;

                // Chart 1: Token Composition
                w.set_stat_chart1_path_bg(full_circle_path(cx, cy, r).into());
                let p_deg = (p_pct as f64) * 3.6;
                let c_deg = (c_pct as f64) * 3.6;
                let t_deg = (t_pct as f64) * 3.6;
                w.set_stat_chart1_path1(arc_svg_path(cx, cy, r, 0.0, p_deg).into());
                w.set_stat_chart1_path2(arc_svg_path(cx, cy, r, p_deg, p_deg + c_deg).into());
                w.set_stat_chart1_path3(
                    arc_svg_path(cx, cy, r, p_deg + c_deg, (p_deg + c_deg + t_deg).min(359.8))
                        .into(),
                );

                // Chart 2: Protocol Reliability (QoS)
                w.set_stat_chart2_path_bg(full_circle_path(cx, cy, r).into());
                let succ_ratio = if reqs > 0 {
                    (succ as f64 / reqs as f64).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let succ_deg = succ_ratio * 359.8;
                w.set_stat_chart2_path_succ(arc_svg_path(cx, cy, r, 0.0, succ_deg).into());
                if succ_ratio < 0.999 {
                    w.set_stat_chart2_path_err(arc_svg_path(cx, cy, r, succ_deg, 359.8).into());
                } else {
                    w.set_stat_chart2_path_err("".into());
                }

                // Chart 3: Model Traffic Share
                let (claude_pct, gemini_pct) = {
                    let logs = s_timer.recent_logs.read();
                    let mut cl = 0;
                    let mut gm = 0;
                    for l in logs.iter() {
                        let m = l.model.to_lowercase();
                        if m.contains("claude") {
                            cl += 1;
                        } else if m.contains("gemini") {
                            gm += 1;
                        }
                    }
                    if cl + gm > 0 {
                        let cp = ((cl as f64 / (cl + gm) as f64) * 100.0).round() as i32;
                        (cp, 100 - cp)
                    } else {
                        (50, 50)
                    }
                };
                w.set_stat_claude_pct(claude_pct);
                w.set_stat_gemini_pct(gemini_pct);

                // Latency Percentiles & Real Cache Hit Rate
                let (p50, p95, p99) = s_timer.get_latency_percentiles();
                let (_, _, _, balancer_hit_rate, _) = b_timer.get_metrics();
                let real_cache_rate = s_timer.get_token_cache_hit_rate(balancer_hit_rate);

                w.set_stat_cache_hit_rate(format!("{:.1}%", real_cache_rate).into());
                w.set_stat_p50_latency(format!("{} ms", p50).into());
                w.set_stat_p95_latency(format!("{} ms", p95).into());
                w.set_stat_p99_latency(format!("{} ms", p99).into());

                // Sync accounts & models if changed
                let current_version = v_timer.load(Ordering::Relaxed);
                let accounts_len = c_timer.read().accounts.len();
                if current_version != last_seen_version
                    || accounts_len != accounts_model.row_count()
                {
                    last_seen_version = current_version;
                    let cfg = c_timer.read();

                    let is_ru = cfg.language.eq_ignore_ascii_case("ru");
                    let is_zh = cfg.language.eq_ignore_ascii_case("zh");

                    let new_acc_data: Vec<AccountData> = cfg
                        .accounts
                        .iter()
                        .map(|acc| AccountData {
                            id: acc.id.clone().into(),
                            name: acc.name.clone().into(),
                            provider: acc.provider.clone().into(),
                            base_url: acc.base_url.clone().into(),
                            api_key_masked: if acc.api_key.is_empty() {
                                "No key".into()
                            } else {
                                "••••••••".into()
                            },
                            fingerprint: acc.fingerprint.clone().into(),
                            tier: acc.tier.clone().into(),
                            quota_used: acc.quota_used as i32,
                            quota_limit: acc.quota_limit as i32,
                            quota_percent: acc.claude_gpt_percent,
                            quota_reset: AppConfig::localize_reset_str(
                                &acc.claude_gpt_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            claude_gpt_percent: acc.claude_gpt_percent,
                            claude_gpt_reset: AppConfig::localize_reset_str(
                                &acc.claude_gpt_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            claude_gpt_weekly_percent: acc.claude_gpt_weekly_percent,
                            claude_gpt_weekly_reset: AppConfig::localize_reset_str(
                                &acc.claude_gpt_weekly_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            gemini_percent: acc.gemini_percent,
                            gemini_reset: AppConfig::localize_reset_str(
                                &acc.gemini_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            gemini_weekly_percent: acc.gemini_weekly_percent,
                            gemini_weekly_reset: AppConfig::localize_reset_str(
                                &acc.gemini_weekly_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            status: acc.status.clone().into(),
                            is_active: acc.is_active,
                            error_message: acc.error_message.clone().unwrap_or_default().into(),
                            is_updating: false,
                            update_status_text: "".into(),
                            added_at_text: format_elapsed_time(
                                acc.added_at.unwrap_or(0),
                                Language::from_str_code(&cfg.language),
                            )
                            .into(),
                            proxy_url: acc.proxy_url.clone().unwrap_or_default().into(),
                        })
                        .collect();

                    if accounts_len != accounts_model.row_count() {
                        let cur_states = states_timer.read();
                        let mut final_accs = new_acc_data;
                        for row in &mut final_accs {
                            if let Some((is_updating, status_text)) =
                                cur_states.get(row.id.as_str())
                            {
                                row.is_updating = *is_updating;
                                row.update_status_text = status_text.clone().into();
                            }
                        }
                        accounts_model.set_vec(final_accs);
                    } else {
                        let cur_states = states_timer.read();
                        for (idx, item) in new_acc_data.into_iter().enumerate() {
                            let mut row = item;
                            if let Some((is_updating, status_text)) =
                                cur_states.get(row.id.as_str())
                            {
                                row.is_updating = *is_updating;
                                row.update_status_text = status_text.clone().into();
                            }
                            accounts_model.set_row_data(idx, row);
                        }
                    }

                    models_model.set_vec(get_display_models(&cfg));

                    w.set_global_claude_percent(cfg.aggregate_claude_gpt_percent());
                    w.set_global_claude_label(cfg.format_claude_quota().into());
                    w.set_global_claude_reset(cfg.nearest_claude_reset().into());
                    w.set_global_claude_weekly_percent(cfg.aggregate_claude_gpt_weekly_percent());
                    w.set_global_claude_weekly_label(cfg.format_claude_weekly_quota().into());
                    w.set_global_claude_weekly_reset(cfg.nearest_claude_weekly_reset().into());
                    w.set_global_gemini_percent(cfg.aggregate_gemini_percent());
                    w.set_global_gemini_label(cfg.format_gemini_quota().into());
                    w.set_global_gemini_reset(cfg.nearest_gemini_reset().into());
                    w.set_global_gemini_weekly_percent(cfg.aggregate_gemini_weekly_percent());
                    w.set_global_gemini_weekly_label(cfg.format_gemini_weekly_quota().into());
                    w.set_global_gemini_weekly_reset(cfg.nearest_gemini_weekly_reset().into());
                    w.set_healthy_accounts_count(
                        cfg.accounts.iter().filter(|a| a.is_active).count() as i32,
                    );
                }

                // Real-time Adaptive Backoff Ladder & Auto-Heal status sync
                for idx in 0..accounts_model.row_count() {
                    if let Some(mut row) = accounts_model.row_data(idx) {
                        let (h_stage, _f_count, rem_opt) =
                            b_timer.get_health_status(row.id.as_str());
                        let expected_status: slint::SharedString = match h_stage {
                            amr::proxy::balancer::HealthStage::Healthy => {
                                let cfg = c_timer.read();
                                if let Some(acc) =
                                    cfg.accounts.iter().find(|a| a.id == row.id.as_str())
                                {
                                    acc.status.clone().into()
                                } else {
                                    continue;
                                }
                            }
                            amr::proxy::balancer::HealthStage::SoftPause => {
                                format!("PAUSE ({}s)", rem_opt.unwrap_or(3)).into()
                            }
                            amr::proxy::balancer::HealthStage::Backoff => {
                                format!("BACKOFF ({}s)", rem_opt.unwrap_or(15)).into()
                            }
                            amr::proxy::balancer::HealthStage::Cooldown => {
                                format!("COOLDOWN ({}s)", rem_opt.unwrap_or(60)).into()
                            }
                            amr::proxy::balancer::HealthStage::Quarantine => {
                                format!("QUARANTINE ({}s)", rem_opt.unwrap_or(300)).into()
                            }
                            amr::proxy::balancer::HealthStage::Probing => "PROBING (1 TRY)".into(),
                        };
                        if row.status != expected_status {
                            row.status = expected_status;
                            accounts_model.set_row_data(idx, row);
                        }
                    }
                }

                // Real-time quarantine countdown in account info modal
                if w.get_show_account_info() {
                    let acc_id = w.get_info_account_id();
                    let health = b_timer.get_detailed_health(acc_id.as_str());
                    if let Some(rem) = health.remaining_secs {
                        w.set_info_account_health_remaining(format!("{}s", rem).into());
                        w.set_info_account_is_cooling(true);
                    } else if w.get_info_account_is_cooling() {
                        let cfg = c_timer.read();
                        let lang = Language::from_str_code(&cfg.language);
                        let t = get_translations(lang);
                        w.set_info_account_is_cooling(false);
                        w.set_info_account_health_remaining(t.lbl_modal_quarantine_active.into());
                        if health.stage == amr::proxy::balancer::HealthStage::Healthy {
                            w.set_info_account_health_stage(t.lbl_stage_healthy.into());
                            w.set_info_account_health_level(0);
                        }
                    }
                }

                let logs = s_timer.get_recent_logs();
                if logs.len() != logs_model.row_count() {
                    logs_model.set_vec(
                        logs.into_iter()
                            .map(|l| LogEntryData {
                                id: l.id.into(),
                                time: l.time.into(),
                                method: l.method.into(),
                                path: l.path.into(),
                                model: l.model.into(),
                                status: l.status as i32,
                                tokens: l.tokens as i32,
                                latency_ms: l.latency_ms as i32,
                            })
                            .collect::<Vec<_>>(),
                    );
                }
            }
        },
    );

    // Window close intercept for Minimize to System Tray
    let c_close = Arc::clone(&config_arc);
    let w_close = window_handle.clone();
    let vis_close = Arc::clone(&is_window_visible);
    let tray_close = Rc::clone(&tray_holder);
    main_window.window().on_close_requested(move || {
        let has_tray = tray_close.borrow().is_some();
        if c_close.read().minimize_to_tray && has_tray {
            if let Some(w) = w_close.upgrade() {
                hide_window_to_tray(&w);
                vis_close.store(false, Ordering::SeqCst);
            }
            slint::CloseRequestResponse::HideWindow
        } else {
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        }
    });

    if !args.start_minimized {
        main_window.show()?;
    } else {
        hide_window_to_tray(&main_window);
    }

    slint::run_event_loop_until_quit()?;

    let _ = stop_sender.send(true);
    if config_arc.read().clear_logs_on_exit {
        stats_arc.clear_logs();
    }
    Ok(())
}
