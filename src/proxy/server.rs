use crate::config::AppConfig;
use crate::proxy::fingerprint::apply_fingerprint;
use crate::proxy::mappers::claude::ClaudeMessageResponse;
use crate::proxy::mappers::openai::*;
use crate::proxy::router::*;
use crate::proxy::stats::ProxyStats;
use crate::proxy::stream::{pass_through_stream, process_claude_to_openai_stream, StreamLoggingContext};
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
    pub fn get_client_for_account(&self, account: &crate::config::UpstreamAccount) -> reqwest::Client {
        if let Some(ref p_url) = account.proxy_url {
            let p_trim = p_url.trim();
            if !p_trim.is_empty() {
                {
                    let r = self.proxy_clients.read();
                    if let Some(c) = r.get(p_trim) {
                        return c.clone();
                    }
                }
                if let Ok(proxy) = reqwest::Proxy::all(p_trim) {
                    if let Ok(c) = reqwest::Client::builder()
                        .proxy(proxy)
                        .tcp_nodelay(true)
                        .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
                        .pool_idle_timeout(Some(std::time::Duration::from_secs(90)))
                        .pool_max_idle_per_host(32)
                        .timeout(std::time::Duration::from_secs(120))
                        .build()
                    {
                        let mut w = self.proxy_clients.write();
                        w.insert(p_trim.to_string(), c.clone());
                        return c;
                    }
                }
            }
        }
        self.http_client.clone()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_upstream_headers(
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
) -> reqwest::header::HeaderMap {
    let mut req_headers = reqwest::header::HeaderMap::new();
    if let Ok(v) = format!("Bearer {}", token.trim()).parse() {
        req_headers.insert(reqwest::header::AUTHORIZATION, v);
    }
    if let Ok(v) = "text/event-stream".parse() {
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
        .route("/v1/dashboard/billing/subscription", get(billing_subscription))
        .route("/v1/dashboard/billing/subscription/", get(billing_subscription))
        .route("/dashboard/billing/subscription", get(billing_subscription))
        .route("/dashboard/billing/subscription/", get(billing_subscription))
        .route("/v1/dashboard/billing/usage", get(billing_usage))
        .route("/v1/dashboard/billing/usage/", get(billing_usage))
        .route("/dashboard/billing/usage", get(billing_usage))
        .route("/dashboard/billing/usage/", get(billing_usage))
        .route("/v1/usage", get(billing_usage))
        .route("/v1/usage/", get(billing_usage))
        .route("/usage", get(billing_usage))
        .route("/usage/", get(billing_usage))
        .route("/oauth/callback", get(oauth_callback))
        .layer(CorsLayer::permissive())
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

async fn oauth_callback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let port = state.config.read().port;
    let redirect_uri = format!("http://127.0.0.1:{}/oauth/callback", port);

    let (access_token, refresh_token_opt, email, tier, quota_used, quota_limit, quota_reset, models, detailed_quota, project_id_opt) =
        if let Some(code) = params.get("code") {
            match crate::proxy::antigravity::exchange_code(&state.http_client, code, &redirect_uri).await {
                Ok(token_resp) => {
                    let acc_token = token_resp.access_token;
                    let ref_token = token_resp.refresh_token;
                    let email = token_resp
                        .id_token
                        .as_deref()
                        .and_then(crate::proxy::antigravity::extract_email_from_jwt)
                        .unwrap_or_else(|| {
                            futures_util::FutureExt::now_or_never(crate::proxy::antigravity::fetch_user_info(&state.http_client, &acc_token))
                                .and_then(|r| r.ok())
                                .map(|i| i.email)
                                .unwrap_or_else(|| "antigravity.user@gmail.com".to_string())
                        });
                    let (project_id, tier) = crate::proxy::antigravity::fetch_project_and_tier(&state.http_client, &acc_token).await;
                    let (models, used, limit, reset, det) = match crate::proxy::antigravity::fetch_models_and_quota(&state.http_client, &acc_token, project_id.as_deref()).await {
                        Ok(res) => (Some(res.0), res.1, res.2, res.3, res.4),
                        Err(_) => {
                            let det = crate::proxy::antigravity::fetch_user_quota_summary(&state.http_client, &acc_token).await;
                            (None, 0, 100_000, "in 4h 00m".to_string(), det)
                        }
                    };
                    (acc_token, ref_token, email, tier, used, limit, reset, models, det, project_id)
                }
                Err(e) => {
                    log::warn!("OAuth exchange error: {}", e);
                    let raw_token = params.get("token").cloned().unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
                    let det = crate::proxy::antigravity::DetailedQuotaInfo {
                        claude_gpt_percent: 100,
                        claude_gpt_reset: "in 4h 00m".to_string(),
                        claude_gpt_weekly_percent: 100,
                        claude_gpt_weekly_reset: "in 7d 00h".to_string(),
                        gemini_percent: 100,
                        gemini_reset: "in 4h 00m".to_string(),
                        gemini_weekly_percent: 100,
                        gemini_weekly_reset: "in 7d 00h".to_string(),
                    };
                    (raw_token, None, "antigravity.user@gmail.com".to_string(), "PRO".to_string(), 0, 100_000, "in 4h 00m".to_string(), None, det, None)
                }
            }
        } else {
            let raw_token = params.get("token").or_else(|| params.get("access_token")).cloned().unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
            let email = params.get("email").cloned().unwrap_or_else(|| "antigravity.user@gmail.com".to_string());
            let det = crate::proxy::antigravity::fetch_user_quota_summary(&state.http_client, &raw_token).await;
            (raw_token, None, email, "PRO".to_string(), 0, 100_000, "in 4h 00m".to_string(), None, det, None)
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
            existing.quota_used = quota_used;
            existing.quota_limit = quota_limit;
            existing.quota_reset = quota_reset;
            existing.claude_gpt_percent = detailed_quota.claude_gpt_percent;
            existing.claude_gpt_reset = detailed_quota.claude_gpt_reset;
            existing.claude_gpt_weekly_percent = detailed_quota.claude_gpt_weekly_percent;
            existing.claude_gpt_weekly_reset = detailed_quota.claude_gpt_weekly_reset;
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
                is_active: true,
                added_at: Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)),
                device_profile: Some(crate::config::DeviceProfile::generate()),
                proxy_url: None,
            });
            cfg.active_account_id = Some(new_id);
        }
        if let Some(m) = models {
            cfg.models = m;
        }
        let _ = cfg.save();
    }

    state.cfg_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    axum::response::Html(format!(r#"
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
    "#, email, tier, env!("CARGO_PKG_VERSION")))
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok", "service": "AMR", "version": env!("CARGO_PKG_VERSION")})))
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

    let total_tokens = state.stats.total_tokens.load(std::sync::atomic::Ordering::Relaxed);
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

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<OpenAIChatRequest>,
) -> Response {
    let start_time = Instant::now();
    let requested_model = payload.model.clone();

    let header_session = headers
        .get("x-session-id")
        .or_else(|| headers.get("session-id"))
        .or_else(|| headers.get("conversation-id"))
        .and_then(|h| h.to_str().ok());

    let session_id = crate::proxy::balancer::LoadBalancer::extract_session_fingerprint(&payload, header_session);

    let selected_account = {
        let cfg = state.config.read();
        state.balancer.select_account(&cfg, &requested_model, Some(&session_id))
    };

    let initial_account = match selected_account {
        Ok(acc) if !acc.api_key.is_empty() => acc,
        Ok(_) => {
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, 502, 0, start_time.elapsed().as_millis() as u64);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": "Active upstream account has empty API token in AMR. Please connect an account in AMR.", "type": "upstream_error", "code": "upstream_account_missing_token"}})),
            ).into_response();
        }
        Err(e) => {
            let lat = start_time.elapsed().as_millis() as u64;
            let status = match e {
                crate::proxy::balancer::RoutingError::NoAccounts => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::TOO_MANY_REQUESTS,
            };
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, status.as_u16(), 0, lat);
            return (
                status,
                Json(json!({
                    "error": {
                        "message": format!("{}", e),
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

    let (resp, should_failover) = execute_chat_request(&state, &payload, &initial_account, &requested_model, start_time).await;

    if should_failover {
        let next_acc_opt = {
            let cfg = state.config.read();
            state.balancer.select_account(&cfg, &requested_model, Some(&session_id)).ok()
        };

        if let Some(next_acc) = next_acc_opt {
            if next_acc.id != initial_account.id {
                log::info!("Automatic failover (429/Auth): rotating session from {} to {}", initial_account.name, next_acc.name);
                let (retry_resp, _) = execute_chat_request(&state, &payload, &next_acc, &requested_model, start_time).await;
                return retry_resp;
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
    let native_effort = payload.reasoning_effort.as_deref().or_else(|| {
        payload.extra.get("reasoning").and_then(|r| {
            if let Some(s) = r.as_str() {
                Some(s)
            } else {
                r.get("effort").and_then(|e| e.as_str())
            }
        })
    }).or_else(|| {
        payload.extra.get("thinking").and_then(|t| {
            t.get("type").and_then(|typ| typ.as_str())
        })
    });

    let (base_clean, reasoning_effort) = parse_model_and_reasoning(requested_model, native_effort);
    let (target_model, effective_effort) = crate::proxy::router::resolve_model_with_effort(&base_clean, account, reasoning_effort);
    let effort_str = match effective_effort {
        Some(crate::proxy::router::ReasoningEffort::Off) => "off",
        Some(crate::proxy::router::ReasoningEffort::Low) => "low",
        Some(crate::proxy::router::ReasoningEffort::Medium) => "medium",
        Some(crate::proxy::router::ReasoningEffort::High) => "high",
        None => "off",
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
            (uuid::Uuid::new_v4().to_string(), format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()))
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

        let endpoints: &[&str] = if lock_prod {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..1]
        } else {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..]
        };

        for endpoint_url in endpoints {
            let req_headers = build_upstream_headers(
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
            );

            if let Ok(r) = client.post(*endpoint_url).headers(req_headers).json(&payload_json).send().await {
                let s = r.status();
                if s.is_success() {
                    state.balancer.record_success(&account.id);
                    res_opt = Some(r);
                    break;
                } else if s == StatusCode::TOO_MANY_REQUESTS {
                    // CRITICAL ANTI-BAN: Step up the backoff ladder and never hammer secondary endpoints
                    if cooldown_429 {
                        state.balancer.record_failure_with_reason(
                            &account.id,
                            429,
                            cooldown_secs,
                            cooldown_jitter,
                            Some("HTTP 429: Too Many Requests (Rate limit / Quota exhausted)"),
                        );
                    }
                    if auto_rotate_fp {
                        let mut cfg = state.config.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                            acc.device_profile = Some(crate::config::DeviceProfile::generate());
                            let _ = cfg.save();
                        }
                    }
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
                state.stats.record_request("POST", "/v1/chat/completions", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": "Antigravity upstream connection failure"}}))).into_response(), false);
            }
        };

        let mut status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;
        let mut is_auth_error = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;

        // If auth error occurred and account has a refresh token, automatically refresh on the fly and retry!
        if is_auth_error {
            if let Some(ref rt) = account.refresh_token {
                log::info!("Upstream token expired for {}, attempting automatic refresh", account.name);
                if let Ok(tok_res) = crate::proxy::antigravity::refresh_token(&state.http_client, rt).await {
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
                        let _ = cfg.save();
                    }
                    state.cfg_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                    for endpoint_url in crate::proxy::antigravity::STREAM_ENDPOINTS {
                        let retry_headers = build_upstream_headers(
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
                        );

                        if let Ok(retry_res) = state.http_client.post(endpoint_url).headers(retry_headers).json(&payload_json).send().await {
                            status = retry_res.status();
                            is_auth_error = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;
                            res = retry_res;
                            if status.is_success() {
                                break;
                            }
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
                state.balancer.update_failure_reason(&account.id, 429, &format!("HTTP 429: {}", error_msg));
            } else if is_auth_error {
                state.balancer.update_failure_reason(&account.id, status.as_u16(), &format!("HTTP {}: Auth failed ({})", status.as_u16(), error_msg));
            } else if status.is_server_error() {
                state.balancer.record_failure_with_reason(
                    &account.id,
                    status.as_u16(),
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP {}: {}", status.as_u16(), error_msg)),
                );
            }

            if is_auth_error || error_msg.contains("invalid_grant") || error_msg.contains("PERMISSION_DENIED") {
                log::warn!("Upstream account auth failed for {}: {}", account.name, error_msg);
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
                state.cfg_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

            return ((return_status, Json(json!({
                "error": {
                    "message": helpful_msg,
                    "type": "upstream_error",
                    "code": err_code
                }
            }))).into_response(), is_429 || is_auth_error);
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
                request_headers: "Content-Type: application/json; Accept: text/event-stream".to_string(),
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
            let mut in_tok = 50;
            let mut out_tok = 150;

            for line in body_text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    let trimmed = data.trim();
                    if trimmed == "[DONE]" {
                        break;
                    }
                    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                        let actual = val.get("response").unwrap_or(&val);
                        if let Some(usage) = actual.get("usageMetadata") {
                            if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                                in_tok = pt;
                            }
                            if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                                out_tok = ct;
                            }
                        }
                        if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                            for cand in candidates {
                                if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                    for part in parts {
                                        let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                        if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                            if is_thought {
                                                let entry = thinking_text.get_or_insert_with(String::new);
                                                entry.push_str(txt);
                                            } else {
                                                full_text.push_str(txt);
                                            }
                                        } else if let Some(inline) = part.get("inlineData") {
                                            let mime = inline.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image/png");
                                            if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                                full_text.push_str(&format!("[Generated Image: {} bytes, {}]\n![Generated Image](data:{};base64,{})", b64.len(), mime, mime, b64));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
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
                    },
                    logprobs: None,
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(OpenAIUsage {
                    prompt_tokens: in_tok,
                    completion_tokens: out_tok,
                    total_tokens: in_tok + out_tok,
                    prompt_tokens_details: Some(PromptTokensDetails {
                        cached_tokens: 0,
                        audio_tokens: 0,
                    }),
                    completion_tokens_details: Some(CompletionTokensDetails {
                        reasoning_tokens: 0,
                        audio_tokens: 0,
                        accepted_prediction_tokens: 0,
                        rejected_prediction_tokens: 0,
                    }),
                }),
            };

            let lat = start_time.elapsed().as_millis() as u64;
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let resp_body_str = serde_json::to_string_pretty(&oai_resp).unwrap_or_default();
            state.stats.record_detailed_request_exact(
                "POST",
                "/v1/chat/completions",
                requested_model,
                200,
                in_tok + out_tok,
                in_tok,
                out_tok,
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
            if let Ok(v) = req_id.parse() { response.headers_mut().insert("x-request-id", v); }
            if let Ok(v) = "user-personal".parse() { response.headers_mut().insert("openai-organization", v); }
            if let Ok(v) = lat.to_string().parse() { response.headers_mut().insert("openai-processing-ms", v); }
            if let Ok(v) = "2020-10-01".parse() { response.headers_mut().insert("openai-version", v); }
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
        let res = match state.http_client.post(&upstream_url).headers(req_headers).json(&claude_req).send().await {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request("POST", "/v1/chat/completions", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": format!("Upstream connection error: {}", e)}}))).into_response(), false);
            }
        };

        let status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;
        let is_auth_error = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;
        let (cooldown_429, cooldown_secs, cooldown_jitter) = {
            let cfg = state.config.read();
            (cfg.cooldown_on_429, cfg.cooldown_duration_secs, cfg.cooldown_jitter_percent)
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
                state.balancer.update_failure_reason(&account.id, 429, &format!("HTTP 429: {}", error_msg));
            } else if is_auth_error {
                state.balancer.update_failure_reason(&account.id, status.as_u16(), &format!("HTTP {}: Auth failed ({})", status.as_u16(), error_msg));
            } else if status.is_server_error() {
                state.balancer.record_failure_with_reason(
                    &account.id,
                    status.as_u16(),
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP {}: {}", status.as_u16(), error_msg)),
                );
            }

            if is_auth_error || error_msg.contains("invalid_grant") || error_msg.contains("PERMISSION_DENIED") {
                log::warn!("Upstream account auth failed for {}: {}", account.name, error_msg);
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
                state.cfg_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            let return_status = if is_auth_error { StatusCode::BAD_GATEWAY } else { status };

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

            return ((return_status, Json(json!({
                "error": {
                    "message": helpful_msg,
                    "type": "upstream_error",
                    "code": err_code
                }
            }))).into_response(), is_429 || is_auth_error);
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
                request_headers: "Content-Type: application/json; Accept: text/event-stream".to_string(),
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
                    return ((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"message": e.to_string()}}))).into_response(), false);
                }
            };
            let oai_resp = convert_claude_response_to_openai(&claude_resp);
            let tokens = oai_resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", requested_model, 200, tokens, lat);

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let mut response = Json(oai_resp).into_response();
            if let Ok(v) = req_id.parse() { response.headers_mut().insert("x-request-id", v); }
            if let Ok(v) = "user-personal".parse() { response.headers_mut().insert("openai-organization", v); }
            if let Ok(v) = lat.to_string().parse() { response.headers_mut().insert("openai-processing-ms", v); }
            if let Ok(v) = "2020-10-01".parse() { response.headers_mut().insert("openai-version", v); }
            (response, false)
        }
    } else {
        // Direct OpenAI or custom endpoint pass-through
        let mut forward_payload = payload.clone();
        forward_payload.model = target_model;
        if let Some(eff) = reasoning_effort {
            match eff {
                ReasoningEffort::Low => forward_payload.reasoning_effort = Some("low".to_string()),
                ReasoningEffort::Medium => forward_payload.reasoning_effort = Some("medium".to_string()),
                ReasoningEffort::High => forward_payload.reasoning_effort = Some("high".to_string()),
                ReasoningEffort::Off => forward_payload.reasoning_effort = None,
            }
        }

        let mut req_headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = format!("Bearer {}", account.api_key.trim()).parse() {
            req_headers.insert("authorization", v);
        }
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let upstream_url = format!("{}/v1/chat/completions", account.base_url.trim_end_matches('/'));
        let res = match state.http_client.post(&upstream_url).headers(req_headers).json(&forward_payload).send().await {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request("POST", "/v1/chat/completions", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": e.to_string()}}))).into_response(), false);
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
                request_headers: "Content-Type: application/json; Accept: text/event-stream".to_string(),
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
            state.stats.record_request("POST", "/v1/chat/completions", requested_model, status.as_u16(), 50, lat);
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
    let model = payload.get("model").and_then(|m| m.as_str()).unwrap_or("unknown").to_string();

    let header_session = headers
        .get("x-session-id")
        .or_else(|| headers.get("session-id"))
        .or_else(|| headers.get("conversation-id"))
        .and_then(|h| h.to_str().ok());

    let session_id = if let Some(sid) = header_session {
        format!("hdr-{}", sid)
    } else if let Some(uid) = payload.get("metadata").and_then(|m| m.get("user_id")).and_then(|u| u.as_str()) {
        format!("usr-{}", uid)
    } else {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Some(sys) = payload.get("system").and_then(|s| s.as_str()) {
            hasher.write(b"[sys]");
            hasher.write(sys.as_bytes());
        }
        if let Some(msgs) = payload.get("messages").and_then(|m| m.as_array()) {
            if let Some(first_user) = msgs.iter().find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user")) {
                let content_str = first_user.get("content").map(|c| c.to_string()).unwrap_or_default();
                hasher.write(b"[u1]");
                hasher.write(content_str.as_bytes());
            }
        }
        format!("ctx-{:016x}", hasher.finish())
    };

    let selected_account = {
        let cfg = state.config.read();
        state.balancer.select_account(&cfg, &model, Some(&session_id))
    };

    let initial_account = match selected_account {
        Ok(acc) if !acc.api_key.is_empty() => acc,
        Ok(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"type": "error", "error": {"type": "authentication_error", "message": "Account has empty API token in AMR"}})),
            ).into_response();
        }
        Err(e) => {
            let status = match e {
                crate::proxy::balancer::RoutingError::NoAccounts => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::TOO_MANY_REQUESTS,
            };
            return (
                status,
                Json(json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": format!("{}", e)
                    }
                })),
            ).into_response();
        }
    };

    let (resp, should_failover) = execute_claude_request(&state, &payload, &initial_account, &model, start_time).await;

    if should_failover {
        let next_acc_opt = {
            let cfg = state.config.read();
            state.balancer.select_account(&cfg, &model, Some(&session_id)).ok()
        };

        if let Some(next_acc) = next_acc_opt {
            if next_acc.id != initial_account.id {
                log::info!("Claude Messages failover (429/Auth) from {} to {}", initial_account.name, next_acc.name);
                let (retry_resp, _) = execute_claude_request(&state, &payload, &next_acc, &model, start_time).await;
                return retry_resp;
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
    let is_streaming = payload.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);

    let (base_clean, reasoning_effort) = parse_model_and_reasoning(
        requested_model,
        payload.get("thinking").and_then(|t| t.get("type")).and_then(|v| v.as_str()),
    );
    let (target_model, effective_effort) = crate::proxy::router::resolve_model_with_effort(&base_clean, account, reasoning_effort);
    let effort_str = match effective_effort {
        Some(crate::proxy::router::ReasoningEffort::Off) => "off",
        Some(crate::proxy::router::ReasoningEffort::Low) => "low",
        Some(crate::proxy::router::ReasoningEffort::Medium) => "medium",
        Some(crate::proxy::router::ReasoningEffort::High) => "high",
        None => "off",
    };

    if account.provider == "antigravity" {
        let (platform_str, ide_ver) = {
            let cfg = state.config.read();
            (cfg.resolved_platform(), cfg.spoof_ide_version.clone())
        };
        let payload_json = crate::proxy::antigravity::build_antigravity_payload_from_claude_with_meta(
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
            (uuid::Uuid::new_v4().to_string(), format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()))
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

        let endpoints: &[&str] = if lock_prod {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..1]
        } else {
            &crate::proxy::antigravity::STREAM_ENDPOINTS[..]
        };

        for endpoint_url in endpoints {
            let req_headers = build_upstream_headers(
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
            );

            if let Ok(r) = client.post(*endpoint_url).headers(req_headers).json(&payload_json).send().await {
                let s = r.status();
                if s.is_success() {
                    state.balancer.record_success(&account.id);
                    res_opt = Some(r);
                    break;
                } else if s == StatusCode::TOO_MANY_REQUESTS {
                    // CRITICAL ANTI-BAN: Step up the backoff ladder and never hammer secondary endpoints
                    if cooldown_429 {
                        state.balancer.record_failure_with_reason(
                            &account.id,
                            429,
                            cooldown_secs,
                            cooldown_jitter,
                            Some("HTTP 429: Too Many Requests (Rate limit / Quota exhausted)"),
                        );
                    }
                    if auto_rotate_fp {
                        let mut cfg = state.config.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                            acc.device_profile = Some(crate::config::DeviceProfile::generate());
                            let _ = cfg.save();
                        }
                    }
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
                state.stats.record_request("POST", "/v1/messages", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"type": "error", "error": {"type": "api_error", "message": "Antigravity upstream connection failure"}}))).into_response(), false);
            }
        };

        let mut status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;
        let mut is_auth_error = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;

        if is_auth_error {
            if let Some(ref rt) = account.refresh_token {
                log::info!("Claude token expired for {}, attempting automatic refresh", account.name);
                if let Ok(tok_res) = crate::proxy::antigravity::refresh_token(&state.http_client, rt).await {
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
                        let _ = cfg.save();
                    }
                    state.cfg_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                    for endpoint_url in crate::proxy::antigravity::STREAM_ENDPOINTS {
                        let retry_headers = build_upstream_headers(
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
                        );

                        if let Ok(retry_res) = state.http_client.post(endpoint_url).headers(retry_headers).json(&payload_json).send().await {
                            status = retry_res.status();
                            is_auth_error = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;
                            res = retry_res;
                            if status.is_success() {
                                break;
                            }
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
                state.balancer.update_failure_reason(&account.id, 429, &format!("HTTP 429: {}", error_msg));
            } else if is_auth_error {
                state.balancer.update_failure_reason(&account.id, status.as_u16(), &format!("HTTP {}: Auth failed ({})", status.as_u16(), error_msg));
            } else if status.is_server_error() {
                state.balancer.record_failure_with_reason(
                    &account.id,
                    status.as_u16(),
                    cooldown_secs,
                    cooldown_jitter,
                    Some(&format!("HTTP {}: {}", status.as_u16(), error_msg)),
                );
            }

            if is_auth_error || error_msg.contains("invalid_grant") || error_msg.contains("PERMISSION_DENIED") {
                log::warn!("Upstream account auth failed for {}: {}", account.name, error_msg);
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
                state.cfg_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }

            let return_status = if is_auth_error { StatusCode::BAD_GATEWAY } else { status };

            return ((return_status, Json(json!({
                "type": "error",
                "error": {
                    "type": "api_error",
                    "message": format!("Google Antigravity: {}", error_msg)
                }
            }))).into_response(), is_429 || is_auth_error);
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
                request_headers: "Content-Type: application/json; Accept: text/event-stream".to_string(),
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
            let mut in_tok = 50;
            let mut out_tok = 150;

            for line in body_text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    let trimmed = data.trim();
                    if trimmed == "[DONE]" {
                        break;
                    }
                    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                        let actual = val.get("response").unwrap_or(&val);
                        if let Some(usage) = actual.get("usageMetadata") {
                            if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                                in_tok = pt;
                            }
                            if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                                out_tok = ct;
                            }
                        }
                        if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                            for cand in candidates {
                                if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                    for part in parts {
                                        let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                        if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                            if is_thought {
                                                let entry = thinking_text.get_or_insert_with(String::new);
                                                entry.push_str(txt);
                                            } else {
                                                full_text.push_str(txt);
                                            }
                                        } else if let Some(inline) = part.get("inlineData") {
                                            let mime = inline.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image/png");
                                            if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                                full_text.push_str(&format!("[Generated Image: {} bytes, {}]\n![Generated Image](data:{};base64,{})", b64.len(), mime, mime, b64));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let mut content_blocks = Vec::new();
            if let Some(th) = thinking_text {
                content_blocks.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": th
                }));
            }
            content_blocks.push(serde_json::json!({
                "type": "text",
                "text": full_text
            }));

            let claude_resp = serde_json::json!({
                "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
                "type": "message",
                "role": "assistant",
                "content": content_blocks,
                "model": target_model,
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": in_tok,
                    "output_tokens": out_tok
                }
            });

            let lat = start_time.elapsed().as_millis() as u64;
            let req_body_str = serde_json::to_string_pretty(&payload).unwrap_or_default();
            let resp_body_str = serde_json::to_string_pretty(&claude_resp).unwrap_or_default();
            state.stats.record_detailed_request_exact(
                "POST",
                "/v1/messages",
                requested_model,
                200,
                in_tok + out_tok,
                in_tok,
                out_tok,
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
        let res = match state.http_client.post(&upstream_url).headers(req_headers).json(payload).send().await {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request("POST", "/v1/messages", requested_model, 502, 0, lat);
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
                request_headers: "Content-Type: application/json; Accept: text/event-stream".to_string(),
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
            state.stats.record_request("POST", "/v1/messages", requested_model, status.as_u16(), 50, lat);
            ((status, text).into_response(), is_429)
        }
    }
}
