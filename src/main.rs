slint::include_modules!();

use aroxy::cli::CliArgs;
use aroxy::config::{AppConfig, RoutingStrategy, UpstreamAccount};
use aroxy::proxy::server::{create_proxy_router, AppState};
use aroxy::proxy::stats::ProxyStats;
use aroxy::tray::{SystemTray, TrayAction};
use clap::Parser;
use parking_lot::RwLock;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

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
    let (stop_sender, stop_receiver) = watch::channel(false);

    let proxy_running = Arc::new(AtomicBool::new(true));

    // Spawn proxy server in background
    let s_cfg = Arc::clone(&config_arc);
    let s_stats = Arc::clone(&stats_arc);
    let mut s_stop_rx = stop_receiver.clone();

    tokio::spawn(async move {
        let port = s_cfg.read().port;
        let addr = format!("127.0.0.1:{}", port);
        log::info!("Starting Aroxy core server on http://{}", addr);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        let state = AppState {
            config: s_cfg,
            stats: s_stats,
            http_client: client,
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
    {
        let cfg = config_arc.read();
        for acc in &cfg.accounts {
            let pct = if acc.quota_limit > 0 {
                ((acc.quota_used as f64 / acc.quota_limit as f64) * 100.0) as i32
            } else {
                0
            };
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
                quota_percent: pct,
                quota_reset: acc.quota_reset.clone().into(),
                status: acc.status.clone().into(),
                is_active: acc.is_active,
            });
        }
    }
    main_window.set_accounts(ModelRc::from(accounts_model.clone()));
    main_window.set_proxy_running(true);
    let port = config_arc.read().port;
    main_window.set_proxy_port(port as i32);
    main_window.set_proxy_host("127.0.0.1".into());

    // Update global quota state on startup
    {
        let cfg = config_arc.read();
        let total_used = cfg.total_quota_used();
        let total_limit = cfg.total_quota_limit();
        let pct = if total_limit > 0 {
            ((total_used as f64 / total_limit as f64) * 100.0) as i32
        } else {
            0
        };
        main_window.set_global_quota_used(total_used.to_string().into());
        main_window.set_global_quota_limit(total_limit.to_string().into());
        main_window.set_global_quota_percent(pct);
        main_window.set_healthy_accounts_count(cfg.accounts.iter().filter(|a| a.is_active).count() as i32);
        main_window.set_routing_strategy_index(cfg.routing_strategy.to_index());
        main_window.set_quota_refresh_interval(cfg.quota_refresh_interval_secs as i32);
    }

    // Models catalog model
    let models_model = Rc::new(VecModel::<ModelItemData>::default());
    {
        let cfg = config_arc.read();
        for m in &cfg.models {
            models_model.push(ModelItemData {
                id: m.id.clone().into(),
                display_name: m.display_name.clone().into(),
                provider: m.provider.clone().into(),
                tier: m.tier.clone().into(),
                is_enabled: m.is_enabled,
                supports_thinking: m.supports_thinking,
            });
        }
    }
    main_window.set_models(ModelRc::from(models_model.clone()));
    main_window.set_expose_reasoning_variants(config_arc.read().expose_reasoning_variants);

    // Toggle model callback
    let c_mod = Arc::clone(&config_arc);
    let m_mod = models_model.clone();
    main_window.on_toggle_model(move |id_slint| {
        let id_str = id_slint.to_string();
        let mut next_val = false;
        {
            let mut cfg = c_mod.write();
            if let Some(item) = cfg.models.iter_mut().find(|m| m.id == id_str) {
                item.is_enabled = !item.is_enabled;
                next_val = item.is_enabled;
            }
            let _ = cfg.save();
        }
        for i in 0..m_mod.row_count() {
            if let Some(mut item) = m_mod.row_data(i) {
                if item.id.as_str() == id_str {
                    item.is_enabled = next_val;
                    m_mod.set_row_data(i, item);
                    break;
                }
            }
        }
    });

    // Toggle reasoning variants callback
    let c_var = Arc::clone(&config_arc);
    let h_var = window_handle.clone();
    main_window.on_toggle_reasoning_variants(move || {
        let next_val = {
            let mut cfg = c_var.write();
            cfg.expose_reasoning_variants = !cfg.expose_reasoning_variants;
            let _ = cfg.save();
            cfg.expose_reasoning_variants
        };
        if let Some(h) = h_var.upgrade() {
            h.set_expose_reasoning_variants(next_val);
        }
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

    let s_stats_clear = Arc::clone(&stats_arc);
    main_window.on_clear_logs(move || {
        s_stats_clear.clear_logs();
    });

    // Add account manually callback
    let c_add = Arc::clone(&config_arc);
    let m_add = accounts_model.clone();
    let h_add = window_handle.clone();
    main_window.on_add_account(move || {
        if let Some(h) = h_add.upgrade() {
            let name = h.get_new_acc_name().to_string();
            let key = h.get_new_acc_key().to_string();
            let fp = h.get_new_acc_fp().to_string();

            if name.is_empty() { return; }

            let id = format!("acc-{}", uuid::Uuid::new_v4().simple());
            let new_acc = UpstreamAccount {
                id: id.clone(),
                name: name.clone(),
                provider: "antigravity".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                api_key: key,
                fingerprint: if fp.is_empty() { "Claude Code 2.x".to_string() } else { fp.clone() },
                tier: "Pro".to_string(),
                quota_used: 0,
                quota_limit: 100_000,
                quota_reset: "in 4h 00m".to_string(),
                status: "Active".to_string(),
                is_active: false,
            };

            {
                let mut cfg = c_add.write();
                cfg.accounts.push(new_acc);
                let _ = cfg.save();
            }

            m_add.push(AccountData {
                id: id.into(),
                name: name.into(),
                provider: "antigravity".into(),
                base_url: "https://api.anthropic.com".into(),
                api_key_masked: "••••••••".into(),
                fingerprint: fp.into(),
                tier: "Pro".into(),
                quota_used: 0,
                quota_limit: 100000,
                quota_percent: 0,
                quota_reset: "in 4h 00m".into(),
                status: "Active".into(),
                is_active: false,
            });

            h.set_new_acc_name("".into());
            h.set_new_acc_key("".into());
        }
    });

    // Add account via Browser OAuth loopback callback
    let port_oauth = config_arc.read().port;
    main_window.on_add_account_browser_oauth(move || {
        let callback_url = format!("http://127.0.0.1:{}/oauth/callback", port_oauth);
        log::info!("Opening browser for Antigravity OAuth: {}", callback_url);
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open").arg(&callback_url).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd").args(["/C", "start", &callback_url]).spawn();
    });

    // Refresh quota callback
    let c_ref = Arc::clone(&config_arc);
    let m_ref = accounts_model.clone();
    main_window.on_refresh_account_quota(move |id_slint| {
        let id_str = id_slint.to_string();
        for i in 0..m_ref.row_count() {
            if let Some(mut acc) = m_ref.row_data(i) {
                if acc.id.as_str() == id_str {
                    acc.status = "Active".into();
                    acc.quota_reset = "in 3h 45m".into();
                    m_ref.set_row_data(i, acc);
                    break;
                }
            }
        }
        let _ = c_ref.read();
    });

    // Delete account callback
    let c_del = Arc::clone(&config_arc);
    let m_del = accounts_model.clone();
    main_window.on_delete_account(move |id_slint| {
        let id_str = id_slint.to_string();
        {
            let mut cfg = c_del.write();
            cfg.accounts.retain(|a| a.id != id_str);
            let _ = cfg.save();
        }
        for i in 0..m_del.row_count() {
            if let Some(acc) = m_del.row_data(i) {
                if acc.id.as_str() == id_str {
                    m_del.remove(i);
                    break;
                }
            }
        }
    });

    // Activate account callback
    let c_act = Arc::clone(&config_arc);
    let m_act = accounts_model.clone();
    main_window.on_activate_account(move |id_slint| {
        let id_str = id_slint.to_string();
        {
            let mut cfg = c_act.write();
            for a in &mut cfg.accounts {
                a.is_active = a.id == id_str;
            }
            cfg.active_account_id = Some(id_str.clone());
            let _ = cfg.save();
        }
        for i in 0..m_act.row_count() {
            if let Some(mut acc) = m_act.row_data(i) {
                acc.is_active = acc.id.as_str() == id_str;
                m_act.set_row_data(i, acc);
            }
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

    // Run Playground Test Callback
    let h_play = window_handle.clone();
    let port_play = config_arc.read().port;
    main_window.on_run_playground_test(move || {
        if let Some(h) = h_play.upgrade() {
            h.set_playground_running(true);
            h.set_playground_status("Dispatching...".into());
            let prompt = h.get_playground_prompt().to_string();
            let model = h.get_playground_model().to_string();
            let h_async = h_play.clone();

            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let client = reqwest::Client::new();
                let test_url = format!("http://127.0.0.1:{}/v1/chat/completions", port_play);

                let req_payload = serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": prompt}],
                    "stream": false
                });

                match client.post(&test_url).json(&req_payload).send().await {
                    Ok(resp) => {
                        let status_code = resp.status();
                        let lat = start.elapsed().as_millis();
                        let body_text = resp.text().await.unwrap_or_default();

                        // Parse response or show body
                        let (content_text, tokens) = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_text) {
                            let text = parsed["choices"][0]["message"]["content"].as_str().unwrap_or(&body_text).to_string();
                            let tok = parsed["usage"]["total_tokens"].as_u64().unwrap_or(45);
                            (text, tok)
                        } else {
                            (body_text, 0)
                        };

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = h_async.upgrade() {
                                ui.set_playground_status(format!("{} OK", status_code.as_u16()).into());
                                ui.set_playground_latency(format!("{} ms", lat).into());
                                ui.set_playground_tokens(tokens.to_string().into());
                                ui.set_playground_response(content_text.into());
                                ui.set_playground_running(false);
                            }
                        });
                    }
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = h_async.upgrade() {
                                ui.set_playground_status("Error".into());
                                ui.set_playground_response(format!("Connection error: {}", e).into());
                                ui.set_playground_running(false);
                            }
                        });
                    }
                }
            });
        }
    });

    // Save Settings Callback
    let c_set = Arc::clone(&config_arc);
    let h_set = window_handle.clone();
    main_window.on_save_settings(move || {
        if let Some(h) = h_set.upgrade() {
            let mut cfg = c_set.write();
            cfg.minimize_to_tray = h.get_setting_minimize_tray();
            cfg.portable_mode = h.get_setting_portable_mode();
            cfg.autostart = h.get_setting_autostart();
            cfg.request_timeout_secs = h.get_setting_timeout_secs() as u64;
            cfg.heartbeat_interval_secs = h.get_setting_heartbeat_secs() as u64;
            let _ = cfg.save();
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

    // Periodic UI stats updater timer
    let timer = slint::Timer::default();
    let s_timer = Arc::clone(&stats_arc);
    let w_timer = window_handle.clone();
    let logs_model = Rc::new(VecModel::<LogEntryData>::default());
    main_window.set_log_entries(ModelRc::from(logs_model.clone()));

    timer.start(slint::TimerMode::Repeated, Duration::from_millis(600), move || {
        if let Some(w) = w_timer.upgrade() {
            let (reqs, tokens, avg_lat) = s_timer.get_metrics();
            w.set_total_requests(reqs.to_string().into());
            w.set_total_tokens(tokens.to_string().into());
            w.set_avg_latency(format!("{} ms", avg_lat).into());

            // Detailed stats
            let (_, succ, errs, prompt_tok, comp_tok, think_tok, _, max_lat) = s_timer.get_detailed_stats();
            w.set_stat_prompt_tokens(prompt_tok.to_string().into());
            w.set_stat_completion_tokens(comp_tok.to_string().into());
            w.set_stat_thinking_tokens(think_tok.to_string().into());
            let succ_rate = if reqs > 0 {
                format!("{:.1}%", (succ as f64 / reqs as f64) * 100.0)
            } else {
                "100%".to_string()
            };
            w.set_stat_success_rate(succ_rate.into());
            w.set_stat_error_count(errs.to_string().into());
            w.set_stat_max_latency(format!("{} ms", max_lat).into());

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

    if !args.start_minimized {
        main_window.show()?;
    }

    slint::run_event_loop()?;

    let _ = stop_sender.send(true);
    Ok(())
}
