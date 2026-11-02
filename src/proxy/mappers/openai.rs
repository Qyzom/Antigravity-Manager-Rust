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
    pub stream_options: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: Value, // Can be string or array of multimodal parts
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    #[serde(default)]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChoice {
    pub index: usize,
    pub message: OpenAIMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIStreamChoice>,
    #[serde(default)]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIStreamChoice {
    pub index: usize,
    pub delta: OpenAIStreamDelta,
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
}

pub fn create_stream_chunk(
    id: &str,
    model: &str,
    content: Option<&str>,
    reasoning: Option<&str>,
    finish_reason: Option<&str>,
) -> String {
    let chunk = OpenAIStreamChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: model.to_string(),
        choices: vec![OpenAIStreamChoice {
            index: 0,
            delta: OpenAIStreamDelta {
                role: if content.is_some() || reasoning.is_some() { None } else { Some("assistant".to_string()) },
                content: content.map(|s| s.to_string()),
                reasoning_content: reasoning.map(|s| s.to_string()),
            },
            finish_reason: finish_reason.map(|s| s.to_string()),
        }],
        usage: None,
    };

    let json = serde_json::to_string(&chunk).unwrap_or_default();
    format!("data: {}\n\n", json)
}
