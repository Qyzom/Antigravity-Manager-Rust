use crate::proxy::mappers::openai::{
    create_stream_chunk, create_stream_chunk_with_usage, create_stream_tool_chunk,
    create_stream_usage_chunk, OpenAIUsage, PromptTokensDetails,
};
use bytes::Bytes;
use futures_util::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::interval;

const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Byte-level SSE frame buffer that preserves split multi-byte UTF-8 sequences across TCP chunks.
struct SseByteBuffer {
    buf: Vec<u8>,
}

impl SseByteBuffer {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
        }
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        if self.buf.len() + bytes.len() > MAX_SSE_BUFFER_BYTES {
            log::warn!("SSE buffer exceeded {} bytes without frame boundary; resetting buffer", MAX_SSE_BUFFER_BYTES);
            self.buf.clear();
        }
        // Strip '\r' on the fly so frame boundaries are always normalized to "\n\n".
        for &b in bytes {
            if b != b'\r' {
                self.buf.push(b);
            }
        }
    }

    fn next_frame(&mut self) -> Option<String> {
        let pos = self.buf.windows(2).position(|w| w == b"\n\n")?;
        let frame = String::from_utf8_lossy(&self.buf[..pos]).into_owned();
        self.buf.drain(..pos + 2);
        Some(frame)
    }
}

fn record_stream_completion(
    log_ctx: &Option<StreamLoggingContext>,
    in_tok: u64,
    out_tok: u64,
    cached_tok: u64,
    resp_display: &str,
) {
    if let Some(ref ctx) = log_ctx {
        let elapsed = ctx.start_time.elapsed().as_millis() as u64;
        ctx.stats.record_detailed_request_full(
            ctx.method,
            ctx.path,
            &ctx.model,
            200,
            in_tok + out_tok,
            in_tok,
            out_tok,
            cached_tok,
            elapsed,
            &ctx.account_name,
            &ctx.reasoning_effort,
            &ctx.request_headers,
            &ctx.request_body,
            resp_display,
            None,
        );
    }
}

fn build_openai_usage(in_tok: u64, out_tok: u64, cached_tok: u64) -> OpenAIUsage {
    OpenAIUsage {
        prompt_tokens: in_tok,
        completion_tokens: out_tok,
        total_tokens: in_tok + out_tok,
        prompt_tokens_details: Some(PromptTokensDetails {
            cached_tokens: cached_tok,
            audio_tokens: 0,
        }),
        completion_tokens_details: None,
        prompt_cache_hit_tokens: if cached_tok > 0 { Some(cached_tok) } else { None },
        prompt_cache_miss_tokens: if cached_tok > 0 {
            Some(in_tok.saturating_sub(cached_tok))
        } else {
            None
        },
    }
}

fn resolve_input_tokens(total_input: u64, log_ctx: &Option<StreamLoggingContext>) -> u64 {
    if total_input > 0 {
        total_input
    } else {
        log_ctx
            .as_ref()
            .map(|c| (c.request_body.len() / 4).max(15) as u64)
            .unwrap_or(25)
    }
}

