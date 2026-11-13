#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiRequest {
    #[serde(default)]
    pub contents: Vec<GeminiContent>,
    #[serde(default, rename = "systemInstruction")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(default, rename = "generationConfig")]
    pub generation_config: Option<Value>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default, rename = "safetySettings")]
    pub safety_settings: Option<Vec<Value>>,
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
    #[serde(default, rename = "inlineData")]
    pub inline_data: Option<Value>,
    #[serde(default, rename = "functionCall")]
    pub function_call: Option<Value>,
    #[serde(default, rename = "functionResponse")]
    pub function_response: Option<Value>,
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

/// Convert incoming Google Gemini native request into Antigravity internal payload
pub fn build_antigravity_from_gemini(
    req_json: &Value,
    target_model: &str,
    project_id: Option<&str>,
) -> Value {
    let mut req_obj = req_json.clone();

    // Ensure contents exists as array
    if req_obj.get("contents").is_none() {
        req_obj["contents"] = json!([]);
    }

    let upstream_model = crate::proxy::antigravity::normalize_antigravity_upstream_model(target_model);

    json!({
        "project": project_id.unwrap_or(""),
        "model": upstream_model,
        "request": req_obj
    })
}

/// Convert Gemini models list from AMR internal models
pub fn build_gemini_models_list(models: &[crate::config::ModelCatalogItem]) -> Value {
    let list: Vec<Value> = models
        .iter()
        .filter(|m| m.is_enabled && m.availability_status != "disabled")
        .map(|m| {
            let model_name = format!("models/{}", m.id);
            json!({
                "name": model_name,
                "version": "001",
                "displayName": m.display_name,
                "description": format!("{} hosted via AMR Antigravity Engine", m.display_name),
                "inputTokenLimit": m.max_tokens,
                "outputTokenLimit": m.max_output_tokens,
                "supportedGenerationMethods": ["generateContent", "streamGenerateContent", "countTokens"],
                "temperature": 0.7,
                "topP": 0.95,
                "topK": 40
            })
        })
        .collect();

    json!({
        "models": list
    })
}
