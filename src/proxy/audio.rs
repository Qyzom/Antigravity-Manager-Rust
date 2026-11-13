use axum::{
    extract::{Multipart, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::config::UpstreamAccount;
use crate::proxy::antigravity::{
    aspect_ratio_from_size_or_hint, build_web_search_payload, extract_clean_error_message,
    extract_grounding_info, normalize_antigravity_upstream_model, GENERATE_ENDPOINTS,
};
use crate::proxy::server::{build_upstream_headers_json, AppState};

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
        "pcm" | "l16" => "audio/pcm",
        _ => "audio/mp3",
    }
}

/// Normalizes audio format name into a valid MIME type
pub fn normalize_audio_mime(fmt: &str) -> &'static str {
    let lower = fmt.trim().to_lowercase();
    let main = lower.split(';').next().unwrap_or(&lower).trim();
    let bare = main.strip_prefix("audio/").unwrap_or(main);
    match bare {
        "mp3" | "mpga" | "mpeg" => "audio/mp3",
        "wav" | "x-wav" | "wave" => "audio/wav",
        "m4a" | "aac" | "mp4" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "flac" | "x-flac" => "audio/flac",
        "webm" => "audio/webm",
        "l16" | "pcm" => "audio/pcm",
        _ => "audio/mp3",
    }
}

fn pick_dynamic_gemini_model(state: &AppState, modality: &str, requested: Option<&str>) -> String {
    if let Some(req_m) = requested.filter(|s| !s.trim().is_empty()) {
        let lower = req_m.to_lowercase();
        if lower != "whisper-1"
            && lower != "stt-1"
            && lower != "web-search"
            && lower != "google-search"
            && !lower.contains("dall-e")
            && !lower.contains("imagen")
        {
            return normalize_antigravity_upstream_model(req_m).to_string();
        }
    }
    let cfg = state.config.read();
    for acc in cfg.accounts.iter().filter(|a| a.is_active) {
        if let Some(found) = acc.available_models.iter().find(|m| {
            !m.disabled
                && m.remaining_fraction > 0.0
                && match modality {
                    "image" => m.modality == "image" || m.id.to_lowercase().contains("image"),
                    "search" => m.modality == "search",
                    _ => m.provider == "gemini" && m.modality != "image",
                }
        }) {
            return found.id.clone();
        }
    }
    match modality {
        "image" => "gemini-3.1-flash-image".to_string(),
        "search" => "gemini-3.5-flash-lite".to_string(),
        _ => "gemini-3.8-flash-medium".to_string(),
    }
}

/// Request schema for OpenAI /v1/images/generations
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub n: Option<u32>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default, alias = "aspectRatio")]
    pub aspect_ratio: Option<String>,
    #[serde(default, alias = "imageSize")]
    pub image_size: Option<String>,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub style: Option<String>,
}

/// Request schema for OpenAI /v1/audio/speech (TTS)
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AudioSpeechRequest {
    #[serde(default = "default_tts_model")]
    pub model: String,
    pub input: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_speech_format")]
    pub response_format: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,
}

fn default_tts_model() -> String {
    "tts-1".to_string()
}

fn default_voice() -> String {
    "alloy".to_string()
}

fn default_speech_format() -> Option<String> {
    Some("wav".to_string())
}

/// Request schema for /v1/search (Web Search / Google Grounding)
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WebSearchRequest {
    #[serde(default, alias = "q", alias = "prompt")]
    pub query: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

fn map_openai_voice_to_gemini(voice: &str) -> String {
    match voice.trim().to_lowercase().as_str() {
        "alloy" | "zephyr" => "Zephyr".to_string(),
        "echo" | "puck" => "Puck".to_string(),
        "fable" | "charon" => "Charon".to_string(),
        "onyx" | "fenrir" => "Fenrir".to_string(),
        "nova" | "kore" => "Kore".to_string(),
        "shimmer" | "aoede" => "Aoede".to_string(),
        "ash" | "orus" => "Orus".to_string(),
        "coral" | "leda" => "Leda".to_string(),
        "sage" | "sulafat" => "Sulafat".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "Zephyr".to_string(),
            }
        }
    }
}

/// Wraps raw linear PCM (little-endian 16-bit mono) in a standard 44-byte RIFF/WAVE header.
fn wrap_pcm_in_wav(pcm_data: &[u8], sample_rate: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let byte_rate = sample_rate * (channels as u32) * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_size = pcm_data.len() as u32;
    let riff_size = 36u32.saturating_add(data_size);

    let mut wav = Vec::with_capacity(44 + pcm_data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm_data);
    wav
}

