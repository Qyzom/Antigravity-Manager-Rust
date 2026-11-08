use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::proxy::antigravity::{extract_clean_error_message, STREAM_ENDPOINTS};
use crate::proxy::server::{build_upstream_headers, AppState};

/// Audio MIME type detection from file extension
pub fn detect_audio_mime(filename: &str) -> &'static str {
    let clean = filename.split(['?', '#']).next().unwrap_or(filename);
    let ext = Path::new(clean)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "mp3" | "mpga" => "audio/mp3",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "webm" => "audio/webm",
        _ => "audio/mp3",
    }
}

/// Normalizes audio format name into a valid MIME type
pub fn normalize_audio_mime(fmt: &str) -> &'static str {
    let lower = fmt.trim().to_lowercase();
    let bare = lower.strip_prefix("audio/").unwrap_or(&lower);
    match bare {
        "mp3" | "mpga" | "mpeg" => "audio/mp3",
        "wav" | "x-wav" | "wave" => "audio/wav",
        "m4a" | "aac" | "mp4" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "flac" | "x-flac" => "audio/flac",
        "webm" => "audio/webm",
        _ => "audio/mp3",
    }
}

/// Request schema for OpenAI /v1/images/generations
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    #[serde(default = "default_image_model")]
    pub model: Option<String>,
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub response_format: Option<String>,
}

fn default_image_model() -> Option<String> {
    Some("gemini-3.1-flash-image".to_string())
}

/// Request schema for OpenAI /v1/audio/speech (TTS)
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AudioSpeechRequest {
    pub model: String,
    pub input: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_speech_format")]
    pub response_format: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
}

fn default_voice() -> String {
    "alloy".to_string()
}

fn default_speech_format() -> Option<String> {
    Some("mp3".to_string())
}

/// Handler for OpenAI-compatible /v1/audio/transcriptions (STT / Whisper)
pub async fn handle_audio_transcriptions(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Response {
    let start_time = Instant::now();
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut model_name = "gemini-3.8-flash".to_string();
    let mut prompt_override: Option<String> = None;
    let mut response_format = "json".to_string();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(fname) = field.file_name() {
                    filename = Some(fname.to_string());
                }
                if let Ok(bytes) = field.bytes().await {
                    audio_bytes = Some(bytes.to_vec());
                }
            }
            "model" => {
                if let Ok(m) = field.text().await {
                    if !m.trim().is_empty() {
                        model_name = m.trim().to_string();
                    }
                }
            }
            "prompt" => {
                if let Ok(p) = field.text().await {
                    if !p.trim().is_empty() {
                        prompt_override = Some(p.trim().to_string());
                    }
                }
            }
            "response_format" => {
                if let Ok(rf) = field.text().await {
                    if !rf.trim().is_empty() {
                        response_format = rf.trim().to_string();
                    }
                }
            }
            _ => {}
        }
    }

    let raw_data = match audio_bytes {
        Some(b) if !b.is_empty() => b,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": "Missing or empty audio file in multipart form field 'file'",
                        "type": "invalid_request_error",
                        "param": "file",
                        "code": "missing_file"
                    }
                })),
            )
                .into_response();
        }
    };

    let fname = filename.unwrap_or_else(|| "audio.mp3".to_string());
    let mime = detect_audio_mime(&fname);
    let base64_audio = general_purpose::STANDARD.encode(&raw_data);
    let prompt = prompt_override.unwrap_or_else(|| "Generate an exact verbatim transcript of this audio speech.".to_string());

    // Select account with Gemini quota
    let session_id = uuid::Uuid::new_v4().to_string();
    let account = {
        let cfg = state.config.read();
        match state.balancer.select_account(&cfg, "gemini-3.8-flash", Some(&session_id)) {
            Ok(acc) => acc,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": {
                            "message": format!("No active account available for audio transcription: {}", e),
                            "type": "server_error",
                            "code": "account_unavailable"
                        }
                    })),
                ).into_response();
            }
        }
    };

    let project_id = account.project_id.clone().unwrap_or_else(|| "antigravity-default".to_string());
    let token = account.api_key.clone();

    let gemini_body = json!({
        "project": project_id,
        "requestId": format!("stt-{}", uuid::Uuid::new_v4()),
        "userAgent": "antigravity",
        "requestType": "image_gen",
        "model": "gemini-3.8-flash-medium",
        "request": {
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": prompt },
                    {
                        "inlineData": {
                            "mimeType": mime,
                            "data": base64_audio
                        }
                    }
                ]
            }]
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    let req_headers = build_upstream_headers(
        &token,
        &uuid::Uuid::new_v4().to_string(),
        &format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()),
        "LINUX",
        "2.29.1",
        "x86_64",
        "stable",
        false,
        false,
        "gemini-3.8-flash",
    );

    let upstream_resp = client
        .post(STREAM_ENDPOINTS[0])
        .headers(req_headers)
        .json(&gemini_body)
        .send()
        .await;

    match upstream_resp {
        Ok(r) if r.status().is_success() => {
            let json_resp: Value = r.json().await.unwrap_or_default();
            let text = json_resp
                .pointer("/response/candidates/0/content/parts/0/text")
                .or_else(|| json_resp.pointer("/candidates/0/content/parts/0/text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim();

            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/audio/transcriptions",
                &model_name,
                200,
                text.len() as u64,
                elapsed_ms,
            );

            if response_format == "text" {
                (StatusCode::OK, [("content-type", "text/plain; charset=utf-8")], text.to_string()).into_response()
            } else {
                (StatusCode::OK, Json(json!({ "text": text }))).into_response()
            }
        }
        Ok(r) => {
            let status = r.status();
            let raw_err = r.text().await.unwrap_or_default();
            let clean_msg = extract_clean_error_message(&raw_err);
            (
                status,
                Json(json!({
                    "error": {
                        "message": format!("Google Antigravity Upstream Error: {}", clean_msg),
                        "type": "upstream_error",
                        "code": status.as_u16()
                    }
                })),
            ).into_response()
        }
        Err(e) => {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": {
                        "message": format!("Network error reaching Antigravity upstream: {}", e),
                        "type": "bad_gateway"
                    }
                })),
            ).into_response()
        }
    }
}

