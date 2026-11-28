use crate::config::AppConfig;
use serde_json::{Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorSyncStatus {
    Synced,
    Outdated,
    NotSynced,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ConnectorDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
}

pub const CONNECTORS: &[ConnectorDescriptor] = &[
    ConnectorDescriptor {
        id: "opencode_api",
        name: "OpenCode — API",
        category: "API",
    },
    ConnectorDescriptor {
        id: "opencode_mcp",
        name: "OpenCode — MCP",
        category: "MCP",
    },
    ConnectorDescriptor {
        id: "claude_code_api",
        name: "Claude Code — API",
        category: "API",
    },
    ConnectorDescriptor {
        id: "claude_code_mcp",
        name: "Claude Code — MCP",
        category: "MCP",
    },
    ConnectorDescriptor {
        id: "codex_api",
        name: "Codex CLI — API",
        category: "API",
    },
    ConnectorDescriptor {
        id: "codex_mcp",
        name: "Codex CLI — MCP",
        category: "MCP",
    },
    ConnectorDescriptor {
        id: "gemini_cli_api",
        name: "Gemini CLI — API",
        category: "API",
    },
    ConnectorDescriptor {
        id: "gemini_cli_mcp",
        name: "Gemini CLI — MCP",
        category: "MCP",
    },
    ConnectorDescriptor {
        id: "deepseek_harness",
        name: "DeepSeek Harness — API",
        category: "API",
    },
    ConnectorDescriptor {
        id: "cursor",
        name: "Cursor — MCP",
        category: "MCP",
    },
    ConnectorDescriptor {
        id: "cline",
        name: "Cline / Roo Code",
        category: "API & MCP",
    },
];

pub fn migrate_legacy_connector_ids(config: &mut AppConfig) -> bool {
    let mut changed = false;
    let mut next = Vec::new();
    for id in &config.enabled_connectors {
        match id.as_str() {
            "opencode" => {
                next.push("opencode_api".to_string());
                next.push("opencode_mcp".to_string());
                changed = true;
            }
            "claude_code" => {
                next.push("claude_code_api".to_string());
                next.push("claude_code_mcp".to_string());
                changed = true;
            }
            "codex" => {
                next.push("codex_api".to_string());
                next.push("codex_mcp".to_string());
                changed = true;
            }
            "gemini_cli" => {
                next.push("gemini_cli_api".to_string());
                next.push("gemini_cli_mcp".to_string());
                changed = true;
            }
            _ => next.push(id.clone()),
        }
    }
    if changed {
        next.sort();
        next.dedup();
        config.enabled_connectors = next;
    }
    changed
}

fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

pub fn connector_config_path(id: &str) -> Option<PathBuf> {
    let home = home_dir()?;
    match id {
        "opencode_api" | "opencode_mcp" => {
            #[cfg(windows)]
            {
                if let Some(appdata) = dirs::config_dir() {
                    return Some(appdata.join("opencode").join("opencode.json"));
                }
            }
            Some(home.join(".config").join("opencode").join("opencode.json"))
        }
        "claude_code_api" => Some(home.join(".claude").join("settings.json")),
        "claude_code_mcp" => Some(home.join(".claude.json")),
        "codex_api" | "codex_mcp" => Some(home.join(".codex").join("config.toml")),
        "gemini_cli_api" => Some(home.join(".gemini").join(".env")),
        "gemini_cli_mcp" => Some(home.join(".gemini").join("settings.json")),
        "deepseek_harness" => Some(home.join(".deepseek").join("config.json")),
        "cursor" => Some(home.join(".cursor").join("mcp.json")),
        "cline" => Some(home.join(".amr").join("connectors").join("cline-amr.json")),
        _ => None,
    }
}

fn is_non_chat_model(id: &str) -> bool {
    let lower = id.to_lowercase();
    lower.contains("transcribe")
        || lower.contains("image")
        || lower.contains("stt")
        || lower.contains("tts")
}

pub fn is_model_unavailable(m: &crate::config::ModelCatalogItem) -> bool {
    m.availability_status == "disabled"
        || m.availability_status == "exhausted"
        || m.quota_percent <= 0
        || m.available_accounts_count == 0
}

fn model_has_xhigh_variant(config: &AppConfig, model_id: &str) -> bool {
    let base = model_id.trim_end_matches("-disabled");
    config.accounts.iter().filter(|a| a.is_active).any(|acc| {
        acc.available_models.iter().any(|am| {
            let clean = am.id.strip_suffix("-disabled").unwrap_or(&am.id);
            let (am_base, eff) = AppConfig::strip_effort_suffix(clean);
            am_base == base && eff == Some("xhigh")
        })
    })
}

pub fn active_chat_models(config: &AppConfig) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = config
        .models
        .iter()
        .filter(|m| m.is_enabled && !m.id.is_empty() && !is_non_chat_model(&m.id))
        .filter(|m| !config.hide_unavailable_models || !is_model_unavailable(m))
        .map(|m| {
            let name = if m.display_name.trim().is_empty() {
                m.id.clone()
            } else {
                m.display_name.clone()
            };
            (m.id.clone(), name)
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items.dedup_by(|a, b| a.0 == b.0);
    items
}

fn base_http_url(config: &AppConfig) -> String {
    let host = if config.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        config.host.as_str()
    };
    format!("http://{}:{}", host, config.port)
}

fn mcp_http_url(config: &AppConfig) -> String {
    format!("{}/mcp", base_http_url(config))
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {}", e))?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;
    tmp.write_all(content)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    tmp.persist(path)
        .map_err(|e| format!("Failed to persist config: {}", e))?;
    Ok(())
}

fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
        } else {
            out.push(c);
            i += 1;
        }
    }

    // Remove trailing commas before '}' or ']' outside strings
    let out_chars: Vec<char> = out.chars().collect();
    let mut cleaned = String::with_capacity(out_chars.len());
    in_string = false;
    escaped = false;
    let mut j = 0;
    while j < out_chars.len() {
        let c = out_chars[j];
        if in_string {
            cleaned.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            j += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            cleaned.push(c);
            j += 1;
            continue;
        }
        if c == ',' {
            let mut k = j + 1;
            while k < out_chars.len() && out_chars[k].is_whitespace() {
                k += 1;
            }
            if k < out_chars.len() && (out_chars[k] == '}' || out_chars[k] == ']') {
                j += 1;
                continue;
            }
        }
        cleaned.push(c);
        j += 1;
    }
    cleaned
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("Read error: {}", e))?;
    let stripped = strip_jsonc(&raw);
    if stripped.trim().is_empty() {
        return Ok(Map::new());
    }
    let val: Value = serde_json::from_str(&stripped).map_err(|e| format!("Invalid JSON: {}", e))?;
    match val {
        Value::Object(map) => Ok(map),
        _ => Err("Config root is not a JSON object".to_string()),
    }
}