fn parse_pcm_rate(mime: &str) -> u32 {
    for part in mime.split(';') {
        let trimmed = part.trim();
        if let Some(val) = trimmed.strip_prefix("rate=") {
            if let Ok(r) = val.trim().parse::<u32>() {
                if (8000..=192000).contains(&r) {
                    return r;
                }
            }
        }
    }
    24000
}

fn detect_container_content_type(bytes: &[u8], upstream_mime: &str) -> &'static str {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return "audio/wav";
    }
    if bytes.len() >= 4 && &bytes[..4] == b"fLaC" {
        return "audio/flac";
    }
    if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
        return "audio/ogg";
    }
    if bytes.len() >= 3 && (&bytes[..3] == b"ID3" || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0))
    {
        return "audio/mpeg";
    }
    match normalize_audio_mime(upstream_mime) {
        "audio/wav" => "audio/wav",
        "audio/mp3" => "audio/mpeg",
        "audio/ogg" => "audio/ogg",
        "audio/flac" => "audio/flac",
        "audio/aac" => "audio/aac",
        "audio/webm" => "audio/webm",
        _ => "audio/wav",
    }
}

/// Shared non-streaming upstream executor for `v1internal:generateContent` with automatic OAuth refresh and balancer telemetry.
async fn execute_generate_content(
    state: &AppState,
    model_hint: &str,
    target_model_for_headers: &str,
    build_payload: impl Fn(&UpstreamAccount) -> Value,
) -> Result<(Value, String), (StatusCode, String)> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut account = {
        let cfg = state.config.read();
        state
            .balancer
            .select_account(&cfg, model_hint, Some(&session_id))
            .map_err(|e| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("No active account available: {}", e),
                )
            })?
    };

    let (
        platform_str,
        ide_ver,
        spoof_arch,
        spoof_channel,
        spoof_go,
        inject_app_meta,
        lock_prod,
        cooldown_429,
        cooldown_secs,
        cooldown_jitter,
        auto_rotate_fp,
    ) = {
        let cfg = state.config.read();
        (
            cfg.resolved_platform(),
            cfg.spoof_ide_version.clone(),
            cfg.spoof_arch.clone(),
            cfg.spoof_channel.clone(),
            cfg.spoof_go_sdk,
            cfg.inject_ide_metadata,
            cfg.lock_production_endpoints,
            cfg.cooldown_on_429,
            cfg.cooldown_duration_secs,
            cfg.cooldown_jitter_percent,
            cfg.auto_rotate_fp_on_limit,
        )
    };

    let client = state.get_client_for_account(&account);
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

    let endpoints: &[&str] = if lock_prod {
        &GENERATE_ENDPOINTS[..1]
    } else {
        &GENERATE_ENDPOINTS[..]
    };

    let payload = build_payload(&account);
    let mut last_res: Option<reqwest::Response> = None;

    for endpoint_url in endpoints {
        let headers = build_upstream_headers_json(
            &account.api_key,
            &client_id,
            &machine_id,
            platform_str,
            &ide_ver,
            &spoof_arch,
            &spoof_channel,
            spoof_go,
            inject_app_meta,
            target_model_for_headers,
        );

        match client
            .post(*endpoint_url)
            .headers(headers)
            .json(&payload)
            .send()
            .await
        {
            Ok(r) => {
                let s = r.status();
                if s.is_success() {
                    state.balancer.record_success(&account.id);
                    let parsed = r.json::<Value>().await.map_err(|e| {
                        (
                            StatusCode::BAD_GATEWAY,
                            format!("Invalid JSON response from Antigravity upstream: {}", e),
                        )
                    })?;
                    return Ok((parsed, account.name));
                }
                if s == StatusCode::TOO_MANY_REQUESTS {
                    if cooldown_429 {
                        state.balancer.record_failure_with_reason(
                            &account.id,
                            429,
                            cooldown_secs,
                            cooldown_jitter,
                            Some("HTTP 429: Too Many Requests"),
                        );
                    }
                    if auto_rotate_fp {
                        let mut cfg = state.config.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                            acc.device_profile = Some(crate::config::DeviceProfile::generate());
                            let _ = cfg.save();
                        }
                    }
                    last_res = Some(r);
                    break;
                }
                last_res = Some(r);
            }
            Err(_) => continue,
        }
    }

    // Automatic OAuth token refresh on 401/403
    if let Some(ref res_ref) = last_res {
        let s = res_ref.status();
        if (s == StatusCode::UNAUTHORIZED || s == StatusCode::FORBIDDEN)
            && account.refresh_token.is_some()
        {
            if let Some(ref rt) = account.refresh_token {
                if let Ok(tok_res) =
                    crate::proxy::antigravity::refresh_token(&state.http_client, rt).await
                {
                    let fresh_token = tok_res.access_token;
                    {
                        let mut cfg = state.config.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                            acc.api_key = fresh_token.clone();
                            if tok_res.refresh_token.is_some() {
                                acc.refresh_token = tok_res.refresh_token.clone();
                            }
                            acc.status = "ACTIVE".to_string();
                            acc.error_message = None;
                        }
                        let _ = cfg.save();
                    }
                    state
                        .cfg_version
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    account.api_key = fresh_token;

                    for endpoint_url in endpoints {
                        let headers = build_upstream_headers_json(
                            &account.api_key,
                            &client_id,
                            &machine_id,
                            platform_str,
                            &ide_ver,
                            &spoof_arch,
                            &spoof_channel,
                            spoof_go,
                            inject_app_meta,
                            target_model_for_headers,
                        );
                        if let Ok(retry_res) = client
                            .post(*endpoint_url)
                            .headers(headers)
                            .json(&payload)
                            .send()
                            .await
                        {
                            if retry_res.status().is_success() {
                                state.balancer.record_success(&account.id);
                                let parsed = retry_res.json::<Value>().await.map_err(|e| {
                                    (
                                        StatusCode::BAD_GATEWAY,
                                        format!(
                                            "Invalid JSON response from Antigravity upstream: {}",
                                            e
                                        ),
                                    )
                                })?;
                                return Ok((parsed, account.name));
                            }
                            last_res = Some(retry_res);
                        }
                    }
                }
            }
        }
    }

    match last_res {
        Some(r) => {
            let status = r.status();
            let raw_err = r.text().await.unwrap_or_default();
            let clean_msg = extract_clean_error_message(&raw_err);
            Err((status, format!("Google Antigravity Upstream Error: {}", clean_msg)))
        }
        None => Err((
            StatusCode::BAD_GATEWAY,
            "Network error reaching Antigravity upstream".to_string(),
        )),
    }
}

