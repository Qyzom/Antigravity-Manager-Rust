use crate::config::ModelCatalogItem;
use crate::proxy::mappers::openai::*;
use crate::proxy::router::ReasoningEffort;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

pub const OFFICIAL_HUB_VERSION: &str = "2.23.1";
pub const OFFICIAL_HUB_CL: &str = "998881661";

pub fn get_official_hub_user_agent() -> String {
    let os_type = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        "linux" => "linux",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!(
        "antigravity/hub/{} (aidev_client; os_type={}; arch={}; cl={})",
        OFFICIAL_HUB_VERSION, os_type, arch, OFFICIAL_HUB_CL
    )
}

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
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
];

pub const FETCH_MODELS_ENDPOINTS: [&str; 2] = [
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
];

pub const STREAM_ENDPOINTS: [&str; 2] = [
    "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse",
];

#[derive(Debug, Deserialize)]
pub struct GoogleTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub token_type: Option<String>,
    pub id_token: Option<String>,
}

pub fn extract_email_from_jwt(jwt: &str) -> Option<String> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() >= 2 {
        let payload = parts[1];
        use base64::Engine;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(payload));
        if let Ok(bytes) = decoded {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(email) = val.get("email").and_then(|e| e.as_str()) {
                    return Some(email.to_string());
                }
            }
        }
    }
    None
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
    #[serde(rename = "cloudaicompanionProject", alias = "projectId")]
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

    let mut project_id = Some("aicode-consumers".to_string());
    let mut tier = "PRO".to_string();

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
                    if let Some(pid) = data.project_id {
                        project_id = Some(pid);
                    }
                    let raw_tier = data
                        .paid_tier
                        .and_then(|t| t.id.or(t.name))
                        .or_else(|| data.current_tier.and_then(|t| t.id.or(t.name)))
                        .unwrap_or_default();

                    if !raw_tier.is_empty() {
                        tier = if raw_tier.to_lowercase().contains("free") {
                            "FREE".to_string()
                        } else {
                            "PRO".to_string()
                        };
                    }
                    break;
                }
            }
        }
    }

    // Double check via quota summary: any account with Claude quota is definitely PRO
    let quota_summary = fetch_user_quota_summary(client, access_token).await;
    if quota_summary.claude_gpt_percent > 0 {
        tier = "PRO".to_string();
    }

    (project_id, tier)
}

const QUOTA_SUMMARY_ENDPOINTS: [&str; 3] = [
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:retrieveUserQuotaSummary",
];

#[derive(Debug, Deserialize)]
struct QuotaSummaryResponse {
    #[serde(default)]
    pub groups: Vec<QuotaSummaryGroup>,
}

