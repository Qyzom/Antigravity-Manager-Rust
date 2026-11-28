#![windows_subsystem = "windows"]

slint::include_modules!();

mod instance;

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

fn refresh_connectors_ui(ui: &MainWindow, config: &AppConfig) {
    let t = get_translations(Language::from_str_code(&config.language));
    let mut items = Vec::new();
    for desc in amr::connectors::CONNECTORS {
        let enabled = config.enabled_connectors.iter().any(|x| x == desc.id);
        let path_str = amr::connectors::connector_config_path(desc.id)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "-".to_string());
        let description = match desc.id {
            "opencode_api" => t.conn_opencode_desc.clone(),
            "opencode_mcp" => format!("MCP ({}) -> opencode.json", desc.category),
            "claude_code_api" => t.conn_claude_code_desc.clone(),
            "claude_code_mcp" => format!("MCP ({}) -> ~/.claude.json", desc.category),
            "codex_api" => t.conn_codex_desc.clone(),
            "codex_mcp" => format!("MCP ({}) -> config.toml", desc.category),
            "gemini_cli_api" => t.conn_gemini_cli_desc.clone(),
            "gemini_cli_mcp" => format!("MCP ({}) -> settings.json", desc.category),
            "deepseek_harness" => t.conn_deepseek_harness_desc.clone(),
            "cursor" => t.conn_cursor_desc.clone(),
            "cline" => t.conn_cline_desc.clone(),
            _ => String::new(),
        };
        let status = amr::connectors::check_connector_status(desc.id, config);
        let (status_text, status_kind) = match status {
            amr::connectors::ConnectorSyncStatus::Synced => (t.conn_status_synced.clone(), "synced"),
            amr::connectors::ConnectorSyncStatus::Outdated => {
                (t.conn_status_outdated.clone(), "outdated")
            }
            amr::connectors::ConnectorSyncStatus::NotSynced => {
                (t.conn_status_not_synced.clone(), "not_synced")
            }
            amr::connectors::ConnectorSyncStatus::Error(e) => {
                (format!("{}: {}", t.conn_status_error, e), "error")
            }
        };
        items.push(ConnectorItemData {
            id: desc.id.into(),
            name: desc.name.into(),
            category: desc.category.into(),
            description: description.into(),
            config_path: path_str.into(),
            enabled,
            status_text: status_text.into(),
            status_kind: status_kind.into(),
        });
    }
    ui.set_connectors_list(ModelRc::new(VecModel::from(items)));
}

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
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", target])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();
        }
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
}

fn is_newer_version(remote: &str, current: &str) -> bool {
    let parse_ver = |s: &str| -> (u32, u32, u32) {
        let clean = s.trim().trim_start_matches('v');
        let parts: Vec<u32> = clean.split('.').filter_map(|p| p.parse::<u32>().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    parse_ver(remote) > parse_ver(current)
}

#[cfg(windows)]
fn focus_winit_hwnd_windows(w: &MainWindow) {
    use i_slint_backend_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use i_slint_backend_winit::WinitWindowAccessor;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
        SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    w.window().with_winit_window(|win| {
        if let Ok(handle) = win.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let hwnd = h.hwnd.get() as windows_sys::Win32::Foundation::HWND;
                unsafe {
                    if IsIconic(hwnd) != 0 {
                        ShowWindow(hwnd, SW_RESTORE);
                    } else {
                        ShowWindow(hwnd, SW_SHOW);
                    }
                    let fg = GetForegroundWindow();
                    let cur_tid = GetCurrentThreadId();
                    let fg_tid = if !fg.is_null() {
                        GetWindowThreadProcessId(fg, std::ptr::null_mut())
                    } else {
                        0
                    };
                    if fg_tid != 0 && fg_tid != cur_tid {
                        AttachThreadInput(cur_tid, fg_tid, 1);
                        SetForegroundWindow(hwnd);
                        BringWindowToTop(hwnd);
                        AttachThreadInput(cur_tid, fg_tid, 0);
                    } else {
                        SetForegroundWindow(hwnd);
                        BringWindowToTop(hwnd);
                    }
                }
            }
        }
        win.focus_window();
    });
}

pub fn show_and_focus_window(w: &MainWindow) {
    w.window().set_minimized(false);
    let _ = w.show();
    w.window().request_redraw();

    #[cfg(not(windows))]
    {
        use i_slint_backend_winit::WinitWindowAccessor;
        w.window().with_winit_window(|win| {
            win.set_minimized(false);
            win.set_visible(true);
            win.focus_window();
        });
    }

    #[cfg(windows)]
    {
        use i_slint_backend_winit::WinitWindowAccessor;
        if w.window().has_winit_window() {
            focus_winit_hwnd_windows(w);
        } else {
            // When launched with --start-minimized, winit creates the HWND asynchronously on about_to_wait.
            let weak = w.as_weak();
            slint::Timer::single_shot(Duration::from_millis(40), move || {
                if let Some(win_up) = weak.upgrade() {
                    focus_winit_hwnd_windows(&win_up);
                }
            });
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

fn configured_ua_for_config(cfg: &AppConfig) -> String {
    antigravity::get_official_hub_user_agent_with_meta(
        Some(cfg.resolved_platform()),
        Some(&cfg.spoof_ide_version),
        Some(&cfg.spoof_arch),
    )
}

/// Shared account quota & model catalog sync helper with proxy isolation, configured UA, and OAuth refresh.
async fn sync_account_quota_and_models(
    app_state: &amr::proxy::server::AppState,
    acc_id: &str,
    ensure_project_id: bool,
    mark_error_on_any_failure: bool,
) -> bool {
    let acc_snapshot = {
        let cfg = app_state.config.read();
        cfg.accounts.iter().find(|a| a.id == acc_id).cloned()
    };
    let Some(acc_snapshot) = acc_snapshot else {
        return false;
    };

    let mut token = acc_snapshot.api_key.clone();
    let refresh_tok = acc_snapshot.refresh_token.clone();
    let mut pid = acc_snapshot.project_id.clone();
    let client = app_state.get_client_for_account(&acc_snapshot);
    let ua = {
        let cfg = app_state.config.read();
        configured_ua_for_config(&cfg)
    };

    if ensure_project_id
        && (pid.is_none() || pid.as_deref() == Some("aicode-consumers"))
    {
        let (pid_opt, tier) =
            antigravity::fetch_project_and_tier_with_ua(&client, &token, Some(&ua)).await;
        if pid_opt.is_some() {
            pid = pid_opt.clone();
            let mut cfg = app_state.config.write();
            if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                acc.project_id = pid_opt;
                acc.tier = tier;
            }
            let _ = cfg.save();
        }
    }

    let mut quota_res = antigravity::fetch_account_models_and_quota_with_ua(
        &client,
        &token,
        pid.as_deref(),
        Some(&ua),
    )
    .await;

    if quota_res.is_err() {
        if let Some(ref rtok) = refresh_tok {
            if let Ok(ref_res) =
                antigravity::refresh_token_with_ua(&client, rtok, Some(&ua)).await
            {
                token = ref_res.access_token.clone();
                {
                    let mut cfg = app_state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                        acc.api_key = token.clone();
                        if ref_res.refresh_token.is_some() {
                            acc.refresh_token = ref_res.refresh_token;
                        }
                    }
                    let _ = cfg.save();
                }
                if ensure_project_id
                    && (pid.is_none() || pid.as_deref() == Some("aicode-consumers"))
                {
                    let (pid_opt, tier) =
                        antigravity::fetch_project_and_tier_with_ua(&client, &token, Some(&ua))
                            .await;
                    if pid_opt.is_some() {
                        pid = pid_opt.clone();
                        let mut cfg = app_state.config.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                            acc.project_id = pid_opt;
                            acc.tier = tier;
                        }
                        let _ = cfg.save();
                    }
                }
                quota_res = antigravity::fetch_account_models_and_quota_with_ua(
                    &client,
                    &token,
                    pid.as_deref(),
                    Some(&ua),
                )
                .await;
            }
        }
    }

    match quota_res {
        Ok((acc_models, _models, used, limit, reset, detailed_quota)) => {
            {
                let mut cfg = app_state.config.write();
                if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                    acc.available_models = acc_models;
                    acc.quota_used = used;
                    acc.quota_limit = limit;
                    acc.quota_reset = reset;
                    acc.claude_percent = detailed_quota.claude_percent;
                    acc.claude_reset = detailed_quota.claude_reset;
                    acc.claude_weekly_percent = detailed_quota.claude_weekly_percent;
                    acc.claude_weekly_reset = detailed_quota.claude_weekly_reset;
                    acc.gemini_percent = detailed_quota.gemini_percent;
                    acc.gemini_reset = detailed_quota.gemini_reset;
                    acc.gemini_weekly_percent = detailed_quota.gemini_weekly_percent;
                    acc.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
                    acc.error_message = None;
                    acc.status = if acc.is_active {
                        "ACTIVE".to_string()
                    } else {
                        "DISABLED".to_string()
                    };
                    if detailed_quota.claude_percent > 0 {
                        acc.tier = "PRO".to_string();
                    }
                }
                cfg.rebuild_models_from_accounts();
                let _ = cfg.save();
            }
            app_state.cfg_version.fetch_add(1, Ordering::SeqCst);
            true
        }
        Err(e) => {
            let err_str = e.to_string();
            let is_auth = err_str.contains("401")
                || err_str.contains("403")
                || err_str.contains("invalid_grant");
            if is_auth || mark_error_on_any_failure {
                {
                    let mut cfg = app_state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == acc_id) {
                        acc.status = "ERROR".to_string();
                        if acc.refresh_token.is_none() {
                            acc.error_message = Some("Google OAuth access token (ya29...) has expired. Please Login via Browser to obtain a persistent session.".to_string());
                        } else if is_auth {
                            acc.error_message = Some(format!("Authentication failed: {}", err_str));
                        }
                    }
                    let _ = cfg.save();
                }
                app_state.cfg_version.fetch_add(1, Ordering::SeqCst);
            }
            false
        }
    }
}

