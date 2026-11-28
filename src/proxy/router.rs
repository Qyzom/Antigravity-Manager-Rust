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

const REASONING_SUFFIXES: &[(&str, Option<ReasoningEffort>)] = &[
    // Low / minimal effort
    (" [low effort]", Some(ReasoningEffort::Low)),
    (" (low effort)", Some(ReasoningEffort::Low)),
    ("-thinking-minimal", Some(ReasoningEffort::Low)),
    ("-thinking-low", Some(ReasoningEffort::Low)),
    (" [minimal]", Some(ReasoningEffort::Low)),
    (" (minimal)", Some(ReasoningEffort::Low)),
    (":minimal", Some(ReasoningEffort::Low)),
    (" minimal", Some(ReasoningEffort::Low)),
    ("-minimal", Some(ReasoningEffort::Low)),
    (" [low]", Some(ReasoningEffort::Low)),
    (" (low)", Some(ReasoningEffort::Low)),
    (":low", Some(ReasoningEffort::Low)),
    (" low", Some(ReasoningEffort::Low)),
    ("-low", Some(ReasoningEffort::Low)),
    // Medium effort
    (" [medium effort]", Some(ReasoningEffort::Medium)),
    (" (medium effort)", Some(ReasoningEffort::Medium)),
    ("-thinking-medium", Some(ReasoningEffort::Medium)),
    (" [medium]", Some(ReasoningEffort::Medium)),
    (" (medium)", Some(ReasoningEffort::Medium)),
    (":medium", Some(ReasoningEffort::Medium)),
    (" medium", Some(ReasoningEffort::Medium)),
    ("-medium", Some(ReasoningEffort::Medium)),
    // High / xhigh / max effort
    (" [xhigh effort]", Some(ReasoningEffort::High)),
    (" (xhigh effort)", Some(ReasoningEffort::High)),
    ("-thinking-xhigh", Some(ReasoningEffort::High)),
    (" [xhigh]", Some(ReasoningEffort::High)),
    (" (xhigh)", Some(ReasoningEffort::High)),
    (":xhigh", Some(ReasoningEffort::High)),
    (" xhigh", Some(ReasoningEffort::High)),
    ("-xhigh", Some(ReasoningEffort::High)),
    (" [max]", Some(ReasoningEffort::High)),
    (" (max)", Some(ReasoningEffort::High)),
    (":max", Some(ReasoningEffort::High)),
    (" max", Some(ReasoningEffort::High)),
    ("-max", Some(ReasoningEffort::High)),
    (" [high effort]", Some(ReasoningEffort::High)),
    (" (high effort)", Some(ReasoningEffort::High)),
    ("-thinking-high", Some(ReasoningEffort::High)),
    (" [high]", Some(ReasoningEffort::High)),
    (" (high)", Some(ReasoningEffort::High)),
    (":high", Some(ReasoningEffort::High)),
    (" high", Some(ReasoningEffort::High)),
    ("-high", Some(ReasoningEffort::High)),
    // Thinking off / none
    (" [thinking off]", Some(ReasoningEffort::Off)),
    (" (thinking off)", Some(ReasoningEffort::Off)),
    (" [no-thinking]", Some(ReasoningEffort::Off)),
    (" (no-thinking)", Some(ReasoningEffort::Off)),
    ("-no-thinking", Some(ReasoningEffort::Off)),
    (" [none]", Some(ReasoningEffort::Off)),
    (" (none)", Some(ReasoningEffort::Off)),
    ("-none", Some(ReasoningEffort::Off)),
    (":none", Some(ReasoningEffort::Off)),
    (" none", Some(ReasoningEffort::Off)),
    (":off", Some(ReasoningEffort::Off)),
    (" off", Some(ReasoningEffort::Off)),
    ("-off", Some(ReasoningEffort::Off)),
    // Default
    (" (default)", None),
    (" default", None),
    ("-default", None),
];