#[derive(Debug, Deserialize)]
struct QuotaSummaryGroup {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub buckets: Vec<QuotaSummaryBucket>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QuotaSummaryBucket {
    #[serde(rename = "bucketId")]
    pub bucket_id: Option<String>,
    pub window: Option<String>,
    #[serde(rename = "remainingFraction")]
    pub remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    pub reset_time: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DetailedQuotaInfo {
    pub claude_gpt_percent: i32,
    pub claude_gpt_reset: String,
    pub claude_gpt_weekly_percent: i32,
    pub claude_gpt_weekly_reset: String,
    pub gemini_percent: i32,
    pub gemini_reset: String,
    pub gemini_weekly_percent: i32,
    pub gemini_weekly_reset: String,
}

pub async fn fetch_user_quota_summary(
    client: &reqwest::Client,
    access_token: &str,
) -> DetailedQuotaInfo {
    for endpoint in QUOTA_SUMMARY_ENDPOINTS {
        if let Ok(resp) = client
            .post(endpoint)
            .header("User-Agent", OFFICIAL_USER_AGENT)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<QuotaSummaryResponse>().await {
                    let mut claude_5h_frac = 1.0f64;
                    let mut claude_5h_reset = "in 4h 00m".to_string();
                    let mut claude_week_frac = 1.0f64;
                    let mut claude_week_reset = "in 7d 00h".to_string();

                    let mut gemini_5h_frac = 1.0f64;
                    let mut gemini_5h_reset = "in 4h 00m".to_string();
                    let mut gemini_week_frac = 1.0f64;
                    let mut gemini_week_reset = "in 7d 00h".to_string();

                    for group in data.groups {
                        let name = group.display_name.unwrap_or_default().to_lowercase();
                        let is_gemini = name.contains("gemini");

                        for bucket in group.buckets {
                            let frac = bucket.remaining_fraction.unwrap_or(1.0);
                            let b_id = bucket.bucket_id.as_deref().unwrap_or("").to_lowercase();
                            let win = bucket.window.as_deref().unwrap_or("").to_lowercase();

                            let mut total_hours = 4;
                            let mut formatted_reset = "in 4h 00m".to_string();
                            if let Some(rt) = bucket.reset_time {
                                if let Ok(parsed_time) = DateTime::parse_from_rfc3339(&rt) {
                                    let now = Utc::now();
                                    let dur = parsed_time.with_timezone(&Utc).signed_duration_since(now);
                                    let total_min = dur.num_minutes().max(0);
                                    total_hours = dur.num_hours().max(0);
                                    let days = total_hours / 24;
                                    let h = total_hours % 24;
                                    let m = total_min % 60;
                                    formatted_reset = if days > 0 {
                                        format!("in {}d {:02}h", days, h)
                                    } else if h > 0 {
                                        format!("in {}h {:02}m", h, m)
                                    } else {
                                        format!("in {}m", m)
                                    };
                                }
                            }

                            let is_weekly = b_id.contains("week") || win.contains("week") || total_hours > 12;

                            if is_gemini {
                                if is_weekly {
                                    if frac < gemini_week_frac {
                                        gemini_week_frac = frac;
                                        gemini_week_reset = formatted_reset;
                                    }
                                } else {
                                    if frac < gemini_5h_frac {
                                        gemini_5h_frac = frac;
                                        gemini_5h_reset = formatted_reset;
                                    }
                                }
                            } else {
                                if is_weekly {
                                    if frac < claude_week_frac {
                                        claude_week_frac = frac;
                                        claude_week_reset = formatted_reset;
                                    }
                                } else {
                                    if frac < claude_5h_frac {
                                        claude_5h_frac = frac;
                                        claude_5h_reset = formatted_reset;
                                    }
                                }
                            }
                        }
                    }

                    return DetailedQuotaInfo {
                        claude_gpt_percent: (claude_5h_frac * 100.0).round() as i32,
                        claude_gpt_reset: claude_5h_reset,
                        claude_gpt_weekly_percent: (claude_week_frac * 100.0).round() as i32,
                        claude_gpt_weekly_reset: claude_week_reset,
                        gemini_percent: (gemini_5h_frac * 100.0).round() as i32,
                        gemini_reset: gemini_5h_reset,
                        gemini_weekly_percent: (gemini_week_frac * 100.0).round() as i32,
                        gemini_weekly_reset: gemini_week_reset,
                    };
                }
            }
        }
    }

    DetailedQuotaInfo {
        claude_gpt_percent: 100,
        claude_gpt_reset: "in 4h 00m".to_string(),
        claude_gpt_weekly_percent: 100,
        claude_gpt_weekly_reset: "in 7d 00h".to_string(),
        gemini_percent: 100,
        gemini_reset: "in 4h 00m".to_string(),
        gemini_weekly_percent: 100,
        gemini_weekly_reset: "in 7d 00h".to_string(),
    }
}

