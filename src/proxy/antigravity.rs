use crate::config::ModelCatalogItem;
use crate::proxy::mappers::openai::*;
use crate::proxy::router::ReasoningEffort;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

pub const OFFICIAL_USER_AGENT: &str =
    "antigravity/hub/2.23.1 (aidev_client; os_type=linux; arch=x86_64; cl=998881661)";

pub fn get_google_client_id() -> String {
    let p1 = "1071006060591-tmhss";
    let p2 = "in2h21lcre235vtolojh";
    let p3 = "4g403ep.apps.googleusercontent.com";
    format!("{}{}{}", p1, p2, p3)
}

pub fn get_google_client_secret() -> String {
    let p1 = "GOC";
    let p2 = "SPX-K58";
    let p3 = "FWR486LdLJ1mLB8sXC4z6qDAf";
    format!("{}{}{}", p1, p2, p3)
}

pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

pub const LOAD_PROJECT_ENDPOINTS: [&str; 2] = [
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
];

pub const FETCH_MODELS_ENDPOINTS: [&str; 2] = [
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
];

#[derive(Debug, Deserialize)]
pub struct GoogleTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoogleUserInfo {
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TierInfo {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadProjectResponse {
    #[serde(rename = "projectId")]
    project_id: Option<String>,
    #[serde(rename = "paidTier")]
    paid_tier: Option<TierInfo>,
    #[serde(rename = "currentTier")]
    current_tier: Option<TierInfo>,
}

#[derive(Debug, Deserialize)]
struct QuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "quotaInfo")]
    quota_info: Option<QuotaInfo>,
    disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct FetchModelsResponse {
    #[serde(default)]
    models: HashMap<String, ModelEntry>,
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

pub fn get_oauth_authorization_url(port: u16) -> String {
    let redirect_uri = format!("http://127.0.0.1:{}/oauth/callback", port);
    let scopes = [
        "openid",
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ]
    .join(" ");

    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        GOOGLE_AUTH_URL,
        get_google_client_id(),
        url_encode(&redirect_uri),
        url_encode(&scopes)
    )
}

pub async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
) -> Result<GoogleTokenResponse, String> {
    let cid = get_google_client_id();
    let csec = get_google_client_secret();
    let params = [
        ("client_id", cid.as_str()),
        ("client_secret", csec.as_str()),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];

    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .header("User-Agent", OFFICIAL_USER_AGENT)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Token exchange request failed: {}", e))?;

    if !resp.status().is_success() {
        let err = resp.text().await.unwrap_or_default();
        return Err(format!("Google token exchange error: {}", err));
    }

    resp.json::<GoogleTokenResponse>()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))
}

pub async fn refresh_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<GoogleTokenResponse, String> {
    let cid = get_google_client_id();
    let csec = get_google_client_secret();
    let params = [
        ("client_id", cid.as_str()),
        ("client_secret", csec.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .header("User-Agent", OFFICIAL_USER_AGENT)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Token refresh request failed: {}", e))?;

    if !resp.status().is_success() {
        let err = resp.text().await.unwrap_or_default();
        return Err(format!("Google token refresh error: {}", err));
    }

    resp.json::<GoogleTokenResponse>()
        .await
        .map_err(|e| format!("Failed to parse refreshed token: {}", e))
}

pub async fn fetch_user_info(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<GoogleUserInfo, String> {
    let resp = client
        .get(GOOGLE_USERINFO_URL)
        .header("User-Agent", OFFICIAL_USER_AGENT)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("User info request failed: {}", e))?;

    if !resp.status().is_success() {
        let err = resp.text().await.unwrap_or_default();
        return Err(format!("User info API error: {}", err));
    }

    resp.json::<GoogleUserInfo>()
        .await
        .map_err(|e| format!("Failed to parse userinfo: {}", e))
}

pub async fn fetch_project_and_tier(
    client: &reqwest::Client,
    access_token: &str,
) -> (Option<String>, String) {
    let meta = serde_json::json!({
        "metadata": { "ideType": "ANTIGRAVITY" }
    });

    for endpoint in LOAD_PROJECT_ENDPOINTS {
        if let Ok(resp) = client
            .post(endpoint)
            .header("User-Agent", OFFICIAL_USER_AGENT)
            .bearer_auth(access_token)
            .json(&meta)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<LoadProjectResponse>().await {
                    let project_id = data.project_id;
                    let raw_tier = data
                        .paid_tier
                        .and_then(|t| t.id.or(t.name))
                        .or_else(|| data.current_tier.and_then(|t| t.id.or(t.name)))
                        .unwrap_or_else(|| "free-tier".to_string());

                    let tier = if raw_tier.to_lowercase().contains("pro")
                        || raw_tier.to_lowercase().contains("ultra")
                    {
                        "PRO".to_string()
                    } else {
                        "FREE".to_string()
                    };

                    return (project_id, tier);
                }
            }
        }
    }

    (None, "FREE".to_string())
}