fn error_response(status: StatusCode, message: String, code: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": if status.is_client_error() { "invalid_request_error" } else { "upstream_error" },
                "code": code
            }
        })),
    )
        .into_response()
}

fn extract_candidate_parts(raw: &Value) -> &[Value] {
    let actual = raw.get("response").unwrap_or(raw);
    actual
        .pointer("/candidates/0/content/parts")
        .and_then(|p| p.as_array())
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

fn collect_non_thought_text(parts: &[Value]) -> String {
    let mut out = String::new();
    for p in parts {
        let is_thought = p.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
        if !is_thought {
            if let Some(txt) = p.get("text").and_then(|t| t.as_str()) {
                out.push_str(txt);
            }
        }
    }
    out.trim().to_string()
}

/// Handler for OpenAI-compatible /v1/audio/transcriptions (STT / Whisper)
pub async fn handle_audio_transcriptions(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Response {
    let start_time = Instant::now();
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut model_name = "whisper-1".to_string();
    let mut prompt_override: Option<String> = None;
    let mut language: Option<String> = None;
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
            "language" => {
                if let Ok(l) = field.text().await {
                    if !l.trim().is_empty() {
                        language = Some(l.trim().to_string());
                    }
                }
            }
            "response_format" => {
                if let Ok(rf) = field.text().await {
                    if !rf.trim().is_empty() {
                        response_format = rf.trim().to_lowercase();
                    }
                }
            }
            _ => {}
        }
    }

    let raw_data = match audio_bytes {
        Some(b) if !b.is_empty() => b,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Missing or empty audio file in multipart form field 'file'".to_string(),
                "missing_file",
            );
        }
    };

    let fname = filename.unwrap_or_else(|| "audio.mp3".to_string());
    let mime = detect_audio_mime(&fname);
    let base64_audio = general_purpose::STANDARD.encode(&raw_data);

    let mut prompt = prompt_override.unwrap_or_else(|| {
        "Generate an exact verbatim transcript of this audio speech. Return only the spoken transcript without commentary.".to_string()
    });
    if let Some(ref lang) = language {
        prompt = format!("Language: {}. {}", lang, prompt);
    }

    let gemini_model = pick_dynamic_gemini_model(&state, "chat", Some(&model_name));

    let res = execute_generate_content(&state, &gemini_model, &gemini_model, |acc| {
        let project_id = acc
            .project_id
            .clone()
            .unwrap_or_else(|| "aicode-consumers".to_string());
        json!({
            "project": project_id,
            "requestId": format!("stt-{}", uuid::Uuid::new_v4()),
            "userAgent": "antigravity",
            "requestType": "agent",
            "model": gemini_model,
            "request": {
                "contents": [{
                    "role": "user",
                    "parts": [
                        {
                            "inlineData": {
                                "mimeType": mime,
                                "data": base64_audio
                            }
                        },
                        { "text": prompt }
                    ]
                }],
                "generationConfig": {
                    "temperature": 0.0
                }
            }
        })
    })
    .await;

    match res {
        Ok((json_resp, account_name)) => {
            let parts = extract_candidate_parts(&json_resp);
            let text = collect_non_thought_text(parts);
            let elapsed_ms = start_time.elapsed().as_millis() as u64;

            state.stats.record_detailed_request(
                "POST",
                "/v1/audio/transcriptions",
                &model_name,
                200,
                text.len() as u64,
                elapsed_ms,
                &account_name,
                "off",
                &format!("file={} ({}, {} bytes)", fname, mime, raw_data.len()),
                "[audio payload]",
                &text,
                None,
            );

            match response_format.as_str() {
                "text" | "srt" | "vtt" => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    text,
                )
                    .into_response(),
                "verbose_json" => (
                    StatusCode::OK,
                    Json(json!({
                        "task": "transcribe",
                        "language": language.unwrap_or_else(|| "auto".to_string()),
                        "model": model_name,
                        "text": text
                    })),
                )
                    .into_response(),
                _ => (StatusCode::OK, Json(json!({ "text": text }))).into_response(),
            }
        }
        Err((status, msg)) => {
            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/audio/transcriptions",
                &model_name,
                status.as_u16(),
                0,
                elapsed_ms,
            );
            error_response(status, msg, "upstream_error")
        }
    }
}