pub fn parse_model_and_reasoning(
    raw_model: &str,
    native_effort: Option<&str>,
) -> (String, Option<ReasoningEffort>) {
    let trimmed = raw_model.trim();

    // Check for effort suffix in model name across all common formats using char boundaries
    let (base_clean, suffix_effort) = {
        let mut matched = None;
        for &(suffix, eff) in REASONING_SUFFIXES {
            if trimmed.len() >= suffix.len() {
                let split_idx = trimmed.len() - suffix.len();
                if trimmed.is_char_boundary(split_idx)
                    && trimmed[split_idx..].eq_ignore_ascii_case(suffix)
                {
                    let base = trimmed[..split_idx].trim().to_string();
                    matched = Some((base, eff));
                    break;
                }
            }
        }
        matched.unwrap_or_else(|| (trimmed.to_string(), None))
    };

    let resolved_effort = suffix_effort.or_else(|| {
        native_effort.and_then(|eff| match eff.to_lowercase().as_str() {
            "low" | "minimal" | "min" => Some(ReasoningEffort::Low),
            "medium" | "mid" | "normal" | "standard" => Some(ReasoningEffort::Medium),
            "high" | "xhigh" | "max" | "extreme" => Some(ReasoningEffort::High),
            "off" | "none" | "disabled" | "zero" | "false" => Some(ReasoningEffort::Off),
            "default" => None,
            _ => None,
        })
    });

    (base_clean, resolved_effort)
}

