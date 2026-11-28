use crate::config::AppConfig;
use crate::proxy::fingerprint::apply_fingerprint;
use crate::proxy::mappers::claude::ClaudeMessageResponse;
use crate::proxy::mappers::openai::*;
use crate::proxy::router::*;
use crate::proxy::stats::ProxyStats;
use crate::proxy::stream::{
    pass_through_stream, process_claude_to_openai_stream, StreamLoggingContext,
};
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<parking_lot::RwLock<AppConfig>>,
    pub stats: Arc<ProxyStats>,
    pub balancer: Arc<crate::proxy::balancer::LoadBalancer>,
    pub http_client: reqwest::Client,
    pub cfg_version: Arc<std::sync::atomic::AtomicU64>,
    pub proxy_clients: Arc<parking_lot::RwLock<std::collections::HashMap<String, reqwest::Client>>>,
}

impl AppState {
    pub fn get_client_for_account(
        &self,
        account: &crate::config::UpstreamAccount,
    ) -> reqwest::Client {
        let p_trim = account.proxy_url.as_deref().unwrap_or("").trim();
        let cache_key = format!("{}|{}", account.id, p_trim);
        {
            let r = self.proxy_clients.read();
            if let Some(c) = r.get(&cache_key) {
                return c.clone();
            }
        }
        let mut builder = reqwest::Client::builder()
            .tcp_nodelay(true)
            .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
            .pool_idle_timeout(Some(std::time::Duration::from_secs(90)))
            .pool_max_idle_per_host(32)
            .timeout(std::time::Duration::from_secs(120));

        if !p_trim.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(p_trim) {
                builder = builder.proxy(proxy);
            }
        }

        if let Ok(c) = builder.build() {
            let mut w = self.proxy_clients.write();
            if w.len() >= 64 {
                w.clear();
            }
            w.insert(cache_key, c.clone());
            return c;
        }
        self.http_client.clone()
    }
}

fn refresh_singleflight_lock(account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let map = LOCKS.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock();
    guard
        .entry(account_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

pub(crate) async fn refresh_account_token_and_save(
    state: &AppState,
    account: &crate::config::UpstreamAccount,
) -> Option<String> {
    let lock = refresh_singleflight_lock(&account.id);
    let _singleflight = lock.lock().await;

    // Check if another concurrent task already refreshed the token while we waited
    {
        let cfg = state.config.read();
        if let Some(current) = cfg.accounts.iter().find(|a| a.id == account.id) {
            if !current.api_key.is_empty() && current.api_key != account.api_key {
                return Some(current.api_key.clone());
            }
        }
    }

    let rt = account.refresh_token.as_deref()?;
    let client = state.get_client_for_account(account);
    let ua = {
        let cfg = state.config.read();
        crate::proxy::antigravity::get_official_hub_user_agent_with_meta(
            Some(cfg.resolved_platform()),
            Some(&cfg.spoof_ide_version),
            Some(&cfg.spoof_arch),
        )
    };
    let tok_res = crate::proxy::antigravity::refresh_token_with_ua(&client, rt, Some(&ua))
        .await
        .ok()?;
    let fresh_token = tok_res.access_token;
    {
        let mut cfg = state.config.write();
        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
            acc.api_key = fresh_token.clone();
            if tok_res.refresh_token.is_some() {
                acc.refresh_token = tok_res.refresh_token;
            }
            acc.status = "ACTIVE".to_string();
            acc.error_message = None;
        }
    }
    let config_arc = state.config.clone();
    tokio::task::spawn_blocking(move || {
        static SAVE_LOCK: std::sync::OnceLock<parking_lot::Mutex<()>> = std::sync::OnceLock::new();
        let _save_guard = SAVE_LOCK.get_or_init(|| parking_lot::Mutex::new(())).lock();
        let snapshot = config_arc.read().clone();
        let _ = snapshot.save();
    });
    state
        .cfg_version
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Some(fresh_token)
}

#[allow(clippy::too_many_arguments)]
pub fn build_upstream_headers_with_accept(
    token: &str,
    _client_id: &str,
    _machine_id: &str,
    platform_str: &str,
    ide_ver: &str,
    spoof_arch: &str,
    _spoof_channel: &str,
    _spoof_go: bool,
    _inject_app_meta: bool,
    target_model: &str,
    accept_header: &str,
) -> reqwest::header::HeaderMap {
    let mut req_headers = reqwest::header::HeaderMap::new();
    if let Ok(v) = format!("Bearer {}", token.trim()).parse() {
        req_headers.insert(reqwest::header::AUTHORIZATION, v);
    }
    if let Ok(v) = accept_header.parse() {
        req_headers.insert(reqwest::header::ACCEPT, v);
    }
    if let Ok(v) = "application/json".parse() {
        req_headers.insert(reqwest::header::CONTENT_TYPE, v);
    }

    let ua = crate::proxy::antigravity::get_official_hub_user_agent_with_meta(
        Some(platform_str),
        Some(ide_ver),
        Some(spoof_arch),
    );
    if let Ok(v) = ua.parse() {
        req_headers.insert(reqwest::header::USER_AGENT, v);
    }

    if target_model.to_lowercase().contains("claude") {
        if let Ok(v) = "claude-code-20250219".parse() {
            req_headers.insert("anthropic-beta", v);
        }
    }

    req_headers
}

#[allow(clippy::too_many_arguments)]
pub fn build_upstream_headers(
    token: &str,
    client_id: &str,
    machine_id: &str,
    platform_str: &str,
    ide_ver: &str,
    spoof_arch: &str,
    spoof_channel: &str,
    spoof_go: bool,
    inject_app_meta: bool,
    target_model: &str,
) -> reqwest::header::HeaderMap {
    build_upstream_headers_with_accept(
        token,
        client_id,
        machine_id,
        platform_str,
        ide_ver,
        spoof_arch,
        spoof_channel,
        spoof_go,
        inject_app_meta,
        target_model,
        "text/event-stream",
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_upstream_headers_json(
    token: &str,
    client_id: &str,
    machine_id: &str,
    platform_str: &str,
    ide_ver: &str,
    spoof_arch: &str,
    spoof_channel: &str,
    spoof_go: bool,
    inject_app_meta: bool,
    target_model: &str,
) -> reqwest::header::HeaderMap {
    build_upstream_headers_with_accept(
        token,
        client_id,
        machine_id,
        platform_str,
        ide_ver,
        spoof_arch,
        spoof_channel,
        spoof_go,
        inject_app_meta,
        target_model,
        "application/json",
    )
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn require_master_api_key(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path();
    if req.method() == axum::http::Method::OPTIONS
        || path == "/health"
        || path == "/health/"
        || path == "/oauth/callback"
    {
        return next.run(req).await;
    }

    let expected_key = state.config.read().master_api_key.trim().to_string();
    if expected_key.is_empty() {
        return next.run(req).await;
    }

    let headers = req.headers();
    let mut provided_key: Option<&str> = None;

    if let Some(auth) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let trimmed = auth.trim();
        if let Some(bearer) = trimmed.strip_prefix("Bearer ").or_else(|| trimmed.strip_prefix("bearer ")) {
            provided_key = Some(bearer.trim());
        }
    }
    if provided_key.is_none() {
        if let Some(k) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            provided_key = Some(k.trim());
        }
    }
    if provided_key.is_none() {
        if let Some(k) = headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()) {
            provided_key = Some(k.trim());
        }
    }

    let query_key = if provided_key.is_none() {
        req.uri().query().and_then(|q| {
            q.split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(k, _)| *k == "key")
                .map(|(_, v)| v.to_string())
        })
    } else {
        None
    };

    let candidate = provided_key.or(query_key.as_deref());
    if let Some(k) = candidate {
        if constant_time_eq(k.as_bytes(), expected_key.as_bytes()) {
            return next.run(req).await;
        }
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "message": "Invalid or missing AMR Master API Key.",
                "type": "authentication_error",
                "code": "invalid_api_key"
            }
        })),
    )
        .into_response()
}

fn is_allowed_local_origin(origin: &str) -> bool {
    const EXACT_PREFIXES: &[&str] = &[
        "http://localhost",
        "https://localhost",
        "http://127.0.0.1",
        "https://127.0.0.1",
        "http://[::1]",
        "https://[::1]",
        "tauri://localhost",
    ];
    EXACT_PREFIXES.iter().any(|&p| {
        origin == p || origin.strip_prefix(p).is_some_and(|rest| rest.starts_with(':'))
    })
}

pub fn create_proxy_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/health/", get(health_check))
        .route("/v1/models", get(list_models))
        .route("/v1/models/", get(list_models))
        .route("/models", get(list_models))
        .route("/models/", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/chat/completions/", post(chat_completions))
        .route("/chat/completions", post(chat_completions))
        .route("/chat/completions/", post(chat_completions))
        .route("/v1/messages", post(claude_messages))
        .route("/v1/messages/", post(claude_messages))
        .route("/v1/audio/transcriptions", post(crate::proxy::audio::handle_audio_transcriptions))
        .route("/v1/audio/transcriptions/", post(crate::proxy::audio::handle_audio_transcriptions))
        .route("/audio/transcriptions", post(crate::proxy::audio::handle_audio_transcriptions))
        .route("/audio/transcriptions/", post(crate::proxy::audio::handle_audio_transcriptions))
        .route("/v1/search", post(crate::proxy::audio::handle_web_search))
        .route("/v1/search/", post(crate::proxy::audio::handle_web_search))
        .route("/search", post(crate::proxy::audio::handle_web_search))
        .route("/search/", post(crate::proxy::audio::handle_web_search))
        // Model Context Protocol (MCP) Endpoints
        .route("/mcp", post(crate::proxy::mcp::mcp_handler))
        .route("/mcp/", post(crate::proxy::mcp::mcp_handler))
        .route("/v1/mcp", post(crate::proxy::mcp::mcp_handler))
        .route("/v1/mcp/", post(crate::proxy::mcp::mcp_handler))
        // Google Gemini Native Protocol Endpoints
        .route("/v1beta/models", get(list_gemini_models))
        .route("/v1beta/models/", get(list_gemini_models))
        .route("/v1beta/models/{*model_action}", post(gemini_content))
        .route(
            "/v1/dashboard/billing/subscription",
            get(billing_subscription),
        )
        .route(
            "/v1/dashboard/billing/subscription/",
            get(billing_subscription),
        )
        .route("/dashboard/billing/subscription", get(billing_subscription))
        .route(
            "/dashboard/billing/subscription/",
            get(billing_subscription),
        )
        .route("/v1/dashboard/billing/usage", get(billing_usage))
        .route("/v1/dashboard/billing/usage/", get(billing_usage))
        .route("/dashboard/billing/usage", get(billing_usage))
        .route("/dashboard/billing/usage/", get(billing_usage))
        .route("/v1/usage", get(billing_usage))
        .route("/v1/usage/", get(billing_usage))
        .route("/usage", get(billing_usage))
        .route("/usage/", get(billing_usage))
        .route("/oauth/callback", get(oauth_callback))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_master_api_key,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::predicate(|origin, _| {
                    is_allowed_local_origin(origin.to_str().unwrap_or(""))
                }))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

