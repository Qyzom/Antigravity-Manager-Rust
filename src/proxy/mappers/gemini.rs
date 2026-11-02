#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(default, rename = "systemInstruction")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(default, rename = "generationConfig")]
    pub generation_config: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiContent {
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiPart {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub thought: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiResponse {
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default, rename = "usageMetadata")]
    pub usage_metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiCandidate {
    pub content: Option<GeminiContent>,
    #[serde(default, rename = "finishReason")]
    pub finish_reason: Option<String>,
}