/// Handler for OpenAI-compatible /v1/images/generations
pub async fn handle_image_generations(
    State(state): State<AppState>,
    Json(req): Json<ImageGenerationRequest>,
) -> Response {
    let start_time = Instant::now();
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Prompt cannot be empty".to_string(),
            "missing_prompt",
        );
    }

    let model = pick_dynamic_gemini_model(&state, "image", req.model.as_deref());
    let aspect_ratio =
        aspect_ratio_from_size_or_hint(req.aspect_ratio.as_deref(), req.size.as_deref())
            .unwrap_or("1:1");
    let requested_n = req.n.unwrap_or(1).clamp(1, 4) as usize;

    let image_size = req.image_size.clone();

    let res = execute_generate_content(&state, &model, &model, |acc| {
        let project_id = acc
            .project_id
            .clone()
            .unwrap_or_else(|| "aicode-consumers".to_string());
        let mut img_cfg = json!({ "aspectRatio": aspect_ratio });
        if let Some(sz) = image_size.as_deref().filter(|s| !s.trim().is_empty()) {
            img_cfg["imageSize"] = json!(sz.trim());
        }
        json!({
            "project": project_id,
            "requestId": format!("img-{}", uuid::Uuid::new_v4()),
            "userAgent": "antigravity",
            "requestType": "image_gen",
            "model": model,
            "request": {
                "contents": [{
                    "role": "user",
                    "parts": [{ "text": prompt }]
                }],
                "generationConfig": {
                    "responseModalities": ["TEXT", "IMAGE"],
                    "imageConfig": img_cfg
                }
            }
        })
    })
    .await;

    match res {
        Ok((json_resp, account_name)) => {
            let parts = extract_candidate_parts(&json_resp);
            let text_caption = collect_non_thought_text(parts);
            let revised_prompt = if text_caption.is_empty() {
                prompt.clone()
            } else {
                text_caption
            };

            let mut images: Vec<(String, String)> = Vec::new();
            for p in parts {
                if let Some(inline) = p.get("inlineData") {
                    if let Some(data) = inline.get("data").and_then(|d| d.as_str()) {
                        let m = inline
                            .get("mimeType")
                            .and_then(|m| m.as_str())
                            .unwrap_or("image/png")
                            .to_string();
                        images.push((data.to_string(), m));
                        if images.len() >= requested_n {
                            break;
                        }
                    }
                }
            }

            if images.is_empty() {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "Upstream did not return any image data in candidates".to_string(),
                    "empty_image_response",
                );
            }

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let is_url_req = req.response_format.as_deref() == Some("url");
            let total_b64_len: usize = images.iter().map(|(b, _)| b.len()).sum();

            let data_items: Vec<Value> = images
                .into_iter()
                .map(|(b64, mime_type)| {
                    if is_url_req {
                        json!({
                            "url": format!("data:{};base64,{}", mime_type, b64),
                            "mime_type": mime_type,
                            "revised_prompt": revised_prompt
                        })
                    } else {
                        json!({
                            "b64_json": b64,
                            "mime_type": mime_type,
                            "revised_prompt": revised_prompt
                        })
                    }
                })
                .collect();

            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            state.stats.record_detailed_request(
                "POST",
                "/v1/images/generations",
                &model,
                200,
                total_b64_len as u64,
                elapsed_ms,
                &account_name,
                "off",
                "Content-Type: application/json",
                &format!("prompt={} aspectRatio={}", prompt, aspect_ratio),
                &format!("[{} generated image(s), {} b64 chars]", data_items.len(), total_b64_len),
                None,
            );

            (
                StatusCode::OK,
                Json(json!({
                    "created": now,
                    "data": data_items
                })),
            )
                .into_response()
        }
        Err((status, msg)) => {
            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/images/generations",
                &model,
                status.as_u16(),
                0,
                elapsed_ms,
            );
            error_response(status, msg, "upstream_error")
        }
    }
}

