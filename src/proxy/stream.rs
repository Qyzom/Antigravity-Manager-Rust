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
