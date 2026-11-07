use crate::proxy::mappers::openai::{create_stream_chunk, create_stream_chunk_with_usage, create_stream_usage_chunk, OpenAIUsage};
use bytes::Bytes;
use futures_util::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::interval;

#[derive(Clone)]
pub struct StreamLoggingContext {
    pub stats: Arc<crate::proxy::stats::ProxyStats>,
    pub method: &'static str,
    pub path: &'static str,
    pub model: String,
    pub account_name: String,
    pub reasoning_effort: String,
    pub request_headers: String,
    pub request_body: String,
    pub start_time: Instant,
}

/// Helper to divide large text blocks into smooth word/punctuation chunks for natural streaming UI rendering.
pub fn emit_smooth_chunks(text: &str) -> Vec<&str> {
    if text.len() <= 20 {
        return vec![text];
    }
    let mut parts = Vec::new();
    let mut last = 0;
    for (i, c) in text.char_indices() {
        let chunk_len = i - last;
        let is_delimiter = matches!(c, ' ' | '\n' | '\t' | '.' | ',' | ';' | '}' | ')' | '{' | '(');
        if (is_delimiter && chunk_len >= 8) || chunk_len >= 28 {
            let next_idx = i + c.len_utf8();
            parts.push(&text[last..next_idx]);
            last = next_idx;
        }
    }
    if last < text.len() {
        parts.push(&text[last..]);
    }
    parts
}