/// Handler for OpenAI-compatible /v1/audio/speech (TTS)
pub async fn handle_audio_speech(
    State(state): State<AppState>,
    Json(req): Json<AudioSpeechRequest>,
) -> Response {
    let start_time = Instant::now();
    let input_text = req.input.trim().to_string();
    if input_text.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Input text cannot be empty".to_string(),
            "missing_input",
        );
    }

    let voice_name = map_openai_voice_to_gemini(&req.voice);
    let speed = req.speed.unwrap_or(1.0).clamp(0.25, 4.0);
    let speech_prompt = if (speed - 1.0).abs() > 0.05 {
        format!(
            "Read the following text aloud naturally at approximately {:.2}x speaking pace, without extra commentary:\n\n{}",
            speed, input_text
        )
    } else {
        format!(
            "Read the following text aloud verbatim without extra commentary:\n\n{}",
            input_text
        )
    };

    let candidate_models: Vec<&'static str> = match req.model.to_lowercase().as_str() {
        "gemini-3.1-flash-tts-preview" => vec![
            "gemini-3.1-flash-tts-preview",
            "gemini-3.8-flash-tts",
            "gemini-2.5-flash",
        ],
        "gemini-3.8-flash-tts" => vec!["gemini-3.8-flash-tts", "gemini-2.5-flash"],
        _ => vec![
            "gemini-3.1-flash-tts-preview",
            "gemini-3.8-flash-tts",
            "gemini-2.5-flash",
            "gemini-3.8-flash-medium",
        ],
    };

    let mut last_err: (StatusCode, String) = (
        StatusCode::BAD_GATEWAY,
        "No TTS upstream model succeeded".to_string(),
    );

    for upstream_model in candidate_models {
        let res = execute_generate_content(&state, "gemini-3.8-flash", upstream_model, |acc| {
            let project_id = acc
                .project_id
                .clone()
                .unwrap_or_else(|| "aicode-consumers".to_string());
            json!({
                "project": project_id,
                "requestId": format!("tts-{}", uuid::Uuid::new_v4()),
                "userAgent": "antigravity",
                "requestType": "agent",
                "model": upstream_model,
                "request": {
                    "contents": [{
                        "role": "user",
                        "parts": [{ "text": speech_prompt }]
                    }],
                    "generationConfig": {
                        "responseModalities": ["AUDIO"],
                        "speechConfig": {
                            "voiceConfig": {
                                "prebuiltVoiceConfig": {
                                    "voiceName": voice_name
                                }
                            }
                        }
                    }
                }
            })
        })
        .await;

        match res {
            Ok((json_resp, account_name)) => {
                let parts = extract_candidate_parts(&json_resp);
                for p in parts {
                    if let Some(inline) = p.get("inlineData") {
                        if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                            let upstream_mime = inline
                                .get("mimeType")
                                .and_then(|m| m.as_str())
                                .unwrap_or("audio/L16;codec=pcm;rate=24000");
                            if let Ok(raw_audio) = general_purpose::STANDARD.decode(b64.trim()) {
                                let final_bytes = if upstream_mime.to_lowercase().contains("l16")
                                    || upstream_mime.to_lowercase().contains("pcm")
                                {
                                    let rate = parse_pcm_rate(upstream_mime);
                                    wrap_pcm_in_wav(&raw_audio, rate, 1, 16)
                                } else {
                                    raw_audio
                                };
                                let content_type =
                                    detect_container_content_type(&final_bytes, upstream_mime);
                                let elapsed_ms = start_time.elapsed().as_millis() as u64;
                                state.stats.record_detailed_request(
                                    "POST",
                                    "/v1/audio/speech",
                                    &req.model,
                                    200,
                                    final_bytes.len() as u64,
                                    elapsed_ms,
                                    &account_name,
                                    "off",
                                    "Content-Type: application/json",
                                    &format!("voice={} model={}", voice_name, upstream_model),
                                    &format!("[audio: {} bytes, {}]", final_bytes.len(), content_type),
                                    None,
                                );

                                return (
                                    StatusCode::OK,
                                    [
                                        ("content-type", content_type),
                                        ("x-amr-audio-voice", voice_name.as_str()),
                                        ("x-amr-upstream-model", upstream_model),
                                    ],
                                    final_bytes,
                                )
                                    .into_response();
                            }
                        }
                    }
                }
                last_err = (
                    StatusCode::NOT_IMPLEMENTED,
                    format!(
                        "Model '{}' responded without audio inlineData (AUDIO TTS output is not enabled on this Cloud Code account/endpoint)",
                        upstream_model
                    ),
                );
            }
            Err((status, msg)) => {
                last_err = (status, msg);
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::TOO_MANY_REQUESTS {
                    break;
                }
            }
        }
    }

    let elapsed_ms = start_time.elapsed().as_millis() as u64;
    state.stats.record_request(
        "POST",
        "/v1/audio/speech",
        &req.model,
        last_err.0.as_u16(),
        0,
        elapsed_ms,
    );
    error_response(last_err.0, last_err.1, "tts_upstream_error")
}