fn write_json_object(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let pretty = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .map_err(|e| format!("JSON serialize error: {}", e))?;
    atomic_write(path, format!("{}\n", pretty).as_bytes())
}

// --- OpenCode API ---
fn build_opencode_amr_provider(config: &AppConfig) -> Value {
    let base_url = format!("{}/v1", base_http_url(config));
    let mut models_map = Map::new();
    for (id, display_name) in active_chat_models(config) {
        let mut model_obj = Map::new();
        model_obj.insert("name".to_string(), Value::String(display_name));

        let supports_thinking = config
            .models
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.supports_thinking)
            .unwrap_or(false);
        if supports_thinking {
            let mut variants = Map::new();
            for level in ["low", "medium", "high"] {
                let mut v = Map::new();
                v.insert(
                    "reasoningEffort".to_string(),
                    Value::String(level.to_string()),
                );
                variants.insert(level.to_string(), Value::Object(v));
            }
            if model_has_xhigh_variant(config, &id) {
                let mut v = Map::new();
                v.insert(
                    "reasoningEffort".to_string(),
                    Value::String("xhigh".to_string()),
                );
                variants.insert("xhigh".to_string(), Value::Object(v));
            }
            model_obj.insert("variants".to_string(), Value::Object(variants));
        }

        models_map.insert(id, Value::Object(model_obj));
    }

    let mut options = Map::new();
    options.insert("baseURL".to_string(), Value::String(base_url));
    options.insert("apiKey".to_string(), Value::String(config.master_api_key.clone()));

    let mut amr = Map::new();
    amr.insert(
        "npm".to_string(),
        Value::String("@ai-sdk/openai-compatible".to_string()),
    );
    amr.insert("name".to_string(), Value::String("AMR".to_string()));
    amr.insert("options".to_string(), Value::Object(options));
    amr.insert("models".to_string(), Value::Object(models_map));
    Value::Object(amr)
}

