slint::include_modules!();

use aroxy::cli::CliArgs;
use aroxy::config::{AppConfig, RoutingStrategy, UpstreamAccount};
use aroxy::proxy::antigravity;
use aroxy::proxy::server::{create_proxy_router, AppState};
use aroxy::proxy::stats::ProxyStats;
use aroxy::tray::{SystemTray, TrayAction};
use clap::Parser;
use parking_lot::RwLock;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

    main_window.set_proxy_running(true);
    let port = config_arc.read().port;
    main_window.set_proxy_port(port as i32);
    main_window.set_proxy_host("127.0.0.1".into());
    main_window.set_expose_reasoning_variants(config_arc.read().expose_reasoning_variants);

    // Initial state populate
    {
        let cfg = config_arc.read();
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
                gemini_percent: acc.gemini_percent,
                gemini_reset: acc.gemini_reset.clone().into(),
                status: acc.status.clone().into(),
                is_active: acc.is_active,
            });
        }

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
        main_window.set_global_claude_percent(cfg.aggregate_claude_gpt_percent());
        main_window.set_global_claude_reset(cfg.nearest_claude_reset().into());
        main_window.set_global_gemini_percent(cfg.aggregate_gemini_percent());
        main_window.set_global_gemini_reset(cfg.nearest_gemini_reset().into());
        main_window.set_healthy_accounts_count(cfg.accounts.iter().filter(|a| a.is_active).count() as i32);
        main_window.set_routing_strategy_index(cfg.routing_strategy.to_index());
        main_window.set_quota_refresh_interval(cfg.quota_refresh_interval_secs as i32);
        main_window.set_cache_affinity_mode_index(cfg.cache_affinity_mode.to_index());
        main_window.set_session_sticky_ttl_secs(cfg.session_sticky_ttl_secs as i32);
        main_window.set_quota_imbalance_threshold(cfg.quota_imbalance_threshold as i32);
        main_window.set_failover_on_429(cfg.failover_on_429);
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

    // Add account manually callback (Auto-parses email & quota from token!)
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

                if token.starts_with("1//") {
                    if let Ok(tok_res) = antigravity::refresh_token(&http_client, &token).await {
                        access_token = tok_res.access_token;
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
                    gemini_percent: detailed_quota.gemini_percent,
                    gemini_reset: detailed_quota.gemini_reset,
                    status: "Active".to_string(),
                    is_active: true,
                };

                {
                    let mut cfg = c_clone.write();
                    cfg.accounts.push(new_acc);
                    if let Some(m) = fetched_models {
                        cfg.models = m;
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

    // Refresh quota callback
    let c_ref = Arc::clone(&config_arc);
    let v_ref = Arc::clone(&cfg_version);
    main_window.on_refresh_account_quota(move |id_slint| {
        let id_str = id_slint.to_string();
        let c_clone = Arc::clone(&c_ref);
        let v_clone = Arc::clone(&v_ref);

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
            if let Ok((models, used, limit, reset, detailed_quota)) = antigravity::fetch_models_and_quota(&client, &token, project_id.as_deref()).await {
                {
                    let mut cfg = c_clone.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == id_str) {
                        acc.quota_used = used;
                        acc.quota_limit = limit;
                        acc.quota_reset = reset;
                        acc.claude_gpt_percent = detailed_quota.claude_gpt_percent;
                        acc.claude_gpt_reset = detailed_quota.claude_gpt_reset;
                        acc.gemini_percent = detailed_quota.gemini_percent;
                        acc.gemini_reset = detailed_quota.gemini_reset;
                        acc.status = "Active".to_string();
                    }
                    cfg.models = models;
                    let _ = cfg.save();
                }
                v_clone.fetch_add(1, Ordering::SeqCst);
            }
        });
    });

    // Show account info callback
    let c_info = Arc::clone(&config_arc);
    let h_info = window_handle.clone();
    main_window.on_show_account_details(move |id_slint| {
        let id_str = id_slint.to_string();
        if let Some(h) = h_info.upgrade() {
            let cfg = c_info.read();
            if let Some(acc) = cfg.accounts.iter().find(|a| a.id == id_str) {
                h.set_info_account_name(acc.name.clone().into());
                h.set_info_account_tier(acc.tier.clone().into());
                h.set_info_account_quota(format!("{} / {} tokens (Resets {})", acc.quota_used, acc.quota_limit, acc.quota_reset).into());
                h.set_info_account_models(format!("{} upstream models synchronized with Antigravity", cfg.models.len()).into());
                h.set_show_account_info(true);
            }
        }
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

    // Periodic UI stats and state updater timer
    let timer = slint::Timer::default();
    let s_timer = Arc::clone(&stats_arc);
    let c_timer = Arc::clone(&config_arc);
    let b_timer = Arc::clone(&balancer_arc);
    let v_timer = Arc::clone(&cfg_version);
    let mut last_seen_version: u64 = 0;
    let w_timer = window_handle.clone();
    let logs_model = Rc::new(VecModel::<LogEntryData>::default());
    main_window.set_log_entries(ModelRc::from(logs_model.clone()));

    timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
        if let Some(w) = w_timer.upgrade() {
            let (reqs, tokens, avg_lat) = s_timer.get_metrics();
            w.set_total_requests(reqs.to_string().into());
            w.set_total_tokens(tokens.to_string().into());
            w.set_avg_latency(format!("{} ms", avg_lat).into());

            // Balancer & Cache Affinity metrics
            let (_routed, _hits, failovers, hit_rate, active_pins) = b_timer.get_metrics();
            w.set_balancer_hit_rate(format!("{:.1}%", hit_rate).into());
            w.set_balancer_active_sessions(active_pins as i32);
            w.set_balancer_failover_count(failovers as i32);

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

            // Sync accounts & models if changed
            let current_version = v_timer.load(Ordering::Relaxed);
            let accounts_len = c_timer.read().accounts.len();
            if current_version != last_seen_version || accounts_len != accounts_model.row_count() {
                last_seen_version = current_version;
                let cfg = c_timer.read();

                accounts_model.set_vec(
                    cfg.accounts
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
                                gemini_percent: acc.gemini_percent,
                                gemini_reset: acc.gemini_reset.clone().into(),
                                status: acc.status.clone().into(),
                                is_active: acc.is_active,
                            }
                        })
                        .collect::<Vec<_>>(),
                );

                models_model.set_vec(
                    cfg.models
                        .iter()
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

                let total_used = cfg.total_quota_used();
                let total_limit = cfg.total_quota_limit();
                let pct = if total_limit > 0 {
                    ((total_used as f64 / total_limit as f64) * 100.0) as i32
                } else {
                    0
                };
                w.set_global_quota_used(total_used.to_string().into());
                w.set_global_quota_limit(total_limit.to_string().into());
                w.set_global_quota_percent(pct);
                w.set_global_claude_percent(cfg.aggregate_claude_gpt_percent());
                w.set_global_claude_reset(cfg.nearest_claude_reset().into());
                w.set_global_gemini_percent(cfg.aggregate_gemini_percent());
                w.set_global_gemini_reset(cfg.nearest_gemini_reset().into());
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

    if !args.start_minimized {
        main_window.show()?;
    }

    slint::run_event_loop()?;

    let _ = stop_sender.send(true);
    Ok(())
}