pub fn resolve_model_with_effort(
    requested_model: &str,
    account: &UpstreamAccount,
    effort: Option<ReasoningEffort>,
) -> (String, Option<ReasoningEffort>) {
    let (base, parsed_eff) = parse_model_and_reasoning(requested_model, None);
    let effective_effort = effort.or(parsed_eff);
    let lower = base.to_lowercase();

    match account.provider.as_str() {
        "antigravity" | "google" | "gemini" => {
            if lower.contains("web-search") || lower.contains("google-search") {
                return ("web-search".to_string(), None);
            } else if lower.contains("whisper") || lower.contains("transcrib") || lower == "stt-1" {
                return ("gemini-3.5-transcribe".to_string(), None);
            } else if lower.contains("image") || lower.contains("imagen") || lower.contains("dall-e") {
                if let Some(img) = account
                    .available_models
                    .iter()
                    .find(|m| !m.disabled && (m.modality == "image" || m.id.to_lowercase().contains("image")))
                {
                    return (img.id.clone(), None);
                }
                return ("gemini-3.1-flash-image".to_string(), None);
            }

            // Dynamic resolution against real upstream models parsed for this account
            if !account.available_models.is_empty() {
                let (req_base, _) = crate::config::AppConfig::strip_effort_suffix(&lower);
                let candidates: Vec<&crate::config::AccountModelStatus> = account
                    .available_models
                    .iter()
                    .filter(|m| {
                        !m.disabled
                            && crate::config::AppConfig::strip_effort_suffix(&m.id.to_lowercase()).0
                                == req_base
                    })
                    .collect();

                if !candidates.is_empty() {
                    let find_suffix = |suf: &str| {
                        candidates
                            .iter()
                            .find(|m| m.id.to_lowercase().ends_with(suf))
                            .map(|m| m.id.clone())
                    };
                    let find_exact = |exact: &str| {
                        candidates
                            .iter()
                            .find(|m| m.id.to_lowercase() == exact)
                            .map(|m| m.id.clone())
                    };

                    let chosen = match effective_effort {
                        Some(ReasoningEffort::Off) => find_exact(req_base)
                            .or_else(|| find_suffix("-none"))
                            .or_else(|| find_suffix("-low"))
                            .unwrap_or_else(|| candidates[0].id.clone()),
                        Some(ReasoningEffort::Low) => find_suffix("-low")
                            .or_else(|| find_suffix("-medium"))
                            .or_else(|| find_exact(req_base))
                            .unwrap_or_else(|| candidates[0].id.clone()),
                        Some(ReasoningEffort::Medium) => find_suffix("-medium")
                            .or_else(|| find_suffix("-high"))
                            .or_else(|| find_exact(req_base))
                            .unwrap_or_else(|| candidates[0].id.clone()),
                        Some(ReasoningEffort::High) => find_suffix("-high")
                            .or_else(|| find_suffix("-thinking"))
                            .or_else(|| find_exact(req_base))
                            .unwrap_or_else(|| candidates[0].id.clone()),
                        None => find_exact(&lower)
                            .or_else(|| find_suffix("-medium"))
                            .or_else(|| find_suffix("-thinking"))
                            .unwrap_or_else(|| candidates[0].id.clone()),
                    };
                    return (chosen, effective_effort);
                }
            }

            // Fallback when account.available_models has not been fetched yet
            if lower.contains("opus") {
                if lower.contains("4-6") || lower.contains("4.6") || !lower.contains("5-5") {
                    (
                        "claude-opus-4-6-thinking".to_string(),
                        effective_effort,
                    )
                } else {
                    match effective_effort {
                        Some(ReasoningEffort::Low) => (
                            "claude-opus-5-5-low".to_string(),
                            Some(ReasoningEffort::Low),
                        ),
                        Some(ReasoningEffort::Medium) => (
                            "claude-opus-5-5-medium".to_string(),
                            Some(ReasoningEffort::Medium),
                        ),
                        Some(ReasoningEffort::High) => (
                            "claude-opus-5-5-high".to_string(),
                            Some(ReasoningEffort::High),
                        ),
                        Some(ReasoningEffort::Off) => {
                            ("claude-opus-5-5".to_string(), Some(ReasoningEffort::Off))
                        }
                        None => (
                            "claude-opus-5-5-high".to_string(),
                            Some(ReasoningEffort::High),
                        ),
                    }
                }
            } else if lower.contains("claude") || lower.contains("sonnet") {
                if lower.contains("4-6") || lower.contains("4.6") || !lower.contains("5-5") {
                    (
                        "claude-sonnet-4-6".to_string(),
                        effective_effort.or(Some(ReasoningEffort::High)),
                    )
                } else {
                    match effective_effort {
                        Some(ReasoningEffort::Low) => (
                            "claude-sonnet-5-5-low".to_string(),
                            Some(ReasoningEffort::Low),
                        ),
                        Some(ReasoningEffort::Medium) => (
                            "claude-sonnet-5-5-medium".to_string(),
                            Some(ReasoningEffort::Medium),
                        ),
                        Some(ReasoningEffort::High) => (
                            "claude-sonnet-5-5-high".to_string(),
                            Some(ReasoningEffort::High),
                        ),
                        Some(ReasoningEffort::Off) => {
                            ("claude-sonnet-5-5".to_string(), Some(ReasoningEffort::Off))
                        }
                        None => (
                            "claude-sonnet-5-5-high".to_string(),
                            Some(ReasoningEffort::High),
                        ),
                    }
                }
            } else if lower.contains("gpt-oss") {
                ("gpt-oss-120b-medium".to_string(), effective_effort)
            } else if lower.contains("argon") {
                match effective_effort {
                    Some(ReasoningEffort::Low) => {
                        ("gemini-4-argon-low".to_string(), Some(ReasoningEffort::Low))
                    }
                    Some(ReasoningEffort::Medium) => (
                        "gemini-4-argon-medium".to_string(),
                        Some(ReasoningEffort::Medium),
                    ),
                    Some(ReasoningEffort::High) => (
                        "gemini-4-argon-high".to_string(),
                        Some(ReasoningEffort::High),
                    ),
                    Some(ReasoningEffort::Off) => {
                        ("gemini-4-argon".to_string(), Some(ReasoningEffort::Off))
                    }
                    None => (
                        "gemini-4-argon-medium".to_string(),
                        Some(ReasoningEffort::Medium),
                    ),
                }
            } else if lower.contains("image") || lower.contains("imagen") || lower.contains("dall-e") {
                ("gemini-3.1-flash-image".to_string(), None)
            } else if lower.contains("web-search") || lower.contains("google-search") {
                ("web-search".to_string(), None)
            } else if lower.contains("whisper") || lower.contains("transcrib") || lower == "stt-1" {
                ("gemini-3.5-transcribe".to_string(), None)
            } else if lower == "gemini-3.8-flash-tiered" || lower.contains("tiered") {
                ("gemini-3.8-flash-tiered".to_string(), effective_effort)
            } else if lower == "gemini-2.5-flash"
                || lower == "gemini-2.5-flash-lite"
                || lower == "gemini-2.5-flash-thinking"
                || lower == "gemini-2.5-pro"
                || lower == "gemini-3-flash"
                || lower == "gemini-3.1-flash-lite"
                || lower == "gemini-3.5-flash-lite"
            {
                (lower, effective_effort)
            } else {
                // Standard OpenAI `gemini-3.8-flash` (plus explicit -low/-medium/-high or fallback)
                // Note: bare `gemini-3.8-flash` returns 404 upstream, so Off uses `gemini-3.8-flash-low` + thinkingBudget=0.
                match effective_effort {
                    Some(ReasoningEffort::Low) => (
                        "gemini-3.8-flash-low".to_string(),
                        Some(ReasoningEffort::Low),
                    ),
                    Some(ReasoningEffort::Medium) => (
                        "gemini-3.8-flash-medium".to_string(),
                        Some(ReasoningEffort::Medium),
                    ),
                    Some(ReasoningEffort::High) => (
                        "gemini-3.8-flash-high".to_string(),
                        Some(ReasoningEffort::High),
                    ),
                    Some(ReasoningEffort::Off) => (
                        "gemini-3.8-flash-low".to_string(),
                        Some(ReasoningEffort::Off),
                    ),
                    None => (
                        "gemini-3.8-flash-medium".to_string(),
                        Some(ReasoningEffort::Medium),
                    ),
                }
            }
        }
        "anthropic" => {
            let m = if lower.contains("claude-3-7") || lower.contains("sonnet-3.7") {
                "claude-3-7-sonnet-20250219".to_string()
            } else if lower.contains("claude-3-5-sonnet") || lower.contains("sonnet") {
                "claude-3-5-sonnet-20241022".to_string()
            } else if lower.contains("claude-3-5-haiku") || lower.contains("haiku") {
                "claude-3-5-haiku-20241022".to_string()
            } else if lower.contains("claude-3-opus") || lower.contains("opus") {
                "claude-3-opus-20240229".to_string()
            } else {
                base
            };
            (m, effective_effort)
        }
        "openai" => {
            let m = if lower.contains("gpt-4o-mini") {
                "gpt-4o-mini".to_string()
            } else if lower.contains("gpt-4o") {
                "gpt-4o".to_string()
            } else if lower.contains("o3-mini") {
                "o3-mini".to_string()
            } else if lower.contains("o1") {
                "o1-preview".to_string()
            } else {
                base
            };
            (m, effective_effort)
        }
        _ => (base, effective_effort),
    }
}