async fn oauth_callback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let port = state.config.read().port;
    let redirect_uri = format!("http://127.0.0.1:{}/oauth/callback", port);

    let Some(oauth_state) = params.get("state").filter(|s| !s.trim().is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            axum::response::Html(
                "<!DOCTYPE html><html><body style='background:#09090b;color:#f87171;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh'><h2>OAuth Error: missing state parameter</h2></body></html>".to_string()
            ),
        )
            .into_response();
    };
    if !crate::proxy::antigravity::consume_oauth_state(oauth_state) {
        return (
            StatusCode::BAD_REQUEST,
            axum::response::Html(
                "<!DOCTYPE html><html><body style='background:#09090b;color:#f87171;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh'><h2>OAuth Error: invalid or expired state</h2></body></html>".to_string()
            ),
        )
            .into_response();
    }

    let Some(code) = params.get("code").filter(|c| !c.trim().is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            axum::response::Html(
                "<!DOCTYPE html><html><body style='background:#09090b;color:#f87171;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh'><h2>OAuth Error: missing authorization code</h2></body></html>".to_string()
            ),
        )
            .into_response();
    };

    let (
        access_token,
        refresh_token_opt,
        email,
        tier,
        quota_used,
        quota_limit,
        quota_reset,
        acc_models,
        models,
        detailed_quota,
        project_id_opt,
    ) = match crate::proxy::antigravity::exchange_code(&state.http_client, code, &redirect_uri).await
    {
        Ok(token_resp) => {
            let acc_token = token_resp.access_token;
            let ref_token = token_resp.refresh_token;
            let email = match token_resp
                .id_token
                .as_deref()
                .and_then(crate::proxy::antigravity::extract_email_from_jwt)
            {
                Some(e) => e,
                None => crate::proxy::antigravity::fetch_user_info(
                    &state.http_client,
                    &acc_token,
                )
                .await
                .map(|i| i.email)
                .unwrap_or_else(|_| "antigravity.user@gmail.com".to_string()),
            };
            let (project_id, tier) = crate::proxy::antigravity::fetch_project_and_tier(
                &state.http_client,
                &acc_token,
            )
            .await;
            let (acc_models, models, used, limit, reset, det) =
                match crate::proxy::antigravity::fetch_account_models_and_quota(
                    &state.http_client,
                    &acc_token,
                    project_id.as_deref(),
                )
                .await
                {
                    Ok(res) => (res.0, Some(res.1), res.2, res.3, res.4, res.5),
                    Err(_) => {
                        let det = crate::proxy::antigravity::fetch_user_quota_summary(
                            &state.http_client,
                            &acc_token,
                        )
                        .await;
                        (Vec::new(), None, 0, 100_000, "in 4h 00m".to_string(), det)
                    }
                };
            (
                acc_token, ref_token, email, tier, used, limit, reset, acc_models, models, det, project_id,
            )
        }
        Err(e) => {
            log::warn!("OAuth exchange error: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                axum::response::Html(format!(
                    "<!DOCTYPE html><html><body style='background:#09090b;color:#f87171;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh'><h2>OAuth exchange failed</h2><p>{}</p></body></html>",
                    escape_html(&e)
                )),
            )
                .into_response();
        }
    };

    // Add or update account in config
    {
        let mut cfg = state.config.write();
        if let Some(existing) = cfg.accounts.iter_mut().find(|a| a.name == email) {
            existing.api_key = access_token;
            if refresh_token_opt.is_some() {
                existing.refresh_token = refresh_token_opt;
            }
            if project_id_opt.is_some() {
                existing.project_id = project_id_opt;
            }
            existing.tier = tier.clone();
            existing.available_models = acc_models;
            existing.quota_used = quota_used;
            existing.quota_limit = quota_limit;
            existing.quota_reset = quota_reset;
            existing.claude_percent = detailed_quota.claude_percent;
            existing.claude_reset = detailed_quota.claude_reset;
            existing.claude_weekly_percent = detailed_quota.claude_weekly_percent;
            existing.claude_weekly_reset = detailed_quota.claude_weekly_reset;
            existing.gemini_percent = detailed_quota.gemini_percent;
            existing.gemini_reset = detailed_quota.gemini_reset;
            existing.gemini_weekly_percent = detailed_quota.gemini_weekly_percent;
            existing.gemini_weekly_reset = detailed_quota.gemini_weekly_reset;
            existing.status = "ACTIVE".to_string();
            existing.error_message = None;
            existing.is_active = true;
            cfg.active_account_id = Some(existing.id.clone());
        } else {
            let new_id = format!("acc-{}", uuid::Uuid::new_v4().simple());
            cfg.accounts.push(crate::config::UpstreamAccount {
                id: new_id.clone(),
                name: email.clone(),
                provider: "antigravity".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                api_key: access_token,
                refresh_token: refresh_token_opt,
                project_id: project_id_opt,
                fingerprint: "Google Antigravity Client (1.107.0)".to_string(),
                tier: tier.clone(),
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
                is_active: true,
                added_at: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                ),
                device_profile: Some(crate::config::DeviceProfile::generate()),
                proxy_url: None,
                available_models: acc_models,
            });
            cfg.active_account_id = Some(new_id);
        }
        if !cfg.accounts.iter().all(|a| a.available_models.is_empty()) {
            cfg.rebuild_models_from_accounts();
        } else if let Some(m) = models {
            cfg.models = m;
        }
        let _ = cfg.save();
    }

    state
        .cfg_version
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    axum::response::Html(format!(
        r#"
        <!DOCTYPE html>
        <html>
        <head>
            <meta charset="utf-8">
            <title>AMR — Account Connected</title>
            <style>
                body {{ background: #09090b; color: #fafafa; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }}
                .card {{ background: #18181b; border: 1px solid #27272a; border-radius: 10px; padding: 32px; max-width: 440px; text-align: center; }}
                h2 {{ color: #4ade80; margin: 0 0 10px 0; font-size: 20px; }}
                p {{ color: #a1a1aa; font-size: 13px; line-height: 1.5; margin: 6px 0; }}
                .badge {{ display: inline-block; background: #27272a; color: #fafafa; padding: 4px 10px; border-radius: 4px; font-size: 12px; font-weight: 700; margin-top: 12px; }}
            </style>
        </head>
        <body>
            <div class="card">
                <h2>Account Connected</h2>
                <p>Authorized as <strong>{}</strong> (Tier: {})</p>
                <p>Antigravity quota and active models have been synchronized with AMR.<br>You can close this tab and return to the application.</p>
                <div class="badge">AMR v{}</div>
            </div>
        </body>
        </html>
    "#,
        escape_html(&email),
        escape_html(&tier),
        env!("CARGO_PKG_VERSION")
    ))
    .into_response()
}

async fn health_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({"status": "ok", "service": "AMR", "version": env!("CARGO_PKG_VERSION")})),
    )
}

async fn billing_subscription(State(state): State<AppState>) -> Response {
    if !state.config.read().enable_billing_emulation {
        return StatusCode::NOT_FOUND.into_response();
    }

    let hard_limit = 1000.0f64;
    let resp = json!({
        "object": "billing_subscription",
        "has_payment_method": true,
        "canceled": false,
        "currency": "USD",
        "access_until": 1893456000,
        "soft_limit": 800.0,
        "hard_limit": hard_limit,
        "system_hard_limit": hard_limit,
        "soft_limit_usd": 800.0,
        "hard_limit_usd": hard_limit,
        "system_hard_limit_usd": hard_limit,
        "plan": {
            "title": "AMR Enterprise Unlimited",
            "id": "enterprise"
        },
        "account_name": "AMR User"
    });

    Json(resp).into_response()
}

async fn billing_usage(State(state): State<AppState>) -> Response {
    if !state.config.read().enable_billing_emulation {
        return StatusCode::NOT_FOUND.into_response();
    }

    let total_tokens = state
        .stats
        .total_tokens
        .load(std::sync::atomic::Ordering::Relaxed);
    let total_cost_cents = ((total_tokens as f64 / 1_000_000.0) * 50.0).round() as u64;

    let resp = json!({
        "object": "list",
        "total_usage": total_cost_cents,
        "total_tokens": total_tokens,
        "daily_costs": []
    });

    Json(resp).into_response()
}

async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let models = state.config.read().get_broadcast_models();
    Json(json!({
        "object": "list",
        "data": models
    }))
}

async fn list_gemini_models(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.config.read();
    let gemini_list = crate::proxy::mappers::gemini::build_gemini_models_list(&cfg.models, cfg.hide_unavailable_models);
    Json(gemini_list)
}

async fn gemini_content(
    State(state): State<AppState>,
    axum::extract::Path(model_action): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let start_time = Instant::now();

    // Parse model_action: e.g. "gemini-3.8-flash:generateContent" or "gemini-3.8-flash:streamGenerateContent"
    let clean_path = model_action.trim_start_matches('/');
    let (raw_model, is_stream) = if let Some(idx) = clean_path.find(':') {
        let (m, action) = clean_path.split_at(idx);
        let action_clean = action.trim_start_matches(':');
        (m.to_string(), action_clean.contains("stream"))
    } else {
        (clean_path.to_string(), false)
    };

    let requested_model = raw_model.trim_start_matches("models/").to_string();

    if !state.config.read().is_model_enabled(&requested_model) {
        let (lang, standardized) = {
            let c = state.config.read();
            (c.language.clone(), c.standardized_error_handling)
        };
        let raw = format!("Model '{}' is disabled by administrator in settings.", requested_model);
        let msg = format_amr_error_message(&raw, StatusCode::FORBIDDEN, &lang, standardized);
        state.stats.record_detailed_request(
            "POST",
            &format!("/v1beta/{}", clean_path),
            &requested_model,
            403,
            0,
            0,
            "Dispatcher",
            "",
            "",
            &payload.to_string(),
            &msg,
            Some(&msg),
        );
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": {
                    "code": 403,
                    "message": msg,
                    "status": "PERMISSION_DENIED"
                }
            })),
        )
            .into_response();
    }

    let header_session = headers
        .get("x-session-id")
        .or_else(|| headers.get("session-id"))
        .or_else(|| headers.get("conversation-id"))
        .and_then(|h| h.to_str().ok());

    let session_id = format!("gemini-{}", header_session.unwrap_or("default"));

    let (
        platform_str,
        ide_ver,
        lock_prod,
        cooldown_429,
        cooldown_secs,
        cooldown_jitter,
        auto_rotate_fp,
        failover_enabled,
        max_accounts,
    ) = {
        let cfg = state.config.read();
        (
            cfg.resolved_platform(),
            cfg.spoof_ide_version.clone(),
            cfg.lock_production_endpoints,
            cfg.cooldown_on_429,
            cfg.cooldown_duration_secs,
            cfg.cooldown_jitter_percent,
            cfg.auto_rotate_fp_on_limit,
            cfg.failover_on_429,
            cfg.accounts.len(),
        )
    };

    let max_attempts = if failover_enabled {
        max_accounts.clamp(1, 8)
    } else {
        1
    };
    let mut excluded_ids: Vec<String> = Vec::new();
    let mut last_status = StatusCode::BAD_GATEWAY;
    let mut last_err = "No response from upstream".to_string();

    for attempt in 0..max_attempts {
        let selected_account = {
            let cfg = state.config.read();
            state.balancer.select_account_excluding(
                &cfg,
                &requested_model,
                Some(&session_id),
                &excluded_ids,
            )
        };

        let account = match selected_account {
            Ok(acc) if !acc.api_key.is_empty() => acc,
            Ok(_) => {
                let err_msg = "Active upstream account has empty API token in AMR.";
                state.stats.record_detailed_request(
                    "POST",
                    &format!("/v1beta/{}", clean_path),
                    &requested_model,
                    502,
                    0,
                    0,
                    "Dispatcher",
                    "",
                    "",
                    &payload.to_string(),
                    err_msg,
                    Some(err_msg),
                );
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": {
                            "code": 502,
                            "message": err_msg,
                            "status": "UNAVAILABLE"
                        }
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                if attempt > 0 {
                    break;
                }
                let status = match e {
                    crate::proxy::balancer::RoutingError::NoAccounts => {
                        StatusCode::SERVICE_UNAVAILABLE
                    }
                    _ => StatusCode::TOO_MANY_REQUESTS,
                };
                let (lang, standardized) = {
                    let c = state.config.read();
                    (c.language.clone(), c.standardized_error_handling)
                };
                let raw = format!("{}", e);
                let msg = format_amr_error_message(&raw, status, &lang, standardized);
                state.stats.record_detailed_request(
                    "POST",
                    &format!("/v1beta/{}", clean_path),
                    &requested_model,
                    status.as_u16(),
                    0,
                    0,
                    "Dispatcher",
                    "",
                    "",
                    &payload.to_string(),
                    &msg,
                    Some(&msg),
                );
                return (
                    status,
                    Json(json!({
                        "error": {
                            "code": status.as_u16(),
                            "message": msg,
                            "status": if status == StatusCode::SERVICE_UNAVAILABLE { "UNAVAILABLE" } else { "RESOURCE_EXHAUSTED" }
                        }
                    })),
                )
                    .into_response();
            }
        };
        excluded_ids.push(account.id.clone());

        let (target_model, _) =
            crate::proxy::router::resolve_model_with_effort(&requested_model, &account, None);

        let payload_json = crate::proxy::mappers::gemini::build_antigravity_from_gemini(
            &payload,
            &target_model,
            account.project_id.as_deref(),
        );

        let client = state.get_client_for_account(&account);

        let (client_id, machine_id) = if let Some(ref dp) = account.device_profile {
            (dp.dev_device_id.clone(), dp.machine_id.clone())
        } else {
            (
                uuid::Uuid::new_v4().to_string(),
                uuid::Uuid::new_v4().simple().to_string(),
            )
        };

        let is_chat = !target_model.contains("image") && target_model != "web-search";
        let use_stream_endpoint = is_stream || is_chat;
        let endpoints: &[&str] = if use_stream_endpoint {
            if lock_prod {
                &crate::proxy::antigravity::STREAM_ENDPOINTS[..1]
            } else {
                &crate::proxy::antigravity::STREAM_ENDPOINTS[..]
            }
        } else if lock_prod {
            &crate::proxy::antigravity::GENERATE_ENDPOINTS[..1]
        } else {
            &crate::proxy::antigravity::GENERATE_ENDPOINTS[..]
        };

        let mut req_token = account.api_key.clone();
        let client_id_str = client_id.clone();
        let machine_id_str = machine_id.clone();
        let mut res_opt = None;
        let mut hit_429 = false;
        let mut hit_auth = false;

        for &endpoint_url in endpoints {
            let headers_to_use = if use_stream_endpoint {
                build_upstream_headers(
                    &req_token,
                    &client_id_str,
                    &machine_id_str,
                    platform_str,
                    &ide_ver,
                    "AUTO",
                    "stable",
                    true,
                    true,
                    &target_model,
                )
            } else {
                build_upstream_headers_json(
                    &req_token,
                    &client_id_str,
                    &machine_id_str,
                    platform_str,
                    &ide_ver,
                    "AUTO",
                    "stable",
                    true,
                    true,
                    &target_model,
                )
            };

            match client
                .post(endpoint_url)
                .headers(headers_to_use)
                .json(&payload_json)
                .send()
                .await
            {
                Ok(res) => {
                    let s = res.status();
                    if s.is_success() {
                        res_opt = Some(res);
                        break;
                    } else if s == StatusCode::TOO_MANY_REQUESTS {
                        let err_body = res.text().await.unwrap_or_default();
                        let clean =
                            crate::proxy::antigravity::extract_clean_error_message(&err_body);
                        last_status = StatusCode::TOO_MANY_REQUESTS;
                        last_err = format!("HTTP 429: {}", clean);
                        hit_429 = true;
                        if cooldown_429 {
                            let retry_hint =
                                crate::proxy::balancer::LoadBalancer::parse_retry_delay_secs(
                                    &err_body,
                                );
                            state.balancer.record_rate_limit_hit(
                                &account.id,
                                429,
                                cooldown_secs,
                                cooldown_jitter,
                                Some(&last_err),
                                retry_hint,
                            );
                        }
                        if auto_rotate_fp {
                            let mut cfg = state.config.write();
                            if let Some(acc) =
                                cfg.accounts.iter_mut().find(|a| a.id == account.id)
                            {
                                acc.device_profile =
                                    Some(crate::config::DeviceProfile::generate());
                                let _ = cfg.save();
                            }
                        }
                        break;
                    } else if (s == StatusCode::UNAUTHORIZED || s == StatusCode::FORBIDDEN)
                        && account.refresh_token.is_some()
                    {
                        if let Some(fresh) = refresh_account_token_and_save(&state, &account).await {
                            req_token = fresh;

                                let retry_headers = if use_stream_endpoint {
                                    build_upstream_headers(
                                        &req_token,
                                        &client_id,
                                        &machine_id,
                                        platform_str,
                                        &ide_ver,
                                        "AUTO",
                                        "stable",
                                        true,
                                        true,
                                        &target_model,
                                    )
                                } else {
                                    build_upstream_headers_json(
                                        &req_token,
                                        &client_id,
                                        &machine_id,
                                        platform_str,
                                        &ide_ver,
                                        "AUTO",
                                        "stable",
                                        true,
                                        true,
                                        &target_model,
                                    )
                                };

                                if let Ok(retry_res) = client
                                    .post(endpoint_url)
                                    .headers(retry_headers)
                                    .json(&payload_json)
                                    .send()
                                    .await
                                {
                                    let rs = retry_res.status();
                                    if rs.is_success() {
                                        res_opt = Some(retry_res);
                                        break;
                                    }
                                    let err_b = retry_res.text().await.unwrap_or_default();
                                    last_status = rs;
                                    last_err = format!("HTTP {}: {}", rs.as_u16(), err_b);
                                    if rs == StatusCode::TOO_MANY_REQUESTS {
                                        hit_429 = true;
                                        if cooldown_429 {
                                            let retry_hint = crate::proxy::balancer::LoadBalancer::parse_retry_delay_secs(&err_b);
                                            state.balancer.record_rate_limit_hit(
                                                &account.id,
                                                429,
                                                cooldown_secs,
                                                cooldown_jitter,
                                                Some(&last_err),
                                                retry_hint,
                                            );
                                        }
                                        break;
                                    } else if rs == StatusCode::UNAUTHORIZED
                                        || rs == StatusCode::FORBIDDEN
                                    {
                                        hit_auth = true;
                                    }
                                }
                            }
                    } else {
                        let err_body = res.text().await.unwrap_or_default();
                        last_status = s;
                        last_err = format!("HTTP {}: {}", s.as_u16(), err_body);
                        if s == StatusCode::UNAUTHORIZED || s == StatusCode::FORBIDDEN {
                            hit_auth = true;
                        }
                    }
                }
                Err(e) => {
                    last_err = format!("Network error: {}", e);
                }
            }
        }

        if let Some(res) = res_opt {
            state.balancer.record_success(&account.id);

            if is_stream {
                let _stream_id = format!("gemini_{}", uuid::Uuid::new_v4().simple());
                let log_ctx = StreamLoggingContext {
                    stats: state.stats.clone(),
                    method: "POST",
                    path: "/v1beta/models (stream)",
                    model: requested_model.clone(),
                    account_name: account.name.clone(),
                    reasoning_effort: "default".to_string(),
                    request_headers: "Content-Type: application/json; Accept: text/event-stream"
                        .to_string(),
                    request_body: serde_json::to_string_pretty(&payload).unwrap_or_default(),
                    start_time,
                };

                let mapped_stream = crate::proxy::stream::pass_through_stream(
                    Box::pin(res.bytes_stream()),
                    Some(log_ctx),
                );

                let resp = Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-cache, no-transform")
                    .header(header::CONNECTION, "keep-alive")
                    .body(Body::from_stream(mapped_stream))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                return resp;
            } else {
                let body_text = res.text().await.unwrap_or_default();
                let lat = start_time.elapsed().as_millis() as u64;

                state.stats.record_request(
                    "POST",
                    "/v1beta/models:generateContent",
                    &requested_model,
                    200,
                    50,
                    lat,
                );

                if let Ok(val) = serde_json::from_str::<Value>(&body_text) {
                    let actual = val.get("response").unwrap_or(&val);
                    return Json(actual.clone()).into_response();
                } else {
                    let mut last_response: Option<Value> = None;
                    for line in body_text.lines() {
                        let trimmed = line.trim();
                        if let Some(json_str) = trimmed.strip_prefix("data: ") {
                            if json_str == "[DONE]" {
                                continue;
                            }
                            if let Ok(v) = serde_json::from_str::<Value>(json_str) {
                                if let Some(resp) = v.get("response") {
                                    last_response = Some(resp.clone());
                                } else {
                                    last_response = Some(v);
                                }
                            }
                        }
                    }
                    if let Some(resp) = last_response {
                        return Json(resp).into_response();
                    }
                    return (StatusCode::OK, body_text).into_response();
                }
            }
        }

        if (hit_429 || hit_auth) && failover_enabled {
            continue;
        }
        break;
    }

    (
        last_status,
        Json(json!({
            "error": {
                "code": last_status.as_u16(),
                "message": format!("Antigravity upstream connection failure for Google Gemini API: {}", last_err),
                "status": if last_status == StatusCode::TOO_MANY_REQUESTS { "RESOURCE_EXHAUSTED" } else { "UNAVAILABLE" }
            }
        })),
    ).into_response()
}

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<OpenAIChatRequest>,
) -> Response {
    let start_time = Instant::now();
    let requested_model = payload.model.clone();

    if !state.config.read().is_model_enabled(&requested_model) {
        let (lang, standardized) = {
            let c = state.config.read();
            (c.language.clone(), c.standardized_error_handling)
        };
        let raw = format!("Model '{}' is disabled by administrator in settings.", requested_model);
        let msg = format_amr_error_message(&raw, StatusCode::FORBIDDEN, &lang, standardized);
        state.stats.record_detailed_request(
            "POST",
            "/v1/chat/completions",
            &requested_model,
            403,
            0,
            0,
            "Dispatcher",
            payload.reasoning_effort.as_deref().unwrap_or(""),
            "",
            &serde_json::to_string(&payload).unwrap_or_default(),
            &msg,
            Some(&msg),
        );
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": {
                    "message": msg,
                    "type": "model_disabled",
                    "code": 403
                }
            })),
        )
            .into_response();
    }

    let header_session = headers
        .get("x-session-id")
        .or_else(|| headers.get("session-id"))
        .or_else(|| headers.get("conversation-id"))
        .and_then(|h| h.to_str().ok());

    let session_id =
        crate::proxy::balancer::LoadBalancer::extract_session_fingerprint(&payload, header_session);

    let selected_account = {
        let cfg = state.config.read();
        state
            .balancer
            .select_account(&cfg, &requested_model, Some(&session_id))
    };

    let initial_account = match selected_account {
        Ok(acc) if !acc.api_key.is_empty() => acc,
        Ok(_) => {
            let err_msg = "Active upstream account has empty API token in AMR. Please connect an account in AMR.";
            state.stats.record_detailed_request(
                "POST",
                "/v1/chat/completions",
                &requested_model,
                502,
                0,
                start_time.elapsed().as_millis() as u64,
                "Dispatcher",
                payload.reasoning_effort.as_deref().unwrap_or(""),
                "",
                &serde_json::to_string(&payload).unwrap_or_default(),
                err_msg,
                Some(err_msg),
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": err_msg, "type": "upstream_error", "code": "upstream_account_missing_token"}})),
            ).into_response();
        }
        Err(e) => {
            let lat = start_time.elapsed().as_millis() as u64;
            let status = match e {
                crate::proxy::balancer::RoutingError::NoAccounts => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::TOO_MANY_REQUESTS,
            };
            let (lang, standardized) = {
                let c = state.config.read();
                (c.language.clone(), c.standardized_error_handling)
            };
            let raw = format!("{}", e);
            let msg = format_amr_error_message(&raw, status, &lang, standardized);
            state.stats.record_detailed_request(
                "POST",
                "/v1/chat/completions",
                &requested_model,
                status.as_u16(),
                0,
                lat,
                "Dispatcher",
                payload.reasoning_effort.as_deref().unwrap_or(""),
                "",
                &serde_json::to_string(&payload).unwrap_or_default(),
                &msg,
                Some(&msg),
            );
            return (
                status,
                Json(json!({
                    "error": {
                        "message": msg,
                        "type": "routing_error",
                        "code": match e {
                            crate::proxy::balancer::RoutingError::ClaudeQuotaExhausted { .. } => "claude_quota_exhausted",
                            crate::proxy::balancer::RoutingError::GeminiQuotaExhausted { .. } => "gemini_quota_exhausted",
                            _ => "no_route_available"
                        }
                    }
                })),
            ).into_response();
        }
    };

    let (mut resp, mut should_failover) = execute_chat_request(
        &state,
        &payload,
        &initial_account,
        &requested_model,
        start_time,
    )
    .await;

    if should_failover && state.config.read().failover_on_429 {
        let mut excluded_ids = vec![initial_account.id.clone()];
        let max_failovers = state.config.read().accounts.len().saturating_sub(1).min(8);

        for _ in 0..max_failovers {
            let next_acc_opt = {
                let cfg = state.config.read();
                state
                    .balancer
                    .select_account_excluding(&cfg, &requested_model, Some(&session_id), &excluded_ids)
                    .ok()
            };

            let Some(next_acc) = next_acc_opt else {
                break;
            };
            excluded_ids.push(next_acc.id.clone());

            log::info!(
                "Automatic failover (429/Auth): rotating session to {}",
                next_acc.name
            );
            let (retry_resp, retry_failover) =
                execute_chat_request(&state, &payload, &next_acc, &requested_model, start_time)
                    .await;
            resp = retry_resp;
            should_failover = retry_failover;
            if !should_failover {
                break;
            }
        }
    }

    resp
}

