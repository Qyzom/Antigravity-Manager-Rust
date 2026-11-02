use crate::config::UpstreamAccount;
use crate::proxy::mappers::claude::*;
use crate::proxy::mappers::openai::*;
use serde_json::Value;

pub fn resolve_model(requested_model: &str, account: &UpstreamAccount) -> String {
    let lower = requested_model.to_lowercase();
    match account.provider.as_str() {
        "anthropic" => {
            if lower.contains("claude-3-7") || lower.contains("sonnet-3.7") {
                "claude-3-7-sonnet-20250219".to_string()
            } else if lower.contains("claude-3-5-sonnet") || lower.contains("sonnet") {
                "claude-3-5-sonnet-20241022".to_string()
            } else if lower.contains("claude-3-5-haiku") || lower.contains("haiku") {
                "claude-3-5-haiku-20241022".to_string()
            } else if lower.contains("claude-3-opus") || lower.contains("opus") {
                "claude-3-opus-20240229".to_string()
            } else {
                requested_model.to_string()
            }
        }
        "openai" => {
            if lower.contains("gpt-4o-mini") {
                "gpt-4o-mini".to_string()
            } else if lower.contains("gpt-4o") {
                "gpt-4o".to_string()
            } else if lower.contains("o1") {
                "o1-preview".to_string()
            } else if lower.contains("o3") {
                "o3-mini".to_string()
            } else {
                requested_model.to_string()
            }
        }
        "gemini" => {
            if lower.contains("flash") {
                "gemini-2.0-flash".to_string()
            } else {
                "gemini-1.5-pro".to_string()
            }
        }
        _ => requested_model.to_string(),
    }
}

pub fn convert_openai_to_claude(req: &OpenAIChatRequest, target_model: &str) -> ClaudeMessageRequest {
    let mut claude_messages = Vec::new();
    let mut system_prompt = None;

    for msg in &req.messages {
        if msg.role == "system" {
            system_prompt = Some(msg.content.clone());
        } else {
            claude_messages.push(ClaudeMessage {
                role: if msg.role == "assistant" { "assistant".to_string() } else { "user".to_string() },
                content: msg.content.clone(),
            });
        }
    }

    // Enable thinking if model supports it or requested
    let is_thinking_model = target_model.contains("3-7") || target_model.contains("claude-3-7");
    let thinking_config = if is_thinking_model {
        Some(ClaudeThinkingConfig {
            thinking_type: "enabled".to_string(),
            budget_tokens: 2048,
        })
    } else {
        None
    };

    ClaudeMessageRequest {
        model: target_model.to_string(),
        messages: claude_messages,
        system: system_prompt,
        max_tokens: req.max_tokens.unwrap_or(if is_thinking_model { 8192 } else { 4096 }),
        stream: req.stream,
        temperature: if is_thinking_model { None } else { req.temperature },
        thinking: thinking_config,
    }
}

pub fn convert_claude_response_to_openai(resp: &ClaudeMessageResponse) -> OpenAIChatResponse {
    let mut full_text = String::new();
    let mut thinking_text = None;

    for block in &resp.content {
        match block {
            ClaudeContentBlock::Text { text } => full_text.push_str(text),
            ClaudeContentBlock::Thinking { thinking, .. } => {
                thinking_text = Some(thinking.clone());
            }
        }
    }

    OpenAIChatResponse {
        id: resp.id.clone(),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: resp.model.clone(),
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIMessage {
                role: "assistant".to_string(),
                content: Value::String(full_text),
                reasoning_content: thinking_text,
                name: None,
            },
            finish_reason: resp.stop_reason.clone().or(Some("stop".to_string())),
        }],
        usage: Some(OpenAIUsage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
        }),
    }
}