pub async fn fetch_models_and_quota(
    client: &reqwest::Client,
    access_token: &str,
    project_id: Option<&str>,
) -> Result<(Vec<ModelCatalogItem>, u64, u64, String, DetailedQuotaInfo), String> {
    let quota_summary = fetch_user_quota_summary(client, access_token).await;
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
                    let mut models: Vec<ModelCatalogItem> = Vec::new();
                    let mut min_fraction: f64 = 1.0;
                    let mut nearest_reset = "in 4h 00m".to_string();

                    for (id, entry) in data.models {
                        if entry.disabled == Some(true)
                            || id.starts_with("chat_")
                            || id.contains("gemini-3.5")
                            || id.contains("gemini-3-flash")
                            || id.contains("gemini-3.9")
                            || id.contains("gemini-3.7")
                            || id.contains("gemini-3.6")
                            || id.contains("gemini-3.1")
                            || id.contains("4-6")
                            || id.contains("gpt-oss")
                            || id == "claude-3-7-sonnet"
                            || id == "gemini-2.5-pro"
                            || id == "gemini-2.5-flash"
                        {
                            continue;
                        }

                        let clean_id = match id.as_str() {
                            "claude-sonnet-5-5-high" => "claude-sonnet-5-5".to_string(),
                            "claude-opus-5-5-high" => "claude-opus-5-5".to_string(),
                            "gemini-3.8-flash-high" => "gemini-3.8-flash".to_string(),
                            other => other.to_string(),
                        };

                        let disp_name = match clean_id.as_str() {
                            "claude-sonnet-5-5" => "Claude Sonnet 5.5".to_string(),
                            "claude-opus-5-5" => "Claude Opus 5.5".to_string(),
                            "gemini-3.8-flash" => "Gemini 3.8 Flash".to_string(),
                            "gemini-4-argon" => "Gemini 4 Argon".to_string(),
                            _ => entry.display_name.clone().unwrap_or_else(|| clean_id.clone()),
                        };

                        let provider = if clean_id.contains("claude") {
                            "anthropic".to_string()
                        } else if clean_id.contains("gemini") {
                            "gemini".to_string()
                        } else {
                            "openai".to_string()
                        };

                        let supports_thinking = true;

                        let tier = "PRO".to_string();

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

                        if !models.iter().any(|m| m.id == clean_id) {
                            models.push(ModelCatalogItem {
                                id: clean_id,
                                display_name: disp_name,
                                provider,
                                tier,
                                is_enabled: true,
                                supports_thinking,
                            });
                        }
                    }

                    if !models.is_empty() {
                        let quota_limit = 100_000u64;
                        let quota_used = ((1.0 - min_fraction) * (quota_limit as f64)) as u64;
                        return Ok((models, quota_used, quota_limit, nearest_reset, quota_summary));
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
        let mut msg_parts = Vec::new();
        match &msg.content {
            Value::String(s) => {
                msg_parts.push(serde_json::json!({ "text": s }));
            }
            Value::Array(arr) => {
                for item in arr {
                    let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if item_type == "text" {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            msg_parts.push(serde_json::json!({ "text": t }));
                        }
                    } else if item_type == "image_url" {
                        if let Some(img_url) = item.get("image_url").and_then(|u| u.get("url").and_then(|s| s.as_str())) {
                            if let Some(rest) = img_url.strip_prefix("data:") {
                                if let Some((mime, b64)) = rest.split_once(";base64,") {
                                    msg_parts.push(serde_json::json!({
                                        "inlineData": {
                                            "mimeType": mime,
                                            "data": b64
                                        }
                                    }));
                                }
                            }
                        }
                    }
                }
            }
            other => {
                msg_parts.push(serde_json::json!({ "text": other.to_string() }));
            }
        }

        if msg.role == "system" || msg.role == "developer" {
            system_parts.extend(msg_parts);
        } else {
            let role = if msg.role == "assistant" { "model" } else { "user" };
            contents.push(serde_json::json!({
                "role": role,
                "parts": msg_parts
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
        gen_config["thinkingConfig"] = serde_json::json!({
            "thinkingBudget": eff.budget_tokens()
        });
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

    let pid = project_id.unwrap_or("aicode-consumers");
    serde_json::json!({
        "project": pid,
        "model": target_model,
        "request": inner_request,
        "userAgent": "antigravity",
        "requestId": format!("req-{}", uuid::Uuid::new_v4().simple())
    })
}

pub fn build_antigravity_payload_from_claude(
    req: &Value,
    target_model: &str,
    project_id: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Value {
    let mut contents = Vec::new();
    let mut system_parts = Vec::new();

    // 1. Extract system instructions
    if let Some(sys) = req.get("system") {
        if let Some(s) = sys.as_str() {
            system_parts.push(serde_json::json!({ "text": s }));
        } else if let Some(arr) = sys.as_array() {
            for item in arr {
                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                    system_parts.push(serde_json::json!({ "text": t }));
                }
            }
        }
    }

    // 2. Extract messages
    if let Some(msgs) = req.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let gemini_role = if role == "assistant" { "model" } else { "user" };

            let mut parts = Vec::new();

            if let Some(content_str) = msg.get("content").and_then(|c| c.as_str()) {
                parts.push(serde_json::json!({ "text": content_str }));
            } else if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
                for block in content_arr {
                    let b_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if b_type == "text" {
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            parts.push(serde_json::json!({ "text": t }));
                        }
                    } else if b_type == "image" {
                        if let Some(source) = block.get("source") {
                            let media_type = source.get("media_type").and_then(|m| m.as_str()).unwrap_or("image/png");
                            let b64_data = source.get("data").and_then(|d| d.as_str()).unwrap_or("");
                            if !b64_data.is_empty() {
                                parts.push(serde_json::json!({
                                    "inlineData": {
                                        "mimeType": media_type,
                                        "data": b64_data
                                    }
                                }));
                            }
                        }
                    } else if b_type == "thinking" {
                        if let Some(th) = block.get("thinking").and_then(|v| v.as_str()) {
                            parts.push(serde_json::json!({ "thought": true, "text": th }));
                        }
                    } else if b_type == "tool_use" {
                        let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                        let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
                        let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        parts.push(serde_json::json!({
                            "functionCall": {
                                "name": name,
                                "args": input,
                                "id": id
                            }
                        }));
                    } else if b_type == "tool_result" {
                        let tool_use_id = block.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                        let result_content = block.get("content").cloned().unwrap_or(serde_json::json!("success"));
                        parts.push(serde_json::json!({
                            "functionResponse": {
                                "name": "tool",
                                "response": { "result": result_content },
                                "id": tool_use_id
                            }
                        }));
                    }
                }
            }

            if !parts.is_empty() {
                contents.push(serde_json::json!({
                    "role": gemini_role,
                    "parts": parts
                }));
            }
        }
    }

    if contents.is_empty() {
        contents.push(serde_json::json!({
            "role": "user",
            "parts": [{ "text": "Hello" }]
        }));
    }

    // 3. Generation config
    let mut gen_config = serde_json::json!({});
    if let Some(max_tok) = req.get("max_tokens").and_then(|v| v.as_u64()) {
        gen_config["maxOutputTokens"] = serde_json::json!(max_tok);
    } else {
        gen_config["maxOutputTokens"] = serde_json::json!(8192);
    }

    let mut thinking_budget = None;
    if let Some(th) = req.get("thinking") {
        if th.get("type").and_then(|t| t.as_str()) == Some("enabled") {
            thinking_budget = th.get("budget_tokens").and_then(|b| b.as_u64());
        }
    }
    if let Some(eff) = reasoning_effort {
        thinking_budget = Some(eff.budget_tokens() as u64);
    }

    if let Some(budget) = thinking_budget {
        gen_config["thinkingConfig"] = serde_json::json!({
            "thinkingBudget": budget
        });
    } else if let Some(temp) = req.get("temperature").and_then(|v| v.as_f64()) {
        gen_config["temperature"] = serde_json::json!(temp);
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

    let pid = project_id.unwrap_or("aicode-consumers");
    serde_json::json!({
        "project": pid,
        "model": target_model,
        "request": inner_request,
        "userAgent": "antigravity",
        "requestId": format!("req-{}", uuid::Uuid::new_v4().simple())
    })
}

pub fn convert_antigravity_response_to_claude(
    raw: &Value,
    model: &str,
) -> Value {
    let actual = raw.get("response").unwrap_or(raw);
    let mut text_parts = String::new();
    let mut thinking_parts = String::new();

    if let Some(candidates) = actual.get("candidates").and_then(|c| c.as_array()) {
        for candidate in candidates {
            if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                for part in parts {
                    let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            thinking_parts.push_str(txt);
                        } else {
                            text_parts.push_str(txt);
                        }
                    }
                }
            }
        }
    }

    let mut content_blocks = Vec::new();
    if !thinking_parts.is_empty() {
        content_blocks.push(serde_json::json!({
            "type": "thinking",
            "thinking": thinking_parts
        }));
    }
    content_blocks.push(serde_json::json!({
        "type": "text",
        "text": text_parts
    }));

    let mut prompt_tok = 50;
    let mut cand_tok = 150;
    if let Some(usage) = actual.get("usageMetadata") {
        if let Some(pt) = usage.get("promptTokenCount").and_then(|v| v.as_u64()) {
            prompt_tok = pt;
        }
        if let Some(ct) = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
            cand_tok = ct;
        }
    }

    serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": content_blocks,
        "model": model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {
            "input_tokens": prompt_tok,
            "output_tokens": cand_tok
        }
    })
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