async fn execute_chat_request(
    state: &AppState,
    payload: &OpenAIChatRequest,
    account: &crate::config::UpstreamAccount,
    requested_model: &str,
    start_time: Instant,
) -> (Response, bool) {
    let is_streaming = payload.stream;

    // Parse model name for reasoning suffixes or native reasoning_effort / thinking config
    let budget_str = payload
        .extra
        .get("budget_tokens")
        .or_else(|| payload.extra.get("reasoning_budget"))
        .or_else(|| payload.extra.get("thinking_budget"))
        .and_then(|v| v.as_u64())
        .map(|b| {
            if b == 0 {
                "off"
            } else if b <= 1500 {
                "low"
            } else if b <= 6000 {
                "medium"
            } else {
                "high"
            }
        });

    let native_effort = payload
        .reasoning_effort
        .as_deref()
        .or_else(|| payload.extra.get("reasoning_effort").and_then(|e| e.as_str()))
        .or_else(|| payload.extra.get("reasoningEffort").and_then(|e| e.as_str()))
        .or_else(|| {
            payload.extra.get("reasoning").and_then(|r| {
                if let Some(s) = r.as_str() {
                    Some(s)
                } else {
                    r.get("effort")
                        .or_else(|| r.get("reasoningEffort"))
                        .and_then(|e| e.as_str())
                }
            })
        })
        .or_else(|| {
            payload.extra.get("thinking").and_then(|t| {
                t.get("effort")
                    .or_else(|| t.get("type"))
                    .and_then(|typ| typ.as_str())
            })
        })
        .or(budget_str);

    let (base_clean, reasoning_effort) = parse_model_and_reasoning(requested_model, native_effort);
    let (target_model, mut effective_effort) =
        crate::proxy::router::resolve_model_with_effort(&base_clean, account, reasoning_effort);
    if effective_effort.is_none() {
        let mode = state.config.read().default_reasoning_mode.clone();
        effective_effort = match mode.as_str() {
            "off" => Some(crate::proxy::router::ReasoningEffort::Off),
            "low" => Some(crate::proxy::router::ReasoningEffort::Low),
            "medium" => Some(crate::proxy::router::ReasoningEffort::Medium),
            "high" => Some(crate::proxy::router::ReasoningEffort::High),
            _ => None,
        };
    }
    let effort_str = match effective_effort {
        Some(crate::proxy::router::ReasoningEffort::Off) => "off",
        Some(crate::proxy::router::ReasoningEffort::Low) => "low",
        Some(crate::proxy::router::ReasoningEffort::Medium) => "medium",
        Some(crate::proxy::router::ReasoningEffort::High) => "high",
        None => "default",
    };

    // If upstream is Antigravity (Google Cloud Code Pa), convert and forward
    if account.provider == "antigravity" {
        let (platform_str, ide_ver) = {
            let cfg = state.config.read();
            (cfg.resolved_platform(), cfg.spoof_ide_version.clone())
        };
        let payload_json = crate::proxy::antigravity::build_antigravity_payload_with_meta(
            payload,
            &target_model,
            account.project_id.as_deref(),
            effective_effort,
            Some(platform_str),
            Some(&ide_ver),
        );

        let client = state.get_client_for_account(account);
        let mut res_opt = None;
        let req_token = account.api_key.clone();

        let (client_id, machine_id) = if let Some(ref dp) = account.device_profile {
            (dp.dev_device_id.clone(), dp.machine_id.clone())
        } else {
            (
                uuid::Uuid::new_v4().to_string(),
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
            )
        };

        let (
            spoof_go,
            lock_prod,
            cooldown_429,
            cooldown_secs,
            cooldown_jitter,
            apply_jitter,
            spoof_arch,
            spoof_channel,
            auto_rotate_fp,
            inject_app_meta,
        ) = {
            let cfg = state.config.read();
            (
                cfg.spoof_go_sdk,
                cfg.lock_production_endpoints,
                cfg.cooldown_on_429,
                cfg.cooldown_duration_secs,
                cfg.cooldown_jitter_percent,
                cfg.request_jitter,
                cfg.spoof_arch.clone(),
                cfg.spoof_channel.clone(),
                cfg.auto_rotate_fp_on_limit,
                cfg.inject_ide_metadata,
            )
        };

        // Apply randomized timing jitter (15-45ms) to prevent synthetic millisecond cadence
        if apply_jitter {
            let jitter_ms = fastrand::u64(15..45);
            tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
        }

        let use_non_stream_generate = !is_streaming
            && (target_model == "web-search" || target_model.contains("image"));
        let endpoints: &[&str] = if use_non_stream_generate {
            if lock_prod {
                &crate::proxy::antigravity::GENERATE_ENDPOINTS[..1]
            } else {
                &crate::proxy::antigravity::GENERATE_ENDPOINTS[..]
            }
        } else if lock_prod {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..1]
        } else {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..]
        };

        for endpoint_url in endpoints {
            let req_headers = if use_non_stream_generate {
                build_upstream_headers_json(
                    &req_token,
                    &client_id,
                    &machine_id,
                    platform_str,
                    &ide_ver,
                    &spoof_arch,
                    &spoof_channel,
                    spoof_go,
                    inject_app_meta,
                    &target_model,
                )
            } else {
                build_upstream_headers(
                    &req_token,
                    &client_id,
                    &machine_id,
                    platform_str,
                    &ide_ver,
                    &spoof_arch,
                    &spoof_channel,
                    spoof_go,
                    inject_app_meta,
                    &target_model,
                )
            };

            if let Ok(r) = client
                .post(*endpoint_url)
                .headers(req_headers)
                .json(&payload_json)
                .send()
                .await
            {
                let s = r.status();
                if s.is_success() {
                    state.balancer.record_success(&account.id);
                    res_opt = Some(r);
                    break;
                } else if s == StatusCode::TOO_MANY_REQUESTS {
                    res_opt = Some(r);
                    break;
                } else {
                    res_opt = Some(r);
                    continue; // Try next endpoint on transient 5xx/404
                }
            }
        }

        let mut res = match res_opt {
            Some(r) => r,
            None => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request(
                    "POST",
                    "/v1/chat/completions",
                    requested_model,
                    502,
                    0,
                    lat,
                );
                return ((StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": "Antigravity upstream connection failure"}}))).into_response(), false);
            }
        };

        let mut status = res.status();
        let mut is_429 = status == StatusCode::TOO_MANY_REQUESTS;
        let mut is_auth_error =
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;

        // If auth error occurred and account has a refresh token, automatically refresh on the fly and retry!
        if is_auth_error && account.refresh_token.is_some() {
            log::info!(
                "Upstream token expired for {}, attempting automatic refresh",
                account.name
            );
            if let Some(fresh_token) = refresh_account_token_and_save(state, account).await {
                    for endpoint_url in endpoints {
                        let retry_headers = if use_non_stream_generate {
                            build_upstream_headers_json(
                                &fresh_token,
                                &client_id,
                                &machine_id,
                                platform_str,
                                &ide_ver,
                                &spoof_arch,
                                &spoof_channel,
                                spoof_go,
                                inject_app_meta,
                                &target_model,
                            )
                        } else {
                            build_upstream_headers(
                                &fresh_token,
                                &client_id,
                                &machine_id,
                                platform_str,
                                &ide_ver,
                                &spoof_arch,
                                &spoof_channel,
                                spoof_go,
                                inject_app_meta,
                                &target_model,
                            )
                        };

                        if let Ok(retry_res) = client
                            .post(*endpoint_url)
                            .headers(retry_headers)
                            .json(&payload_json)
                            .send()
                            .await
                        {
                            status = retry_res.status();
                            is_429 = status == StatusCode::TOO_MANY_REQUESTS;
                            is_auth_error = status == StatusCode::UNAUTHORIZED
                                || status == StatusCode::FORBIDDEN;
                            res = retry_res;
                            if status.is_success() {
                                state.balancer.record_success(&account.id);
                                break;
                            }
                            if is_429 {
                                break;
                            }
                        }
                    }
            }
        }

        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;

            let error_msg = crate::proxy::antigravity::extract_clean_error_message(&err_text);

            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            state.stats.record_detailed_request(
                "POST",
                "/v1/chat/completions",
                requested_model,
                status.as_u16(),
                0,
                lat,
                &account.name,
                effort_str,
                "Content-Type: application/json",
                &req_body_str,
                &err_text,
                Some(&error_msg),
            );

            if is_429 && cooldown_429 {
                let retry_hint =
                    crate::proxy::balancer::LoadBalancer::parse_retry_delay_secs(&err_text);
                state.balancer.record_rate_limit_hit(
                    &account.id,
                    429,
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP 429: {}", error_msg)),
                    retry_hint,
                );
                if auto_rotate_fp {
                    let mut cfg = state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                        acc.device_profile = Some(crate::config::DeviceProfile::generate());
                        let _ = cfg.save();
                    }
                }
            } else if is_auth_error {
                state.balancer.clear_session_bindings(&account.id);
                state.balancer.update_failure_reason(
                    &account.id,
                    status.as_u16(),
                    &format!("HTTP {}: Auth failed ({})", status.as_u16(), error_msg),
                );
            } else if status.is_server_error() {
                state.balancer.record_rate_limit_hit(
                    &account.id,
                    status.as_u16(),
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP {}: {}", status.as_u16(), error_msg)),
                    None,
                );
            }

            if is_auth_error
                || error_msg.contains("invalid_grant")
                || error_msg.contains("PERMISSION_DENIED")
            {
                log::warn!(
                    "Upstream account auth failed for {}: {}",
                    account.name,
                    error_msg
                );
                let helpful_err = if account.refresh_token.is_none() {
                    "Google OAuth access token (ya29...) has expired. Please Login via Browser to obtain a persistent session.".to_string()
                } else {
                    format!("HTTP {}: {}", status.as_u16(), error_msg)
                };
                {
                    let mut cfg = state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                        acc.status = "ERROR".to_string();
                        // Never deactivate account permanently so it can retry or auto-recover!
                        acc.error_message = Some(helpful_err);
                    }
                    let _ = cfg.save();
                }
                state
                    .cfg_version
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            let return_status = if is_auth_error {
                StatusCode::BAD_GATEWAY
            } else {
                status
            };

            let err_code = if is_auth_error {
                "upstream_token_expired".to_string()
            } else if is_429 {
                "rate_limit_exceeded".to_string()
            } else {
                "upstream_error".to_string()
            };

            let helpful_msg = if is_auth_error {
                format!(
                    "Upstream Google account token expired for '{}'. Please log in via browser in AMR (OAuth) to restore session. Upstream details: {}",
                    account.name, error_msg
                )
            } else {
                format!("Google Antigravity Upstream: {}", error_msg)
            };

            return (
                (
                    return_status,
                    Json(json!({
                        "error": {
                            "message": helpful_msg,
                            "type": "upstream_error",
                            "code": err_code
                        }
                    })),
                )
                    .into_response(),
                is_429 || is_auth_error,
            );
        }

        if is_streaming {
            let stream_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let log_ctx = StreamLoggingContext {
                stats: state.stats.clone(),
                method: "POST",
                path: "/v1/chat/completions (stream)",
                model: requested_model.to_string(),
                account_name: account.name.clone(),
                reasoning_effort: effort_str.to_string(),
                request_headers: "Content-Type: application/json; Accept: text/event-stream"
                    .to_string(),
                request_body: req_body_str,
                start_time,
            };
            let mapped_stream = crate::proxy::stream::process_antigravity_to_openai_stream(
                Box::pin(res.bytes_stream()),
                stream_id,
                target_model,
                Some(log_ctx),
            );

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache, no-transform")
                .header(header::CONNECTION, "keep-alive")
                .header("x-accel-buffering", "no")
                .header("x-request-id", req_id)
                .header("openai-organization", "user-personal")
                .header("openai-version", "2020-10-01")
                .body(Body::from_stream(mapped_stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            (r, false)
        } else {
            let body_text = res.text().await.unwrap_or_default();
            let mut full_text = String::new();
            let mut thinking_text = None;
            let mut sources_md_final = String::new();
            let mut in_tok = 50;
            let mut out_tok = 150;
            let mut cached_tok = 0;

            let parse_json_chunk = |val: &Value,
                                    full_text: &mut String,
                                    thinking_text: &mut Option<String>,
                                    sources_md_final: &mut String,
                                    in_tok: &mut u64,
                                    out_tok: &mut u64,
                                    cached_tok: &mut u64| {
                let actual = val.get("response").unwrap_or(val);
                if let Some(usage) = actual.get("usageMetadata") {
                    if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                        *in_tok = pt;
                    }
                    if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                        *out_tok = ct;
                    }
                    if let Some(cct) = usage.get("cachedContentTokenCount").and_then(|v| v.as_u64())
                    {
                        *cached_tok = cct;
                    }
                }
                if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                    for cand in candidates {
                        if let Some(parts) = cand
                            .get("content")
                            .and_then(|c| c.get("parts"))
                            .and_then(|p| p.as_array())
                        {
                            for part in parts {
                                let is_thought = part
                                    .get("thought")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                    if is_thought {
                                        let entry = thinking_text.get_or_insert_with(String::new);
                                        entry.push_str(txt);
                                    } else {
                                        full_text.push_str(txt);
                                    }
                                } else if let Some(inline) = part.get("inlineData") {
                                    let mime = inline
                                        .get("mimeType")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("image/png");
                                    if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                        full_text.push_str(&format!(
                                            "[Generated Image: {} bytes, {}]\n![Generated Image](data:{};base64,{})",
                                            b64.len(),
                                            mime,
                                            mime,
                                            b64
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                let (_q, _r, smd) = crate::proxy::antigravity::extract_grounding_info(actual);
                if !smd.is_empty() {
                    *sources_md_final = smd;
                }
            };

            if let Ok(direct_val) = serde_json::from_str::<Value>(body_text.trim()) {
                parse_json_chunk(
                    &direct_val,
                    &mut full_text,
                    &mut thinking_text,
                    &mut sources_md_final,
                    &mut in_tok,
                    &mut out_tok,
                    &mut cached_tok,
                );
            } else {
                for line in body_text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        let trimmed = data.trim();
                        if trimmed == "[DONE]" {
                            break;
                        }
                        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                            parse_json_chunk(
                                &val,
                                &mut full_text,
                                &mut thinking_text,
                                &mut sources_md_final,
                                &mut in_tok,
                                &mut out_tok,
                                &mut cached_tok,
                            );
                        }
                    }
                }
            }

            if !sources_md_final.is_empty() {
                full_text.push_str(&sources_md_final);
            }

            let oai_resp = OpenAIChatResponse {
                id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
                object: "chat.completion".to_string(),
                created: chrono::Utc::now().timestamp() as u64,
                model: target_model,
                system_fingerprint: "fp_antigravity_hub".to_string(),
                choices: vec![OpenAIChoice {
                    index: 0,
                    message: OpenAIMessage {
                        role: "assistant".to_string(),
                        content: Value::String(full_text),
                        reasoning_content: thinking_text,
                        refusal: None,
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    logprobs: None,
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(OpenAIUsage {
                    prompt_tokens: in_tok,
                    completion_tokens: out_tok,
                    total_tokens: in_tok + out_tok,
                    prompt_tokens_details: Some(PromptTokensDetails {
                        cached_tokens: cached_tok,
                        audio_tokens: 0,
                    }),
                    completion_tokens_details: Some(CompletionTokensDetails {
                        reasoning_tokens: 0,
                        audio_tokens: 0,
                        accepted_prediction_tokens: 0,
                        rejected_prediction_tokens: 0,
                    }),
                    prompt_cache_hit_tokens: if cached_tok > 0 { Some(cached_tok) } else { None },
                    prompt_cache_miss_tokens: if cached_tok > 0 { Some(in_tok.saturating_sub(cached_tok)) } else { None },
                }),
            };

            let lat = start_time.elapsed().as_millis() as u64;
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let resp_body_str = serde_json::to_string_pretty(&oai_resp).unwrap_or_default();
            state.stats.record_detailed_request_full(
                "POST",
                "/v1/chat/completions",
                requested_model,
                200,
                in_tok + out_tok,
                in_tok,
                out_tok,
                cached_tok,
                lat,
                &account.name,
                effort_str,
                "Content-Type: application/json",
                &req_body_str,
                &resp_body_str,
                None,
            );

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let mut response = Json(oai_resp).into_response();
            if let Ok(v) = req_id.parse() {
                response.headers_mut().insert("x-request-id", v);
            }
            if let Ok(v) = "user-personal".parse() {
                response.headers_mut().insert("openai-organization", v);
            }
            if let Ok(v) = lat.to_string().parse() {
                response.headers_mut().insert("openai-processing-ms", v);
            }
            if let Ok(v) = "2020-10-01".parse() {
                response.headers_mut().insert("openai-version", v);
            }
            (response, false)
        }
    } else if account.provider == "anthropic" {
        let claude_req = convert_openai_to_claude(payload, &target_model, reasoning_effort);
        let mut req_headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = account.api_key.parse() {
            req_headers.insert("x-api-key", v);
        }
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let upstream_url = format!("{}/v1/messages", account.base_url.trim_end_matches('/'));
        let client = state.get_client_for_account(account);
        let res = match client
            .post(&upstream_url)
            .headers(req_headers)
            .json(&claude_req)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request(
                    "POST",
                    "/v1/chat/completions",
                    requested_model,
                    502,
                    0,
                    lat,
                );
                return ((StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": format!("Upstream connection error: {}", e)}}))).into_response(), false);
            }
        };

        let status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;
        let is_auth_error = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;
        let (cooldown_429, cooldown_secs, cooldown_jitter) = {
            let cfg = state.config.read();
            (
                cfg.cooldown_on_429,
                cfg.cooldown_duration_secs,
                cfg.cooldown_jitter_percent,
            )
        };

        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;

            let error_msg = crate::proxy::antigravity::extract_clean_error_message(&err_text);

            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            state.stats.record_detailed_request(
                "POST",
                "/v1/chat/completions",
                requested_model,
                status.as_u16(),
                0,
                lat,
                &account.name,
                effort_str,
                "Content-Type: application/json",
                &req_body_str,
                &err_text,
                Some(&error_msg),
            );

            if is_429 && cooldown_429 {
                state.balancer.update_failure_reason(
                    &account.id,
                    429,
                    &format!("HTTP 429: {}", error_msg),
                );
            } else if is_auth_error {
                state.balancer.clear_session_bindings(&account.id);
                state.balancer.update_failure_reason(
                    &account.id,
                    status.as_u16(),
                    &format!("HTTP {}: Auth failed ({})", status.as_u16(), error_msg),
                );
            } else if status.is_server_error() {
                state.balancer.record_failure_with_reason(
                    &account.id,
                    status.as_u16(),
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP {}: {}", status.as_u16(), error_msg)),
                );
            }

            if is_auth_error
                || error_msg.contains("invalid_grant")
                || error_msg.contains("PERMISSION_DENIED")
            {
                log::warn!(
                    "Upstream account auth failed for {}: {}",
                    account.name,
                    error_msg
                );
                let helpful_err = if account.refresh_token.is_none() {
                    "Google OAuth access token (ya29...) has expired. Please Login via Browser to obtain a persistent session.".to_string()
                } else {
                    format!("HTTP {}: {}", status.as_u16(), error_msg)
                };
                {
                    let mut cfg = state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                        acc.status = "ERROR".to_string();
                        // Never deactivate account permanently so it can retry or auto-recover!
                        acc.error_message = Some(helpful_err);
                    }
                    let _ = cfg.save();
                }
                state
                    .cfg_version
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            let return_status = if is_auth_error {
                StatusCode::BAD_GATEWAY
            } else {
                status
            };

            let err_code = if is_auth_error {
                "upstream_token_expired".to_string()
            } else if is_429 {
                "rate_limit_exceeded".to_string()
            } else {
                "upstream_error".to_string()
            };

            let helpful_msg = if is_auth_error {
                format!(
                    "Upstream Google account token expired for '{}'. Please log in via browser in AMR (OAuth) to restore session. Upstream details: {}",
                    account.name, error_msg
                )
            } else {
                format!("Google Antigravity Upstream: {}", error_msg)
            };

            return (
                (
                    return_status,
                    Json(json!({
                        "error": {
                            "message": helpful_msg,
                            "type": "upstream_error",
                            "code": err_code
                        }
                    })),
                )
                    .into_response(),
                is_429 || is_auth_error,
            );
        }

        if is_streaming {
            let stream_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let log_ctx = StreamLoggingContext {
                stats: state.stats.clone(),
                method: "POST",
                path: "/v1/chat/completions (stream)",
                model: requested_model.to_string(),
                account_name: account.name.clone(),
                reasoning_effort: effort_str.to_string(),
                request_headers: "Content-Type: application/json; Accept: text/event-stream"
                    .to_string(),
                request_body: req_body_str,
                start_time,
            };
            let mapped_stream = process_claude_to_openai_stream(
                Box::pin(res.bytes_stream()),
                stream_id,
                target_model,
                Some(log_ctx),
            );

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache, no-transform")
                .header(header::CONNECTION, "keep-alive")
                .header("x-accel-buffering", "no")
                .header("x-request-id", req_id)
                .header("openai-organization", "user-personal")
                .header("openai-version", "2020-10-01")
                .body(Body::from_stream(mapped_stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            (r, false)
        } else {
            let claude_resp = match res.json::<ClaudeMessageResponse>().await {
                Ok(r) => r,
                Err(e) => {
                    return (
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": {"message": e.to_string()}})),
                        )
                            .into_response(),
                        false,
                    );
                }
            };
            let oai_resp = convert_claude_response_to_openai(&claude_resp);
            let tokens = oai_resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/chat/completions",
                requested_model,
                200,
                tokens,
                lat,
            );

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let mut response = Json(oai_resp).into_response();
            if let Ok(v) = req_id.parse() {
                response.headers_mut().insert("x-request-id", v);
            }
            if let Ok(v) = "user-personal".parse() {
                response.headers_mut().insert("openai-organization", v);
            }
            if let Ok(v) = lat.to_string().parse() {
                response.headers_mut().insert("openai-processing-ms", v);
            }
            if let Ok(v) = "2020-10-01".parse() {
                response.headers_mut().insert("openai-version", v);
            }
            (response, false)
        }
    } else {
        // Direct OpenAI or custom endpoint pass-through
        let mut forward_payload = payload.clone();
        forward_payload.model = target_model;
        if let Some(eff) = reasoning_effort {
            match eff {
                ReasoningEffort::Low => forward_payload.reasoning_effort = Some("low".to_string()),
                ReasoningEffort::Medium => {
                    forward_payload.reasoning_effort = Some("medium".to_string())
                }
                ReasoningEffort::High => {
                    forward_payload.reasoning_effort = Some("high".to_string())
                }
                ReasoningEffort::Off => forward_payload.reasoning_effort = None,
            }
        }

        let mut req_headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = format!("Bearer {}", account.api_key.trim()).parse() {
            req_headers.insert("authorization", v);
        }
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let upstream_url = format!(
            "{}/v1/chat/completions",
            account.base_url.trim_end_matches('/')
        );
        let client = state.get_client_for_account(account);
        let res = match client
            .post(&upstream_url)
            .headers(req_headers)
            .json(&forward_payload)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request(
                    "POST",
                    "/v1/chat/completions",
                    requested_model,
                    502,
                    0,
                    lat,
                );
                return (
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error": {"message": e.to_string()}})),
                    )
                        .into_response(),
                    false,
                );
            }
        };

        let status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;

        if is_streaming && status.is_success() {
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let log_ctx = StreamLoggingContext {
                stats: state.stats.clone(),
                method: "POST",
                path: "/v1/chat/completions (stream)",
                model: requested_model.to_string(),
                account_name: account.name.clone(),
                reasoning_effort: effort_str.to_string(),
                request_headers: "Content-Type: application/json; Accept: text/event-stream"
                    .to_string(),
                request_body: req_body_str,
                start_time,
            };
            let stream = pass_through_stream(Box::pin(res.bytes_stream()), Some(log_ctx));

            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache, no-transform")
                .header(header::CONNECTION, "keep-alive")
                .header("x-accel-buffering", "no")
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            (r, false)
        } else {
            let text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/chat/completions",
                requested_model,
                status.as_u16(),
                50,
                lat,
            );
            ((status, text).into_response(), is_429)
        }
    }
}

