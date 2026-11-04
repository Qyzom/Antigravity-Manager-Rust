use crate::config::UpstreamAccount;
use crate::proxy::mappers::claude::*;
use crate::proxy::mappers::openai::*;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Off,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn budget_tokens(&self) -> u32 {
        match self {
            ReasoningEffort::Off => 0,
            ReasoningEffort::Low => 1024,
            ReasoningEffort::Medium => 4096,
            ReasoningEffort::High => 16384,
        }
    }
}

pub fn parse_model_and_reasoning(
    raw_model: &str,
    native_effort: Option<&str>,
) -> (String, Option<ReasoningEffort>) {
    let lower = raw_model.trim().to_lowercase();

    // Check for effort suffix in model name across all common formats
    let (base_clean, suffix_effort) = if lower.ends_with(" (low)") {
        (raw_model[..raw_model.len() - 6].trim().to_string(), Some(ReasoningEffort::Low))
    } else if lower.ends_with(" [low effort]") {
        (raw_model[..raw_model.len() - 13].trim().to_string(), Some(ReasoningEffort::Low))
    } else if lower.ends_with(" (low effort)") {
        (raw_model[..raw_model.len() - 13].trim().to_string(), Some(ReasoningEffort::Low))
    } else if lower.ends_with(" [low]") {
        (raw_model[..raw_model.len() - 6].trim().to_string(), Some(ReasoningEffort::Low))
    } else if lower.ends_with(":low") {
        (raw_model[..raw_model.len() - 4].trim().to_string(), Some(ReasoningEffort::Low))
    } else if lower.ends_with(" (medium)") {
        (raw_model[..raw_model.len() - 9].trim().to_string(), Some(ReasoningEffort::Medium))
    } else if lower.ends_with(" [medium effort]") {
        (raw_model[..raw_model.len() - 16].trim().to_string(), Some(ReasoningEffort::Medium))
    } else if lower.ends_with(" (medium effort)") {
        (raw_model[..raw_model.len() - 16].trim().to_string(), Some(ReasoningEffort::Medium))
    } else if lower.ends_with(" [medium]") {
        (raw_model[..raw_model.len() - 9].trim().to_string(), Some(ReasoningEffort::Medium))
    } else if lower.ends_with(":medium") {
        (raw_model[..raw_model.len() - 7].trim().to_string(), Some(ReasoningEffort::Medium))
    } else if lower.ends_with(" (high)") {
        (raw_model[..raw_model.len() - 7].trim().to_string(), Some(ReasoningEffort::High))
    } else if lower.ends_with(" [high effort]") {
        (raw_model[..raw_model.len() - 14].trim().to_string(), Some(ReasoningEffort::High))
    } else if lower.ends_with(" (high effort)") {
        (raw_model[..raw_model.len() - 14].trim().to_string(), Some(ReasoningEffort::High))
    } else if lower.ends_with(" [high]") {
        (raw_model[..raw_model.len() - 7].trim().to_string(), Some(ReasoningEffort::High))
    } else if lower.ends_with(":high") {
        (raw_model[..raw_model.len() - 5].trim().to_string(), Some(ReasoningEffort::High))
    } else if lower.ends_with(" (no-thinking)") {
        (raw_model[..raw_model.len() - 14].trim().to_string(), Some(ReasoningEffort::Off))
    } else if lower.ends_with(" [thinking off]") {
        (raw_model[..raw_model.len() - 15].trim().to_string(), Some(ReasoningEffort::Off))
    } else if lower.ends_with(" (thinking off)") {
        (raw_model[..raw_model.len() - 15].trim().to_string(), Some(ReasoningEffort::Off))
    } else if lower.ends_with(" [no-thinking]") {
        (raw_model[..raw_model.len() - 14].trim().to_string(), Some(ReasoningEffort::Off))
    } else if lower.ends_with(":off") {
        (raw_model[..raw_model.len() - 4].trim().to_string(), Some(ReasoningEffort::Off))
    } else if lower.ends_with("-thinking-low") {
        (raw_model[..raw_model.len() - 13].trim().to_string(), Some(ReasoningEffort::Low))
    } else if lower.ends_with("-thinking-medium") {
        (raw_model[..raw_model.len() - 16].trim().to_string(), Some(ReasoningEffort::Medium))
    } else if lower.ends_with("-thinking-high") {
        (raw_model[..raw_model.len() - 14].trim().to_string(), Some(ReasoningEffort::High))
    } else if lower.ends_with("-no-thinking") {
        (raw_model[..raw_model.len() - 12].trim().to_string(), Some(ReasoningEffort::Off))
    } else {
        (raw_model.to_string(), None)
    };

    let resolved_effort = suffix_effort.or_else(|| {
        native_effort.and_then(|eff| match eff.to_lowercase().as_str() {
            "low" | "minimal" => Some(ReasoningEffort::Low),
            "medium" | "default" => Some(ReasoningEffort::Medium),
            "high" | "xhigh" | "max" => Some(ReasoningEffort::High),
            "off" | "none" | "disabled" => Some(ReasoningEffort::Off),
            _ => None,
        })
    });

    (base_clean, resolved_effort)
}