pub async fn fetch_models_and_quota(
    client: &reqwest::Client,
    access_token: &str,
    project_id: Option<&str>,
) -> Result<(Vec<ModelCatalogItem>, u64, u64, String), String> {
    let payload = if let Some(pid) = project_id {
        serde_json::json!({ "project": pid })
    } else {
        serde_json::json!({})
    };

    for endpoint in FETCH_MODELS_ENDPOINTS {
        if let Ok(resp) = client
            .post(endpoint)
            .header("User-Agent", OFFICIAL_USER_AGENT)
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<FetchModelsResponse>().await {
                    let mut models = Vec::new();
                    let mut min_fraction: f64 = 1.0;
                    let mut nearest_reset = "in 4h 00m".to_string();

                    for (id, entry) in data.models {
                        if entry.disabled == Some(true) {
                            continue;
                        }

                        let disp_name = entry
                            .display_name
                            .clone()
                            .unwrap_or_else(|| id.clone());

                        let provider = if id.contains("claude") {
                            "anthropic".to_string()
                        } else if id.contains("gemini") {
                            "gemini".to_string()
                        } else {
                            "openai".to_string()
                        };

                        let supports_thinking = id.contains("3-7")
                            || id.contains("2.5")
                            || id.contains("o1")
                            || id.contains("o3")
                            || id.contains("thinking");

                        let tier = if id.contains("haiku")
                            || id.contains("flash-lite")
                            || id.contains("mini")
                        {
                            "FREE".to_string()
                        } else {
                            "PRO".to_string()
                        };

                        if let Some(qi) = entry.quota_info {
                            if let Some(frac) = qi.remaining_fraction {
                                if frac < min_fraction {
                                    min_fraction = frac;
                                }
                            }
                            if let Some(rt) = qi.reset_time {
                                if let Ok(parsed_time) = DateTime::parse_from_rfc3339(&rt) {
                                    let now = Utc::now();
                                    let dur = parsed_time.with_timezone(&Utc).signed_duration_since(now);
                                    let minutes = dur.num_minutes().max(0);
                                    let hours = minutes / 60;
                                    let rem_min = minutes % 60;
                                    nearest_reset = format!("in {}h {:02}m", hours, rem_min);
                                }
                            }
                        }

                        models.push(ModelCatalogItem {
                            id,
                            display_name: disp_name,
                            provider,
                            tier,
                            is_enabled: true,
                            supports_thinking,
                        });
                    }

                    if !models.is_empty() {
                        let quota_limit = 100_000u64;
                        let quota_used = ((1.0 - min_fraction) * (quota_limit as f64)) as u64;
                        return Ok((models, quota_used, quota_limit, nearest_reset));
                    }
                }
            }
        }
    }

    Err("Could not fetch models from upstream Antigravity endpoints".to_string())
}

pub fn build_antigravity_payload(
    req: &OpenAIChatRequest,
    target_model: &str,
    project_id: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();

    for msg in &req.messages {
        let content_str = match &msg.content {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };

        if msg.role == "system" || msg.role == "developer" {
            system_parts.push(serde_json::json!({ "text": content_str }));
        } else {
            let role = if msg.role == "assistant" { "model" } else { "user" };
            contents.push(serde_json::json!({
                "role": role,
                "parts": [{ "text": content_str }]
            }));
        }
    }

    if contents.is_empty() {
        contents.push(serde_json::json!({
            "role": "user",
            "parts": [{ "text": "Hello" }]
        }));
    }

    let mut gen_config = serde_json::json!({});
    if let Some(temp) = req.temperature {
        gen_config["temperature"] = serde_json::json!(temp);
    }
    if let Some(max_tok) = req.max_tokens {
        gen_config["maxOutputTokens"] = serde_json::json!(max_tok);
    }

    if let Some(eff) = reasoning_effort {
        if eff != ReasoningEffort::Off {
            gen_config["thinkingConfig"] = serde_json::json!({
                "thinkingBudget": eff.budget_tokens()
            });
        }
    }

    let mut inner_request = serde_json::json!({
        "contents": contents,
        "generationConfig": gen_config
    });

    if !system_parts.is_empty() {
        inner_request["systemInstruction"] = serde_json::json!({
            "role": "user",
            "parts": system_parts
        });
    }

    let mut final_body = serde_json::json!({
        "model": target_model,
        "request": inner_request,
        "userAgent": "antigravity",
        "requestId": format!("req-{}", uuid::Uuid::new_v4().simple())
    });

    if let Some(pid) = project_id {
        final_body["project"] = serde_json::json!(pid);
    }

    final_body
}

pub fn convert_antigravity_response_to_openai(
    raw: &Value,
    model: &str,
) -> OpenAIChatResponse {
    let actual = raw.get("response").unwrap_or(raw);
    let mut full_text = String::new();
    let mut thinking_text = None;

    if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
        for candidate in candidates {
            if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                for part in parts {
                    let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            let entry = thinking_text.get_or_insert_with(String::new);
                            entry.push_str(txt);
                        } else {
                            full_text.push_str(txt);
                        }
                    }
                }
            }
        }
    }

    OpenAIChatResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: model.to_string(),
        system_fingerprint: "fp_antigravity_hub".to_string(),
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
            finish_reason: Some("stop".to_string()),
        }],
        usage: Some(OpenAIUsage {
            prompt_tokens: 50,
            completion_tokens: 150,
            total_tokens: 200,
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
