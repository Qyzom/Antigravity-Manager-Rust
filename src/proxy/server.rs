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
    pub http_client: reqwest::Client,
}

pub fn create_proxy_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/v1/models", get(list_models))
        .route("/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/chat/completions", post(chat_completions))
        .route("/v1/messages", post(claude_messages))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok", "service": "Aroxy"})))
}

async fn list_models() -> impl IntoResponse {
    let models = json!({
        "object": "list",
        "data": [
            {"id": "claude-3-7-sonnet", "object": "model", "owned_by": "anthropic"},
            {"id": "claude-3-5-sonnet", "object": "model", "owned_by": "anthropic"},
            {"id": "claude-3-5-haiku", "object": "model", "owned_by": "anthropic"},
            {"id": "gpt-4o", "object": "model", "owned_by": "openai"},
            {"id": "gpt-4o-mini", "object": "model", "owned_by": "openai"},
            {"id": "o1", "object": "model", "owned_by": "openai"},
            {"id": "o3-mini", "object": "model", "owned_by": "openai"},
            {"id": "gemini-2.0-flash", "object": "model", "owned_by": "google"}
        ]
    });
    Json(models)
}

async fn chat_completions(
    State(state): State<AppState>,
    _headers: HeaderMap,
    Json(payload): Json<OpenAIChatRequest>,
) -> Response {
    let start_time = Instant::now();
    let requested_model = payload.model.clone();

    let (active_account, is_streaming) = {
        let cfg = state.config.read();
        let acc = cfg.get_active_account().cloned();
        (acc, payload.stream)
    };

    let account = match active_account {
        Some(acc) if !acc.api_key.is_empty() => acc,
        _ => {
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, 401, 0, start_time.elapsed().as_millis() as u64);
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"message": "No active upstream account or API key configured in Aroxy"}})),
            ).into_response();
        }
    };

    let target_model = resolve_model(&requested_model, &account);

    // If upstream is Anthropic, map OpenAI -> Claude format
    if account.provider == "anthropic" {
        let claude_req = convert_openai_to_claude(&payload, &target_model);
        let mut req_headers = reqwest::header::HeaderMap::new();
        req_headers.insert("x-api-key", account.api_key.parse().unwrap());
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let upstream_url = format!("{}/v1/messages", account.base_url.trim_end_matches('/'));
        let res = match state.http_client.post(&upstream_url).headers(req_headers).json(&claude_req).send().await {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request("POST", "/v1/chat/completions", &requested_model, 502, 0, lat);
                return (StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": format!("Upstream connection error: {}", e)}}))).into_response();
            }
        };

        let status = res.status();
        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, status.as_u16(), 0, lat);
            return (status, err_text).into_response();
        }

        if is_streaming {
            let stream_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
            let mapped_stream = process_claude_to_openai_stream(Box::pin(res.bytes_stream()), stream_id, target_model);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions (stream)", &requested_model, 200, 150, lat);

            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .body(Body::from_stream(mapped_stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        } else {
            let claude_resp = match res.json::<ClaudeMessageResponse>().await {
                Ok(r) => r,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"message": e.to_string()}}))).into_response();
                }
            };
            let oai_resp = convert_claude_response_to_openai(&claude_resp);
            let tokens = oai_resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, 200, tokens, lat);
            Json(oai_resp).into_response()
        }
    } else {
        // Direct OpenAI or custom endpoint pass-through
        let mut forward_payload = payload;
        forward_payload.model = target_model;

        let mut req_headers = reqwest::header::HeaderMap::new();
        req_headers.insert("authorization", format!("Bearer {}", account.api_key).parse().unwrap());
        apply_fingerprint(&mut req_headers, &account.fingerprint);

        let upstream_url = format!("{}/v1/chat/completions", account.base_url.trim_end_matches('/'));
        let res = match state.http_client.post(&upstream_url).headers(req_headers).json(&forward_payload).send().await {
            Ok(r) => r,
            Err(e) => {
                let lat = start_time.elapsed().as_millis() as u64;
                state.stats.record_request("POST", "/v1/chat/completions", &requested_model, 502, 0, lat);
                return (StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": e.to_string()}}))).into_response();
            }
        };

        let status = res.status();
        if is_streaming && status.is_success() {
            let stream = pass_through_stream(Box::pin(res.bytes_stream()));
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions (stream)", &requested_model, 200, 100, lat);

            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        } else {
            let text = res.text().await.unwrap_or_default();
            let lat = start_time.elapsed().as_millis() as u64;
            state.stats.record_request("POST", "/v1/chat/completions", &requested_model, status.as_u16(), 50, lat);
            (status, text).into_response()
        }
    }
}

async fn claude_messages(
    State(state): State<AppState>,
    _headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let start_time = Instant::now();
    let model = payload.get("model").and_then(|m| m.as_str()).unwrap_or("unknown").to_string();

    let active_account = {
        let cfg = state.config.read();
        cfg.get_active_account().cloned()
    };

    let account = match active_account {
        Some(acc) if !acc.api_key.is_empty() => acc,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"message": "No active upstream account configured in Aroxy"}})),
            ).into_response();
        }
    };

    let mut req_headers = reqwest::header::HeaderMap::new();
    req_headers.insert("x-api-key", account.api_key.parse().unwrap());
    apply_fingerprint(&mut req_headers, &account.fingerprint);

    let upstream_url = format!("{}/v1/messages", account.base_url.trim_end_matches('/'));
    let res = match state.http_client.post(&upstream_url).headers(req_headers).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": {"message": e.to_string()}}))).into_response();
        }
    };

    let is_streaming = payload.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let status = res.status();

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