/// Handler for /v1/search (Antigravity GetWebSearchResults / Google Grounding)
pub async fn handle_web_search(
    State(state): State<AppState>,
    Json(req): Json<WebSearchRequest>,
) -> Response {
    let start_time = Instant::now();
    let query = req.query.trim().to_string();
    if query.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Search query cannot be empty".to_string(),
            "missing_query",
        );
    }

    let target_model = pick_dynamic_gemini_model(&state, "search", req.model.as_deref());
    let domain = req.domain.clone().filter(|d| !d.trim().is_empty());

    let res = execute_generate_content(&state, &target_model, &target_model, |acc| {
        build_web_search_payload(&query, domain.as_deref(), &target_model, acc.project_id.as_deref())
    })
    .await;

    match res {
        Ok((json_resp, account_name)) => {
            let actual = json_resp.get("response").unwrap_or(&json_resp);
            let parts = extract_candidate_parts(&json_resp);
            let answer = collect_non_thought_text(parts);
            let (search_queries, results, _sources_md) = extract_grounding_info(actual);
            let grounding_metadata = actual
                .pointer("/candidates/0/groundingMetadata")
                .cloned()
                .unwrap_or(Value::Null);

            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            state.stats.record_detailed_request(
                "POST",
                "/v1/search",
                "web-search",
                200,
                answer.len() as u64,
                elapsed_ms,
                &account_name,
                "off",
                "Content-Type: application/json",
                &query,
                &answer,
                None,
            );

            (
                StatusCode::OK,
                Json(json!({
                    "object": "web_search.result",
                    "query": query,
                    "domain": domain,
                    "model": target_model,
                    "answer": answer,
                    "search_queries": search_queries,
                    "results": results,
                    "grounding_metadata": grounding_metadata
                })),
            )
                .into_response()
        }
        Err((status, msg)) => {
            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            state.stats.record_request(
                "POST",
                "/v1/search",
                "web-search",
                status.as_u16(),
                0,
                elapsed_ms,
            );
            error_response(status, msg, "web_search_error")
        }
    }
}