async fn claude_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let start_time = Instant::now();
    let model = payload
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();

    let header_session = headers
        .get("x-session-id")
        .or_else(|| headers.get("session-id"))
        .or_else(|| headers.get("conversation-id"))
        .and_then(|h| h.to_str().ok());

    let session_id = if let Some(sid) = header_session {
        format!("hdr-{}", sid)
    } else if let Some(uid) = payload
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(|u| u.as_str())
    {
        format!("usr-{}", uid)
    } else {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Some(sys) = payload.get("system").and_then(|s| s.as_str()) {
            hasher.write(b"[sys]");
            hasher.write(sys.as_bytes());
        }
        if let Some(msgs) = payload.get("messages").and_then(|m| m.as_array()) {
            if let Some(first_user) = msgs
                .iter()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            {
                let content_str = first_user
                    .get("content")
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                hasher.write(b"[u1]");
                hasher.write(content_str.as_bytes());
            }
        }
        format!("ctx-{:016x}", hasher.finish())
    };

    if !state.config.read().is_model_enabled(&model) {
        let (lang, standardized) = {
            let c = state.config.read();
            (c.language.clone(), c.standardized_error_handling)
        };
        let raw = format!("Model '{}' is disabled by administrator in settings.", model);
        let msg = format_amr_error_message(&raw, StatusCode::FORBIDDEN, &lang, standardized);
        state.stats.record_detailed_request(
            "POST",
            "/v1/messages",
            &model,
            403,
            0,
            0,
            "Dispatcher",
            "",
            "",
            &payload.to_string(),
            &msg,
            Some(&msg),
        );
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "type": "error",
                "error": {
                    "type": "model_disabled",
                    "message": msg
                }
            })),
        )
            .into_response();
    }

    let selected_account = {
        let cfg = state.config.read();
        state
            .balancer
            .select_account(&cfg, &model, Some(&session_id))
    };

    let initial_account = match selected_account {
        Ok(acc) if !acc.api_key.is_empty() => acc,
        Ok(_) => {
            let err_msg = "Account has empty API token in AMR";
            state.stats.record_detailed_request(
                "POST",
                "/v1/messages",
                &model,
                502,
                0,
                start_time.elapsed().as_millis() as u64,
                "Dispatcher",
                "",
                "",
                &payload.to_string(),
                err_msg,
                Some(err_msg),
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"type": "error", "error": {"type": "authentication_error", "message": err_msg}})),
            ).into_response();
        }
        Err(e) => {
            let lat = start_time.elapsed().as_millis() as u64;
            let status = match e {
                crate::proxy::balancer::RoutingError::NoAccounts => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::TOO_MANY_REQUESTS,
            };
            let (lang, standardized) = {
                let c = state.config.read();
                (c.language.clone(), c.standardized_error_handling)
            };
            let raw = format!("{}", e);
            let msg = format_amr_error_message(&raw, status, &lang, standardized);
            state.stats.record_detailed_request(
                "POST",
                "/v1/messages",
                &model,
                status.as_u16(),
                0,
                lat,
                "Dispatcher",
                "",
                "",
                &payload.to_string(),
                &msg,
                Some(&msg),
            );
            return (
                status,
                Json(json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": msg
                    }
                })),
            )
                .into_response();
        }
    };

    let (mut resp, mut should_failover) =
        execute_claude_request(&state, &payload, &initial_account, &model, start_time).await;

    if should_failover && state.config.read().failover_on_429 {
        let mut excluded_ids = vec![initial_account.id.clone()];
        let max_failovers = state.config.read().accounts.len().saturating_sub(1).min(8);

        for _ in 0..max_failovers {
            let next_acc_opt = {
                let cfg = state.config.read();
                state
                    .balancer
                    .select_account_excluding(&cfg, &model, Some(&session_id), &excluded_ids)
                    .ok()
            };

            let Some(next_acc) = next_acc_opt else {
                break;
            };
            excluded_ids.push(next_acc.id.clone());

            log::info!(
                "Claude Messages failover (429/Auth): rotating to {}",
                next_acc.name
            );
            let (retry_resp, retry_failover) =
                execute_claude_request(&state, &payload, &next_acc, &model, start_time).await;
            resp = retry_resp;
            should_failover = retry_failover;
            if !should_failover {
                break;
            }
        }
    }

    resp
}