fn sync_opencode_api(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    if enable {
        if !root.contains_key("$schema") {
            root.insert(
                "$schema".to_string(),
                Value::String("https://opencode.ai/config.json".to_string()),
            );
        }
        let mut providers = match root.get("provider") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        providers.insert("AMR".to_string(), build_opencode_amr_provider(config));
        root.insert("provider".to_string(), Value::Object(providers));
    } else if let Some(Value::Object(providers)) = root.get_mut("provider") {
        providers.remove("AMR");
    }
    write_json_object(path, &root)
}

fn check_opencode_api_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(providers)) = root.get("provider") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let Some(existing) = providers.get("AMR") else {
        return ConnectorSyncStatus::NotSynced;
    };
    if existing == &build_opencode_amr_provider(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- OpenCode MCP ---
fn build_opencode_amr_mcp(config: &AppConfig) -> Value {
    let mut mcp = Map::new();
    mcp.insert("type".to_string(), Value::String("remote".to_string()));
    mcp.insert("url".to_string(), Value::String(mcp_http_url(config)));
    mcp.insert("enabled".to_string(), Value::Bool(true));
    Value::Object(mcp)
}

fn sync_opencode_mcp(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    if enable {
        if !root.contains_key("$schema") {
            root.insert(
                "$schema".to_string(),
                Value::String("https://opencode.ai/config.json".to_string()),
            );
        }
        let mut mcps = match root.get("mcp") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        mcps.insert("AMR".to_string(), build_opencode_amr_mcp(config));
        root.insert("mcp".to_string(), Value::Object(mcps));
    } else if let Some(Value::Object(mcps)) = root.get_mut("mcp") {
        mcps.remove("AMR");
    }
    write_json_object(path, &root)
}

fn check_opencode_mcp_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(mcps)) = root.get("mcp") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let Some(existing) = mcps.get("AMR") else {
        return ConnectorSyncStatus::NotSynced;
    };
    if existing == &build_opencode_amr_mcp(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- Claude Code API ---
fn sync_claude_code_api(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    let base_url = base_http_url(config);
    if enable {
        let mut env = match root.get("env") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        env.insert("ANTHROPIC_BASE_URL".to_string(), Value::String(base_url));
        env.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            Value::String(config.master_api_key.clone()),
        );
        root.insert("env".to_string(), Value::Object(env));
    } else if let Some(Value::Object(env)) = root.get_mut("env") {
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("ANTHROPIC_AUTH_TOKEN");
    }
    write_json_object(path, &root)
}

fn check_claude_code_api_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(env)) = root.get("env") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let url = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str());
    let key = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str());
    match (url, key) {
        (None, None) => ConnectorSyncStatus::NotSynced,
        (Some(u), Some(k)) if u == base_http_url(config) && k == config.master_api_key => {
            ConnectorSyncStatus::Synced
        }
        _ => ConnectorSyncStatus::Outdated,
    }
}

// --- Claude Code MCP ---
fn build_claude_mcp_entry(config: &AppConfig) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String("http".to_string()));
    obj.insert("url".to_string(), Value::String(mcp_http_url(config)));
    Value::Object(obj)
}

fn sync_claude_code_mcp(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    if enable {
        let mut servers = match root.get("mcpServers") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        servers.insert("AMR".to_string(), build_claude_mcp_entry(config));
        root.insert("mcpServers".to_string(), Value::Object(servers));
    } else if let Some(Value::Object(servers)) = root.get_mut("mcpServers") {
        servers.remove("AMR");
    }
    write_json_object(path, &root)
}