pub fn resolve_model(requested_model: &str, account: &UpstreamAccount) -> String {
    let (m, _) = resolve_model_with_effort(requested_model, account, None);
    m
}

pub fn convert_openai_to_claude(
    req: &OpenAIChatRequest,
    target_model: &str,
    effort: Option<ReasoningEffort>,
) -> ClaudeMessageRequest {
    let mut claude_messages: Vec<ClaudeMessage> = Vec::new();
    let mut system_parts: Vec<String> = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" || msg.role == "developer" {
            let txt = match &msg.content {
                Value::String(s) => s.clone(),
                Value::Array(arr) => arr
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                other => other.to_string(),
            };
            if !txt.trim().is_empty() {
                system_parts.push(txt);
            }
        } else if msg.role == "tool" {
            let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
            let content_val = match &msg.content {
                Value::String(s) => Value::String(s.clone()),
                other => Value::String(other.to_string()),
            };
            let tool_res = serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content_val
            });
            if let Some(last) = claude_messages.last_mut() {
                if last.role == "user" {
                    if let Value::Array(ref mut arr) = last.content {
                        arr.push(tool_res);
                        continue;
                    }
                }
            }
            claude_messages.push(ClaudeMessage {
                role: "user".to_string(),
                content: Value::Array(vec![tool_res]),
            });
        } else if msg.role == "assistant" && msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
            let mut blocks: Vec<Value> = Vec::new();
            if let Value::String(s) = &msg.content {
                if !s.is_empty() {
                    blocks.push(serde_json::json!({ "type": "text", "text": s }));
                }
            }
            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("toolu_0");
                    let name = tc.pointer("/function/name").and_then(|v| v.as_str()).unwrap_or("tool");
                    let args_str = tc.pointer("/function/arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                    let input: Value = serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input
                    }));
                }
            }
            claude_messages.push(ClaudeMessage {
                role: "assistant".to_string(),
                content: Value::Array(blocks),
            });
        } else {
            claude_messages.push(ClaudeMessage {
                role: if msg.role == "assistant" {
                    "assistant".to_string()
                } else {
                    "user".to_string()
                },
                content: msg.content.clone(),
            });
        }
    }

    let system_prompt = if system_parts.is_empty() {
        None
    } else {
        Some(Value::String(system_parts.join("\n\n")))
    };

    let is_thinking_capable = target_model.contains("3-7")
        || target_model.contains("claude-3-7")
        || target_model.contains("4-6")
        || target_model.contains("5-5")
        || target_model.contains("thinking");
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

    let budget = thinking_config
        .as_ref()
        .map(|t| t.budget_tokens)
        .unwrap_or(0);
    let requested_max =
        req.max_completion_tokens
            .or(req.max_tokens)
            .unwrap_or(if thinking_config.is_some() {
                16384
            } else {
                8192
            });
    let max_tok = requested_max.max(budget + 4096);

    ClaudeMessageRequest {
        model: target_model.to_string(),
        messages: claude_messages,
        system: system_prompt,
        max_tokens: max_tok,
        stream: req.stream,
        temperature: if thinking_config.is_some() {
            None
        } else {
            req.temperature
        },
        thinking: thinking_config,
    }
}