async fn execute_claude_request(
    state: &AppState,
    payload: &Value,
    account: &crate::config::UpstreamAccount,
    requested_model: &str,
    start_time: Instant,
) -> (Response, bool) {
    let is_streaming = payload
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    let claude_effort = payload
        .get("thinking")
        .and_then(|t| {
            t.get("effort")
                .or_else(|| t.get("type"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| payload.get("reasoning_effort").and_then(|v| v.as_str()))
        .or_else(|| payload.get("reasoningEffort").and_then(|v| v.as_str()));

    let (base_clean, reasoning_effort) = parse_model_and_reasoning(
        requested_model,
        claude_effort,
    );
    let (target_model, mut effective_effort) =
        crate::proxy::router::resolve_model_with_effort(&base_clean, account, reasoning_effort);
    if effective_effort.is_none() {
        let mode = state.config.read().default_reasoning_mode.clone();
        effective_effort = match mode.as_str() {
            "off" => Some(crate::proxy::router::ReasoningEffort::Off),
            "low" => Some(crate::proxy::router::ReasoningEffort::Low),
            "medium" => Some(crate::proxy::router::ReasoningEffort::Medium),
            "high" => Some(crate::proxy::router::ReasoningEffort::High),
            _ => None,
        };
    }
    let effort_str = match effective_effort {
        Some(crate::proxy::router::ReasoningEffort::Off) => "off",
        Some(crate::proxy::router::ReasoningEffort::Low) => "low",
        Some(crate::proxy::router::ReasoningEffort::Medium) => "medium",
        Some(crate::proxy::router::ReasoningEffort::High) => "high",
        None => "default",
    };

    if account.provider == "antigravity" {
        let (platform_str, ide_ver) = {
            let cfg = state.config.read();
            (cfg.resolved_platform(), cfg.spoof_ide_version.clone())
        };
        let payload_json =
            crate::proxy::antigravity::build_antigravity_payload_from_claude_with_meta(
                payload,
                &target_model,
                account.project_id.as_deref(),
                effective_effort,
                Some(platform_str),
                Some(&ide_ver),
            );

        let client = state.get_client_for_account(account);
        let mut res_opt = None;
        let req_token = account.api_key.clone();

        let (client_id, machine_id) = if let Some(ref dp) = account.device_profile {
            (dp.dev_device_id.clone(), dp.machine_id.clone())
        } else {
            (
                uuid::Uuid::new_v4().to_string(),
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
            )
        };

        let (
            spoof_go,
            lock_prod,
            cooldown_429,
            cooldown_secs,
            cooldown_jitter,
            apply_jitter,
            spoof_arch,
            spoof_channel,
            auto_rotate_fp,
            inject_app_meta,
        ) = {
            let cfg = state.config.read();
            (
                cfg.spoof_go_sdk,
                cfg.lock_production_endpoints,
                cfg.cooldown_on_429,
                cfg.cooldown_duration_secs,
                cfg.cooldown_jitter_percent,
                cfg.request_jitter,
                cfg.spoof_arch.clone(),
                cfg.spoof_channel.clone(),
                cfg.auto_rotate_fp_on_limit,
                cfg.inject_ide_metadata,
            )
        };

        // Apply randomized timing jitter (15-45ms) to prevent synthetic millisecond cadence
        if apply_jitter {
            let jitter_ms = fastrand::u64(15..45);
            tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
        }

        let use_non_stream_generate = !is_streaming
            && (target_model == "web-search" || target_model.contains("image"));
        let endpoints: &[&str] = if use_non_stream_generate {
            if lock_prod {
                &crate::proxy::antigravity::GENERATE_ENDPOINTS[..1]
            } else {
                &crate::proxy::antigravity::GENERATE_ENDPOINTS[..]
            }
        } else if lock_prod {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..1]
        } else {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..]
        };

        for endpoint_url in endpoints {
            let req_headers = if use_non_stream_generate {
                build_upstream_headers_json(
                    &req_token,
                    &client_id,
                    &machine_id,
                    platform_str,
                    &ide_ver,
                    &spoof_arch,
                    &spoof_channel,
                    spoof_go,
                    inject_app_meta,
                    &target_model,
                )
            } else {
                build_upstream_headers(
                    &req_token,
                    &client_id,
                    &machine_id,
                    platform_str,
                    &ide_ver,
                    &spoof_arch,
                    &spoof_channel,
                    spoof_go,
                    inject_app_meta,
                    &target_model,
                )
            };

            if let Ok(r) = client
                .post(*endpoint_url)
                .headers(req_headers)
                .json(&payload_json)
                .send()
                .await
            {
                let s = r.status();
                if s.is_success() {
                    state.balancer.record_success(&account.id);
                    res_opt = Some(r);
                    break;
                } else if s == StatusCode::TOO_MANY_REQUESTS {
                    res_opt = Some(r);
                    break;
                } else {
                    res_opt = Some(r);
                    continue; // Try next endpoint on transient 5xx/404
                }
            }
        }

        let mut res = match res_opt {
            Some(r) => r,
            None => {
                let lat = start_time.elapsed().as_millis() as u64;
                state
                    .stats
                    .record_request("POST", "/v1/messages", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"type": "error", "error": {"type": "api_error", "message": "Antigravity upstream connection failure"}}))).into_response(), false);
            }
        };

        let mut status = res.status();
        let mut is_429 = status == StatusCode::TOO_MANY_REQUESTS;
        let mut is_auth_error =
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;

        if is_auth_error && account.refresh_token.is_some() {
            log::info!(
                "Claude token expired for {}, attempting automatic refresh",
                account.name
            );
            if let Some(fresh_token) = refresh_account_token_and_save(state, account).await {
                    for endpoint_url in endpoints {
                        let retry_headers = if use_non_stream_generate {
                            build_upstream_headers_json(
                                &fresh_token,
                                &client_id,
                                &machine_id,
                                platform_str,
                                &ide_ver,
                                &spoof_arch,
                                &spoof_channel,
                                spoof_go,
                                inject_app_meta,
                                &target_model,
                            )
                        } else {
                            build_upstream_headers(
                                &fresh_token,
                                &client_id,
                                &machine_id,
                                platform_str,
                                &ide_ver,
                                &spoof_arch,
                                &spoof_channel,
                                spoof_go,
                                inject_app_meta,
                                &target_model,
                            )
                        };

                        if let Ok(retry_res) = client
                            .post(*endpoint_url)
                            .headers(retry_headers)
                            .json(&payload_json)
                            .send()
                            .await
                        {
                            status = retry_res.status();
                            is_429 = status == StatusCode::TOO_MANY_REQUESTS;
                            is_auth_error = status == StatusCode::UNAUTHORIZED
                                || status == StatusCode::FORBIDDEN;
                            res = retry_res;
                            if status.is_success() {
                                state.balancer.record_success(&account.id);
                                break;
                            }
                            if is_429 {
                                break;
                            }
                        }
                    }
            }
        }

        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;

            let error_msg = crate::proxy::antigravity::extract_clean_error_message(&err_text);

            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            state.stats.record_detailed_request(
                "POST",
                "/v1/messages",
                requested_model,
                status.as_u16(),
                0,
                lat,
                &account.name,
                effort_str,
                "Content-Type: application/json",
                &req_body_str,
                &err_text,
                Some(&error_msg),
            );

            if is_429 && cooldown_429 {
                let retry_hint =
                    crate::proxy::balancer::LoadBalancer::parse_retry_delay_secs(&err_text);
                state.balancer.record_rate_limit_hit(
                    &account.id,
                    429,
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP 429: {}", error_msg)),
                    retry_hint,
                );
                if auto_rotate_fp {
                    let mut cfg = state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                        acc.device_profile = Some(crate::config::DeviceProfile::generate());
                        let _ = cfg.save();
                    }
                }
            } else if is_auth_error {
                state.balancer.clear_session_bindings(&account.id);
                state.balancer.update_failure_reason(
                    &account.id,
                    status.as_u16(),
                    &format!("HTTP {}: Auth failed ({})", status.as_u16(), error_msg),
                );
            } else if status.is_server_error() {
                state.balancer.record_rate_limit_hit(
                    &account.id,
                    status.as_u16(),
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP {}: {}", status.as_u16(), error_msg)),
                    None,
                );
            }

            if is_auth_error
                || error_msg.contains("invalid_grant")
                || error_msg.contains("PERMISSION_DENIED")
            {
                log::warn!(
                    "Upstream account auth failed for {}: {}",
                    account.name,
                    error_msg
                );
                let helpful_err = if account.refresh_token.is_none() {
                    "Google OAuth access token (ya29...) has expired. Please Login via Browser to obtain a persistent session.".to_string()
                } else {
                    format!("HTTP {}: {}", status.as_u16(), error_msg)
                };
                {
                    let mut cfg = state.config.write();
                    if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                        acc.status = "ERROR".to_string();
                        // Never deactivate account permanently so it can retry or auto-recover!
                        acc.error_message = Some(helpful_err);
                    }
                    let _ = cfg.save();
                }
                state
                    .cfg_version
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            let return_status = if is_auth_error {
                StatusCode::BAD_GATEWAY
            } else {
                status
            };

            return (
                (
                    return_status,
                    Json(json!({
                        "type": "error",
                        "error": {
                            "type": "api_error",
                            "message": format!("Google Antigravity: {}", error_msg)
                        }
                    })),
                )
                    .into_response(),
                is_429 || is_auth_error,
            );
        }

        if is_streaming {
            let stream_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let log_ctx = StreamLoggingContext {
                stats: state.stats.clone(),
                method: "POST",
                path: "/v1/messages (stream)",
                model: requested_model.to_string(),
                account_name: account.name.clone(),
                reasoning_effort: effort_str.to_string(),
                request_headers: "Content-Type: application/json; Accept: text/event-stream"
                    .to_string(),
                request_body: req_body_str,
                start_time,
            };
            let mapped_stream = crate::proxy::stream::process_antigravity_to_claude_stream(
                Box::pin(res.bytes_stream()),
                stream_id,
                target_model,
                Some(log_ctx),
            );

            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache, no-transform")
                .header(header::CONNECTION, "keep-alive")
                .header("x-accel-buffering", "no")
                .header("anthropic-version", "2023-06-01")
                .body(Body::from_stream(mapped_stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            (r, false)
        } else {
            let body_text = res.text().await.unwrap_or_default();
            let mut full_text = String::new();
            let mut thinking_text = None;
            let mut sources_md_final = String::new();
            let mut in_tok = 50;
            let mut out_tok = 150;
            let mut cached_tok = 0;

            let parse_json_chunk = |val: &Value,
                                    full_text: &mut String,
                                    thinking_text: &mut Option<String>,
                                    sources_md_final: &mut String,
                                    in_tok: &mut u64,
                                    out_tok: &mut u64,
                                    cached_tok: &mut u64| {
                let actual = val.get("response").unwrap_or(val);
                if let Some(usage) = actual.get("usageMetadata") {
                    if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                        *in_tok = pt;
                    }
                    if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                        *out_tok = ct;
                    }
                    if let Some(cct) = usage.get("cachedContentTokenCount").and_then(|v| v.as_u64())
                    {
                        *cached_tok = cct;
                    }
                }
                if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                    for cand in candidates {
                        if let Some(parts) = cand
                            .get("content")
                            .and_then(|c| c.get("parts"))
                            .and_then(|p| p.as_array())
                        {
                            for part in parts {
                                let is_thought = part
                                    .get("thought")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                    if is_thought {
                                        let entry = thinking_text.get_or_insert_with(String::new);
                                        entry.push_str(txt);
                                    } else {
                                        full_text.push_str(txt);
                                    }
                                } else if let Some(inline) = part.get("inlineData") {
                                    let mime = inline
                                        .get("mimeType")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("image/png");
                                    if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                        full_text.push_str(&format!(
                                            "[Generated Image: {} bytes, {}]\n![Generated Image](data:{};base64,{})",
                                            b64.len(),
                                            mime,
                                            mime,
                                            b64
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                let (_q, _r, smd) = crate::proxy::antigravity::extract_grounding_info(actual);
                if !smd.is_empty() {
                    *sources_md_final = smd;
                }
            };

            if let Ok(direct_val) = serde_json::from_str::<Value>(body_text.trim()) {
                parse_json_chunk(
                    &direct_val,
                    &mut full_text,
                    &mut thinking_text,
                    &mut sources_md_final,
                    &mut in_tok,
                    &mut out_tok,
                    &mut cached_tok,
                );
            } else {
                for line in body_text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        let trimmed = data.trim();
                        if trimmed == "[DONE]" {
                            break;
                        }
                        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                            parse_json_chunk(
                                &val,
                                &mut full_text,
                                &mut thinking_text,
                                &mut sources_md_final,
                                &mut in_tok,
                                &mut out_tok,
                                &mut cached_tok,
                            );
                        }
                    }
                }
            }

            if !sources_md_final.is_empty() {
                full_text.push_str(&sources_md_final);
            }

            let mut content_blocks = Vec::new();
            if let Some(th) = thinking_text {
                content_blocks.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": th,
                    "signature": format!("sig_{:016x}", fastrand::u64(..))
                }));
            }
            content_blocks.push(serde_json::json!({
                "type": "text",
                "text": full_text
            }));

            let mut usage_obj = serde_json::json!({
                "input_tokens": in_tok,
                "output_tokens": out_tok
            });
            if cached_tok > 0 {
                usage_obj["cache_read_input_tokens"] = serde_json::json!(cached_tok);
            }

            let claude_resp = serde_json::json!({
                "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
                "type": "message",
                "role": "assistant",
                "content": content_blocks,
                "model": target_model,
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": usage_obj
            });

            let lat = start_time.elapsed().as_millis() as u64;
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let resp_body_str = serde_json::to_string_pretty(&claude_resp).unwrap_or_default();
            state.stats.record_detailed_request_full(
                "POST",
                "/v1/messages",
                requested_model,
                200,
                in_tok + out_tok,
                in_tok,
                out_tok,
                cached_tok,
                lat,
                &account.name,
                effort_str,
                "Content-Type: application/json",
                &req_body_str,
                &resp_body_str,
                None,
            );

            let mut response = Json(claude_resp).into_response();
            if let Ok(v) = "2023-06-01".parse() {
                response.headers_mut().insert("anthropic-version", v);
            }
            (response, false)
        }
    } else {
        // Direct Anthropic API pass-through
        let mut req_headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = account.api_key.parse() {
            req_headers.insert("x-api-key", v);
        }
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let upstream_url = format!("{}/v1/messages", account.base_url.trim_end_matches('/'));
        let client = state.get_client_for_account(account);
        let res = match client
            .post(&upstream_url)
            .headers(req_headers)
            .json(payload)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state
                    .stats
                    .record_request("POST", "/v1/messages", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"type": "error", "error": {"type": "api_error", "message": e.to_string()}}))).into_response(), false);
            }
        };

        let status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;

        if is_streaming && status.is_success() {
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let log_ctx = StreamLoggingContext {
                stats: state.stats.clone(),
                method: "POST",
                path: "/v1/messages (stream)",
                model: requested_model.to_string(),
                account_name: account.name.clone(),
                reasoning_effort: effort_str.to_string(),
                request_headers: "Content-Type: application/json; Accept: text/event-stream"
                    .to_string(),
                request_body: req_body_str,
                start_time,
            };
            let stream = pass_through_stream(Box::pin(res.bytes_stream()), Some(log_ctx));

            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache, no-transform")
                .header(header::CONNECTION, "keep-alive")
                .header("x-accel-buffering", "no")
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            (r, false)
        } else {
            let text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/messages",
                requested_model,
                status.as_u16(),
                50,
                lat,
            );
            ((status, text).into_response(), is_429)
        }
    }
}

