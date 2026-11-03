slint::include_modules!();

use aroxy::autostart::set_autostart;
use aroxy::cli::CliArgs;
use aroxy::config::{AppConfig, RoutingStrategy, UpstreamAccount};
use aroxy::i18n::{get_translations, Language};
use aroxy::proxy::antigravity;
use aroxy::proxy::server::{create_proxy_router, AppState};
use aroxy::proxy::stats::ProxyStats;
use aroxy::sys_metrics::get_process_memory_string;
use aroxy::tray::{SystemTray, TrayAction};
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
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", target]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
}

fn apply_translations(ui: &MainWindow, lang: Language) {
    let t = get_translations(lang);
    let i18n_struct = I18nData {
        tab_dashboard: t.tab_dashboard.into(),
        tab_accounts: t.tab_accounts.into(),
        tab_models: t.tab_models.into(),
        tab_proxy: t.tab_proxy.into(),
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
        lbl_compact_ui: t.lbl_compact_ui.into(),
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
    };
    ui.set_i18n(i18n_struct);
    ui.set_current_language(lang.to_index());
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = CliArgs::parse();
    let mut config = AppConfig::load();
    if let Some(port) = args.port {
        config.port = port;
    }

    let config_arc = Arc::new(RwLock::new(config));
    let stats_arc = Arc::new(ProxyStats::new());
    let balancer_arc = Arc::new(aroxy::proxy::balancer::LoadBalancer::new());
    let cfg_version = Arc::new(AtomicU64::new(1));
    let (stop_sender, stop_receiver) = watch::channel(false);

    let proxy_running = Arc::new(AtomicBool::new(true));

    // Spawn proxy server in background
    let s_cfg = Arc::clone(&config_arc);
    let s_stats = Arc::clone(&stats_arc);
    let s_balancer = Arc::clone(&balancer_arc);
    let s_ver = Arc::clone(&cfg_version);
    let mut s_stop_rx = stop_receiver.clone();

    tokio::spawn(async move {
        let (host, port) = {
            let cfg = s_cfg.read();
            (cfg.host.clone(), cfg.port)
        };
        let addr = format!("{}:{}", host, port);
        log::info!("Starting Aroxy core server on http://{}", addr);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        let state = AppState {
            config: s_cfg,
            stats: s_stats,
            balancer: s_balancer,
            http_client: client,
            cfg_version: s_ver,
        };

        let app = create_proxy_router(state);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("Failed to bind to {}: {}", addr, e);
                return;
            }
        };

        let server = axum::serve(listener, app);
        tokio::select! {
            res = server => {
                if let Err(e) = res {
                    log::error!("Server error: {}", e);
                }
            }
            _ = s_stop_rx.changed() => {
                log::info!("Aroxy server received shutdown signal.");
            }
        }
    });

    // Background dual-cadence account refresh task
    let bg_cfg = Arc::clone(&config_arc);
    let bg_ver = Arc::clone(&cfg_version);
    let mut bg_stop_rx = stop_receiver.clone();

    tokio::spawn(async move {
        let mut last_active_refresh = tokio::time::Instant::now();
        let mut last_pool_refresh = tokio::time::Instant::now();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                _ = bg_stop_rx.changed() => {
                    break;
                }
            }

            let (active_id, active_mins, pool_mins) = {
                let cfg = bg_cfg.read();
                (
                    cfg.active_account_id.clone(),
                    cfg.active_account_refresh_mins.clamp(1, 10),
                    cfg.pool_account_refresh_mins.clamp(1, 60),
                )
            };

            let now = tokio::time::Instant::now();
            let refresh_active = now.duration_since(last_active_refresh) >= Duration::from_secs((active_mins as u64) * 60);
            let refresh_pool = now.duration_since(last_pool_refresh) >= Duration::from_secs((pool_mins as u64) * 60);

            if refresh_active || refresh_pool {
                let mut accounts_to_refresh = Vec::new();
                {
                    let cfg = bg_cfg.read();
                    for acc in &cfg.accounts {
                        if acc.status == "ERROR" || !acc.is_active {
                            continue;
                        }
                        let is_active_acc = active_id.as_deref() == Some(&acc.id);
                        if is_active_acc && refresh_active {
                            accounts_to_refresh.push((acc.id.clone(), acc.api_key.clone(), acc.refresh_token.clone(), true));
                        } else if !is_active_acc && refresh_pool {
                            accounts_to_refresh.push((acc.id.clone(), acc.api_key.clone(), acc.refresh_token.clone(), false));
                        }
                    }
                }

                if refresh_active {
                    last_active_refresh = now;
                }
                if refresh_pool {
                    last_pool_refresh = now;
                }

                for (id, mut token, refresh_tok, is_active_acc) in accounts_to_refresh {
                    let mut quota_res = antigravity::fetch_models_and_quota(&client, &token, None).await;

                    if quota_res.is_err() {
                        if let Some(ref rtok) = refresh_tok {
                            if let Ok(ref_res) = antigravity::refresh_token(&client, rtok).await {
                                token = ref_res.access_token.clone();
                                {
                                    let mut cfg = bg_cfg.write();
                                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id) {
                                        acc.api_key = token.clone();
                                    }
                                    let _ = cfg.save();
                                }
                                quota_res = antigravity::fetch_models_and_quota(&client, &token, None).await;
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
                                    acc.claude_gpt_weekly_percent = detailed_quota.claude_gpt_weekly_percent;
                                    acc.claude_gpt_weekly_reset = detailed_quota.claude_gpt_weekly_reset;
                                    acc.gemini_percent = detailed_quota.gemini_percent;
                                    acc.gemini_reset = detailed_quota.gemini_reset;
                                    acc.gemini_weekly_percent = detailed_quota.gemini_weekly_percent;
                                    acc.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
                                    acc.error_message = None;
                                    acc.status = "ACTIVE".to_string();
                                }
                                if is_active_acc {
                                    let clean: Vec<_> = models.into_iter().filter(|x| !x.id.starts_with("chat_") && !x.id.contains("gemini-3.5")).collect();
                                    if !clean.is_empty() {
                                        cfg.models = clean;
                                    }
                                }
                                let _ = cfg.save();
                            }
                            bg_ver.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("401") || err_str.contains("403") || err_str.contains("invalid_grant") {
                                {
                                    let mut cfg = bg_cfg.write();
                                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id) {
                                        acc.status = "ERROR".to_string();
                                        acc.is_active = false;
                                        acc.error_message = Some(format!("Authentication failed: {}", err_str));
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

        let (acc_id, token, refresh_tok) = {
            let cfg = startup_cfg.read();
            if let Some(acc) = cfg.accounts.iter().find(|a| a.is_active && a.status != "ERROR" && !a.api_key.is_empty()) {
                (acc.id.clone(), acc.api_key.clone(), acc.refresh_token.clone())
            } else {
                return;
            }
        };

        log::info!("Synchronizing active upstream models on proxy launch...");
        let mut res = antigravity::fetch_models_and_quota(&client, &token, None).await;

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
                    res = antigravity::fetch_models_and_quota(&client, &new_tok, None).await;
                }
            }
        }

        if let Ok((models, used, limit, reset, detailed_quota)) = res {
            log::info!("Successfully synchronized {} models from Google Antigravity on startup", models.len());
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
        log::info!("Aroxy running in headless daemon mode. Press Ctrl+C to stop.");
        tokio::signal::ctrl_c().await?;
        let _ = stop_sender.send(true);
        return Ok(());
    }

    // Launch Slint GUI
    let main_window = MainWindow::new()?;
    let window_handle = main_window.as_weak();

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
        main_window.set_compact_ui_mode(cfg.compact_ui_mode);
        main_window.set_setting_check_updates(cfg.check_updates_startup);
        main_window.set_setting_clear_logs_exit(cfg.clear_logs_on_exit);
        main_window.set_quota_protection_threshold(cfg.quota_protection_threshold as i32);
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
            format!("{} KB", (f_size + 1023) / 1024)
        } else {
            format!("{:.1} MB", size_mb)
        };
        main_window.set_config_folder_size(size_display.into());

        for acc in &cfg.accounts {
            accounts_model.push(AccountData {
                id: acc.id.clone().into(),
                name: acc.name.clone().into(),
                provider: acc.provider.clone().into(),
                base_url: acc.base_url.clone().into(),
                api_key_masked: if acc.api_key.is_empty() { "No key".into() } else { "••••••••".into() },
                fingerprint: acc.fingerprint.clone().into(),
                tier: acc.tier.clone().into(),
                quota_used: acc.quota_used as i32,
                quota_limit: acc.quota_limit as i32,
                quota_percent: acc.claude_gpt_percent,
                quota_reset: acc.claude_gpt_reset.clone().into(),
                claude_gpt_percent: acc.claude_gpt_percent,
                claude_gpt_reset: acc.claude_gpt_reset.clone().into(),
                claude_gpt_weekly_percent: acc.claude_gpt_weekly_percent,
                claude_gpt_weekly_reset: acc.claude_gpt_weekly_reset.clone().into(),
                gemini_percent: acc.gemini_percent,
                gemini_reset: acc.gemini_reset.clone().into(),
                gemini_weekly_percent: acc.gemini_weekly_percent,
                gemini_weekly_reset: acc.gemini_weekly_reset.clone().into(),
                status: acc.status.clone().into(),
                is_active: acc.is_active,
                error_message: acc.error_message.clone().unwrap_or_default().into(),
                is_updating: false,
                update_status_text: "".into(),
            });
        }

        let filtered_models = if cfg.group_reasoning_clones {
            cfg.models.iter().filter(|m| !m.id.ends_with("-low") && !m.id.ends_with("-medium") && !m.id.ends_with("-high")).collect::<Vec<_>>()
        } else {
            cfg.models.iter().collect::<Vec<_>>()
        };

        for m in filtered_models {
            models_model.push(ModelItemData {
                id: m.id.clone().into(),
                display_name: m.display_name.clone().into(),
                provider: m.provider.clone().into(),
                tier: m.tier.clone().into(),
                is_enabled: m.is_enabled,
                supports_thinking: m.supports_thinking,
            });
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
        main_window.set_healthy_accounts_count(cfg.accounts.iter().filter(|a| a.is_active).count() as i32);
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
            let filtered_models = if next_val {
                cfg.models.iter().filter(|m| !m.id.ends_with("-low") && !m.id.ends_with("-medium") && !m.id.ends_with("-high")).collect::<Vec<_>>()
            } else {
                cfg.models.iter().collect::<Vec<_>>()
            };
            m_grp.set_vec(
                filtered_models
                    .into_iter()
                    .map(|m| ModelItemData {
                        id: m.id.clone().into(),
                        display_name: m.display_name.clone().into(),
                        provider: m.provider.clone().into(),
                        tier: m.tier.clone().into(),
                        is_enabled: m.is_enabled,
                        supports_thinking: m.supports_thinking,
                    })
                    .collect::<Vec<_>>(),
            );
        }
        v_grp.fetch_add(1, Ordering::SeqCst);
    });

    // Toggle proxy server state
    let p_running = Arc::clone(&proxy_running);
    let h_weak = window_handle.clone();
    main_window.on_toggle_proxy(move || {
        let current = p_running.load(Ordering::SeqCst);
        let next = !current;
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
            if token.is_empty() { return; }

            h.set_new_acc_token("".into());

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
                            format!("account-{}@antigravity", &uuid::Uuid::new_v4().simple().to_string()[..6])
                        }
                    }
                };

                let (project_id, tier) = antigravity::fetch_project_and_tier(&http_client, &access_token).await;

                let (fetched_models, quota_used, quota_limit, quota_reset, detailed_quota) =
                    match antigravity::fetch_models_and_quota(&http_client, &access_token, project_id.as_deref()).await {
                        Ok(res) => (Some(res.0), res.1, res.2, res.3, res.4),
                        Err(_) => {
                            let det = antigravity::fetch_user_quota_summary(&http_client, &access_token).await;
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
                    fingerprint: "Claude Code 2.x".to_string(),
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
                    is_active: true,
                };

                {
                    let mut cfg = c_clone.write();
                    cfg.accounts.push(new_acc);
                    if let Some(m) = fetched_models {
                        // Filter out any uncurated internal junk models
                        let clean_models: Vec<_> = m.into_iter().filter(|x| !x.id.starts_with("chat_") && !x.id.contains("gemini-3.5")).collect();
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
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open").arg(&auth_url).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd").args(["/C", "start", &auth_url]).spawn();
    });

    // Shared update states for account interactive feedback: id -> (is_updating, status_text)
    let account_update_states = Arc::new(parking_lot::RwLock::new(std::collections::HashMap::<String, (bool, String)>::new()));

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
                acc.status = if acc.is_active { "ACTIVE".to_string() } else { "DISABLED".to_string() };
            }
            let _ = cfg.save();
        }
        for i in 0..acc_model_tog.row_count() {
            if let Some(mut row) = acc_model_tog.row_data(i) {
                if row.id == id_slint {
                    row.is_active = !row.is_active;
                    row.status = if row.is_active { "ACTIVE".into() } else { "DISABLED".into() };
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

        states_ref.write().insert(id_str.clone(), (true, "Updating...".to_string()));
        v_ref.fetch_add(1, Ordering::SeqCst);

        tokio::spawn(async move {
            let (token, project_id) = {
                let cfg = c_clone.read();
                if let Some(acc) = cfg.accounts.iter().find(|a| a.id == id_str) {
                    (acc.api_key.clone(), None::<String>)
                } else {
                    return;
                }
            };

            let client = reqwest::Client::new();
            let res = antigravity::fetch_models_and_quota(&client, &token, project_id.as_deref()).await;
            let success = match res {
                Ok((models, used, limit, reset, detailed_quota)) => {
                    {
                        let mut cfg = c_clone.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
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
                        let clean_models: Vec<_> = models.into_iter().filter(|x| !x.id.starts_with("chat_") && !x.id.contains("gemini-3.5")).collect();
                        if !clean_models.is_empty() {
                            cfg.models = clean_models;
                        }
                        let _ = cfg.save();
                    }
                    true
                }
                Err(_) => false,
            };

            states_clone.write().insert(id_str.clone(), (false, if success { "Updated ✓".to_string() } else { "Failed ✕".to_string() }));
            v_clone.fetch_add(1, Ordering::SeqCst);

            tokio::time::sleep(tokio::time::Duration::from_millis(2500)).await;
            states_clone.write().remove(&id_str);
            v_clone.fetch_add(1, Ordering::SeqCst);
        });
    });

    // Open account details dialog callback
    let c_info = Arc::clone(&config_arc);
    let h_info = window_handle.clone();
    main_window.on_open_account_info(move |id_slint| {
        let id_str = id_slint.to_string();
        if let Some(h) = h_info.upgrade() {
            let cfg = c_info.read();
            if let Some(acc) = cfg.accounts.iter().find(|a| a.id == id_str) {
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
                h.set_info_account_models(format!("{} upstream models available", cfg.models.len()).into());
                h.set_info_account_error(acc.error_message.clone().unwrap_or_default().into());
                h.set_show_account_info(true);
            }
        }
    });

    // Master API Key Callbacks
    let c_mkey = Arc::clone(&config_arc);
    let h_mkey = window_handle.clone();
    main_window.on_copy_master_key(move || {
        let key = {
            let cfg = c_mkey.read();
            cfg.master_api_key.clone()
        };
        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(key));
        if let Some(h) = h_mkey.upgrade() {
            h.set_master_key_status_text("Copied ✓".into());
        }
        let h_timer = h_mkey.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_timer.upgrade() {
                    h.set_master_key_status_text("Copy Key".into());
                }
            });
        });
    });

    let c_regen = Arc::clone(&config_arc);
    let h_regen = window_handle.clone();
    main_window.on_regenerate_master_key(move || {
        let new_key = format!("sk-aroxy-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        {
            let mut cfg = c_regen.write();
            cfg.master_api_key = new_key.clone();
            let _ = cfg.save();
        }
        if let Some(h) = h_regen.upgrade() {
            h.set_master_api_key(new_key.into());
            h.set_master_key_status_text("Regenerated ✓".into());
        }
        let h_timer = h_regen.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_timer.upgrade() {
                    h.set_master_key_status_text("Copy Key".into());
                }
            });
        });
    });

    // GitHub Update Check Callback
    let h_gh = window_handle.clone();
    main_window.on_check_github_updates(move || {
        if let Some(h) = h_gh.upgrade() {
            h.set_github_checking_update(true);
            h.set_github_update_status("Checking GitHub...".into());
        }
        let h_res = h_gh.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default();
            let res = client
                .get("https://api.github.com/repos/Qyzom/Aroxy/releases/latest")
                .header("User-Agent", "Aroxy-Client")
                .send()
                .await;

            let status_str = match res {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                            if tag == "v0.0.2" {
                                "v0.0.2 (Latest ✓)".to_string()
                            } else {
                                format!("Update Available ({})", tag)
                            }
                        } else {
                            "v0.0.2 (Latest ✓)".to_string()
                        }
                    } else {
                        "v0.0.2 (Up-to-date)".to_string()
                    }
                }
                _ => "v0.0.2 (Up-to-date)".to_string(),
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(h) = h_res.upgrade() {
                    h.set_github_checking_update(false);
                    h.set_github_update_status(status_str.into());
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
    main_window.on_open_test_dialog(move |model_id_slint| {
        if let Some(h) = h_test_dlg.upgrade() {
            h.set_test_model_id(model_id_slint);
            h.set_test_status("Ready".into());
            h.set_test_latency("0 ms".into());
            h.set_test_tokens("0".into());
            h.set_test_response("".into());
            h.set_show_test_dialog(true);
        }
    });

    // Test execution cancellation flag
    let test_cancelled = Arc::new(AtomicBool::new(false));

    // Cancel test callback
    let cancel_flag = Arc::clone(&test_cancelled);
    let h_cancel = window_handle.clone();
    main_window.on_cancel_test_request(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        if let Some(h) = h_cancel.upgrade() {
            h.set_test_status("Cancelled".into());
            h.set_test_running(false);
        }
    });

    // Execute test request callback
    let h_test_exec = window_handle.clone();
    let port_test = config_arc.read().port;
    let cancel_exec = Arc::clone(&test_cancelled);
    main_window.on_execute_test_request(move || {
        if let Some(h) = h_test_exec.upgrade() {
            cancel_exec.store(false, Ordering::SeqCst);
            h.set_test_running(true);
            h.set_test_status("Dispatching...".into());
            h.set_test_response("Waiting for first token...".into());

            let prompt = h.get_test_prompt().to_string();
            let model = h.get_test_model_id().to_string();
            let timeout_val: u64 = h.get_test_timeout_secs().to_string().parse().unwrap_or(60);
            let max_tok: u64 = h.get_test_max_tokens().to_string().parse().unwrap_or(2048);
            let temp: f64 = h.get_test_temperature().to_string().parse().unwrap_or(0.7);
            let is_stream = h.get_test_stream();

            let h_async = h_test_exec.clone();
            let cancel_ref = Arc::clone(&cancel_exec);

            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(timeout_val))
                    .build()
                    .unwrap_or_default();

                let test_url = format!("http://127.0.0.1:{}/v1/chat/completions", port_test);
                let req_payload = serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": temp,
                    "max_tokens": max_tok,
                    "stream": is_stream
                });

                if cancel_ref.load(Ordering::SeqCst) {
                    return;
                }

                match client.post(&test_url).json(&req_payload).send().await {
                    Ok(resp) => {
                        let status_code = resp.status();
                        let lat = start.elapsed().as_millis();
                        let body_text = resp.text().await.unwrap_or_default();

                        if cancel_ref.load(Ordering::SeqCst) {
                            return;
                        }

                        let (content_text, tokens) = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_text) {
                            let text = parsed["choices"][0]["message"]["content"].as_str().unwrap_or(&body_text).to_string();
                            let tok = parsed["usage"]["total_tokens"].as_u64().unwrap_or(45);
                            (text, tok)
                        } else {
                            (body_text, 0)
                        };

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = h_async.upgrade() {
                                ui.set_test_status(format!("{} OK", status_code.as_u16()).into());
                                ui.set_test_latency(format!("{} ms", lat).into());
                                ui.set_test_tokens(tokens.to_string().into());
                                ui.set_test_response(content_text.into());
                                ui.set_test_running(false);
                            }
                        });
                    }
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = h_async.upgrade() {
                                ui.set_test_status("Error".into());
                                ui.set_test_response(format!("Request error: {}", e).into());
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
            if let Ok(logs) = s_log_modal.recent_logs.read() {
                if let Some(item) = logs.iter().find(|l| l.id == id_str) {
                    h.set_modal_log_time(item.time.clone().into());
                    h.set_modal_log_method(item.method.clone().into());
                    h.set_modal_log_path(item.path.clone().into());
                    h.set_modal_log_model(item.model.clone().into());
                    h.set_modal_log_status(if item.status == 200 { "200 OK".into() } else { format!("{} Error", item.status).into() });
                    h.set_modal_log_tokens(item.tokens.to_string().into());
                    h.set_modal_log_latency(format!("{} ms", item.latency_ms).into());
                    h.set_show_log_modal(true);
                }
            }
        }
    });

    // Language switcher callback
    let c_lang = Arc::clone(&config_arc);
    let h_lang = window_handle.clone();
    main_window.on_set_language(move |idx| {
        let lang = Language::from_index(idx);
        {
            let mut cfg = c_lang.write();
            cfg.language = lang.to_code().to_string();
            let _ = cfg.save();
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
        let mode = aroxy::config::CacheAffinityMode::from_index(index);
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

    let c_cmp = Arc::clone(&config_arc);
    let h_cmp = window_handle.clone();
    main_window.on_toggle_compact_mode(move || {
        let next_val = {
            let mut cfg = c_cmp.write();
            cfg.compact_ui_mode = !cfg.compact_ui_mode;
            let _ = cfg.save();
            cfg.compact_ui_mode
        };
        if let Some(h) = h_cmp.upgrade() {
            h.set_compact_ui_mode(next_val);
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

    main_window.on_open_github_repo(move || {
        open_in_system("https://github.com/qyzom/aroxy");
    });

    main_window.on_open_github_releases(move || {
        open_in_system("https://github.com/qyzom/aroxy/releases");
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

    let c_cmp = Arc::clone(&config_arc);
    main_window.on_toggle_compact_mode(move || {
        let mut cfg = c_cmp.write();
        cfg.compact_ui_mode = !cfg.compact_ui_mode;
        let _ = cfg.save();
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

    // Initialize System Tray
    let w_tray = window_handle.clone();
    let _tray = SystemTray::new(move |action| match action {
        TrayAction::Show => {
            if let Some(w) = w_tray.upgrade() {
                let _ = w.show();
            }
        }
        TrayAction::Toggle => {
            if let Some(w) = w_tray.upgrade() {
                let _ = w.show();
            }
        }
        TrayAction::Exit => {
            std::process::exit(0);
        }
    });

    // Periodic UI stats and state updater timer
    let timer = slint::Timer::default();
    let s_timer = Arc::clone(&stats_arc);
    let c_timer = Arc::clone(&config_arc);
    let v_timer = Arc::clone(&cfg_version);
    let states_timer = Arc::clone(&account_update_states);
    let mut last_seen_version: u64 = 0;
    let w_timer = window_handle.clone();

    timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
        if let Some(w) = w_timer.upgrade() {
            let (reqs, tokens, avg_lat) = s_timer.get_metrics();
            w.set_total_requests(reqs.to_string().into());
            w.set_total_tokens(tokens.to_string().into());
            w.set_avg_latency(format!("{} ms", avg_lat).into());
            w.set_app_memory(get_process_memory_string().into());

            // Detailed stats
            let (_, succ, errs, prompt_tok, comp_tok, think_tok, _, max_lat) = s_timer.get_detailed_stats();
            w.set_stat_prompt_tokens(prompt_tok.to_string().into());
            w.set_stat_completion_tokens(comp_tok.to_string().into());
            w.set_stat_thinking_tokens(think_tok.to_string().into());

            let total_tok = prompt_tok + comp_tok + think_tok;
            if total_tok > 0 {
                let p_pct = ((prompt_tok as f64 / total_tok as f64) * 100.0).round() as i32;
                let c_pct = ((comp_tok as f64 / total_tok as f64) * 100.0).round() as i32;
                let t_pct = 100 - p_pct - c_pct;
                w.set_stat_prompt_pct(p_pct.max(1));
                w.set_stat_comp_pct(c_pct.max(1));
                w.set_stat_think_pct(t_pct.max(1));
            } else {
                w.set_stat_prompt_pct(50);
                w.set_stat_comp_pct(40);
                w.set_stat_think_pct(10);
            }

            let succ_rate = if reqs > 0 {
                format!("{:.1}%", (succ as f64 / reqs as f64) * 100.0)
            } else {
                "100%".to_string()
            };
            w.set_stat_success_rate(succ_rate.into());
            w.set_stat_error_count(errs.to_string().into());
            w.set_stat_max_latency(format!("{} ms", max_lat).into());

            // Sync accounts & models if changed
            let current_version = v_timer.load(Ordering::Relaxed);
            let accounts_len = c_timer.read().accounts.len();
            if current_version != last_seen_version || accounts_len != accounts_model.row_count() {
                last_seen_version = current_version;
                let cfg = c_timer.read();

                let new_acc_data: Vec<AccountData> = cfg.accounts
                    .iter()
                    .map(|acc| {
                        AccountData {
                            id: acc.id.clone().into(),
                            name: acc.name.clone().into(),
                            provider: acc.provider.clone().into(),
                            base_url: acc.base_url.clone().into(),
                            api_key_masked: if acc.api_key.is_empty() { "No key".into() } else { "••••••••".into() },
                            fingerprint: acc.fingerprint.clone().into(),
                            tier: acc.tier.clone().into(),
                            quota_used: acc.quota_used as i32,
                            quota_limit: acc.quota_limit as i32,
                            quota_percent: acc.claude_gpt_percent,
                            quota_reset: acc.claude_gpt_reset.clone().into(),
                            claude_gpt_percent: acc.claude_gpt_percent,
                            claude_gpt_reset: acc.claude_gpt_reset.clone().into(),
                            claude_gpt_weekly_percent: acc.claude_gpt_weekly_percent,
                            claude_gpt_weekly_reset: acc.claude_gpt_weekly_reset.clone().into(),
                            gemini_percent: acc.gemini_percent,
                            gemini_reset: acc.gemini_reset.clone().into(),
                            gemini_weekly_percent: acc.gemini_weekly_percent,
                            gemini_weekly_reset: acc.gemini_weekly_reset.clone().into(),
                            status: acc.status.clone().into(),
                            is_active: acc.is_active,
                            error_message: acc.error_message.clone().unwrap_or_default().into(),
                            is_updating: false,
                            update_status_text: "".into(),
                        }
                    })
                    .collect();

                if accounts_len != accounts_model.row_count() {
                    let cur_states = states_timer.read();
                    let mut final_accs = new_acc_data;
                    for row in &mut final_accs {
                        if let Some((is_updating, status_text)) = cur_states.get(row.id.as_str()) {
                            row.is_updating = *is_updating;
                            row.update_status_text = status_text.clone().into();
                        }
                    }
                    accounts_model.set_vec(final_accs);
                } else {
                    let cur_states = states_timer.read();
                    for (idx, item) in new_acc_data.into_iter().enumerate() {
                        let mut row = item;
                        if let Some((is_updating, status_text)) = cur_states.get(row.id.as_str()) {
                            row.is_updating = *is_updating;
                            row.update_status_text = status_text.clone().into();
                        }
                        accounts_model.set_row_data(idx, row);
                    }
                }

                let filtered_models = if cfg.group_reasoning_clones {
                    cfg.models.iter().filter(|m| !m.id.ends_with("-low") && !m.id.ends_with("-medium") && !m.id.ends_with("-high")).collect::<Vec<_>>()
                } else {
                    cfg.models.iter().collect::<Vec<_>>()
                };

                models_model.set_vec(
                    filtered_models
                        .into_iter()
                        .map(|m| ModelItemData {
                            id: m.id.clone().into(),
                            display_name: m.display_name.clone().into(),
                            provider: m.provider.clone().into(),
                            tier: m.tier.clone().into(),
                            is_enabled: m.is_enabled,
                            supports_thinking: m.supports_thinking,
                        })
                        .collect::<Vec<_>>(),
                );

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
                w.set_healthy_accounts_count(cfg.accounts.iter().filter(|a| a.is_active).count() as i32);
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
    });

    // Window close intercept for Minimize to System Tray
    let c_close = Arc::clone(&config_arc);
    let w_close = window_handle.clone();
    main_window.window().on_close_requested(move || {
        if c_close.read().minimize_to_tray {
            if let Some(w) = w_close.upgrade() {
                let _ = w.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        } else {
            slint::CloseRequestResponse::HideWindow
        }
    });

    if !args.start_minimized {
        main_window.show()?;
    }

    slint::run_event_loop()?;

    let _ = stop_sender.send(true);
    if config_arc.read().clear_logs_on_exit {
        stats_arc.clear_logs();
    }
    Ok(())
}
