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
use std::time::Instant;

use crate::config::UpstreamAccount;
use crate::proxy::antigravity::{
    build_web_search_payload, extract_clean_error_message, extract_grounding_info,
    normalize_antigravity_upstream_model, GENERATE_ENDPOINTS,
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

pub(crate) fn pick_dynamic_gemini_model(state: &AppState, modality: &str, requested: Option<&str>) -> String {
    if let Some(req_m) = requested.filter(|s| !s.trim().is_empty()) {
        let lower = req_m.to_lowercase();
        if lower != "whisper-1"
            && lower != "stt-1"
            && lower != "gemini-3.5-transcribe"
            && lower != "web-search"
            && lower != "google-search"
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
                    "search" => m.modality == "search",
                    _ => m.provider == "gemini",
                }
        }) {
            return found.id.clone();
        }
    }
    match modality {
        "search" => "gemini-3.5-flash-lite".to_string(),
        _ => "gemini-3.8-flash-medium".to_string(),
    }
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

/// Wraps raw linear PCM (little-endian 16-bit mono) in a standard 44-byte RIFF/WAVE header.
pub(crate) fn wrap_pcm_in_wav(pcm_data: &[u8], sample_rate: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
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

/// Shared non-streaming upstream executor for `v1internal:generateContent` with automatic OAuth refresh, 429 failover, and balancer telemetry.
pub(crate) async fn execute_generate_content(
    state: &AppState,
    model_hint: &str,
    target_model_for_headers: &str,
    build_payload: impl Fn(&UpstreamAccount) -> Value,
) -> Result<(Value, String), (StatusCode, String)> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let failover_enabled = state.config.read().failover_on_429;
    let max_attempts = if failover_enabled { 5 } else { 1 };
    let mut excluded_ids: Vec<String> = Vec::new();
    let mut final_err: Option<(StatusCode, String)> = None;

    for _attempt in 0..max_attempts {
        let mut account = {
            let cfg = state.config.read();
            match state.balancer.select_account_excluding(
                &cfg,
                model_hint,
                Some(&session_id),
                &excluded_ids,
            ) {
                Ok(acc) => acc,
                Err(e) => {
                    if let Some(prev) = final_err {
                        return Err(prev);
                    }
                    let status = match e {
                        crate::proxy::balancer::RoutingError::NoAccounts => StatusCode::SERVICE_UNAVAILABLE,
                        _ => StatusCode::TOO_MANY_REQUESTS,
                    };
                    return Err((status, format!("{}", e)));
                }
            }
        };
        excluded_ids.push(account.id.clone());

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
        let mut last_res: Option<wreq::Response> = None;

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
                    last_res = Some(r);
                    if s == StatusCode::TOO_MANY_REQUESTS {
                        break;
                    }
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
                if let Some(fresh_token) = crate::proxy::server::refresh_account_token_and_save(state, &account).await {
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
                                let rs = retry_res.status();
                                last_res = Some(retry_res);
                                if rs == StatusCode::TOO_MANY_REQUESTS {
                                    break;
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
                let is_429 = status == StatusCode::TOO_MANY_REQUESTS;
                let is_auth = status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN;

                if is_429 && cooldown_429 {
                    let retry_hint = crate::proxy::balancer::LoadBalancer::parse_retry_delay_secs(&raw_err);
                    state.balancer.record_rate_limit_hit(
                        &account.id,
                        429,
                        cooldown_secs,
                        cooldown_jitter,
                        Some(&format!("HTTP 429: {}", clean_msg)),
                        retry_hint,
                    );
                    if auto_rotate_fp {
                        let mut cfg = state.config.write();
                        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.id == account.id) {
                            acc.device_profile = Some(crate::config::DeviceProfile::generate());
                            let _ = cfg.save();
                        }
                    }
                }

                final_err = Some((status, format!("Google Antigravity Upstream Error: {}", clean_msg)));
                if (is_429 || is_auth) && failover_enabled {
                    continue;
                }
                return Err(final_err.unwrap());
            }
            None => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    "Network error reaching Antigravity upstream".to_string(),
                ));
            }
        }
    }

    Err(final_err.unwrap_or((
        StatusCode::BAD_GATEWAY,
        "Network error reaching Antigravity upstream".to_string(),
    )))
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
    if !state.config.read().is_model_enabled("gemini-3.5-transcribe")
        && !state.config.read().is_model_enabled("whisper-1")
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "AMR: Model 'gemini-3.5-transcribe' is disabled by administrator in settings.".to_string(),
            "model_disabled",
        );
    }
    let start_time = Instant::now();
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut model_name = "gemini-3.5-transcribe".to_string();
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



/// Handler for /v1/search (Antigravity GetWebSearchResults / Google Grounding)
pub async fn handle_web_search(
    State(state): State<AppState>,
    Json(req): Json<WebSearchRequest>,
) -> Response {
    if !state.config.read().is_model_enabled("web-search") {
        return error_response(
            StatusCode::FORBIDDEN,
            "AMR: Model 'web-search' is disabled by administrator in settings.".to_string(),
            "model_disabled",
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_audio_mime() {
        assert_eq!(detect_audio_mime("file.mp3"), "audio/mp3");
        assert_eq!(detect_audio_mime("recording.wav"), "audio/wav");
        assert_eq!(detect_audio_mime("voice.ogg"), "audio/ogg");
        assert_eq!(detect_audio_mime("track.m4a"), "audio/aac");
        assert_eq!(detect_audio_mime("audio.flac"), "audio/flac");
    }

    #[test]
    fn test_wrap_pcm_in_wav() {
        let fake_pcm = vec![0u8; 100];
        let wav = wrap_pcm_in_wav(&fake_pcm, 24000, 1, 16);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 100);
    }
}