pub fn resolve_model(requested_model: &str, account: &UpstreamAccount) -> String {
    let (base, _) = parse_model_and_reasoning(requested_model, None);
    let lower = base.to_lowercase();

    match account.provider.as_str() {
        "antigravity" => {
            if lower.contains("opus") {
                "claude-opus-5-5-high".to_string()
            } else if lower.contains("claude") || lower.contains("sonnet") {
                "claude-sonnet-5-5-high".to_string()
            } else if lower.contains("argon") {
                "gemini-4-argon".to_string()
            } else if lower.contains("gemini") || lower.contains("flash") {
                "gemini-3.8-flash-high".to_string()
            } else {
                "claude-sonnet-5-5-high".to_string()
            }
        }
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
                base
            }
        }
        "openai" => {
            if lower.contains("gpt-4o-mini") {
                "gpt-4o-mini".to_string()
            } else if lower.contains("gpt-4o") {
                "gpt-4o".to_string()
            } else if lower.contains("o3-mini") {
                "o3-mini".to_string()
            } else if lower.contains("o1") {
                "o1-preview".to_string()
            } else {
                base
            }
        }
        "gemini" => {
            if lower.contains("flash") {
                "gemini-2.0-flash".to_string()
            } else {
                "gemini-1.5-pro".to_string()
            }
        }
        _ => base,
    }
}

pub fn convert_openai_to_claude(
    req: &OpenAIChatRequest,
    target_model: &str,
    effort: Option<ReasoningEffort>,
) -> ClaudeMessageRequest {
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

    let is_thinking_capable = target_model.contains("3-7") || target_model.contains("claude-3-7");
    let thinking_config = if is_thinking_capable {
        match effort {
            Some(ReasoningEffort::Off) => None,
            Some(eff) => Some(ClaudeThinkingConfig {
                thinking_type: "enabled".to_string(),
                budget_tokens: eff.budget_tokens(),
            }),
            None => Some(ClaudeThinkingConfig {
                thinking_type: "enabled".to_string(),
                budget_tokens: 4096, // default medium
            }),
        }
    } else {
        None
    };

    let max_tok = if thinking_config.is_some() {
        req.max_tokens.unwrap_or(8192).max(4096)
    } else {
        req.max_tokens.unwrap_or(4096)
    };

    ClaudeMessageRequest {
        model: target_model.to_string(),
        messages: claude_messages,
        system: system_prompt,
        max_tokens: max_tok,
        stream: req.stream,
        temperature: if thinking_config.is_some() { None } else { req.temperature },
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
        system_fingerprint: "fp_44709d6fcb".to_string(),
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIMessage {
                role: "assistant".to_string(),
                content: Value::String(full_text),
                reasoning_content: thinking_text,
                refusal: None,
                name: None,
            },
            logprobs: None,
            finish_reason: resp.stop_reason.clone().or(Some("stop".to_string())),
        }],
        usage: Some(OpenAIUsage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            prompt_tokens_details: Some(PromptTokensDetails {
                cached_tokens: 0,
                audio_tokens: 0,
            }),
            completion_tokens_details: Some(CompletionTokensDetails {
                reasoning_tokens: 0,
                audio_tokens: 0,
                accepted_prediction_tokens: 0,
                rejected_prediction_tokens: 0,
            }),
        }),
    }
}
