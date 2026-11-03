use crate::config::AppConfig;
use crate::proxy::fingerprint::apply_fingerprint;
use crate::proxy::mappers::claude::ClaudeMessageResponse;
use crate::proxy::mappers::openai::*;
use crate::proxy::router::*;
use crate::proxy::stats::ProxyStats;
use crate::proxy::stream::{pass_through_stream, process_claude_to_openai_stream};
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
}

pub fn create_proxy_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/v1/models", get(list_models))
        .route("/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/chat/completions", post(chat_completions))
        .route("/v1/messages", post(claude_messages))
        .route("/oauth/callback", get(oauth_callback))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn oauth_callback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let port = state.config.read().port;
    let redirect_uri = format!("http://127.0.0.1:{}/oauth/callback", port);

    let (access_token, email, tier, quota_used, quota_limit, quota_reset, models, detailed_quota) =
        if let Some(code) = params.get("code") {
            match crate::proxy::antigravity::exchange_code(&state.http_client, code, &redirect_uri).await {
                Ok(token_resp) => {
                    let acc_token = token_resp.access_token;
                    let email = match crate::proxy::antigravity::fetch_user_info(&state.http_client, &acc_token).await {
                        Ok(info) => info.email,
                        Err(_) => "antigravity.user@gmail.com".to_string(),
                    };
                    let (project_id, tier) = crate::proxy::antigravity::fetch_project_and_tier(&state.http_client, &acc_token).await;
                    let (models, used, limit, reset, det) = match crate::proxy::antigravity::fetch_models_and_quota(&state.http_client, &acc_token, project_id.as_deref()).await {
                        Ok(res) => (Some(res.0), res.1, res.2, res.3, res.4),
                        Err(_) => {
                            let det = crate::proxy::antigravity::fetch_user_quota_summary(&state.http_client, &acc_token).await;
                            (None, 0, 100_000, "in 4h 00m".to_string(), det)
                        }
                    };
                    (acc_token, email, tier, used, limit, reset, models, det)
                }
                Err(e) => {
                    log::warn!("OAuth exchange error: {}", e);
                    let raw_token = params.get("token").cloned().unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
                    let det = crate::proxy::antigravity::DetailedQuotaInfo {
                        claude_gpt_percent: 100,
                        claude_gpt_reset: "in 4h 00m".to_string(),
                        gemini_percent: 100,
                        gemini_reset: "in 4h 00m".to_string(),
                    };
                    (raw_token, "antigravity.user@gmail.com".to_string(), "PRO".to_string(), 0, 100_000, "in 4h 00m".to_string(), None, det)
                }
            }
        } else {
            let raw_token = params.get("token").or_else(|| params.get("access_token")).cloned().unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
            let email = params.get("email").cloned().unwrap_or_else(|| "antigravity.user@gmail.com".to_string());
            let det = crate::proxy::antigravity::fetch_user_quota_summary(&state.http_client, &raw_token).await;
            (raw_token, email, "PRO".to_string(), 0, 100_000, "in 4h 00m".to_string(), None, det)
        };

    // Add or update account in config
    {
        let mut cfg = state.config.write();
        if let Some(existing) = cfg.accounts.iter_mut().find(|a| a.name == email) {
            existing.api_key = access_token;
            existing.tier = tier.clone();
            existing.quota_used = quota_used;
            existing.quota_limit = quota_limit;
            existing.quota_reset = quota_reset;
            existing.claude_gpt_percent = detailed_quota.claude_gpt_percent;
            existing.claude_gpt_reset = detailed_quota.claude_gpt_reset;
            existing.gemini_percent = detailed_quota.gemini_percent;
            existing.gemini_reset = detailed_quota.gemini_reset;
            existing.status = "Active".to_string();
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
                fingerprint: "Claude Code 2.x".to_string(),
                tier: tier.clone(),
                quota_used,
                quota_limit,
                quota_reset,
                claude_gpt_percent: detailed_quota.claude_gpt_percent,
                claude_gpt_reset: detailed_quota.claude_gpt_reset,
                gemini_percent: detailed_quota.gemini_percent,
                gemini_reset: detailed_quota.gemini_reset,
                status: "Active".to_string(),
                is_active: true,
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
            <title>Aroxy — Account Connected</title>
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
                <p>Antigravity quota and active models have been synchronized with Aroxy.<br>You can close this tab and return to the application.</p>
                <div class="badge">Aroxy v0.0.2</div>
            </div>
        </body>
        </html>
    "#, email, tier))
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok", "service": "Aroxy", "version": "0.0.2"})))
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
        state.balancer.select_account(&cfg, Some(&session_id))
    };

    let initial_account = match selected_account {
        Some(acc) if !acc.api_key.is_empty() => acc,
        _ => {
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, 401, 0, start_time.elapsed().as_millis() as u64);
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"message": "No active upstream account or API key configured in Aroxy"}})),
            ).into_response();
        }
    };

    let (resp, is_429) = execute_chat_request(&state, &payload, &initial_account, &requested_model, start_time).await;

    if is_429 {
        state.balancer.mark_cooldown(&initial_account.id, 60);
        let next_acc_opt = {
            let cfg = state.config.read();
            if cfg.failover_on_429 {
                state.balancer.select_account(&cfg, Some(&session_id))
            } else {
                None
            }
        };

        if let Some(next_acc) = next_acc_opt {
            if next_acc.id != initial_account.id {
                log::info!("Automatic 429 failover: rotating session from {} to {}", initial_account.name, next_acc.name);
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

    // Parse model name for reasoning suffixes or native reasoning_effort
    let (base_clean, reasoning_effort) = parse_model_and_reasoning(requested_model, payload.reasoning_effort.as_deref());
    let target_model = resolve_model(&base_clean, account);

    // If upstream is Antigravity (Google Cloud Code Pa), convert and forward
    if account.provider == "antigravity" {
        let payload_json = crate::proxy::antigravity::build_antigravity_payload(
            payload,
            &target_model,
            None,
            reasoning_effort,
        );

        let endpoint_url = if is_streaming {
            "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
        } else {
            "https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent"
        };

        let mut req_headers = reqwest::header::HeaderMap::new();
        req_headers.insert("authorization", format!("Bearer {}", account.api_key).parse().unwrap());
        req_headers.insert("User-Agent", crate::proxy::antigravity::OFFICIAL_USER_AGENT.parse().unwrap());
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let res = match state.http_client.post(endpoint_url).headers(req_headers).json(&payload_json).send().await {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request("POST", "/v1/chat/completions", requested_model, 502, 0, lat);
                return ((StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": format!("Antigravity connection error: {}", e)}}))).into_response(), false);
            }
        };

        let status = res.status();
        let is_429 = status == StatusCode::TOO_MANY_REQUESTS;

        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", requested_model, status.as_u16(), 0, lat);
            return ((status, err_text).into_response(), is_429);
        }

        if is_streaming {
            let stream_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
            let mapped_stream = crate::proxy::stream::process_antigravity_to_openai_stream(Box::pin(res.bytes_stream()), stream_id, target_model);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions (stream)", requested_model, 200, 150, lat);

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .header("x-request-id", req_id)
                .header("openai-organization", "user-personal")
                .header("openai-processing-ms", lat.to_string())
                .header("openai-version", "2020-10-01")
                .body(Body::from_stream(mapped_stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            (r, false)
        } else {
            let val = match res.json::<serde_json::Value>().await {
                Ok(v) => v,
                Err(e) => {
                    return ((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"message": e.to_string()}}))).into_response(), false);
                }
            };
            let oai_resp = crate::proxy::antigravity::convert_antigravity_response_to_openai(&val, &target_model);
            let tokens = oai_resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", requested_model, 200, tokens, lat);

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let mut response = Json(oai_resp).into_response();
            response.headers_mut().insert("x-request-id", req_id.parse().unwrap());
            response.headers_mut().insert("openai-organization", "user-personal".parse().unwrap());
            response.headers_mut().insert("openai-processing-ms", lat.to_string().parse().unwrap());
            response.headers_mut().insert("openai-version", "2020-10-01".parse().unwrap());
            (response, false)
        }
    } else if account.provider == "anthropic" {
        let claude_req = convert_openai_to_claude(payload, &target_model, reasoning_effort);
        let mut req_headers = reqwest::header::HeaderMap::new();
        req_headers.insert("x-api-key", account.api_key.parse().unwrap());
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

        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", requested_model, status.as_u16(), 0, lat);
            return ((status, err_text).into_response(), is_429);
        }

        if is_streaming {
            let stream_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
            let mapped_stream = process_claude_to_openai_stream(Box::pin(res.bytes_stream()), stream_id, target_model);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions (stream)", requested_model, 200, 150, lat);

            let req_id = format!("req_{}", uuid::Uuid::new_v4().simple());
            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .header("x-request-id", req_id)
                .header("openai-organization", "user-personal")
                .header("openai-processing-ms", lat.to_string())
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
            response.headers_mut().insert("x-request-id", req_id.parse().unwrap());
            response.headers_mut().insert("openai-organization", "user-personal".parse().unwrap());
            response.headers_mut().insert("openai-processing-ms", lat.to_string().parse().unwrap());
            response.headers_mut().insert("openai-version", "2020-10-01".parse().unwrap());
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
        req_headers.insert("authorization", format!("Bearer {}", account.api_key).parse().unwrap());
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
            let stream = pass_through_stream(Box::pin(res.bytes_stream()));
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions (stream)", requested_model, 200, 100, lat);

            let r = Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
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
        state.balancer.select_account(&cfg, Some(&session_id))
    };

    let initial_account = match selected_account {
        Some(acc) if !acc.api_key.is_empty() => acc,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"message": "No active upstream account configured in Aroxy"}})),
            ).into_response();
        }
    };

    let mut req_headers = reqwest::header::HeaderMap::new();
    req_headers.insert("x-api-key", initial_account.api_key.parse().unwrap());
    apply_fingerprint(&mut req_headers, &initial_account.fingerprint);

    let upstream_url = format!("{}/v1/messages", initial_account.base_url.trim_end_matches('/'));
    let res = match state.http_client.post(&upstream_url).headers(req_headers).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": e.to_string()}}))).into_response();
        }
    };

    let status = res.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        state.balancer.mark_cooldown(&initial_account.id, 60);
        let next_acc_opt = {
            let cfg = state.config.read();
            if cfg.failover_on_429 {
                state.balancer.select_account(&cfg, Some(&session_id))
            } else {
                None
            }
        };

        if let Some(next_acc) = next_acc_opt {
            if next_acc.id != initial_account.id {
                log::info!("Claude Messages 429 failover from {} to {}", initial_account.name, next_acc.name);
                let mut retry_headers = reqwest::header::HeaderMap::new();
                retry_headers.insert("x-api-key", next_acc.api_key.parse().unwrap());
                apply_fingerprint(&mut retry_headers, &next_acc.fingerprint);
                let retry_url = format!("{}/v1/messages", next_acc.base_url.trim_end_matches('/'));
                if let Ok(retry_res) = state.http_client.post(&retry_url).headers(retry_headers).json(&payload).send().await {
                        let is_streaming = payload.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
                        let ret_status = retry_res.status();
                        if is_streaming && ret_status.is_success() {
                            let stream = pass_through_stream(Box::pin(retry_res.bytes_stream()));
                            let lat = start_time.elapsed().as_millis() as u64;
                            state.stats.record_request("POST", "/v1/messages (stream)", &model, 200, 150, lat);
                            return Response::builder()
                                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                                .header(header::CACHE_CONTROL, "no-cache")
                                .header(header::CONNECTION, "keep-alive")
                                .body(Body::from_stream(stream))
                                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
                        } else {
                            let text = retry_res.text().await.unwrap_or_default();
                            let lat = start_time.elapsed().as_millis() as u64;
                            state.stats.record_request("POST", "/v1/messages", &model, ret_status.as_u16(), 50, lat);
                            return (ret_status, text).into_response();
                        }
                    }
                }
            }
        }

    let is_streaming = payload.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);

    if is_streaming && status.is_success() {
        let stream = pass_through_stream(Box::pin(res.bytes_stream()));
        let lat = start_time.elapsed().as_millis() as u64;
        state.stats.record_request("POST", "/v1/messages (stream)", &model, 200, 150, lat);

        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    } else {
        let text = res.text().await.unwrap_or_default();
        let lat = start_time.elapsed().as_millis() as u64;
        state.stats.record_request("POST", "/v1/messages", &model, status.as_u16(), 50, lat);
        (status, text).into_response()
    }
}