pub fn format_amr_error_message(raw: &str, status: StatusCode, lang: &str, standardized: bool) -> String {
    if !standardized {
        return raw.to_string();
    }
    let prefix = match lang.to_lowercase().as_str() {
        "ru" => "AMR: Ошибка спуфинга — ",
        "zh" => "AMR: 伪装错误 — ",
        _ => "AMR: Spoof error — ",
    };
    if raw.starts_with(prefix) {
        return raw.to_string();
    }
    let lower_raw = raw.to_lowercase();
    let detail = match status {
        StatusCode::TOO_MANY_REQUESTS => {
            if lower_raw.contains("reset in") || lower_raw.contains("сброс через") {
                raw.to_string()
            } else if lower_raw.contains("cooling down") || lower_raw.contains("retry in") {
                let retry_part = lower_raw
                    .find("retry in ")
                    .and_then(|pos| raw[pos + 9..].split([')', '.', ' ']).next())
                    .map(|s| s.trim_end_matches('s'))
                    .and_then(|s| s.parse::<u64>().ok());
                match (lang.to_lowercase().as_str(), retry_part) {
                    ("ru", Some(secs)) => format!("Все аккаунты на временной паузе после лимита (повтор через {}с).", secs),
                    ("ru", None) => "Все аккаунты на временной паузе после лимита. Повторите запрос через несколько секунд.".to_string(),
                    ("zh", Some(secs)) => format!("所有账号均处于临时冷却期（{} 秒后重试）。", secs),
                    ("zh", None) => "所有账号均处于临时冷却期，请稍后重试。".to_string(),
                    (_, Some(secs)) => format!("All upstream accounts are temporarily cooling down (retry in {}s).", secs),
                    (_, None) => "All upstream accounts are temporarily cooling down. Please retry shortly.".to_string(),
                }
            } else if lower_raw.contains("quota exhausted") {
                raw.to_string()
            } else {
                match lang.to_lowercase().as_str() {
                    "ru" => "Все подключенные аккаунты исчерпали квоту запросов (HTTP 429). Повторите запрос позже.".to_string(),
                    "zh" => "所有已连接账号的配额均已耗尽 (HTTP 429)，请稍后再试。".to_string(),
                    _ => "All connected accounts exhausted quota (HTTP 429). Please retry later.".to_string(),
                }
            }
        }
        StatusCode::SERVICE_UNAVAILABLE => match lang.to_lowercase().as_str() {
            "ru" => "Нет доступных активных аккаунтов в пуле AMR.".to_string(),
            "zh" => "AMR 账号池中没有可用的活跃账号。".to_string(),
            _ => "No active accounts available in AMR pool.".to_string(),
        },
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            if lower_raw.contains("disabled") || lower_raw.contains("unavailable") || lower_raw.contains("отключен") {
                raw.to_string()
            } else {
                match lang.to_lowercase().as_str() {
                    "ru" => "Ошибка авторизации Google OAuth или доступ запрещен.".to_string(),
                    "zh" => "Google OAuth 认证失败或权限不足。".to_string(),
                    _ => "Authentication failure or permission denied.".to_string(),
                }
            }
        }
        StatusCode::NOT_FOUND => match lang.to_lowercase().as_str() {
            "ru" => "Запрошенная модель или ресурс не найден (HTTP 404). Проверьте ID модели в настройках.".to_string(),
            "zh" => "请求的模型或资源未找到 (HTTP 404)，请检查设置中的模型 ID。".to_string(),
            _ => "The requested model or resource was not found (HTTP 404). Please verify model ID in settings.".to_string(),
        },
        StatusCode::BAD_GATEWAY => match lang.to_lowercase().as_str() {
            "ru" => "Вышестоящий сервис Google Antigravity вернул ошибку шлюза (HTTP 502).".to_string(),
            "zh" => "上游 Google Antigravity 服务网关异常 (HTTP 502)。".to_string(),
            _ => "Upstream Google Antigravity gateway error (HTTP 502).".to_string(),
        },
        _ => raw.to_string(),
    };
    format!("{}{}", prefix, detail)
}