pub fn process_claude_to_openai_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
    log_ctx: Option<StreamLoggingContext>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut current_event_type = String::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut accumulated_response = String::new();

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            if let Ok(text) = std::str::from_utf8(&bytes) {
                                buffer.push_str(text);
                                if buffer.contains("\r\n") {
                                    buffer = buffer.replace("\r\n", "\n");
                                }

                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_chunk = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    for line in event_chunk.lines() {
                                        if let Some(event) = line.strip_prefix("event: ") {
                                            current_event_type = event.trim().to_string();
                                        } else if let Some(data) = line.strip_prefix("data: ") {
                                            if let Ok(val) = serde_json::from_str::<Value>(data.trim()) {
                                                if current_event_type == "message_start" {
                                                    if let Some(msg) = val.get("message") {
                                                        if let Some(usage) = msg.get("usage") {
                                                            if let Some(it) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                                                total_input_tokens = it;
                                                            }
                                                        }
                                                    }
                                                } else if current_event_type == "content_block_delta" {
                                                    if let Some(delta) = val.get("delta") {
                                                        let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                                        if delta_type == "text_delta" {
                                                            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                                                if accumulated_response.len() < 65536 {
                                                                    accumulated_response.push_str(text);
                                                                }
                                                                total_output_tokens += (text.len() / 4).max(1) as u64;

                                                                let sub_chunks = emit_smooth_chunks(text);
                                                                let count = sub_chunks.len();
                                                                for (i, sub) in sub_chunks.into_iter().enumerate() {
                                                                    let chunk = create_stream_chunk(&stream_id, &model_name, Some(sub), None, None);
                                                                    yield Ok(Bytes::from(chunk));
                                                                    if count > 1 && i + 1 < count {
                                                                        tokio::task::yield_now().await;
                                                                    }
                                                                }
                                                            }
                                                        } else if delta_type == "thinking_delta" {
                                                            if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                                                                let chunk = create_stream_chunk(&stream_id, &model_name, None, Some(thinking), None);
                                                                yield Ok(Bytes::from(chunk));
                                                            }
                                                        }
                                                    }
                                                } else if current_event_type == "message_delta" {
                                                    if let Some(usage) = val.get("usage") {
                                                        if let Some(ot) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                                                            total_output_tokens = ot;
                                                        }
                                                    }
                                                    if let Some(stop_reason) = val.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str()) {
                                                        let in_tok = if total_input_tokens > 0 { total_input_tokens } else { log_ctx.as_ref().map(|c| (c.request_body.len() / 4).max(15) as u64).unwrap_or(25) };
                                                        let out_tok = total_output_tokens.max(1);
                                                        let usage_val = OpenAIUsage {
                                                            prompt_tokens: in_tok,
                                                            completion_tokens: out_tok,
                                                            total_tokens: in_tok + out_tok,
                                                            prompt_tokens_details: None,
                                                            completion_tokens_details: None,
                                                        };
                                                        let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some(stop_reason), Some(usage_val.clone()));
                                                        yield Ok(Bytes::from(chunk));

                                                        let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
                                                        yield Ok(Bytes::from(u_chunk));
                                                    }
                                                } else if current_event_type == "message_stop" {
                                                    yield Ok(Bytes::from("data: [DONE]\n\n"));

                                                    if let Some(ref ctx) = log_ctx {
                                                        let in_tok = if total_input_tokens > 0 { total_input_tokens } else { (ctx.request_body.len() / 4).max(15) as u64 };
                                                        let out_tok = total_output_tokens.max(1);
                                                        let elapsed = ctx.start_time.elapsed().as_millis() as u64;
                                                        let resp_display = if accumulated_response.is_empty() {
                                                            "[Claude Stream Complete]".to_string()
                                                        } else {
                                                            accumulated_response.clone()
                                                        };
                                                        ctx.stats.record_detailed_request_exact(
                                                            ctx.method,
                                                            ctx.path,
                                                            &ctx.model,
                                                            200,
                                                            in_tok + out_tok,
                                                            in_tok,
                                                            out_tok,
                                                            elapsed,
                                                            &ctx.account_name,
                                                            &ctx.reasoning_effort,
                                                            &ctx.request_headers,
                                                            &ctx.request_body,
                                                            &resp_display,
                                                            None,
                                                        );
                                                    }
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => {
                            break;
                        }
                    }
                }
            }
        }

        let in_tok = if total_input_tokens > 0 { total_input_tokens } else { log_ctx.as_ref().map(|c| (c.request_body.len() / 4).max(15) as u64).unwrap_or(25) };
        let out_tok = total_output_tokens.max(1);
        let usage_val = OpenAIUsage {
            prompt_tokens: in_tok,
            completion_tokens: out_tok,
            total_tokens: in_tok + out_tok,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };
        let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some("stop"), Some(usage_val.clone()));
        yield Ok(Bytes::from(chunk));
        let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
        yield Ok(Bytes::from(u_chunk));
        yield Ok(Bytes::from("data: [DONE]\n\n"));

        if let Some(ref ctx) = log_ctx {
            let elapsed = ctx.start_time.elapsed().as_millis() as u64;
            let resp_display = if accumulated_response.is_empty() {
                "[Claude Stream Finished]".to_string()
            } else {
                accumulated_response
            };
            ctx.stats.record_detailed_request_exact(
                ctx.method,
                ctx.path,
                &ctx.model,
                200,
                in_tok + out_tok,
                in_tok,
                out_tok,
                elapsed,
                &ctx.account_name,
                &ctx.reasoning_effort,
                &ctx.request_headers,
                &ctx.request_body,
                &resp_display,
                None,
            );
        }
    }
}