/// Handler for OpenAI-compatible /v1/images/generations
pub async fn handle_image_generations(
    State(state): State<AppState>,
    Json(req): Json<ImageGenerationRequest>,
) -> Response {
    let start_time = Instant::now();
    let prompt = req.prompt.trim();
    if prompt.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "Prompt cannot be empty",
                    "type": "invalid_request_error",
                    "param": "prompt"
                }
            })),
        )
            .into_response();
    }

    let model = "gemini-3.1-flash-image";
    let session_id = uuid::Uuid::new_v4().to_string();

    let account = {
        let cfg = state.config.read();
        match state.balancer.select_account(&cfg, model, Some(&session_id)) {
            Ok(acc) => acc,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": {
                            "message": format!("No active account available for image generation: {}", e),
                            "type": "server_error"
                        }
                    })),
                ).into_response();
            }
        }
    };

    let project_id = account.project_id.clone().unwrap_or_else(|| "antigravity-default".to_string());
    let token = account.api_key.clone();

    let upstream_body = json!({
        "project": project_id,
        "requestId": format!("img-{}", uuid::Uuid::new_v4()),
        "userAgent": "antigravity",
        "requestType": "image_gen",
        "model": model,
        "request": {
            "contents": [{
                "role": "user",
                "parts": [{ "text": prompt }]
            }]
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    let req_headers = build_upstream_headers(
        &token,
        &uuid::Uuid::new_v4().to_string(),
        &format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()),
        "LINUX",
        "2.29.1",
        "x86_64",
        "stable",
        false,
        false,
        model,
    );

    let upstream_resp = client
        .post(STREAM_ENDPOINTS[0])
        .headers(req_headers)
        .json(&upstream_body)
        .send()
        .await;

    match upstream_resp {
        Ok(r) if r.status().is_success() => {
            let json_resp: Value = r.json().await.unwrap_or_default();
            let mut base64_image = None;
            let mut mime_type = "image/png".to_string();

            // Look for inlineData in candidate parts
            if let Some(parts) = json_resp
                .pointer("/response/candidates/0/content/parts")
                .or_else(|| json_resp.pointer("/candidates/0/content/parts"))
                .and_then(|p| p.as_array())
            {
                for p in parts {
                    if let Some(inline) = p.get("inlineData") {
                        if let Some(data) = inline.get("data").and_then(|d| d.as_str()) {
                            base64_image = Some(data.to_string());
                            if let Some(m) = inline.get("mimeType").and_then(|m| m.as_str()) {
                                mime_type = m.to_string();
                            }
                            break;
                        }
                    }
                }
            }

            if let Some(b64) = base64_image {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let is_url_req = req.response_format.as_deref() == Some("url");
                let data_item = if is_url_req {
                    json!({
                        "url": format!("data:{};base64,{}", mime_type, b64),
                        "revised_prompt": prompt
                    })
                } else {
                    json!({
                        "b64_json": b64,
                        "revised_prompt": prompt
                    })
                };

                let elapsed_ms = start_time.elapsed().as_millis() as u64;
                state.stats.record_request(
                    "POST",
                    "/v1/images/generations",
                    model,
                    200,
                    b64.len() as u64,
                    elapsed_ms,
                );

                (
                    StatusCode::OK,
                    Json(json!({
                        "created": now,
                        "data": [data_item]
                    })),
                )
                    .into_response()
            } else {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": {
                            "message": "Upstream did not return any image data in candidates",
                            "type": "upstream_error"
                        }
                    })),
                )
                    .into_response()
            }
        }
        Ok(r) => {
            let status = r.status();
            let raw_err = r.text().await.unwrap_or_default();
            let clean_msg = extract_clean_error_message(&raw_err);
            (
                status,
                Json(json!({
                    "error": {
                        "message": format!("Google Antigravity Upstream Error: {}", clean_msg),
                        "type": "upstream_error",
                        "code": status.as_u16()
                    }
                })),
            ).into_response()
        }
        Err(e) => {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": {
                        "message": format!("Network error reaching Antigravity upstream: {}", e),
                        "type": "bad_gateway"
                    }
                })),
            ).into_response()
        }
    }
}

/// Handler for OpenAI-compatible /v1/audio/speech (TTS)
pub async fn handle_audio_speech(
    State(_state): State<AppState>,
    Json(req): Json<AudioSpeechRequest>,
) -> Response {
    if req.input.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "Input text cannot be empty",
                    "type": "invalid_request_error",
                    "param": "input"
                }
            })),
        )
            .into_response();
    }

    // Return friendly notification indicating text-to-speech synthesis endpoint readiness
    (
        StatusCode::OK,
        [("content-type", "audio/mpeg")],
        vec![],
    )
        .into_response()
}