fn format_stream_response_json(
    stream_id: &str,
    model: &str,
    content: &str,
    reasoning: &str,
    in_tok: u64,
    out_tok: u64,
    cached_tok: u64,
) -> String {
    let mut message = serde_json::Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(content.to_string()));
    if !reasoning.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.to_string()),
        );
    }
    let resp = serde_json::json!({
        "id": stream_id,
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": in_tok,
            "completion_tokens": out_tok,
            "total_tokens": in_tok + out_tok,
            "prompt_tokens_details": {
                "cached_tokens": cached_tok
            }
        }
    });
    serde_json::to_string_pretty(&resp).unwrap_or_else(|_| content.to_string())
}

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
        let is_delimiter = matches!(
            c,
            ' ' | '\n' | '\t' | '.' | ',' | ';' | '}' | ')' | '{' | '('
        );
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
        let mut sse_buf = SseByteBuffer::new();
        let mut current_event_type = String::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_cached_tokens: u64 = 0;
        let mut accumulated_response = String::new();
        let mut tool_call_index: usize = 0;
        let mut current_tool_index: Option<usize> = None;
        let mut has_emitted_tools = false;

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            sse_buf.push_bytes(&bytes);
                            while let Some(event_chunk) = sse_buf.next_frame() {
                                for line in event_chunk.lines() {
                                    if let Some(event) = line.strip_prefix("event: ") {
                                        current_event_type = event.trim().to_string();
                                    } else if let Some(data) = line.strip_prefix("data: ") {
                                        if let Ok(val) = serde_json::from_str::<Value>(data.trim()) {
                                            if current_event_type == "message_start" {
                                                if let Some(usage) = val.pointer("/message/usage") {
                                                    if let Some(it) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                                        total_input_tokens = it;
                                                    }
                                                    if let Some(crt) = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()) {
                                                        total_cached_tokens = crt;
                                                    }
                                                }
                                            } else if current_event_type == "content_block_start" {
                                                if let Some(cb) = val.get("content_block") {
                                                    if cb.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                                        let id = cb.get("id").and_then(|v| v.as_str()).unwrap_or("toolu_0");
                                                        let name = cb.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                                                        let idx = tool_call_index;
                                                        tool_call_index += 1;
                                                        current_tool_index = Some(idx);
                                                        has_emitted_tools = true;
                                                        let tool_call_obj = serde_json::json!({
                                                            "index": idx,
                                                            "id": id,
                                                            "type": "function",
                                                            "function": {
                                                                "name": name,
                                                                "arguments": ""
                                                            }
                                                        });
                                                        yield Ok(Bytes::from(create_stream_tool_chunk(&stream_id, &model_name, vec![tool_call_obj])));
                                                    }
                                                }
                                            } else if current_event_type == "content_block_stop" {
                                                current_tool_index = None;
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
                                                    } else if delta_type == "input_json_delta" {
                                                        if let Some(partial) = delta.get("partial_json").and_then(|t| t.as_str()) {
                                                            let idx = current_tool_index.unwrap_or(0);
                                                            total_output_tokens += (partial.len() / 4).max(1) as u64;
                                                            let tool_call_obj = serde_json::json!({
                                                                "index": idx,
                                                                "function": {
                                                                    "arguments": partial
                                                                }
                                                            });
                                                            yield Ok(Bytes::from(create_stream_tool_chunk(&stream_id, &model_name, vec![tool_call_obj])));
                                                        }
                                                    }
                                                }
                                            } else if current_event_type == "message_delta" {
                                                if let Some(ot) = val.pointer("/usage/output_tokens").and_then(|v| v.as_u64()) {
                                                    total_output_tokens = ot;
                                                }
                                                if let Some(raw_stop) = val.pointer("/delta/stop_reason").and_then(|s| s.as_str()) {
                                                    let stop_reason = if has_emitted_tools || raw_stop == "tool_use" {
                                                        "tool_calls"
                                                    } else if raw_stop == "max_tokens" {
                                                        "length"
                                                    } else {
                                                        "stop"
                                                    };
                                                    let in_tok = resolve_input_tokens(total_input_tokens, &log_ctx);
                                                    let out_tok = total_output_tokens.max(1);
                                                    let usage_val = build_openai_usage(in_tok, out_tok, total_cached_tokens);
                                                    let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some(stop_reason), Some(usage_val.clone()));
                                                    yield Ok(Bytes::from(chunk));
                                                    let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
                                                    yield Ok(Bytes::from(u_chunk));
                                                }
                                            } else if current_event_type == "message_stop" {
                                                yield Ok(Bytes::from("data: [DONE]\n\n"));
                                                let in_tok = resolve_input_tokens(total_input_tokens, &log_ctx);
                                                let out_tok = total_output_tokens.max(1);
                                                let resp_display = if accumulated_response.is_empty() {
                                                    "[Claude Stream Complete]"
                                                } else {
                                                    accumulated_response.as_str()
                                                };
                                                record_stream_completion(&log_ctx, in_tok, out_tok, total_cached_tokens, resp_display);
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        }

        let in_tok = resolve_input_tokens(total_input_tokens, &log_ctx);
        let out_tok = total_output_tokens.max(1);
        let usage_val = build_openai_usage(in_tok, out_tok, total_cached_tokens);
        let fallback_stop = if has_emitted_tools { "tool_calls" } else { "stop" };
        let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some(fallback_stop), Some(usage_val.clone()));
        yield Ok(Bytes::from(chunk));
        let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
        yield Ok(Bytes::from(u_chunk));
        yield Ok(Bytes::from("data: [DONE]\n\n"));

        let resp_display = if accumulated_response.is_empty() {
            "[Claude Stream Finished]"
        } else {
            accumulated_response.as_str()
        };
        record_stream_completion(&log_ctx, in_tok, out_tok, total_cached_tokens, resp_display);
    }
}