pub fn process_antigravity_to_openai_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
    log_ctx: Option<StreamLoggingContext>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut estimated_output_tokens: u64 = 0;
        let mut accumulated_response = String::new();

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            if let Ok(text) = std::str::from_utf8(&bytes) {
                                buffer.push_str(text);
                                if buffer.contains("\r\n") {
                                    buffer = buffer.replace("\r\n", "\n");
                                }

                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_chunk = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    for line in event_chunk.lines() {
                                        if let Some(data) = line.strip_prefix("data: ") {
                                            let trimmed = data.trim();
                                            if trimmed == "[DONE]" {
                                                let in_tok = if total_input_tokens > 0 {
                                                    total_input_tokens
                                                } else {
                                                    log_ctx.as_ref().map(|c| (c.request_body.len() / 4).max(15) as u64).unwrap_or(25)
                                                };
                                                let out_tok = if total_output_tokens > 0 { total_output_tokens } else { estimated_output_tokens.max(1) };
                                                let usage_val = OpenAIUsage {
                                                    prompt_tokens: in_tok,
                                                    completion_tokens: out_tok,
                                                    total_tokens: in_tok + out_tok,
                                                    prompt_tokens_details: None,
                                                    completion_tokens_details: None,
                                                };
                                                let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
                                                yield Ok(Bytes::from(u_chunk));
                                                yield Ok(Bytes::from("data: [DONE]\n\n"));

                                                if let Some(ref ctx) = log_ctx {
                                                    let elapsed = ctx.start_time.elapsed().as_millis() as u64;
                                                    let resp_display = if accumulated_response.is_empty() {
                                                        "[Stream Complete]".to_string()
                                                    } else {
                                                        accumulated_response.clone()
                                                    };
                                                    ctx.stats.record_detailed_request_exact(
                                                        ctx.method,
                                                        ctx.path,
                                                        &ctx.model,
                                                        200,
                                                        in_tok + out_tok,
                                                        in_tok,
                                                        out_tok,
                                                        elapsed,
                                                        &ctx.account_name,
                                                        &ctx.reasoning_effort,
                                                        &ctx.request_headers,
                                                        &ctx.request_body,
                                                        &resp_display,
                                                        None,
                                                    );
                                                }
                                                return;
                                            }

                                            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                                                let actual = val.get("response").unwrap_or(&val);

                                                // Parse upstream token usage metadata
                                                if let Some(usage) = actual.get("usageMetadata") {
                                                    if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                                                        total_input_tokens = pt;
                                                    }
                                                    if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                                                        total_output_tokens = ct;
                                                    }
                                                }

                                                if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                                                    for candidate in candidates {
                                                        if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                                            for part in parts {
                                                                let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                                                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                                                    if accumulated_response.len() < 65536 {
                                                                        accumulated_response.push_str(txt);
                                                                    }
                                                                    estimated_output_tokens += (txt.len() / 4).max(1) as u64;

                                                                    if is_thought {
                                                                        let chunk = create_stream_chunk(&stream_id, &model_name, None, Some(txt), None);
                                                                        yield Ok(Bytes::from(chunk));
                                                                    } else {
                                                                        // Emit smooth micro-chunks with zero artificial latency
                                                                        let sub_chunks = emit_smooth_chunks(txt);
                                                                        let count = sub_chunks.len();
                                                                        for (i, sub) in sub_chunks.into_iter().enumerate() {
                                                                            let chunk = create_stream_chunk(&stream_id, &model_name, Some(sub), None, None);
                                                                            yield Ok(Bytes::from(chunk));
                                                                            if count > 1 && i + 1 < count {
                                                                                tokio::task::yield_now().await;
                                                                            }
                                                                        }
                                                                    }
                                                                } else if let Some(inline) = part.get("inlineData") {
                                                                    let mime = inline.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image/png");
                                                                    if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                                                        let img_md = format!("[Generated Image: {} bytes, {}]\n![Generated Image](data:{};base64,{})", b64.len(), mime, mime, b64);
                                                                        if accumulated_response.len() < 65536 {
                                                                            accumulated_response.push_str(&img_md);
                                                                        }
                                                                        let chunk = create_stream_chunk(&stream_id, &model_name, Some(&img_md), None, None);
                                                                        yield Ok(Bytes::from(chunk));
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        if let Some(finish) = candidate.get("finishReason").and_then(|f| f.as_str()) {
                                                            let stop_reason = match finish {
                                                                "STOP" => "stop",
                                                                "MAX_TOKENS" => "length",
                                                                "SAFETY" => "content_filter",
                                                                _ => "stop",
                                                            };

                                                            let in_tok = if total_input_tokens > 0 {
                                                                total_input_tokens
                                                            } else {
                                                                log_ctx.as_ref().map(|c| (c.request_body.len() / 4).max(15) as u64).unwrap_or(25)
                                                            };
                                                            let out_tok = if total_output_tokens > 0 { total_output_tokens } else { estimated_output_tokens.max(1) };
                                                            let usage_val = OpenAIUsage {
                                                                prompt_tokens: in_tok,
                                                                completion_tokens: out_tok,
                                                                total_tokens: in_tok + out_tok,
                                                                prompt_tokens_details: None,
                                                                completion_tokens_details: None,
                                                            };

                                                            // 1. Emit chunk with stop reason (OpenAI standard: delta is empty {})
                                                            let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some(stop_reason), None);
                                                            yield Ok(Bytes::from(chunk));

                                                            // 2. Emit standard OpenAI usage chunk for strict tools (DeepSeek Harness / Cursor / continue.dev)
                                                            let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
                                                            yield Ok(Bytes::from(u_chunk));

                                                            // 3. Emit final [DONE]
                                                            yield Ok(Bytes::from("data: [DONE]\n\n"));

                                                            if let Some(ref ctx) = log_ctx {
                                                                let elapsed = ctx.start_time.elapsed().as_millis() as u64;
                                                                let resp_display = if accumulated_response.is_empty() {
                                                                    "[Stream Complete]".to_string()
                                                                } else {
                                                                    accumulated_response.clone()
                                                                };
                                                                ctx.stats.record_detailed_request_exact(
                                                                    ctx.method,
                                                                    ctx.path,
                                                                    &ctx.model,
                                                                    200,
                                                                    in_tok + out_tok,
                                                                    in_tok,
                                                                    out_tok,
                                                                    elapsed,
                                                                    &ctx.account_name,
                                                                    &ctx.reasoning_effort,
                                                                    &ctx.request_headers,
                                                                    &ctx.request_body,
                                                                    &resp_display,
                                                                    None,
                                                                );
                                                            }
                                                            return;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => {
                            break;
                        }
                    }
                }
            }
        }

        let in_tok = if total_input_tokens > 0 {
            total_input_tokens
        } else {
            log_ctx.as_ref().map(|c| (c.request_body.len() / 4).max(15) as u64).unwrap_or(25)
        };
        let out_tok = if total_output_tokens > 0 { total_output_tokens } else { estimated_output_tokens.max(1) };
        let usage_val = OpenAIUsage {
            prompt_tokens: in_tok,
            completion_tokens: out_tok,
            total_tokens: in_tok + out_tok,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some("stop"), Some(usage_val.clone()));
        yield Ok(Bytes::from(chunk));
        let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
        yield Ok(Bytes::from(u_chunk));
        yield Ok(Bytes::from("data: [DONE]\n\n"));

        if let Some(ref ctx) = log_ctx {
            let elapsed = ctx.start_time.elapsed().as_millis() as u64;
            let resp_display = if accumulated_response.is_empty() {
                "[Stream Finished]".to_string()
            } else {
                accumulated_response
            };
            ctx.stats.record_detailed_request_exact(
                ctx.method,
                ctx.path,
                &ctx.model,
                200,
                in_tok + out_tok,
                in_tok,
                out_tok,
                elapsed,
                &ctx.account_name,
                &ctx.reasoning_effort,
                &ctx.request_headers,
                &ctx.request_body,
                &resp_display,
                None,
            );
        }
    }
}

