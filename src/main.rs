slint::include_modules!();

use aroxy::cli::CliArgs;
use aroxy::config::{AppConfig, UpstreamAccount};
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

    // Spawn the proxy server in background
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
            accounts_model.push(AccountData {
                id: acc.id.clone().into(),
                name: acc.name.clone().into(),
                provider: acc.provider.clone().into(),
                base_url: acc.base_url.clone().into(),
                api_key_masked: if acc.api_key.is_empty() { "No key".into() } else { "••••••••".into() },
                fingerprint: acc.fingerprint.clone().into(),
                is_active: acc.is_active,
            });
        }
    }
    main_window.set_accounts(ModelRc::from(accounts_model.clone()));
    main_window.set_proxy_running(true);
    main_window.set_proxy_port(config_arc.read().port as i32);

    // Models catalog model
    let models_model = Rc::new(VecModel::<ModelItemData>::default());
    {
        let cfg = config_arc.read();
        for m in &cfg.models {
            models_model.push(ModelItemData {
                id: m.id.clone().into(),
                display_name: m.display_name.clone().into(),
                provider: m.provider.clone().into(),
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

    // Callbacks
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

    // Add account callback
    let c_add = Arc::clone(&config_arc);
    let m_add = accounts_model.clone();
    let h_add = window_handle.clone();
    main_window.on_add_account(move || {
        if let Some(h) = h_add.upgrade() {
            let name = h.get_new_acc_name().to_string();
            let url = h.get_new_acc_url().to_string();
            let key = h.get_new_acc_key().to_string();
            let fp = h.get_new_acc_fp().to_string();

            if name.is_empty() { return; }

            let id = format!("acc-{}", uuid::Uuid::new_v4());
            let new_acc = UpstreamAccount {
                id: id.clone(),
                name: name.clone(),
                provider: if url.contains("anthropic") { "anthropic".to_string() } else { "openai".to_string() },
                base_url: url.clone(),
                api_key: key,
                fingerprint: fp.clone(),
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
                provider: (if url.contains("anthropic") { "anthropic" } else { "openai" }).into(),
                base_url: url.into(),
                api_key_masked: "••••••••".into(),
                fingerprint: fp.into(),
                is_active: false,
            });

            h.set_new_acc_name("".into());
            h.set_new_acc_key("".into());
        }
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