fn check_claude_code_mcp_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(servers)) = root.get("mcpServers") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let Some(existing) = servers.get("AMR") else {
        return ConnectorSyncStatus::NotSynced;
    };
    if existing == &build_claude_mcp_entry(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- Codex CLI (separate API & MCP managed blocks) ---
const CODEX_API_START: &str = "# >>> AMR API START >>>";
const CODEX_API_END: &str = "# <<< AMR API END <<<";
const CODEX_MCP_START: &str = "# >>> AMR MCP START >>>";
const CODEX_MCP_END: &str = "# <<< AMR MCP END <<<";

fn build_codex_api_block(config: &AppConfig) -> String {
    let base_url = format!("{}/v1", base_http_url(config));
    format!(
        "{}\n[model_providers.AMR]\nname = \"AMR\"\nbase_url = \"{}\"\nexperimental_bearer_token = \"{}\"\nwire_api = \"chat\"\n{}\n",
        CODEX_API_START, base_url, config.master_api_key, CODEX_API_END
    )
}

fn build_codex_mcp_block(config: &AppConfig) -> String {
    format!(
        "{}\n[mcp_servers.AMR]\nurl = \"{}\"\n{}\n",
        CODEX_MCP_START,
        mcp_http_url(config),
        CODEX_MCP_END
    )
}

fn strip_managed_block(raw: &str, start_marker: &str, end_marker: &str) -> String {
    let start_pos = raw.lines().position(|l| l.trim() == start_marker);
    let end_pos = raw.lines().position(|l| l.trim() == end_marker);
    if !matches!((start_pos, end_pos), (Some(s), Some(e)) if s < e) {
        return raw.to_string();
    }
    let mut out = String::new();
    let mut inside = false;
    for line in raw.lines() {
        if line.trim() == start_marker {
            inside = true;
            continue;
        }
        if line.trim() == end_marker {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn sync_codex_block(
    path: &Path,
    start_marker: &str,
    end_marker: &str,
    block: &str,
    enable: bool,
) -> Result<(), String> {
    let raw = if path.exists() {
        fs::read_to_string(path).map_err(|e| format!("Read error: {}", e))?
    } else {
        String::new()
    };
    // Also clean up old unified block if present
    let no_legacy = strip_managed_block(&raw, "# >>> AMR MANAGED START >>>", "# <<< AMR MANAGED END <<<");
    let mut cleaned = strip_managed_block(&no_legacy, start_marker, end_marker);
    if enable {
        if !cleaned.is_empty() && !cleaned.ends_with("\n\n") {
            if !cleaned.ends_with('\n') {
                cleaned.push('\n');
            }
            cleaned.push('\n');
        }
        cleaned.push_str(block);
    }
    atomic_write(path, cleaned.as_bytes())
}

fn check_codex_block_status(
    path: &Path,
    start_marker: &str,
    expected_block: &str,
) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let raw = match fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e.to_string()),
    };
    if !raw.contains(start_marker) {
        return ConnectorSyncStatus::NotSynced;
    }
    if raw.contains(expected_block.trim()) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- Gemini CLI API (.env) ---
fn sync_gemini_cli_api(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let raw = if path.exists() {
        fs::read_to_string(path).map_err(|e| format!("Read error: {}", e))?
    } else {
        String::new()
    };
    let mut lines: Vec<String> = raw
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("GOOGLE_GEMINI_BASE_URL=") && !t.starts_with("GEMINI_API_KEY=")
        })
        .map(|l| l.to_string())
        .collect();
    if enable {
        lines.push(format!("GOOGLE_GEMINI_BASE_URL={}", base_http_url(config)));
        lines.push(format!("GEMINI_API_KEY={}", config.master_api_key));
    }
    let content = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    atomic_write(path, content.as_bytes())
}

fn check_gemini_cli_api_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let raw = match fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e.to_string()),
    };
    let expected_url = format!("GOOGLE_GEMINI_BASE_URL={}", base_http_url(config));
    let expected_key = format!("GEMINI_API_KEY={}", config.master_api_key);
    let has_url = raw.lines().any(|l| l.trim().starts_with("GOOGLE_GEMINI_BASE_URL="));
    let has_key = raw.lines().any(|l| l.trim().starts_with("GEMINI_API_KEY="));
    if !has_url && !has_key {
        return ConnectorSyncStatus::NotSynced;
    }
    if raw.lines().any(|l| l.trim() == expected_url) && raw.lines().any(|l| l.trim() == expected_key) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- Gemini CLI MCP (settings.json) ---