fn detect_model_modality(id: &str) -> &'static str {
    let lower = id.to_lowercase();
    if lower.contains("whisper") || lower.contains("transcrib") || lower == "stt-1" {
        "stt"
    } else if lower.contains("web-search") || lower.contains("google-search") {
        "search"
    } else {
        "chat"
    }
}

fn get_display_models(cfg: &AppConfig) -> Vec<ModelItemData> {
    let mut result = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for m in &cfg.models {
        if !seen_ids.insert(m.id.clone()) {
            continue;
        }
        if cfg.hide_unavailable_models && amr::connectors::is_model_unavailable(m) {
            continue;
        }
        let modality = detect_model_modality(&m.id);

        result.push(ModelItemData {
            id: m.id.clone().into(),
            display_name: m.display_name.clone().into(),
            provider: m.provider.clone().into(),
            tier: m.tier.clone().into(),
            is_enabled: m.is_enabled,
            supports_thinking: m.supports_thinking,
            modality: modality.into(),
            availability_status: m.availability_status.clone().into(),
            quota_percent: m.quota_percent,
            notice: m.notice.clone().into(),
            available_accounts_count: m.available_accounts_count as i32,
        });
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

        btn_stop_proxy: t.btn_stop_proxy.into(),
        btn_start_proxy: t.btn_start_proxy.into(),

        dash_title: t.dash_title.into(),
        dash_subtitle: t.dash_subtitle.into(),
        stat_requests: t.stat_requests.into(),
        stat_app_ram: t.stat_app_ram.into(),
        stat_proxy_engine: t.stat_proxy_engine.into(),
        quota_claude_title: t.quota_claude_title.into(),
        quota_gemini_title: t.quota_gemini_title.into(),

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
        btn_close: t.btn_close.into(),

        proxy_title: t.proxy_title.into(),
        proxy_subtitle: t.proxy_subtitle.into(),
        sec_network: t.sec_network.into(),
        sec_routing: t.sec_routing.into(),
        sec_cache: t.sec_cache.into(),
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
        lbl_hide_brand: t.lbl_hide_brand.into(),
        sec_system: t.sec_system.into(),
        lbl_tray: t.lbl_tray.into(),
        lbl_autostart: t.lbl_autostart.into(),
        lbl_portable: t.lbl_portable.into(),
        lbl_show_logs: t.lbl_show_logs.into(),
        lbl_show_stats: t.lbl_show_stats.into(),
        lbl_billing_emulation: t.lbl_billing_emulation.into(),
        lbl_billing_emulation_desc: t.lbl_billing_emulation_desc.into(),
        lbl_reasoning_effort: t.lbl_reasoning_effort.into(),
        lbl_added_at: t.lbl_added_at.into(),
        lbl_check_updates: t.lbl_check_updates.into(),
        lbl_clear_logs_exit: t.lbl_clear_logs_exit.into(),
        lbl_quota_protection: t.lbl_quota_protection.into(),
        lbl_active_refresh: t.lbl_active_refresh.into(),
        lbl_pool_refresh: t.lbl_pool_refresh.into(),
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
        lbl_modal_endpoint: t.lbl_modal_endpoint.into(),
        lbl_modal_models: t.lbl_modal_models.into(),
        lbl_modal_error_details: t.lbl_modal_error_details.into(),
        lbl_updating: t.lbl_updating.into(),
        lbl_active_refresh_desc: t.lbl_active_refresh_desc.into(),
        lbl_pool_refresh_desc: t.lbl_pool_refresh_desc.into(),
        lbl_modal_machine_id: t.lbl_modal_machine_id.into(),
        lbl_modal_device_id: t.lbl_modal_device_id.into(),
        lbl_modal_proxy_url: t.lbl_modal_proxy_url.into(),
        lbl_modal_proxy_placeholder: t.lbl_modal_proxy_placeholder.into(),
        btn_save_proxy: t.btn_save_proxy.into(),
        btn_regenerate_fp: t.btn_regenerate_fp.into(),
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
        btn_off: t.btn_off.into(),
        btn_default: t.btn_default.into(),
        opt_percent: t.opt_percent.into(),
        opt_fraction: t.opt_fraction.into(),
        about_app_desc: t
            .about_app_desc
            .replace("{version}", env!("CARGO_PKG_VERSION"))
            .into(),
        about_spec_runtime: t.about_spec_runtime.into(),
        about_spec_transport: t.about_spec_transport.into(),
        about_spec_gui: t.about_spec_gui.into(),
        btn_github_releases: t.btn_github_releases.into(),
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
        log_detail_headers: t.log_detail_headers.into(),
        lbl_catalog_models_header: t.lbl_catalog_models_header.into(),
        lbl_rolling_5h: t.lbl_rolling_5h.into(),
        lbl_rolling_weekly: t.lbl_rolling_weekly.into(),
        lbl_proxy_engine_active: t.lbl_proxy_engine_active.into(),
        lbl_proxy_engine_standby: t.lbl_proxy_engine_standby.into(),
        lbl_stat_success: t.lbl_stat_success.into(),
        lbl_stat_error: t.lbl_stat_error.into(),
        lbl_tokens_upper: t.lbl_tokens_upper.into(),
        lbl_active_upper: t.lbl_active_upper.into(),
        lbl_antifraud_badge: t.lbl_antifraud_badge.into(),
        lbl_antifraud_desc: t.lbl_antifraud_desc.into(),
        lbl_proxy_engine_online: t.lbl_proxy_engine_online.into(),
        lbl_just_now: t.lbl_just_now.into(),
        lbl_auto_device_id: t.lbl_auto_device_id.into(),
        lbl_empty_response: t.lbl_empty_response.into(),
        lbl_acc_claude_5h: t.lbl_acc_claude_5h.into(),
        lbl_acc_claude_week: t.lbl_acc_claude_week.into(),
        lbl_acc_gemini_5h: t.lbl_acc_gemini_5h.into(),
        lbl_acc_gemini_week: t.lbl_acc_gemini_week.into(),
        lbl_config_files_hint: t.lbl_config_files_hint.into(),
        spoofing_title: t.spoofing_title.into(),
        spoofing_subtitle: t.spoofing_subtitle.into(),
        lbl_spoof_platform: t.lbl_spoof_platform.into(),
        lbl_spoof_ide_version: t.lbl_spoof_ide_version.into(),
        lbl_cooldown_jitter: t.lbl_cooldown_jitter.into(),
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
        lbl_stealth_sec_protocol: t.lbl_stealth_sec_protocol.into(),
        lbl_stealth_sec_identity: t.lbl_stealth_sec_identity.into(),
        lbl_stealth_sec_backoff: t.lbl_stealth_sec_backoff.into(),
        lbl_tag_default: t.lbl_tag_default.into(),
        lbl_modal_effort_prefix: t.lbl_modal_effort_prefix.into(),
        about_spec_runtime_val: t.about_spec_runtime_val.into(),
        about_spec_transport_val: t.about_spec_transport_val.into(),
        about_spec_gui_val: t.about_spec_gui_val.into(),
        lbl_endpoints_openai: t.lbl_endpoints_openai.into(),
        lbl_endpoints_claude: t.lbl_endpoints_claude.into(),
        opt_cache_first_desc: t.opt_cache_first_desc.into(),
        opt_cache_balanced_desc: t.opt_cache_balanced_desc.into(),
        opt_cache_spread_desc: t.opt_cache_spread_desc.into(),
        lbl_badge_thinking: t.lbl_badge_thinking.into(),
        lbl_strip_telemetry: t.lbl_strip_telemetry.into(),
        lbl_strip_telemetry_desc: t.lbl_strip_telemetry_desc.into(),
        lbl_auto_rotate_fp: t.lbl_auto_rotate_fp.into(),
        lbl_auto_rotate_fp_desc: t.lbl_auto_rotate_fp_desc.into(),
        lbl_normalize_headers: t.lbl_normalize_headers.into(),
        lbl_normalize_headers_desc: t.lbl_normalize_headers_desc.into(),
        lbl_version_amr_release: t.lbl_version_amr_release.into(),
        lbl_version_antigravity_latest: t.lbl_version_antigravity_latest.into(),
        lbl_modal_health_title: t.lbl_modal_health_title.into(),
        lbl_modal_quarantine_remaining: t.lbl_modal_quarantine_remaining.into(),
        lbl_modal_quarantine_reason: t.lbl_modal_quarantine_reason.into(),
        lbl_modal_quarantine_active: t.lbl_modal_quarantine_active.into(),
        lbl_modal_quarantine_no_error: t.lbl_modal_quarantine_no_error.into(),
        btn_modal_reset_quarantine: t.btn_modal_reset_quarantine.into(),
        lbl_stage_healthy: t.lbl_stage_healthy.into(),
        lbl_model_disabled: t.lbl_model_disabled.into(),
        lbl_model_exhausted: t.lbl_model_exhausted.into(),
        lbl_model_notice: t.lbl_model_notice.into(),
        lbl_mcp_title: t.lbl_mcp_title.into(),
        lbl_mcp_desc: t.lbl_mcp_desc.into(),
        btn_mcp_test: t.btn_mcp_test.into(),
        btn_mcp_config: t.btn_mcp_config.into(),
        lbl_mcp_search_results: t.lbl_mcp_search_results.into(),
        lbl_mcp_search_results_desc: t.lbl_mcp_search_results_desc.into(),
        lbl_mcp_config_title: t.lbl_mcp_config_title.into(),
        lbl_mcp_config_desc: t.lbl_mcp_config_desc.into(),
        diag_dialog_title: t.diag_dialog_title.into(),
        btn_run_diagnostic: t.btn_run_diagnostic.into(),
        lbl_diag_testing: t.lbl_diag_testing.into(),
        lbl_diag_error: t.lbl_diag_error.into(),
        lbl_account_breakdown_header: t.lbl_account_breakdown_header.into(),
        btn_test_proxy: t.btn_test_proxy.into(),
        lbl_testing_proxy: t.lbl_testing_proxy.into(),
        lbl_account_models_header: t.lbl_account_models_header.into(),
        sec_error_spoofing: t.sec_error_spoofing.into(),
        sec_error_spoofing_desc: t.sec_error_spoofing_desc.into(),
        lbl_hide_unavailable_models: t.lbl_hide_unavailable_models.into(),
        lbl_hide_unavailable_models_desc: t.lbl_hide_unavailable_models_desc.into(),
        lbl_diag_quota_notice: t.lbl_diag_quota_notice.into(),
        btn_show_key: t.btn_show_key.into(),
        btn_hide_key: t.btn_hide_key.into(),
        nav_connectors: t.nav_connectors.into(),
        connectors_title: t.connectors_title.into(),
        connectors_subtitle: t.connectors_subtitle.into(),
        conn_status_synced: t.conn_status_synced.into(),
        conn_btn_sync_now: t.conn_btn_sync_now.into(),
        conn_lbl_config_path: t.conn_lbl_config_path.into(),
        lbl_show_connectors: t.lbl_show_connectors.into(),
        mcp_test_dialog_title: t.mcp_test_dialog_title.into(),
        lbl_default_reasoning_title: t.lbl_default_reasoning_title.into(),
        lbl_default_reasoning_desc: t.lbl_default_reasoning_desc.into(),
        btn_reasoning_native: t.btn_reasoning_native.into(),
        btn_reasoning_off: t.btn_reasoning_off.into(),
        btn_reasoning_low: t.btn_reasoning_low.into(),
        btn_reasoning_medium: t.btn_reasoning_medium.into(),
        btn_reasoning_high: t.btn_reasoning_high.into(),
    };
    ui.set_github_update_status(t.btn_check_updates.clone().into());
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
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .arg("-f")
            .arg("-t")
            .arg(&icons_base)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = std::process::Command::new("fc-cache")
            .arg("-f")
            .arg(&fonts_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
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
            claude_percent: 100,
            claude_reset: "in 2h 44m".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 4d 21h".into(),
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
            claude_percent: 100,
            claude_reset: "in 4h 00m".into(),
            claude_weekly_percent: 62,
            claude_weekly_reset: "in 1d 02h".into(),
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

    let models = get_display_models(&AppConfig::default());
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

    // Stats tab metrics (zeros when no live stats)
    main_window.set_stat_prompt_tokens("0".into());
    main_window.set_stat_completion_tokens("0".into());
    main_window.set_stat_thinking_tokens("0".into());
    main_window.set_stat_prompt_pct(0);
    main_window.set_stat_comp_pct(0);
    main_window.set_stat_think_pct(0);
    main_window.set_stat_success_rate("0%".into());
    main_window.set_stat_success_pct(0);
    main_window.set_stat_error_count("0".into());
    main_window.set_stat_error_pct(0);
    main_window.set_stat_max_latency("0 ms".into());
    main_window.set_stat_claude_pct(0);
    main_window.set_stat_gemini_pct(0);

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

    let mut single_instance_guard = match instance::acquire_single_instance(!args.start_minimized) {
        instance::InstanceAcquireResult::Primary(guard) => Some(guard),
        instance::InstanceAcquireResult::AlreadyRunning => {
            log::info!("AMR is already running; signaled existing instance and exiting.");
            return Ok(());
        }
    };

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
    let (proxy_reload_tx, mut proxy_reload_rx) = watch::channel(0u64);

    let proxy_running = Arc::new(AtomicBool::new(false));

    // Shared AppState for server and direct UI actions (diagnostics, MCP, proxy testing)
    let shared_http_client = reqwest::Client::builder()
        .tcp_nodelay(true)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .pool_idle_timeout(Some(Duration::from_secs(90)))
        .pool_max_idle_per_host(32)
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    let shared_app_state = Arc::new(AppState {
        config: Arc::clone(&config_arc),
        stats: Arc::clone(&stats_arc),
        balancer: Arc::clone(&balancer_arc),
        http_client: shared_http_client,
        cfg_version: Arc::clone(&cfg_version),
        proxy_clients: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
    });

    // Spawn proxy server in background
    let s_cfg = Arc::clone(&config_arc);
    let mut s_stop_rx = stop_receiver.clone();
    let p_running_srv = Arc::clone(&proxy_running);
    let s_state = (*shared_app_state).clone();

    tokio::spawn(async move {
        let state = s_state;

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
                    let bind_err = format!("Port {} is already in use or cannot be bound", addr);
                    state.stats.record_detailed_request(
                        "BIND",
                        &addr,
                        "proxy-listener",
                        500,
                        0,
                        0,
                        "System",
                        "-",
                        "",
                        "",
                        "",
                        Some(bind_err.as_str()),
                    );
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
                _ = proxy_reload_rx.changed() => {
                    log::info!("AMR server reloading listener with new network config.");
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
    let bg_balancer = Arc::clone(&balancer_arc);
    let bg_state = Arc::clone(&shared_app_state);
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

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = bg_stop_rx.changed() => {
                    break;
                }
            }

            bg_balancer.cleanup_expired_sessions(3600);

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

                let active_acc_id = {
                    let cfg = bg_cfg.read();
                    cfg.accounts
                        .iter()
                        .find(|a| active_id.as_deref() == Some(&a.id) && a.is_active)
                        .map(|a| a.id.clone())
                };

                if let Some(id) = active_acc_id {
                    sync_account_quota_and_models(&bg_state, &id, false, false).await;
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

                let pool_account_ids: Vec<String> = {
                    let cfg = bg_cfg.read();
                    cfg.accounts
                        .iter()
                        .filter(|a| {
                            a.is_active
                                && active_id.as_deref() != Some(&a.id)
                                && !(a.status == "ERROR" && a.refresh_token.is_none())
                        })
                        .map(|a| a.id.clone())
                        .collect()
                };

                for id in pool_account_ids {
                    // Stagger pool account checks with randomized 3 to 7 second pauses to avoid bot-like bursting
                    tokio::time::sleep(Duration::from_millis(AppConfig::random_delay_ms(
                        3000, 7000,
                    )))
                    .await;
                    sync_account_quota_and_models(&bg_state, &id, false, false).await;
                }
            }
        }
    });

    // Startup dynamic catalog & quota sync across all active accounts
    let startup_cfg = Arc::clone(&config_arc);
    let startup_state = Arc::clone(&shared_app_state);
    tokio::spawn(async move {
        let active_account_ids: Vec<String> = {
            let cfg = startup_cfg.read();
            cfg.accounts
                .iter()
                .filter(|a| a.is_active && !a.api_key.is_empty())
                .map(|a| a.id.clone())
                .collect()
        };

        if active_account_ids.is_empty() {
            return;
        }

        log::info!(
            "Synchronizing {} active upstream account(s) on proxy launch...",
            active_account_ids.len()
        );

        for (idx, acc_id) in active_account_ids.into_iter().enumerate() {
            if idx > 0 {
                tokio::time::sleep(Duration::from_millis(AppConfig::random_delay_ms(400, 1200)))
                    .await;
            }
            if sync_account_quota_and_models(&startup_state, &acc_id, true, false).await {
                log::info!("Synchronized upstream models for {} on startup", acc_id);
            }
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
                            use i_slint_backend_winit::WinitWindowAccessor;
                            let is_showing = w
                                .window()
                                .with_winit_window(|win| {
                                    win.is_visible().unwrap_or(true)
                                        && !win.is_minimized().unwrap_or(false)
                                })
                                .unwrap_or_else(|| vis.load(Ordering::SeqCst));
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

    if let Some(ref mut guard) = single_instance_guard {
        let w_ipc = window_handle.clone();
        let vis_ipc = Arc::clone(&is_window_visible);
        guard.start_activation_listener(move || {
            let win_weak = w_ipc.clone();
            let vis = Arc::clone(&vis_ipc);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_weak.upgrade() {
                    show_and_focus_window(&w);
                    vis.store(true, Ordering::SeqCst);
                }
            });
        });
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
        main_window.set_group_reasoning_clones(cfg.group_reasoning_clones);
        main_window.set_quota_display_mode(cfg.quota_display_mode.clone().into());
        main_window.set_quota_view_mode(cfg.quota_view_mode.clone().into());
        main_window.set_hide_sidebar_brand(cfg.hide_sidebar_brand);
        main_window.set_show_logs_tab(cfg.show_logs_tab);
        main_window.set_show_stats_tab(cfg.show_stats_tab);
        main_window.set_show_connectors_tab(cfg.show_connectors_tab);
        main_window.set_default_reasoning_mode(cfg.default_reasoning_mode.clone().into());
        main_window.set_mcp_enabled(cfg.mcp_enabled);
        main_window.set_mcp_search_max_results(cfg.mcp_search_max_results as i32);
        main_window.set_standardized_error_handling(cfg.standardized_error_handling);
        main_window.set_hide_unavailable_models(cfg.hide_unavailable_models);
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
        main_window.set_stealth_spoof_platform(cfg.spoof_platform.clone().into());
        main_window.set_stealth_spoof_ide_version(cfg.spoof_ide_version.clone().into());
        main_window.set_stealth_strip_telemetry(cfg.strip_telemetry_traces);
        main_window.set_stealth_auto_rotate_fp(cfg.auto_rotate_fp_on_limit);
        main_window.set_stealth_normalize_headers(cfg.normalize_headers);
        main_window.set_stealth_cooldown_jitter_pct(cfg.cooldown_jitter_percent as i32);
        main_window.set_active_account_refresh_mins(cfg.active_account_refresh_mins as i32);
        main_window.set_pool_account_refresh_mins(cfg.pool_account_refresh_mins as i32);
        main_window.set_setting_minimize_tray(cfg.minimize_to_tray);
        main_window.set_setting_autostart(cfg.autostart);
        main_window.set_setting_portable_mode(cfg.portable_mode);
        main_window.set_routing_strategy_index(cfg.routing_strategy.to_index());
        main_window.set_cache_affinity_mode_index(cfg.cache_affinity_mode.to_index());
        main_window.set_failover_on_429(cfg.failover_on_429);
        main_window.set_logging_enabled(cfg.log_enabled);
        main_window.set_max_log_capacity(cfg.max_log_entries as i32);
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
                quota_percent: acc.claude_percent,
                quota_reset: AppConfig::localize_reset_str(&acc.claude_reset, is_ru, is_zh)
                    .into(),
                claude_percent: acc.claude_percent,
                claude_reset: AppConfig::localize_reset_str(
                    &acc.claude_reset,
                    is_ru,
                    is_zh,
                )
                .into(),
                claude_weekly_percent: acc.claude_weekly_percent,
                claude_weekly_reset: AppConfig::localize_reset_str(
                    &acc.claude_weekly_reset,
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

        main_window.set_global_claude_percent(cfg.aggregate_claude_percent());
        main_window.set_global_claude_label(cfg.format_claude_quota().into());
        main_window.set_global_claude_reset(cfg.nearest_claude_reset().into());
        main_window.set_global_claude_weekly_percent(cfg.aggregate_claude_weekly_percent());
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
    let h_mod = window_handle.clone();
    main_window.on_toggle_model(move |id_slint| {
        let id_str = id_slint.to_string();
        {
            let mut cfg = c_mod.write();
            if let Some(item) = cfg.models.iter_mut().find(|m| m.id == id_str) {
                item.is_enabled = !item.is_enabled;
            }
            let _ = cfg.save();
            amr::connectors::sync_all_enabled(&cfg);
            if let Some(h) = h_mod.upgrade() {
                refresh_connectors_ui(&h, &cfg);
            }
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
            cfg.rebuild_models_from_accounts();
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
                let mut builder = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30));
                if let Some(ref p_url) = proxy_opt {
                    if let Ok(p) = reqwest::Proxy::all(p_url) {
                        builder = builder.proxy(p);
                    }
                }
                let http_client = builder.build().unwrap_or_default();
                let ua = configured_ua_for_config(&c_clone.read());
                let mut access_token = token.clone();
                let mut refresh_token_opt = None;

                if token.starts_with("1//") {
                    refresh_token_opt = Some(token.clone());
                    if let Ok(tok_res) =
                        antigravity::refresh_token_with_ua(&http_client, &token, Some(&ua)).await
                    {
                        access_token = tok_res.access_token;
                        if tok_res.refresh_token.is_some() {
                            refresh_token_opt = tok_res.refresh_token;
                        }
                    }
                }

                let email = match antigravity::fetch_user_info_with_ua(
                    &http_client,
                    &access_token,
                    Some(&ua),
                )
                .await
                {
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

                let (project_id, tier) = antigravity::fetch_project_and_tier_with_ua(
                    &http_client,
                    &access_token,
                    Some(&ua),
                )
                .await;

                let (acc_models, _fetched_models, quota_used, quota_limit, quota_reset, detailed_quota) =
                    match antigravity::fetch_account_models_and_quota_with_ua(
                        &http_client,
                        &access_token,
                        project_id.as_deref(),
                        Some(&ua),
                    )
                    .await
                    {
                        Ok(res) => (res.0, Some(res.1), res.2, res.3, res.4, res.5),
                        Err(_) => {
                            let det = antigravity::fetch_user_quota_summary_with_ua(
                                &http_client,
                                &access_token,
                                Some(&ua),
                            )
                            .await;
                            (Vec::new(), None, 0, 100_000, "in 4h 00m".to_string(), det)
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
                    claude_percent: detailed_quota.claude_percent,
                    claude_reset: detailed_quota.claude_reset,
                    claude_weekly_percent: detailed_quota.claude_weekly_percent,
                    claude_weekly_reset: detailed_quota.claude_weekly_reset,
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
                    available_models: acc_models,
                };

                {
                    let mut cfg = c_clone.write();
                    cfg.accounts.push(new_acc);
                    cfg.rebuild_models_from_accounts();
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
    let state_ref = Arc::clone(&shared_app_state);
    main_window.on_refresh_account_quota(move |id_slint| {
        let id_str = id_slint.to_string();
        let c_clone = Arc::clone(&c_ref);
        let v_clone = Arc::clone(&v_ref);
        let states_clone = Arc::clone(&states_ref);
        let state_clone = Arc::clone(&state_ref);

        let lang = Language::from_str_code(&c_ref.read().language);
        let updating_lbl = match lang {
            Language::Ru => "Обновление...",
            Language::Zh => "正在刷新...",
            Language::En => "Updating...",
        };
        states_ref.write().insert(id_str.clone(), (true, updating_lbl.to_string()));
        v_ref.fetch_add(1, Ordering::SeqCst);

        tokio::spawn(async move {
            let success = sync_account_quota_and_models(&state_clone, &id_str, true, true).await;

            let current_lang = Language::from_str_code(&c_clone.read().language);
            let result_str = if success {
                match current_lang {
                    Language::Ru => "Обновлено",
                    Language::Zh => "已刷新",
                    Language::En => "Updated",
                }
            } else {
                match current_lang {
                    Language::Ru => "Ошибка",
                    Language::Zh => "失败",
                    Language::En => "Failed",
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
                h.set_info_account_status(acc.status.clone().into());
                h.set_info_account_claude_5h_pct(acc.claude_percent);
                h.set_info_account_claude_5h_reset(acc.claude_reset.clone().into());
                h.set_info_account_claude_week_pct(acc.claude_weekly_percent);
                h.set_info_account_claude_week_reset(acc.claude_weekly_reset.clone().into());
                h.set_info_account_gemini_5h_pct(acc.gemini_percent);
                h.set_info_account_gemini_5h_reset(acc.gemini_reset.clone().into());
                h.set_info_account_gemini_week_pct(acc.gemini_weekly_percent);
                h.set_info_account_gemini_week_reset(acc.gemini_weekly_reset.clone().into());
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

                let active_models_count = acc
                    .available_models
                    .iter()
                    .filter(|m| !m.disabled && m.remaining_fraction > 0.0)
                    .count();
                let total_models_count = acc.available_models.len();
                h.set_info_account_models_summary(
                    format!("{} / {} models active & healthy", active_models_count, total_models_count).into(),
                );
                h.set_proxy_test_status("".into());
                h.set_proxy_test_is_running(false);

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

    // Test account proxy connection callback
    let h_test_proxy = window_handle.clone();
    main_window.on_test_account_proxy(move |proxy_url_slint| {
        let proxy_url = proxy_url_slint.to_string();
        if let Some(h) = h_test_proxy.upgrade() {
            h.set_proxy_test_is_running(true);
            h.set_proxy_test_status("Testing connection...".into());
        }
        let h_inner = h_test_proxy.clone();
        tokio::spawn(async move {
            let res = amr::proxy::server::test_proxy_connection(&proxy_url).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_inner.upgrade() {
                    h.set_proxy_test_is_running(false);
                    match res {
                        Ok(ms) => h.set_proxy_test_status(format!("Online ({} ms)", ms).into()),
                        Err(e) => h.set_proxy_test_status(format!("Failed: {}", e).into()),
                    }
                }
            });
        });
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

    // Initial connectors migration, auto-sync, and UI population
    {
        let mut cfg = config_arc.write();
        if amr::connectors::migrate_legacy_connector_ids(&mut cfg) {
            let _ = cfg.save();
        }
        amr::connectors::sync_all_enabled(&cfg);
        refresh_connectors_ui(&main_window, &cfg);
    }

    let c_conn_toggle = Arc::clone(&config_arc);
    let h_conn_toggle = window_handle.clone();
    main_window.on_toggle_connector(move |id_slint, enable| {
        let id = id_slint.to_string();
        let mut cfg = c_conn_toggle.write();
        if enable {
            if !cfg.enabled_connectors.contains(&id) {
                cfg.enabled_connectors.push(id.clone());
            }
            let _ = amr::connectors::sync_connector(&id, &cfg, true);
        } else {
            cfg.enabled_connectors.retain(|x| x != &id);
            let _ = amr::connectors::sync_connector(&id, &cfg, false);
        }
        let _ = cfg.save();
        if let Some(h) = h_conn_toggle.upgrade() {
            refresh_connectors_ui(&h, &cfg);
        }
    });

    let c_conn_sync = Arc::clone(&config_arc);
    let h_conn_sync = window_handle.clone();
    main_window.on_sync_connector_now(move |id_slint| {
        let id = id_slint.to_string();
        {
            let cfg = c_conn_sync.read();
            let _ = amr::connectors::sync_connector(&id, &cfg, true);
            if let Some(h) = h_conn_sync.upgrade() {
                refresh_connectors_ui(&h, &cfg);
                h.set_conn_sync_flash_id(id.clone().into());
            }
        }
        let h_timer = h_conn_sync.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(1400)).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_timer.upgrade() {
                    if h.get_conn_sync_flash_id().as_str() == id {
                        h.set_conn_sync_flash_id("".into());
                    }
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
            amr::connectors::sync_all_enabled(&cfg);
            cfg.language.clone()
        };
        if let Some(h) = h_regen.upgrade() {
            let t = amr::i18n::get_translations(amr::i18n::Language::from_str_code(&lang));
            h.set_master_api_key(new_key.into());
            h.set_master_key_status_text(t.lbl_updated.into());
            let cfg = c_regen.read();
            refresh_connectors_ui(&h, &cfg);
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
    let latest_update_tag: Arc<parking_lot::RwLock<Option<String>>> =
        Arc::new(parking_lot::RwLock::new(None));
    let latest_tag_for_click = Arc::clone(&latest_update_tag);
    let latest_tag_for_startup = Arc::clone(&latest_update_tag);
    let h_gh_startup = window_handle.clone();
    let c_gh_startup = Arc::clone(&config_arc);

    main_window.on_check_github_updates(move || {
        if latest_tag_for_click.read().is_some() {
            open_in_system("https://github.com/Qyzom/Antigravity-Manager-Rust/releases/latest");
            return;
        }

        let lang_code = c_gh_lang.read().language.clone();
        let t_init = amr::i18n::get_translations(amr::i18n::Language::from_str_code(&lang_code));
        if let Some(h) = h_gh.upgrade() {
            h.set_github_checking_update(true);
            h.set_github_update_status(t_init.lbl_checking_updates.into());
        }
        let h_res = h_gh.clone();
        let h_gh_reset = h_gh.clone();
        let lang_for_async = lang_code.clone();
        let latest_tag_async = Arc::clone(&latest_tag_for_click);
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
            let mut has_update = false;
            let status_str = match res {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                            if is_newer_version(tag, env!("CARGO_PKG_VERSION")) {
                                has_update = true;
                                *latest_tag_async.write() = Some(tag.to_string());
                                format!("{} {}", t.btn_download_update, tag)
                            } else {
                                format!("{} ({})", current_ver, t.lbl_latest_version)
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

            // Automatically revert button text only if no update was found
            if !has_update {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(h) = h_gh_reset.upgrade() {
                        let t_rev = amr::i18n::get_translations(amr::i18n::Language::from_str_code(
                            &lang_for_async,
                        ));
                        h.set_github_update_status(t_rev.btn_check_updates.into());
                    }
                });
            }
        });
    });

    // Check for updates on startup if enabled
    if config_arc.read().check_updates_startup {
        let h_startup = h_gh_startup.clone();
        let c_startup = c_gh_startup.clone();
        let latest_tag_startup = Arc::clone(&latest_tag_for_startup);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default();
            if let Ok(resp) = client
                .get("https://api.github.com/repos/Qyzom/Antigravity-Manager-Rust/releases/latest")
                .header("User-Agent", "AMR-Client")
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                            if is_newer_version(tag, env!("CARGO_PKG_VERSION")) {
                                *latest_tag_startup.write() = Some(tag.to_string());
                                let lang_code = c_startup.read().language.clone();
                                let t = amr::i18n::get_translations(
                                    amr::i18n::Language::from_str_code(&lang_code),
                                );
                                let status_str = format!("{} {}", t.btn_download_update, tag);
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(h) = h_startup.upgrade() {
                                        h.set_github_update_status(status_str.into());
                                    }
                                });
                            }
                        }
                    }
                }
            }
        });
    }

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

    // Open Model Diagnostics Dialog callback
    let h_diag_dlg = window_handle.clone();
    let c_diag_dlg = Arc::clone(&config_arc);
    main_window.on_open_diagnostic_dialog(move |model_id_slint| {
        let model_id = model_id_slint.to_string();
        if let Some(h) = h_diag_dlg.upgrade() {
            let cfg = c_diag_dlg.read();
            let lang = Language::from_str_code(&cfg.language);
            let t = get_translations(lang);

            let (model_name, thinking) = if let Some(m) = cfg.models.iter().find(|m| m.id == model_id) {
                (m.display_name.clone(), m.supports_thinking)
            } else {
                (model_id.clone(), false)
            };

            // Accounts breakdown
            let mut breakdown_lines = Vec::new();
            for acc in &cfg.accounts {
                let acc_label = if !acc.name.is_empty() { &acc.name } else { &acc.id };
                if !acc.is_active {
                    breakdown_lines.push(format!("{}: {}", acc_label, t.lbl_stage_quarantine));
                    continue;
                }
                if let Some(m) = acc.available_models.iter().find(|m| m.id == model_id || m.id.starts_with(&model_id) || model_id.starts_with(&m.id)) {
                    if m.disabled {
                        breakdown_lines.push(format!("{}: {}", acc_label, t.lbl_model_disabled));
                    } else {
                        let pct = (m.remaining_fraction * 100.0).round() as i32;
                        breakdown_lines.push(format!("{}: {}% quota", acc_label, pct));
                    }
                } else {
                    breakdown_lines.push(format!("{}: Not in catalog", acc_label));
                }
            }
            let breakdown_text = if breakdown_lines.is_empty() {
                "No accounts configured".to_string()
            } else {
                breakdown_lines.join("\n")
            };

            let notice = cfg.models.iter().find(|m| m.id == model_id).map(|m| m.notice.clone()).unwrap_or_default();
            h.set_diag_model_id(model_id.into());
            h.set_diag_model_name(model_name.into());
            h.set_diag_model_notice(notice.into());
            h.set_diag_model_thinking(thinking);
            h.set_diag_accounts_breakdown(breakdown_text.into());
            h.set_diag_status_text("".into());
            h.set_diag_latency_text("—".into());
            h.set_diag_last_error("".into());
            h.set_diag_is_running(false);

            h.set_show_diagnostic_dialog(true);
        }
    });

    // Run Model Diagnostic Check callback
    let h_run_diag = window_handle.clone();
    let state_diag = Arc::clone(&shared_app_state);
    main_window.on_run_model_diagnostic(move |model_id_slint| {
        let model_id = model_id_slint.to_string();
        if let Some(h) = h_run_diag.upgrade() {
            h.set_diag_is_running(true);
            h.set_diag_status_text("Running diagnostic check...".into());
            h.set_diag_latency_text("—".into());
            h.set_diag_last_error("".into());
        }
        let h_inner = h_run_diag.clone();
        let st = Arc::clone(&state_diag);
        tokio::spawn(async move {
            let (status, latency_ms, msg) = amr::proxy::server::run_diagnostic_check(&st, &model_id).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_inner.upgrade() {
                    h.set_diag_is_running(false);
                    h.set_diag_latency_text(format!("{} ms", latency_ms).into());
                    if status == 200 {
                        h.set_diag_status_text("Success (200 OK)".into());
                        h.set_diag_last_error("".into());
                    } else {
                        h.set_diag_status_text(format!("Failed (HTTP {})", status).into());
                        h.set_diag_last_error(msg.into());
                    }
                }
            });
        });
    });

    // Open MCP Configuration Dialog callback
    let h_mcp_cfg_dlg = window_handle.clone();
    main_window.on_open_mcp_config_dialog(move || {
        if let Some(h) = h_mcp_cfg_dlg.upgrade() {
            h.set_show_mcp_config_dialog(true);
        }
    });

    // Run MCP Tools Diagnostic Check callback
    let h_mcp_diag = window_handle.clone();
    let state_mcp = Arc::clone(&shared_app_state);
    main_window.on_run_mcp_diagnostic(move || {
        if let Some(h) = h_mcp_diag.upgrade() {
            h.set_mcp_is_testing(true);
            h.set_mcp_test_status("Testing...".into());
            h.set_mcp_test_result_text("".into());
        }
        let h_inner = h_mcp_diag.clone();
        let st = Arc::clone(&state_mcp);
        tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            let fetch_res = amr::proxy::mcp::execute_mcp_tool_direct(&st, "fetch_web_page", "https://example.com").await;
            let search_res = amr::proxy::mcp::execute_mcp_tool_direct(&st, "google_web_search", "Rust").await;
            let elapsed = t0.elapsed().as_millis();

            let fetch_ok = !fetch_res.contains("Failed to fetch") && !fetch_res.contains("Security error");
            let search_ok = !search_res.contains("Search failed") && !search_res.is_empty();

            let summary = if search_ok && fetch_ok {
                format!("Success (Search & Fetch active • {} ms)", elapsed)
            } else if search_ok {
                format!("Search OK, Fetch failed ({} ms)", elapsed)
            } else if fetch_ok {
                format!("Fetch OK, Search failed ({} ms)", elapsed)
            } else {
                format!("Failed ({} ms)", elapsed)
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_inner.upgrade() {
                    h.set_mcp_is_testing(false);
                    h.set_mcp_test_status("Test MCP Tools".into());
                    h.set_mcp_test_result_text(summary.into());
                }
            });
        });
    });

    // MCP Search Max Results callback
    let c_mcp_limit = Arc::clone(&config_arc);
    let h_mcp_limit = window_handle.clone();
    main_window.on_set_mcp_search_max_results(move |val| {
        let limit = (val as u32).clamp(1, 25);
        {
            let mut cfg = c_mcp_limit.write();
            cfg.mcp_search_max_results = limit;
            let _ = cfg.save();
        }
        if let Some(h) = h_mcp_limit.upgrade() {
            h.set_mcp_search_max_results(limit as i32);
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
                let truncate_ui = |s: &str| -> String {
                    const MAX_UI_BYTES: usize = 12_000;
                    if s.len() <= MAX_UI_BYTES {
                        s.to_string()
                    } else {
                        let mut end = MAX_UI_BYTES;
                        while !s.is_char_boundary(end) && end > 0 {
                            end -= 1;
                        }
                        format!(
                            "{}\n\n... [Truncated {} bytes for UI smoothness — click Copy for full payload]",
                            &s[..end],
                            s.len() - end
                        )
                    }
                };
                let req_full = if item.request_body.is_empty() {
                    "{}".to_string()
                } else {
                    item.request_body.clone()
                };
                let resp_full = if item.response_body.is_empty() {
                    if let Some(ref err) = item.error_msg {
                        err.clone()
                    } else {
                        "(empty or streaming response)".to_string()
                    }
                } else {
                    item.response_body.clone()
                };
                h.set_modal_log_req_body(truncate_ui(&req_full).into());
                h.set_modal_log_resp_body(truncate_ui(&resp_full).into());
                h.set_modal_log_req_body_full(req_full.into());
                h.set_modal_log_resp_body_full(resp_full.into());
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

    let c_sconn = Arc::clone(&config_arc);
    let h_sconn = window_handle.clone();
    main_window.on_toggle_show_connectors(move || {
        let next_val = {
            let mut cfg = c_sconn.write();
            cfg.show_connectors_tab = !cfg.show_connectors_tab;
            let _ = cfg.save();
            cfg.show_connectors_tab
        };
        if let Some(h) = h_sconn.upgrade() {
            h.set_show_connectors_tab(next_val);
        }
    });

    let c_def_reason = Arc::clone(&config_arc);
    let h_def_reason = window_handle.clone();
    main_window.on_set_default_reasoning_mode(move |mode| {
        let m = mode.to_string();
        {
            let mut cfg = c_def_reason.write();
            cfg.default_reasoning_mode = m.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_def_reason.upgrade() {
            h.set_default_reasoning_mode(m.into());
        }
    });

    let c_mcp = Arc::clone(&config_arc);
    let h_mcp = window_handle.clone();
    main_window.on_toggle_mcp_enabled(move || {
        let next_val = {
            let mut cfg = c_mcp.write();
            cfg.mcp_enabled = !cfg.mcp_enabled;
            let _ = cfg.save();
            cfg.mcp_enabled
        };
        if let Some(h) = h_mcp.upgrade() {
            h.set_mcp_enabled(next_val);
        }
    });

    let c_err_norm = Arc::clone(&config_arc);
    let h_err_norm = window_handle.clone();
    main_window.on_toggle_standardized_error_handling(move || {
        let next_val = {
            let mut cfg = c_err_norm.write();
            cfg.standardized_error_handling = !cfg.standardized_error_handling;
            let _ = cfg.save();
            cfg.standardized_error_handling
        };
        if let Some(h) = h_err_norm.upgrade() {
            h.set_standardized_error_handling(next_val);
        }
    });

    let c_hide_unavail = Arc::clone(&config_arc);
    let h_hide_unavail = window_handle.clone();
    let m_hide_unavail = models_model.clone();
    main_window.on_toggle_hide_unavailable_models(move || {
        let (next_val, display_models) = {
            let mut cfg = c_hide_unavail.write();
            cfg.hide_unavailable_models = !cfg.hide_unavailable_models;
            let _ = cfg.save();
            amr::connectors::sync_all_enabled(&cfg);
            (cfg.hide_unavailable_models, get_display_models(&cfg))
        };
        m_hide_unavail.set_vec(display_models);
        if let Some(h) = h_hide_unavail.upgrade() {
            h.set_hide_unavailable_models(next_val);
            let cfg = c_hide_unavail.read();
            refresh_connectors_ui(&h, &cfg);
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

    main_window.on_open_github_releases(move || {
        open_in_system("https://github.com/Qyzom/Antigravity-Manager-Rust/releases");
    });

    let c_pport = Arc::clone(&config_arc);
    let h_pport = window_handle.clone();
    let p_reload_port = proxy_reload_tx.clone();
    main_window.on_set_proxy_port_str(move |s| {
        if let Ok(p) = s.to_string().trim().parse::<u16>() {
            let mut cfg = c_pport.write();
            cfg.port = p;
            let _ = cfg.save();
            if let Some(h) = h_pport.upgrade() {
                h.set_proxy_port(p as i32);
            }
            p_reload_port.send_modify(|v| *v += 1);
        }
    });

    let c_phost = Arc::clone(&config_arc);
    let h_phost = window_handle.clone();
    let p_reload_host = proxy_reload_tx.clone();
    main_window.on_set_proxy_host_str(move |s| {
        let host_str = s.to_string().trim().to_string();
        if !host_str.is_empty() {
            let mut cfg = c_phost.write();
            cfg.host = host_str.clone();
            let _ = cfg.save();
            if let Some(h) = h_phost.upgrade() {
                h.set_proxy_host(host_str.into());
            }
            p_reload_host.send_modify(|v| *v += 1);
        }
    });

    let c_ptim = Arc::clone(&config_arc);
    main_window.on_set_proxy_timeout_str(move |s| {
        if let Ok(t) = s.to_string().trim().parse::<u64>() {
            let mut cfg = c_ptim.write();
            cfg.request_timeout_secs = t;
            let _ = cfg.save();
        }
    });

    let c_phb = Arc::clone(&config_arc);
    main_window.on_set_proxy_heartbeat_str(move |s| {
        if let Ok(hb) = s.to_string().trim().parse::<u64>() {
            let mut cfg = c_phb.write();
            cfg.heartbeat_interval_secs = hb;
            let _ = cfg.save();
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
                    (0, 0, 0)
                };
                w.set_stat_prompt_pct(p_pct);
                w.set_stat_comp_pct(c_pct);
                w.set_stat_think_pct(t_pct);

                let (succ_pct, err_pct, succ_rate) = if reqs > 0 {
                    let s = (succ as f64 / reqs as f64) * 100.0;
                    let e = (errs as f64 / reqs as f64) * 100.0;
                    (s.round() as i32, e.round() as i32, format!("{:.1}%", s))
                } else {
                    (0, 0, "0%".to_string())
                };
                w.set_stat_success_rate(succ_rate.into());
                w.set_stat_success_pct(succ_pct);
                w.set_stat_error_count(errs.to_string().into());
                w.set_stat_error_pct(err_pct);
                w.set_stat_max_latency(format!("{} ms", max_lat).into());

                // Model Traffic Share
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
                        (0, 0)
                    }
                };
                w.set_stat_claude_pct(claude_pct);
                w.set_stat_gemini_pct(gemini_pct);

                // Latency Percentiles & Real Cache Hit Rate
                let (p50, p95, p99) = s_timer.get_latency_percentiles();
                let (_, _, _, balancer_hit_rate, _) = b_timer.get_metrics();
                let real_cache_rate = if reqs > 0 {
                    s_timer.get_token_cache_hit_rate(balancer_hit_rate)
                } else {
                    0.0
                };

                w.set_stat_cache_hit_rate(format!("{:.1}%", real_cache_rate).into());
                w.set_stat_p50_latency(format!("{} ms", p50).into());
                w.set_stat_p95_latency(format!("{} ms", p95).into());
                w.set_stat_p99_latency(format!("{} ms", p99).into());

                // MCP & Dynamic Catalog Metrics
                w.set_stat_mcp_total(s_timer.mcp_total_calls.load(Ordering::Relaxed).to_string().into());
                w.set_stat_mcp_search(s_timer.mcp_search_calls.load(Ordering::Relaxed).to_string().into());
                w.set_stat_mcp_fetch(s_timer.mcp_fetch_calls.load(Ordering::Relaxed).to_string().into());
                w.set_stat_catalog_refreshes(s_timer.catalog_refreshes.load(Ordering::Relaxed).to_string().into());

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
                            quota_percent: acc.claude_percent,
                            quota_reset: AppConfig::localize_reset_str(
                                &acc.claude_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            claude_percent: acc.claude_percent,
                            claude_reset: AppConfig::localize_reset_str(
                                &acc.claude_reset,
                                is_ru,
                                is_zh,
                            )
                            .into(),
                            claude_weekly_percent: acc.claude_weekly_percent,
                            claude_weekly_reset: AppConfig::localize_reset_str(
                                &acc.claude_weekly_reset,
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
                    refresh_connectors_ui(&w, &cfg);
                    let cfg_for_sync = cfg.clone();

                    w.set_global_claude_percent(cfg.aggregate_claude_percent());
                    w.set_global_claude_label(cfg.format_claude_quota().into());
                    w.set_global_claude_reset(cfg.nearest_claude_reset().into());
                    w.set_global_claude_weekly_percent(cfg.aggregate_claude_weekly_percent());
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
                    drop(cfg);
                    let w_conn_sync = w_timer.clone();
                    tokio::task::spawn_blocking(move || {
                        amr::connectors::sync_all_enabled(&cfg_for_sync);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w_ui) = w_conn_sync.upgrade() {
                                refresh_connectors_ui(&w_ui, &cfg_for_sync);
                            }
                        });
                    });
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
                let newest_id = logs.first().map(|l| l.id.as_str()).unwrap_or("");
                let current_first_id = if logs_model.row_count() > 0 {
                    logs_model.row_data(0).map(|r| r.id.to_string()).unwrap_or_default()
                } else {
                    String::new()
                };
                if logs.len() != logs_model.row_count() || newest_id != current_first_id {
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
            *tray_close.borrow_mut() = None;
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

    *tray_holder.borrow_mut() = None;
    drop(single_instance_guard);
    let _ = stop_sender.send(true);
    if config_arc.read().clear_logs_on_exit {
        stats_arc.clear_logs();
    }
    Ok(())
}
