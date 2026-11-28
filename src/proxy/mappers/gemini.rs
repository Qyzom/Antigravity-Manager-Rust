use serde_json::{json, Value};

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
    let pid = project_id.filter(|p| !p.trim().is_empty()).unwrap_or("aicode-consumers");
    let ts = chrono::Utc::now().timestamp_millis();
    let conv = &uuid::Uuid::new_v4().simple().to_string()[..16];
    let traj = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let request_id = format!("agent/{}/{}/{}/1", conv, ts, traj);

    json!({
        "project": pid,
        "requestId": request_id,
        "request": req_obj,
        "model": upstream_model,
        "userAgent": "antigravity",
        "requestType": "agent",
    })
}

/// Convert Gemini models list from AMR internal models
pub fn build_gemini_models_list(
    models: &[crate::config::ModelCatalogItem],
    hide_unavailable: bool,
) -> Value {
    let list: Vec<Value> = models
        .iter()
        .filter(|m| {
            if !m.is_enabled || m.availability_status == "disabled" {
                return false;
            }
            if hide_unavailable
                && (m.availability_status == "exhausted"
                    || (m.quota_percent <= 0 && m.available_accounts_count == 0))
            {
                return false;
            }
            true
        })
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