fn build_gemini_mcp_entry(config: &AppConfig) -> Value {
    let mut obj = Map::new();
    obj.insert("httpUrl".to_string(), Value::String(mcp_http_url(config)));
    Value::Object(obj)
}

fn sync_gemini_cli_mcp(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    if enable {
        let mut servers = match root.get("mcpServers") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        servers.insert("AMR".to_string(), build_gemini_mcp_entry(config));
        root.insert("mcpServers".to_string(), Value::Object(servers));
    } else if let Some(Value::Object(servers)) = root.get_mut("mcpServers") {
        servers.remove("AMR");
    }
    write_json_object(path, &root)
}

fn check_gemini_cli_mcp_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(servers)) = root.get("mcpServers") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let Some(existing) = servers.get("AMR") else {
        return ConnectorSyncStatus::NotSynced;
    };
    if existing == &build_gemini_mcp_entry(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- DeepSeek Harness ---
fn build_deepseek_harness_entry(config: &AppConfig) -> Value {
    let models: Vec<Value> = active_chat_models(config)
        .into_iter()
        .map(|(id, name)| {
            let mut obj = Map::new();
            obj.insert("id".to_string(), Value::String(id));
            obj.insert("name".to_string(), Value::String(name));
            Value::Object(obj)
        })
        .collect();
    let mut amr = Map::new();
    amr.insert(
        "base_url".to_string(),
        Value::String(format!("{}/v1", base_http_url(config))),
    );
    amr.insert(
        "api_key".to_string(),
        Value::String(config.master_api_key.clone()),
    );
    amr.insert("models".to_string(), Value::Array(models));
    Value::Object(amr)
}

fn sync_deepseek_harness(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    if enable {
        let mut providers = match root.get("providers") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        providers.insert("AMR".to_string(), build_deepseek_harness_entry(config));
        root.insert("providers".to_string(), Value::Object(providers));
    } else if let Some(Value::Object(providers)) = root.get_mut("providers") {
        providers.remove("AMR");
    }
    write_json_object(path, &root)
}

fn check_deepseek_harness_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(providers)) = root.get("providers") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let Some(existing) = providers.get("AMR") else {
        return ConnectorSyncStatus::NotSynced;
    };
    if existing == &build_deepseek_harness_entry(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- Cursor (~/.cursor/mcp.json) ---
fn build_cursor_mcp_entry(config: &AppConfig) -> Value {
    let mut obj = Map::new();
    obj.insert("url".to_string(), Value::String(mcp_http_url(config)));
    Value::Object(obj)
}

fn sync_cursor(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    if enable {
        let mut servers = match root.get("mcpServers") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        servers.insert("AMR".to_string(), build_cursor_mcp_entry(config));
        root.insert("mcpServers".to_string(), Value::Object(servers));
    } else if let Some(Value::Object(servers)) = root.get_mut("mcpServers") {
        servers.remove("AMR");
    }
    write_json_object(path, &root)
}

fn check_cursor_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    let Some(Value::Object(servers)) = root.get("mcpServers") else {
        return ConnectorSyncStatus::NotSynced;
    };
    let Some(existing) = servers.get("AMR") else {
        return ConnectorSyncStatus::NotSynced;
    };
    if existing == &build_cursor_mcp_entry(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

// --- Cline / Roo Code (~/.amr/connectors/cline-amr.json) ---
fn build_cline_profile(config: &AppConfig) -> Map<String, Value> {
    let models: Vec<Value> = active_chat_models(config)
        .into_iter()
        .map(|(id, name)| {
            let mut obj = Map::new();
            obj.insert("id".to_string(), Value::String(id));
            obj.insert("name".to_string(), Value::String(name));
            Value::Object(obj)
        })
        .collect();
    let mut mcp_amr = Map::new();
    mcp_amr.insert("url".to_string(), Value::String(mcp_http_url(config)));
    mcp_amr.insert("type".to_string(), Value::String("streamableHttp".to_string()));
    let mut mcp_servers = Map::new();
    mcp_servers.insert("AMR".to_string(), Value::Object(mcp_amr));

    let mut root = Map::new();
    root.insert("provider".to_string(), Value::String("openai-compatible".to_string()));
    root.insert(
        "baseUrl".to_string(),
        Value::String(format!("{}/v1", base_http_url(config))),
    );
    root.insert(
        "apiKey".to_string(),
        Value::String(config.master_api_key.clone()),
    );
    root.insert("models".to_string(), Value::Array(models));
    root.insert("mcpServers".to_string(), Value::Object(mcp_servers));
    root
}

fn sync_cline(path: &Path, config: &AppConfig, enable: bool) -> Result<(), String> {
    if enable {
        write_json_object(path, &build_cline_profile(config))
    } else {
        if path.exists() {
            fs::remove_file(path).map_err(|e| format!("Remove error: {}", e))?;
        }
        Ok(())
    }
}

fn check_cline_status(path: &Path, config: &AppConfig) -> ConnectorSyncStatus {
    if !path.exists() {
        return ConnectorSyncStatus::NotSynced;
    }
    let root = match read_json_object(path) {
        Ok(r) => r,
        Err(e) => return ConnectorSyncStatus::Error(e),
    };
    if root == build_cline_profile(config) {
        ConnectorSyncStatus::Synced
    } else {
        ConnectorSyncStatus::Outdated
    }
}

pub fn sync_connector(id: &str, config: &AppConfig, enable: bool) -> Result<(), String> {
    let path = connector_config_path(id).ok_or_else(|| format!("Unknown connector: {}", id))?;
    match id {
        "opencode_api" => sync_opencode_api(&path, config, enable),
        "opencode_mcp" => sync_opencode_mcp(&path, config, enable),
        "claude_code_api" => sync_claude_code_api(&path, config, enable),
        "claude_code_mcp" => sync_claude_code_mcp(&path, config, enable),
        "codex_api" => sync_codex_block(
            &path,
            CODEX_API_START,
            CODEX_API_END,
            &build_codex_api_block(config),
            enable,
        ),
        "codex_mcp" => sync_codex_block(
            &path,
            CODEX_MCP_START,
            CODEX_MCP_END,
            &build_codex_mcp_block(config),
            enable,
        ),
        "gemini_cli_api" => sync_gemini_cli_api(&path, config, enable),
        "gemini_cli_mcp" => sync_gemini_cli_mcp(&path, config, enable),
        "deepseek_harness" => sync_deepseek_harness(&path, config, enable),
        "cursor" => sync_cursor(&path, config, enable),
        "cline" => sync_cline(&path, config, enable),
        _ => Err(format!("Unsupported connector: {}", id)),
    }
}

pub fn check_connector_status(id: &str, config: &AppConfig) -> ConnectorSyncStatus {
    let Some(path) = connector_config_path(id) else {
        return ConnectorSyncStatus::Error("Unknown connector".to_string());
    };
    match id {
        "opencode_api" => check_opencode_api_status(&path, config),
        "opencode_mcp" => check_opencode_mcp_status(&path, config),
        "claude_code_api" => check_claude_code_api_status(&path, config),
        "claude_code_mcp" => check_claude_code_mcp_status(&path, config),
        "codex_api" => check_codex_block_status(&path, CODEX_API_START, &build_codex_api_block(config)),
        "codex_mcp" => check_codex_block_status(&path, CODEX_MCP_START, &build_codex_mcp_block(config)),
        "gemini_cli_api" => check_gemini_cli_api_status(&path, config),
        "gemini_cli_mcp" => check_gemini_cli_mcp_status(&path, config),
        "deepseek_harness" => check_deepseek_harness_status(&path, config),
        "cursor" => check_cursor_status(&path, config),
        "cline" => check_cline_status(&path, config),
        _ => ConnectorSyncStatus::Error("Unknown connector".to_string()),
    }
}

pub fn sync_all_enabled(config: &AppConfig) {
    for id in &config.enabled_connectors {
        if let Err(e) = sync_connector(id, config, true) {
            eprintln!("[AMR Connectors] Failed to auto-sync {}: {}", id, e);
        }
    }
}