pub async fn test_proxy_connection(proxy_url: &str) -> Result<u64, String> {
    let clean = proxy_url.trim();
    if clean.is_empty() {
        return Err("Proxy URL is empty".to_string());
    }
    let proxy = reqwest::Proxy::all(clean).map_err(|e| format!("Invalid proxy URL: {}", e))?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Failed to build proxy client: {}", e))?;

    let t0 = std::time::Instant::now();
    let res = client
        .get("https://daily-cloudcode-pa.googleapis.com")
        .send()
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    let _ = res.status();
    Ok(t0.elapsed().as_millis() as u64)
}

pub async fn run_diagnostic_check(state: &AppState, model_id: &str) -> (u16, u64, String) {
    let t0 = std::time::Instant::now();
    let cfg = state.config.read().clone();
    if !cfg.is_model_enabled(model_id) {
        return (403, 0, "Model is disabled in settings".to_string());
    }

    // 1. Audio STT (Gemini Transcribe) diagnostics
    if model_id.contains("whisper") || model_id.contains("transcrib") {
        use base64::Engine;
        let sample_pcm = vec![0u8; 3200];
        let wav_data = crate::proxy::audio::wrap_pcm_in_wav(&sample_pcm, 16000, 1, 16);
        let base64_audio = base64::engine::general_purpose::STANDARD.encode(&wav_data);
        let gemini_model = crate::proxy::audio::pick_dynamic_gemini_model(state, "chat", Some(model_id));
        let res = crate::proxy::audio::execute_generate_content(state, &gemini_model, &gemini_model, |acc| {
            let project_id = acc
                .project_id
                .clone()
                .unwrap_or_else(|| "aicode-consumers".to_string());
            serde_json::json!({
                "project": project_id,
                "requestId": format!("stt-diag-{}", uuid::Uuid::new_v4()),
                "userAgent": "antigravity",
                "requestType": "agent",
                "model": gemini_model,
                "request": {
                    "contents": [{
                        "role": "user",
                        "parts": [
                            {
                                "inlineData": {
                                    "mimeType": "audio/wav",
                                    "data": base64_audio
                                }
                            },
                            {
                                "text": "Transcribe."
                            }
                        ]
                    }]
                }
            })
        }).await;
        let latency = t0.elapsed().as_millis() as u64;
        return match res {
            Ok(_) => (200, latency, "OK: Audio Transcription (STT) is responsive and healthy".to_string()),
            Err((status, msg)) => (status.as_u16(), latency, format!("Whisper Diagnostic failed (HTTP {}): {}", status, msg)),
        };
    }

    let acc_res = state.balancer.select_account(&cfg, model_id, None);
    let mut account = match acc_res {
        Ok(a) => a,
        Err(e) => {
            let code = match e {
                crate::proxy::balancer::RoutingError::NoAccounts => 503,
                _ => 429,
            };
            return (code, t0.elapsed().as_millis() as u64, format!("Routing failed: {}", e));
        }
    };

    let (target_model, _) = crate::proxy::router::resolve_model_with_effort(model_id, &account, None);

    let client_id = account
        .device_profile
        .as_ref()
        .map(|p| p.dev_device_id.clone())
        .unwrap_or_else(|| "device-id-fallback".to_string());
    let machine_id = account
        .device_profile
        .as_ref()
        .map(|p| p.machine_id.clone())
        .unwrap_or_else(|| "machine-id-fallback".to_string());

    let platform = cfg.resolved_platform();
    let ide_ver = cfg.spoof_ide_version.clone();
    let spoof_arch = cfg.spoof_arch.clone();
    let spoof_channel = cfg.spoof_channel.clone();
    let spoof_go = cfg.spoof_go_sdk;
    let inject_meta = cfg.inject_ide_metadata;

    let is_chat = !target_model.contains("whisper") && !target_model.contains("transcrib") && target_model != "web-search";
    let headers = if is_chat {
        build_upstream_headers(
            &account.api_key,
            &client_id,
            &machine_id,
            platform,
            &ide_ver,
            &spoof_arch,
            &spoof_channel,
            spoof_go,
            inject_meta,
            &target_model,
        )
    } else {
        build_upstream_headers_json(
            &account.api_key,
            &client_id,
            &machine_id,
            platform,
            &ide_ver,
            &spoof_arch,
            &spoof_channel,
            spoof_go,
            inject_meta,
            &target_model,
        )
    };

    let test_req = crate::proxy::mappers::openai::OpenAIChatRequest {
        model: target_model.clone(),
        messages: vec![crate::proxy::mappers::openai::OpenAIMessage {
            role: "user".to_string(),
            content: serde_json::Value::String("Hello, diagnostic test.".to_string()),
            reasoning_content: None,
            refusal: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        stream: false,
        temperature: Some(0.2),
        max_tokens: Some(10),
        max_completion_tokens: None,
        reasoning_effort: None,
        stream_options: None,
        tools: None,
        tool_choice: None,
        extra: std::collections::HashMap::new(),
    };

    let test_payload = crate::proxy::antigravity::build_antigravity_payload(
        &test_req,
        &target_model,
        account.project_id.as_deref(),
        None,
    );

    let client = state.get_client_for_account(&account);
    let endpoints = if is_chat {
        if cfg.lock_production_endpoints {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..1]
        } else {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..]
        }
    } else if cfg.lock_production_endpoints {
        &crate::proxy::antigravity::GENERATE_ENDPOINTS[..1]
    } else {
        &crate::proxy::antigravity::GENERATE_ENDPOINTS[..]
    };

    let mut last_status = 502;
    let mut last_err = "No response from upstream".to_string();

    for &endpoint_url in endpoints {
        match client.post(endpoint_url).headers(headers.clone()).json(&test_payload).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                last_status = status;
                let latency = t0.elapsed().as_millis() as u64;
                if resp.status().is_success() {
                    state.balancer.record_success(&account.id);
                    return (status, latency, "OK: Model is responsive and healthy".to_string());
                } else if (status == 401 || status == 403) && account.refresh_token.is_some() {
                    if let Some(fresh_token) = refresh_account_token_and_save(state, &account).await {
                            account.api_key = fresh_token;
                            let retry_headers = if is_chat {
                                build_upstream_headers(
                                    &account.api_key,
                                    &client_id,
                                    &machine_id,
                                    platform,
                                    &ide_ver,
                                    &spoof_arch,
                                    &spoof_channel,
                                    spoof_go,
                                    inject_meta,
                                    &target_model,
                                )
                            } else {
                                build_upstream_headers_json(
                                    &account.api_key,
                                    &client_id,
                                    &machine_id,
                                    platform,
                                    &ide_ver,
                                    &spoof_arch,
                                    &spoof_channel,
                                    spoof_go,
                                    inject_meta,
                                    &target_model,
                                )
                            };
                            if let Ok(retry_resp) = client.post(endpoint_url).headers(retry_headers).json(&test_payload).send().await {
                                let retry_status = retry_resp.status().as_u16();
                                let retry_lat = t0.elapsed().as_millis() as u64;
                                if retry_resp.status().is_success() {
                                    state.balancer.record_success(&account.id);
                                    return (retry_status, retry_lat, "OK: Model is responsive and healthy (token auto-refreshed)".to_string());
                                } else {
                                    let err_text = retry_resp.text().await.unwrap_or_default();
                                    return (retry_status, retry_lat, format!("Upstream HTTP {}: {}", retry_status, err_text));
                                }
                            }
                        }
                    return (status, latency, format!("Upstream HTTP {}: Authentication failed", status));
                } else {
                    let err_text = resp.text().await.unwrap_or_default();
                    last_err = format!("Upstream HTTP {}: {}", status, err_text);
                }
            }
            Err(e) => {
                last_err = format!("Network error: {}", e);
            }
        }
    }

    (last_status, t0.elapsed().as_millis() as u64, last_err)
}