pub fn process_antigravity_to_openai_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
    log_ctx: Option<StreamLoggingContext>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut sse_buf = SseByteBuffer::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_cached_tokens: u64 = 0;
        let mut estimated_output_tokens: u64 = 0;
        let mut accumulated_response = String::new();
        let mut accumulated_reasoning = String::new();
        let mut tool_call_index: usize = 0;
        let mut has_emitted_tools: bool = false;
        let mut sent_role: bool = false;

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            sse_buf.push_bytes(&bytes);
                            while let Some(event_chunk) = sse_buf.next_frame() {
                                for line in event_chunk.lines() {
                                    if let Some(data) = line.strip_prefix("data: ") {
                                        let trimmed = data.trim();
                                        if trimmed == "[DONE]" {
                                            let in_tok = resolve_input_tokens(total_input_tokens, &log_ctx);
                                            let out_tok = if total_output_tokens > 0 { total_output_tokens } else { estimated_output_tokens.max(1) };
                                            let usage_val = build_openai_usage(in_tok, out_tok, total_cached_tokens);
                                            let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
                                            yield Ok(Bytes::from(u_chunk));
                                            yield Ok(Bytes::from("data: [DONE]\n\n"));

                                            let resp_display = if accumulated_response.is_empty() {
                                                "[Stream Complete]".to_string()
                                            } else {
                                                accumulated_response.clone()
                                            };
                                            record_stream_completion(&log_ctx, in_tok, out_tok, total_cached_tokens, &resp_display);
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
                                                if let Some(cct) = usage.get("cachedContentTokenCount").and_then(|v| v.as_u64()) {
                                                    total_cached_tokens = cct;
                                                }
                                            }

                                            if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                                                for candidate in candidates {
                                                    if let Some(parts) = candidate.pointer("/content/parts").and_then(|p| p.as_array()) {
                                                        for part in parts {
                                                            let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                                            if let Some(fc) = part.get("functionCall") {
                                                                let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                                                                let args_val = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                                                                let args_str = serde_json::to_string(&args_val).unwrap_or_else(|_| "{}".to_string());
                                                                let call_id = fc.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())
                                                                    .unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple()));

                                                                if let Some(sig) = part.get("thoughtSignature")
                                                                    .or_else(|| part.get("thought_signature"))
                                                                    .or_else(|| candidate.get("thoughtSignature"))
                                                                    .or_else(|| candidate.get("thought_signature"))
                                                                    .and_then(|s| s.as_str())
                                                                {
                                                                    crate::proxy::antigravity::record_thought_signature(&call_id, name, sig);
                                                                }

                                                                if accumulated_response.len() < 65536 {
                                                                    accumulated_response.push_str(&format!("[Call: {}({})]", name, args_str));
                                                                }
                                                                estimated_output_tokens += (args_str.len() / 4).max(1) as u64;

                                                                let tool_call_obj = serde_json::json!({
                                                                    "index": tool_call_index,
                                                                    "id": call_id,
                                                                    "type": "function",
                                                                    "function": {
                                                                        "name": name,
                                                                        "arguments": args_str
                                                                    }
                                                                });
                                                                tool_call_index += 1;
                                                                has_emitted_tools = true;

                                                                if !sent_role {
                                                                    sent_role = true;
                                                                    let role_chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, None, None);
                                                                    yield Ok(Bytes::from(role_chunk));
                                                                }
                                                                let chunk = create_stream_tool_chunk(&stream_id, &model_name, vec![tool_call_obj]);
                                                                yield Ok(Bytes::from(chunk));
                                                            } else if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                                                if !txt.is_empty() {
                                                                    if !sent_role {
                                                                        sent_role = true;
                                                                        let role_chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, None, None);
                                                                        yield Ok(Bytes::from(role_chunk));
                                                                    }
                                                                    if is_thought {
                                                                        if accumulated_reasoning.len() < 65536 {
                                                                            accumulated_reasoning.push_str(txt);
                                                                        }
                                                                    } else if accumulated_response.len() < 65536 {
                                                                        accumulated_response.push_str(txt);
                                                                    }
                                                                    estimated_output_tokens += (txt.len() / 4).max(1) as u64;

                                                                    if is_thought {
                                                                        let chunk = create_stream_chunk(&stream_id, &model_name, None, Some(txt), None);
                                                                        yield Ok(Bytes::from(chunk));
                                                                    } else {
                                                                        let sub_chunks = emit_smooth_chunks(txt);
                                                                        let count = sub_chunks.len();
                                                                        for (i, sub) in sub_chunks.into_iter().enumerate() {
                                                                            if !sub.is_empty() {
                                                                                let chunk = create_stream_chunk(&stream_id, &model_name, Some(sub), None, None);
                                                                                yield Ok(Bytes::from(chunk));
                                                                                if count > 1 && i + 1 < count {
                                                                                    tokio::task::yield_now().await;
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            } else if let Some(inline) = part.get("inlineData") {
                                                                let mime = inline.get("mimeType").and_then(|m| m.as_str()).unwrap_or("image/png");
                                                                if let Some(b64) = inline.get("data").and_then(|d| d.as_str()) {
                                                                    if !sent_role {
                                                                        sent_role = true;
                                                                        let role_chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, None, None);
                                                                        yield Ok(Bytes::from(role_chunk));
                                                                    }
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
                                                    if candidate.get("groundingMetadata").is_some() {
                                                        let (_q, _r, sources_md) = crate::proxy::antigravity::extract_grounding_info(actual);
                                                        if !sources_md.is_empty() {
                                                            if !sent_role {
                                                                sent_role = true;
                                                                let role_chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, None, None);
                                                                yield Ok(Bytes::from(role_chunk));
                                                            }
                                                            if accumulated_response.len() < 65536 {
                                                                accumulated_response.push_str(&sources_md);
                                                            }
                                                            let chunk = create_stream_chunk(&stream_id, &model_name, Some(&sources_md), None, None);
                                                            yield Ok(Bytes::from(chunk));
                                                        }
                                                    }
                                                    if let Some(finish) = candidate.get("finishReason").and_then(|f| f.as_str()) {
                                                        let stop_reason = if has_emitted_tools {
                                                            "tool_calls"
                                                        } else {
                                                            match finish {
                                                                "STOP" => "stop",
                                                                "MAX_TOKENS" => "length",
                                                                "SAFETY" => "content_filter",
                                                                _ => "stop",
                                                            }
                                                        };

                                                        let in_tok = resolve_input_tokens(total_input_tokens, &log_ctx);
                                                        let out_tok = if total_output_tokens > 0 { total_output_tokens } else { estimated_output_tokens.max(1) };
                                                        let usage_val = build_openai_usage(in_tok, out_tok, total_cached_tokens);

                                                        let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some(stop_reason), None);
                                                        yield Ok(Bytes::from(chunk));
                                                        let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
                                                        yield Ok(Bytes::from(u_chunk));
                                                        yield Ok(Bytes::from("data: [DONE]\n\n"));

                                                        let resp_display = format_stream_response_json(
                                                            &stream_id,
                                                            &model_name,
                                                            &accumulated_response,
                                                            &accumulated_reasoning,
                                                            in_tok,
                                                            out_tok,
                                                            total_cached_tokens,
                                                        );
                                                        record_stream_completion(&log_ctx, in_tok, out_tok, total_cached_tokens, &resp_display);
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        }

        let in_tok = resolve_input_tokens(total_input_tokens, &log_ctx);
        let out_tok = if total_output_tokens > 0 { total_output_tokens } else { estimated_output_tokens.max(1) };
        let usage_val = build_openai_usage(in_tok, out_tok, total_cached_tokens);
        let fallback_stop = if has_emitted_tools { "tool_calls" } else { "stop" };
        let chunk = create_stream_chunk_with_usage(&stream_id, &model_name, None, None, Some(fallback_stop), Some(usage_val.clone()));
        yield Ok(Bytes::from(chunk));
        let u_chunk = create_stream_usage_chunk(&stream_id, &model_name, usage_val);
        yield Ok(Bytes::from(u_chunk));
        yield Ok(Bytes::from("data: [DONE]\n\n"));

        let resp_display = format_stream_response_json(
            &stream_id,
            &model_name,
            &accumulated_response,
            &accumulated_reasoning,
            in_tok,
            out_tok,
            total_cached_tokens,
        );
        record_stream_completion(&log_ctx, in_tok, out_tok, total_cached_tokens, &resp_display);
    }
}

fn claude_signature_delta_sse(block_index: usize) -> Bytes {
    let sig_delta = serde_json::json!({
        "type": "content_block_delta",
        "index": block_index,
        "delta": {
            "type": "signature_delta",
            "signature": format!("sig_{:016x}", fastrand::u64(..))
        }
    });
    Bytes::from(format!("event: content_block_delta\ndata: {}\n\n", sig_delta))
}

fn claude_block_stop_sse(block_index: usize) -> Bytes {
    Bytes::from(format!(
        "event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n",
        block_index
    ))
}

pub fn process_antigravity_to_claude_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
    log_ctx: Option<StreamLoggingContext>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut sse_buf = SseByteBuffer::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

        let mut message_start_sent = false;
        let mut active_block: Option<&'static str> = None;
        let mut block_index: usize = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_input_tokens: u64 = resolve_input_tokens(0, &log_ctx);
        let mut total_cached_tokens: u64 = 0;
        let mut accumulated_response = String::new();
        let mut has_emitted_tools = false;

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            sse_buf.push_bytes(&bytes);
                            while let Some(event_chunk) = sse_buf.next_frame() {
                                for line in event_chunk.lines() {
                                    if let Some(data) = line.strip_prefix("data: ") {
                                        let trimmed = data.trim();
                                        if trimmed == "[DONE]" {
                                            if let Some(active) = active_block {
                                                if active == "thinking" {
                                                    yield Ok(claude_signature_delta_sse(block_index));
                                                }
                                                yield Ok(claude_block_stop_sse(block_index));
                                            }
                                            let stop_reason = if has_emitted_tools { "tool_use" } else { "end_turn" };
                                            yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{}\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", stop_reason, total_output_tokens)));
                                            yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

                                            let resp_display = if accumulated_response.is_empty() {
                                                "[Claude Stream Complete]"
                                            } else {
                                                accumulated_response.as_str()
                                            };
                                            record_stream_completion(&log_ctx, total_input_tokens, total_output_tokens, total_cached_tokens, resp_display);
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
                                                if let Some(cct) = usage.get("cachedContentTokenCount").and_then(|v| v.as_u64()) {
                                                    total_cached_tokens = cct;
                                                }
                                            }

                                            if !message_start_sent {
                                                message_start_sent = true;
                                                let mut start_usage = serde_json::json!({
                                                    "input_tokens": total_input_tokens,
                                                    "output_tokens": 0
                                                });
                                                if total_cached_tokens > 0 {
                                                    start_usage["cache_read_input_tokens"] = serde_json::json!(total_cached_tokens);
                                                }
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
                                                        "usage": start_usage
                                                    }
                                                });
                                                yield Ok(Bytes::from(format!("event: message_start\ndata: {}\n\n", start_event)));
                                            }

                                            if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                                                for candidate in candidates {
                                                    if let Some(parts) = candidate.pointer("/content/parts").and_then(|p| p.as_array()) {
                                                        for part in parts {
                                                            let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                                            if let Some(fc) = part.get("functionCall") {
                                                                if let Some(active) = active_block {
                                                                    if active == "thinking" {
                                                                        yield Ok(claude_signature_delta_sse(block_index));
                                                                    }
                                                                    yield Ok(claude_block_stop_sse(block_index));
                                                                    block_index += 1;
                                                                    active_block = None;
                                                                }
                                                                let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                                                                let args_val = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                                                                let args_str = serde_json::to_string(&args_val).unwrap_or_else(|_| "{}".to_string());
                                                                let call_id = fc.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())
                                                                    .unwrap_or_else(|| format!("toolu_{}", uuid::Uuid::new_v4().simple()));

                                                                if let Some(sig) = part.get("thoughtSignature")
                                                                    .or_else(|| part.get("thought_signature"))
                                                                    .or_else(|| candidate.get("thoughtSignature"))
                                                                    .or_else(|| candidate.get("thought_signature"))
                                                                    .and_then(|s| s.as_str())
                                                                {
                                                                    crate::proxy::antigravity::record_thought_signature(&call_id, name, sig);
                                                                }

                                                                if accumulated_response.len() < 65536 {
                                                                    accumulated_response.push_str(&format!("[Call: {}({})]", name, args_str));
                                                                }
                                                                has_emitted_tools = true;

                                                                let tool_start = serde_json::json!({
                                                                    "type": "content_block_start",
                                                                    "index": block_index,
                                                                    "content_block": {
                                                                        "type": "tool_use",
                                                                        "id": call_id,
                                                                        "name": name,
                                                                        "input": {}
                                                                    }
                                                                });
                                                                yield Ok(Bytes::from(format!("event: content_block_start\ndata: {}\n\n", tool_start)));

                                                                let tool_delta = serde_json::json!({
                                                                    "type": "content_block_delta",
                                                                    "index": block_index,
                                                                    "delta": {
                                                                        "type": "input_json_delta",
                                                                        "partial_json": args_str
                                                                    }
                                                                });
                                                                yield Ok(Bytes::from(format!("event: content_block_delta\ndata: {}\n\n", tool_delta)));
                                                                yield Ok(claude_block_stop_sse(block_index));
                                                                block_index += 1;
                                                            } else if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                                                if txt.is_empty() {
                                                                    continue;
                                                                }
                                                                if accumulated_response.len() < 65536 {
                                                                    accumulated_response.push_str(txt);
                                                                }

                                                                if is_thought {
                                                                    if active_block == Some("text") {
                                                                        yield Ok(claude_block_stop_sse(block_index));
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
                                                                        yield Ok(claude_signature_delta_sse(block_index));
                                                                        yield Ok(claude_block_stop_sse(block_index));
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
                                                                        if active_block == Some("thinking") {
                                                                            yield Ok(claude_signature_delta_sse(block_index));
                                                                            yield Ok(claude_block_stop_sse(block_index));
                                                                            block_index += 1;
                                                                        }
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
                                                        if let Some(active) = active_block {
                                                            if active == "thinking" {
                                                                yield Ok(claude_signature_delta_sse(block_index));
                                                            }
                                                            yield Ok(claude_block_stop_sse(block_index));
                                                        }
                                                        let stop_reason = if has_emitted_tools {
                                                            "tool_use"
                                                        } else if finish == "MAX_TOKENS" {
                                                            "max_tokens"
                                                        } else {
                                                            "end_turn"
                                                        };
                                                        yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{}\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", stop_reason, total_output_tokens)));
                                                        yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

                                                        let resp_display = if accumulated_response.is_empty() {
                                                            "[Claude Stream Complete]"
                                                        } else {
                                                            accumulated_response.as_str()
                                                        };
                                                        record_stream_completion(&log_ctx, total_input_tokens, total_output_tokens, total_cached_tokens, resp_display);
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        }

        if let Some(active) = active_block {
            if active == "thinking" {
                yield Ok(claude_signature_delta_sse(block_index));
            }
            yield Ok(claude_block_stop_sse(block_index));
        }
        if message_start_sent {
            let stop_reason = if has_emitted_tools { "tool_use" } else { "end_turn" };
            yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{}\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", stop_reason, total_output_tokens)));
            yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
        }

        let resp_display = if accumulated_response.is_empty() {
            "[Claude Stream Finished]"
        } else {
            accumulated_response.as_str()
        };
        record_stream_completion(&log_ctx, total_input_tokens, total_output_tokens, total_cached_tokens, resp_display);
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
