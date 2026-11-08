use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_completion_tokens: Option<u32>,
    #[serde(default, alias = "reasoningEffort", alias = "reasoning-effort")]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub stream_options: Option<Value>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: Value, // Can be string or array of multimodal parts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub system_fingerprint: String,
    pub choices: Vec<OpenAIChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChoice {
    pub index: usize,
    pub message: OpenAIMessage,
    #[serde(default)]
    pub logprobs: Option<Value>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_hit_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_miss_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptTokensDetails {
    pub cached_tokens: u64,
    pub audio_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionTokensDetails {
    pub reasoning_tokens: u64,
    pub audio_tokens: u64,
    pub accepted_prediction_tokens: u64,
    pub rejected_prediction_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub system_fingerprint: String,
    pub choices: Vec<OpenAIStreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamChoice {
    pub index: usize,
    pub delta: OpenAIStreamDelta,
    #[serde(default)]
    pub logprobs: Option<Value>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAIStreamDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
}

pub fn create_stream_tool_chunk(id: &str, model: &str, tool_calls: Vec<Value>) -> String {
    let chunk = OpenAIStreamChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: model.to_string(),
        system_fingerprint: "fp_44709d6fcb".to_string(),
        choices: vec![OpenAIStreamChoice {
            index: 0,
            delta: OpenAIStreamDelta {
                role: None,
                content: None,
                reasoning_content: None,
                reasoning: None,
                refusal: None,
                tool_calls: Some(tool_calls),
            },
            logprobs: None,
            finish_reason: None,
        }],
        usage: None,
    };

    let json = serde_json::to_string(&chunk).unwrap_or_default();
    format!("data: {}\n\n", json)
}

pub fn create_stream_chunk(
    id: &str,
    model: &str,
    content: Option<&str>,
    reasoning: Option<&str>,
    finish_reason: Option<&str>,
) -> String {
    create_stream_chunk_with_usage(id, model, content, reasoning, finish_reason, None)
}

pub fn create_stream_chunk_with_usage(
    id: &str,
    model: &str,
    content: Option<&str>,
    reasoning: Option<&str>,
    finish_reason: Option<&str>,
    usage: Option<OpenAIUsage>,
) -> String {
    let is_finish = finish_reason.is_some();
    let chunk = OpenAIStreamChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: model.to_string(),
        system_fingerprint: "fp_44709d6fcb".to_string(),
        choices: vec![OpenAIStreamChoice {
            index: 0,
            delta: OpenAIStreamDelta {
                role: if is_finish || content.is_some() || reasoning.is_some() {
                    None
                } else {
                    Some("assistant".to_string())
                },
                content: content.map(|s| s.to_string()),
                reasoning_content: reasoning.map(|s| s.to_string()),
                reasoning: reasoning.map(|s| s.to_string()),
                refusal: None,
                tool_calls: None,
            },
            logprobs: None,
            finish_reason: finish_reason.map(|s| s.to_string()),
        }],
        usage,
    };

    let json = serde_json::to_string(&chunk).unwrap_or_default();
    format!("data: {}\n\n", json)
}

pub fn create_stream_usage_chunk(id: &str, model: &str, mut usage: OpenAIUsage) -> String {
    let cached = usage
        .prompt_tokens_details
        .as_ref()
        .map(|d| d.cached_tokens)
        .unwrap_or(0);
    if usage.prompt_tokens_details.is_none() {
        usage.prompt_tokens_details = Some(PromptTokensDetails {
            cached_tokens: 0,
            audio_tokens: 0,
        });
    }
    if usage.prompt_cache_hit_tokens.is_none() && cached > 0 {
        usage.prompt_cache_hit_tokens = Some(cached);
    }
    if usage.prompt_cache_miss_tokens.is_none() && cached > 0 {
        usage.prompt_cache_miss_tokens = Some(usage.prompt_tokens.saturating_sub(cached));
    }
    if usage.completion_tokens_details.is_none() {
        usage.completion_tokens_details = Some(CompletionTokensDetails {
            reasoning_tokens: 0,
            audio_tokens: 0,
            accepted_prediction_tokens: 0,
            rejected_prediction_tokens: 0,
        });
    }
    let chunk = OpenAIStreamChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: model.to_string(),
        system_fingerprint: "fp_44709d6fcb".to_string(),
        choices: vec![],
        usage: Some(usage),
    };

    let json = serde_json::to_string(&chunk).unwrap_or_default();
    format!("data: {}\n\n", json)
}