pub fn convert_claude_response_to_openai(resp: &ClaudeMessageResponse) -> OpenAIChatResponse {
    let mut full_text = String::new();
    let mut thinking_text = None;
    let mut tool_calls: Vec<Value> = Vec::new();

    for block in &resp.content {
        match block {
            ClaudeContentBlock::Text { text } => full_text.push_str(text),
            ClaudeContentBlock::Thinking { thinking, .. } => {
                let entry = thinking_text.get_or_insert_with(String::new);
                entry.push_str(thinking);
            }
            ClaudeContentBlock::ToolUse { id, name, input } => {
                let args_str = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args_str
                    }
                }));
            }
            ClaudeContentBlock::Unknown => {}
        }
    }

    let finish_reason = if !tool_calls.is_empty() {
        Some("tool_calls".to_string())
    } else {
        match resp.stop_reason.as_deref() {
            Some("end_turn") => Some("stop".to_string()),
            Some("max_tokens") => Some("length".to_string()),
            Some("tool_use") => Some("tool_calls".to_string()),
            Some(other) => Some(other.to_string()),
            None => Some("stop".to_string()),
        }
    };

    let cached = resp.usage.cache_read_input_tokens.unwrap_or(0);
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
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
            },
            logprobs: None,
            finish_reason,
        }],
        usage: Some(OpenAIUsage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            prompt_tokens_details: Some(PromptTokensDetails {
                cached_tokens: cached,
                audio_tokens: 0,
            }),
            completion_tokens_details: Some(CompletionTokensDetails {
                reasoning_tokens: 0,
                audio_tokens: 0,
                accepted_prediction_tokens: 0,
                rejected_prediction_tokens: 0,
            }),
            prompt_cache_hit_tokens: if cached > 0 { Some(cached) } else { None },
            prompt_cache_miss_tokens: if cached > 0 { Some(resp.usage.input_tokens.saturating_sub(cached)) } else { None },
        }),
    }
}
