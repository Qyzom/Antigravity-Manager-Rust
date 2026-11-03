use crate::proxy::mappers::openai::create_stream_chunk;
use bytes::Bytes;
use futures_util::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::interval;

pub fn process_claude_to_openai_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut current_event_type = String::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await; // first tick is instant

        loop {
            tokio::select! {
                _ = ping_timer.tick() => {
                    // Send SSE keepalive comment to avoid client timeout during thinking phase
                    yield Ok(Bytes::from(": ping\n\n"));
                }
                item = upstream_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            if let Ok(text) = std::str::from_utf8(&bytes) {
                                buffer.push_str(text);
                                let normalized = buffer.replace("\r\n", "\n");
                                buffer = normalized;

                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_chunk = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    for line in event_chunk.lines() {
                                        if let Some(event) = line.strip_prefix("event: ") {
                                            current_event_type = event.trim().to_string();
                                        } else if let Some(data) = line.strip_prefix("data: ") {
                                            if let Ok(val) = serde_json::from_str::<Value>(data.trim()) {
                                                // Handle Claude event types
                                                if current_event_type == "content_block_delta" {
                                                    if let Some(delta) = val.get("delta") {
                                                        let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                                        if delta_type == "text_delta" {
                                                            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                                                let chunk = create_stream_chunk(&stream_id, &model_name, Some(text), None, None);
                                                                yield Ok(Bytes::from(chunk));
                                                            }
                                                        } else if delta_type == "thinking_delta" {
                                                            if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                                                                let chunk = create_stream_chunk(&stream_id, &model_name, None, Some(thinking), None);
                                                                yield Ok(Bytes::from(chunk));
                                                            }
                                                        }
                                                    }
                                                } else if current_event_type == "message_delta" {
                                                    if let Some(stop_reason) = val.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str()) {
                                                        let chunk = create_stream_chunk(&stream_id, &model_name, None, None, Some(stop_reason));
                                                        yield Ok(Bytes::from(chunk));
                                                    }
                                                } else if current_event_type == "message_stop" {
                                                    yield Ok(Bytes::from("data: [DONE]\n\n"));
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(_)) => {
                            yield Ok(Bytes::from("data: [DONE]\n\n"));
                            break;
                        }
                        None => {
                            yield Ok(Bytes::from("data: [DONE]\n\n"));
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub fn process_antigravity_to_openai_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut ping_timer = interval(Duration::from_secs(12));
        ping_timer.tick().await;

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
                                let normalized = buffer.replace("\r\n", "\n");
                                buffer = normalized;

                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_chunk = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    for line in event_chunk.lines() {
                                        if let Some(data) = line.strip_prefix("data: ") {
                                            let trimmed = data.trim();
                                            if trimmed == "[DONE]" {
                                                yield Ok(Bytes::from("data: [DONE]\n\n"));
                                                return;
                                            }
                                            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                                                let actual = val.get("response").unwrap_or(&val);
                                                if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
                                                    for candidate in candidates {
                                                        if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                                            for part in parts {
                                                                let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                                                                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                                                    if is_thought {
                                                                        let chunk = create_stream_chunk(&stream_id, &model_name, None, Some(txt), None);
                                                                        yield Ok(Bytes::from(chunk));
                                                                    } else {
                                                                        let chunk = create_stream_chunk(&stream_id, &model_name, Some(txt), None, None);
                                                                        yield Ok(Bytes::from(chunk));
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        if let Some(finish) = candidate.get("finishReason").and_then(|f| f.as_str()) {
                                                            if finish == "STOP" {
                                                                let chunk = create_stream_chunk(&stream_id, &model_name, None, None, Some("stop"));
                                                                yield Ok(Bytes::from(chunk));
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
                            yield Ok(Bytes::from("data: [DONE]\n\n"));
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub fn process_antigravity_to_claude_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    stream_id: String,
    model_name: String,
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
                                let normalized = buffer.replace("\r\n", "\n");
                                buffer = normalized;

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
                            if active_block.is_some() {
                                yield Ok(Bytes::from(format!("event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":{}}}\n\n", block_index)));
                            }
                            if message_start_sent {
                                yield Ok(Bytes::from(format!("event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":{}}}}}\n\n", total_output_tokens)));
                                yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub fn pass_through_stream(
    mut upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::stream! {
        while let Some(item) = upstream_stream.next().await {
            match item {
                Ok(bytes) => yield Ok(bytes),
                Err(_) => break,
            }
        }
    }
}