pub fn process_antigravity_to_claude_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
    log_ctx: Option<StreamLoggingContext>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

        let mut message_start_sent = false;
        let mut active_block: Option<&'static str> = None;
        let mut block_index: usize = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_input_tokens: u64 = 50;
        let mut accumulated_response = String::new();

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            if let Ok(text) = std::str::from_utf8(&bytes) {
                                buffer.push_str(text);
                                if buffer.contains("\r\n") {
                                    buffer = buffer.replace("\r\n", "\n");
                                }

                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_chunk = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    for line in event_chunk.lines() {
                                        if let Some(data) = line.strip_prefix("data: ") {
                                            let trimmed = data.trim();
                                            if trimmed == "[DONE]" {
                                                if active_block.is_some() {
                                                    yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n", block_index)));
                                                }
                                                yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", total_output_tokens)));
                                                yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

                                                if let Some(ref ctx) = log_ctx {
                                                    let elapsed = ctx.start_time.elapsed().as_millis() as u64;
                                                    let resp_display = if accumulated_response.is_empty() {
                                                        "[Claude Stream Complete]".to_string()
                                                    } else {
                                                        accumulated_response.clone()
                                                    };
                                                    ctx.stats.record_detailed_request_exact(
                                                        ctx.method,
                                                        ctx.path,
                                                        &ctx.model,
                                                        200,
                                                        total_input_tokens + total_output_tokens,
                                                        total_input_tokens,
                                                        total_output_tokens,
                                                        elapsed,
                                                        &ctx.account_name,
                                                        &ctx.reasoning_effort,
                                                        &ctx.request_headers,
                                                        &ctx.request_body,
                                                        &resp_display,
                                                        None,
                                                    );
                                                }
                                                return;
                                            }

                                            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                                                let actual = val.get("response").unwrap_or(&val);

                                                if let Some(usage) = actual.get("usageMetadata") {
                                                    if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
                                                        total_input_tokens = pt;
                                                    }
                                                    if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                                                        total_output_tokens = ct;
                                                    }
                                                }

                                                // 1. Emit message_start on first valid response frame
                                                if !message_start_sent {
                                                    message_start_sent = true;
                                                    let start_event = serde_json::json!({
                                                        "type": "message_start",
                                                        "message": {
                                                            "id": &stream_id,
                                                            "type": "message",
                                                            "role": "assistant",
                                                            "content": [],
                                                            "model": &model_name,
                                                            "stop_reason": null,
                                                            "stop_sequence": null,
                                                            "usage": {
                                                                "input_tokens": total_input_tokens,
                                                                "output_tokens": 0
                                                            }
                                                        }
                                                    });
                                                    yield Ok(Bytes::from(format!("event: message_start\ndata: {}\n\n", start_event)));
                                                }

                                                // 2. Parse candidates and parts
                                                if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                                                    for candidate in candidates {
                                                        if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                                            for part in parts {
                                                                let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                                                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                                                    if accumulated_response.len() < 65536 {
                                                                        accumulated_response.push_str(txt);
                                                                    }

                                                                    if is_thought {
                                                                        if active_block == Some("text") {
                                                                            yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n", block_index)));
                                                                            block_index += 1;
                                                                            active_block = None;
                                                                        }
                                                                        if active_block != Some("thinking") {
                                                                            active_block = Some("thinking");
                                                                            let block_start = serde_json::json!({
                                                                                "type": "content_block_start",
                                                                                "index": block_index,
                                                                                "content_block": {
                                                                                    "type": "thinking",
                                                                                    "thinking": ""
                                                                                }
                                                                            });
                                                                            yield Ok(Bytes::from(format!("event: content_block_start\ndata: {}\n\n", block_start)));
                                                                        }
                                                                        let delta = serde_json::json!({
                                                                            "type": "content_block_delta",
                                                                            "index": block_index,
                                                                            "delta": {
                                                                                "type": "thinking_delta",
                                                                                "thinking": txt
                                                                            }
                                                                        });
                                                                        yield Ok(Bytes::from(format!("event: content_block_delta\ndata: {}\n\n", delta)));
                                                                    } else {
                                                                        if active_block == Some("thinking") {
                                                                            yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n", block_index)));
                                                                            block_index += 1;
                                                                            active_block = None;
                                                                        }
                                                                        if active_block != Some("text") {
                                                                            active_block = Some("text");
                                                                            let block_start = serde_json::json!({
                                                                                "type": "content_block_start",
                                                                                "index": block_index,
                                                                                "content_block": {
                                                                                    "type": "text",
                                                                                    "text": ""
                                                                                }
                                                                            });
                                                                            yield Ok(Bytes::from(format!("event: content_block_start\ndata: {}\n\n", block_start)));
                                                                        }
                                                                        let delta = serde_json::json!({
                                                                            "type": "content_block_delta",
                                                                            "index": block_index,
                                                                            "delta": {
                                                                                "type": "text_delta",
                                                                                "text": txt
                                                                            }
                                                                        });
                                                                        yield Ok(Bytes::from(format!("event: content_block_delta\ndata: {}\n\n", delta)));
                                                                    }
                                                                } else if let Some(inline) = part.get("inlineData") {
                                                                    let mime = inline.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image/png");
                                                                    if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                                                        let img_md = format!("[Generated Image: {} bytes, {}]\n![Generated Image](data:{};base64,{})", b64.len(), mime, mime, b64);
                                                                        if accumulated_response.len() < 65536 {
                                                                            accumulated_response.push_str(&img_md);
                                                                        }
                                                                        if active_block != Some("text") {
                                                                            active_block = Some("text");
                                                                            let block_start = serde_json::json!({
                                                                                "type": "content_block_start",
                                                                                "index": block_index,
                                                                                "content_block": {
                                                                                    "type": "text",
                                                                                    "text": ""
                                                                                }
                                                                            });
                                                                            yield Ok(Bytes::from(format!("event: content_block_start\ndata: {}\n\n", block_start)));
                                                                        }
                                                                        let delta = serde_json::json!({
                                                                            "type": "content_block_delta",
                                                                            "index": block_index,
                                                                            "delta": {
                                                                                "type": "text_delta",
                                                                                "text": img_md
                                                                            }
                                                                        });
                                                                        yield Ok(Bytes::from(format!("event: content_block_delta\ndata: {}\n\n", delta)));
                                                                    }
                                                                }
                                                            }
                                                        }

                                                        if let Some(finish) = candidate.get("finishReason").and_then(|f| f.as_str()) {
                                                            if finish == "STOP" {
                                                                if active_block.is_some() {
                                                                    yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n", block_index)));
                                                                }
                                                                yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", total_output_tokens)));
                                                                yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

                                                                if let Some(ref ctx) = log_ctx {
                                                                    let elapsed = ctx.start_time.elapsed().as_millis() as u64;
                                                                    let resp_display = if accumulated_response.is_empty() {
                                                                        "[Claude Stream Complete]".to_string()
                                                                    } else {
                                                                        accumulated_response.clone()
                                                                    };
                                                                    ctx.stats.record_detailed_request_exact(
                                                                        ctx.method,
                                                                        ctx.path,
                                                                        &ctx.model,
                                                                        200,
                                                                        total_input_tokens + total_output_tokens,
                                                                        total_input_tokens,
                                                                        total_output_tokens,
                                                                        elapsed,
                                                                        &ctx.account_name,
                                                                        &ctx.reasoning_effort,
                                                                        &ctx.request_headers,
                                                                        &ctx.request_body,
                                                                        &resp_display,
                                                                        None,
                                                                    );
                                                                }
                                                                return;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => {
                            break;
                        }
                    }
                }
            }
        }

        if active_block.is_some() {
            yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n", block_index)));
        }
        if message_start_sent {
            yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", total_output_tokens)));
            yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
        }

        if let Some(ref ctx) = log_ctx {
            let elapsed = ctx.start_time.elapsed().as_millis() as u64;
            let resp_display = if accumulated_response.is_empty() {
                "[Claude Stream Finished]".to_string()
            } else {
                accumulated_response
            };
            ctx.stats.record_detailed_request_exact(
                ctx.method,
                ctx.path,
                &ctx.model,
                200,
                total_input_tokens + total_output_tokens,
                total_input_tokens,
                total_output_tokens,
                elapsed,
                &ctx.account_name,
                &ctx.reasoning_effort,
                &ctx.request_headers,
                &ctx.request_body,
                &resp_display,
                None,
            );
        }
    }
}

pub fn pass_through_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    log_ctx: Option<StreamLoggingContext>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut total_bytes = 0usize;
        while let Some(item) = upstream_stream.next().await {
            match item {
                Ok(bytes) => {
                    total_bytes += bytes.len();
                    yield Ok(bytes);
                }
                Err(_) => break,
            }
        }

        if let Some(ctx) = log_ctx {
            let elapsed = ctx.start_time.elapsed().as_millis() as u64;
            let est_out_tok = (total_bytes / 4).max(1) as u64;
            let est_in_tok = (ctx.request_body.len() / 4).max(10) as u64;
            ctx.stats.record_detailed_request_exact(
                ctx.method,
                ctx.path,
                &ctx.model,
                200,
                est_in_tok + est_out_tok,
                est_in_tok,
                est_out_tok,
                elapsed,
                &ctx.account_name,
                &ctx.reasoning_effort,
                &ctx.request_headers,
                &ctx.request_body,
                &format!("[Pass-Through Stream Completed: {} bytes transferred]", total_bytes),
                None,
            );
        }
    }
}
