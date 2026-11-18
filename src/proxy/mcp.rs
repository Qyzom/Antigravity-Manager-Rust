use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::proxy::server::AppState;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

use std::sync::atomic::Ordering;

/// Standalone Model Context Protocol (MCP) HTTP Endpoint handler (`/mcp` / `/v1/mcp`).
/// Fully compliant with standard Anthropic / Cursor / Windsurf / Cline MCP specification.
pub async fn execute_mcp_tool_direct(state: &AppState, tool_name: &str, argument: &str) -> String {
    match tool_name {
        "google_web_search" => {
            let req = json!({ "query": argument });
            execute_mcp_web_search(state, &req).await
        }
        "fetch_web_page" => {
            execute_mcp_fetch_url(state, argument).await
        }
        _ => format!("Unknown MCP tool: {}", tool_name),
    }
}

pub async fn mcp_handler(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    let req_id = req.id.clone();

    if !state.config.read().mcp_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: None,
                error: Some(json!({
                    "code": -32603,
                    "message": "AMR: MCP server is disabled in settings"
                })),
            }),
        )
            .into_response();
    }

    match req.method.as_str() {
        "initialize" => {
            let res = json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": "antigravity-manager-rust-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            });
            Json(JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: Some(res),
                error: None,
            })
            .into_response()
        }

        "notifications/initialized" => {
            // Client ACK notification - no response required in standard JSON-RPC
            StatusCode::NO_CONTENT.into_response()
        }

        "tools/list" => {
            let tools = json!({
                "tools": [
                    {
                        "name": "google_web_search",
                        "description": "Perform live internet search through Google Search index with source citations and URL links.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "Search query keywords"
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "fetch_web_page",
                        "description": "Fetch live or cached web page contents by URL for coding, documentation, or research.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "url": {
                                    "type": "string",
                                    "description": "Full HTTP or HTTPS web URL to fetch"
                                }
                            },
                            "required": ["url"]
                        }
                    }
                ]
            });
            Json(JsonRpcResponse {
                jsonrpc: "2.0",
                id: req_id,
                result: Some(tools),
                error: None,
            })
            .into_response()
        }

        "tools/call" => {
            state.stats.mcp_total_calls.fetch_add(1, Ordering::Relaxed);
            let params = req.params.unwrap_or_default();
            let tool_name = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            match tool_name {
                "google_web_search" => {
                    state.stats.mcp_search_calls.fetch_add(1, Ordering::Relaxed);
                    let query = arguments
                        .get("query")
                        .and_then(|q| q.as_str())
                        .unwrap_or("")
                        .trim();

                    if query.is_empty() {
                        state.stats.mcp_error_calls.fetch_add(1, Ordering::Relaxed);
                        return Json(JsonRpcResponse {
                            jsonrpc: "2.0",
                            id: req_id,
                            result: None,
                            error: Some(json!({
                                "code": -32602,
                                "message": "Missing required argument 'query'"
                            })),
                        })
                        .into_response();
                    }

                    let search_req = json!({ "query": query });
                    let res = execute_mcp_web_search(&state, &search_req).await;
                    Json(JsonRpcResponse {
                        jsonrpc: "2.0",
                        id: req_id,
                        result: Some(json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": res
                                }
                            ]
                        })),
                        error: None,
                    })
                    .into_response()
                }

                "fetch_web_page" => {
                    state.stats.mcp_fetch_calls.fetch_add(1, Ordering::Relaxed);
                    let url_str = arguments
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .trim();

                    if url_str.is_empty() {
                        return Json(JsonRpcResponse {
                            jsonrpc: "2.0",
                            id: req_id,
                            result: None,
                            error: Some(json!({
                                "code": -32602,
                                "message": "Missing required argument 'url'"
                            })),
                        })
                        .into_response();
                    }

                    let res = execute_mcp_fetch_url(&state, url_str).await;
                    Json(JsonRpcResponse {
                        jsonrpc: "2.0",
                        id: req_id,
                        result: Some(json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": res
                                }
                            ]
                        })),
                        error: None,
                    })
                    .into_response()
                }

                _ => Json(JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: req_id,
                    result: None,
                    error: Some(json!({
                        "code": -32601,
                        "message": format!("Tool '{}' not found", tool_name)
                    })),
                })
                .into_response(),
            }
        }

        _ => Json(JsonRpcResponse {
            jsonrpc: "2.0",
            id: req_id,
            result: None,
            error: Some(json!({
                "code": -32601,
                "message": format!("Method '{}' not implemented", req.method)
            })),
        })
        .into_response(),
    }
}

async fn execute_mcp_web_search(state: &AppState, payload: &Value) -> String {
    let query = payload.get("query").and_then(|q| q.as_str()).unwrap_or("");

    let selected_account = {
        let cfg = state.config.read();
        state
            .balancer
            .select_account(&cfg, "gemini-3.8-flash", None)
    };

    let account = match selected_account {
        Ok(acc) => acc,
        Err(e) => return format!("Search failed: {}", e),
    };

    let (platform_str, ide_ver) = {
        let cfg = state.config.read();
        (cfg.resolved_platform(), cfg.spoof_ide_version.clone())
    };

    let search_payload = crate::proxy::antigravity::build_web_search_payload(
        query,
        None,
        "gemini-2.5-flash",
        account.project_id.as_deref(),
    );

    let client = state.get_client_for_account(&account);
    let token = account.api_key.clone();
    let (client_id, machine_id) = if let Some(ref dp) = account.device_profile {
        (dp.dev_device_id.clone(), dp.machine_id.clone())
    } else {
        (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().simple().to_string())
    };

    let headers = crate::proxy::server::build_upstream_headers_json(
        &token,
        &client_id,
        &machine_id,
        platform_str,
        &ide_ver,
        "AUTO",
        "stable",
        true,
        true,
        "gemini-3.5-flash-lite",
    );

    for &endpoint in crate::proxy::antigravity::GENERATE_ENDPOINTS.iter() {
        if let Ok(res) = client
            .post(endpoint)
            .headers(headers.clone())
            .json(&search_payload)
            .send()
            .await
        {
            if res.status().is_success() {
                if let Ok(val) = res.json::<Value>().await {
                    let mut text = String::new();
                    let actual = val.get("response").unwrap_or(&val);
                    if let Some(cands) = actual.get("candidates").and_then(|c| c.as_array()) {
                        for cand in cands {
                            if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                for part in parts {
                                    if let Some(t) = part.get("text").and_then(|txt| txt.as_str()) {
                                        text.push_str(t);
                                    }
                                }
                            }
                        }
                    }
                    let (_, _, sources) = crate::proxy::antigravity::extract_grounding_info(actual);
                    if !sources.is_empty() {
                        text.push_str(&sources);
                    }
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
        }
    }

    format!("Search query '{}' executed, but no grounding text was returned.", query)
}

async fn execute_mcp_fetch_url(state: &AppState, url_str: &str) -> String {
    // Resolve once and pin the connection to the checked addresses. Do not use
    // environment proxies or follow redirects, which could bypass this check.
    let _ = state;
    let url = match reqwest::Url::parse(url_str) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.username().is_empty() && url.password().is_none() => url,
        _ => return "Invalid HTTP or HTTPS URL".into(),
    };
    let Some(host) = url.host_str() else { return "URL has no host".into(); };
    let Some(port) = url.port_or_known_default() else { return "URL has no port".into(); };
    let addresses: Vec<_> = match tokio::net::lookup_host((host, port)).await {
        Ok(addresses) => addresses.collect(),
        Err(_) => return "Could not resolve URL host".into(),
    };
    if addresses.is_empty() || addresses.iter().any(|a| !is_public_address(a.ip())) {
        return "URL resolves to a non-public address".into();
    }
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .timeout(std::time::Duration::from_secs(15))
        .build() {
            Ok(client) => client,
            Err(_) => return "Could not create HTTP client".into(),
        };
    match client
        .get(url_str)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36",
        )
        .header("Accept", "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        Ok(mut resp) => {
            if resp.status().is_success() {
                const MAX_BYTES: usize = 2 * 1024 * 1024;
                let mut body = Vec::new();
                loop {
                    match resp.chunk().await {
                        Ok(Some(chunk)) if body.len() + chunk.len() <= MAX_BYTES => body.extend_from_slice(&chunk),
                        Ok(Some(_)) => return "Page exceeds the 2 MiB size limit".into(),
                        Ok(None) => break,
                        Err(_) => return "Could not read page".into(),
                    }
                }
                let html = String::from_utf8_lossy(&body);
                // Strip tags for clean markdown/text content
                let clean = strip_html_tags(&html);
                if clean.chars().count() > 30000 {
                    format!("{}\n\n[Content truncated at 30,000 characters]", clean.chars().take(30000).collect::<String>())
                } else {
                    clean
                }
            } else {
                format!("Failed to fetch URL {}: HTTP {}", url_str, resp.status())
            }
        }
        Err(e) => format!("Error connecting to URL {}: {}", url_str, e),
    }
}

fn is_public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0 || a == 10 || a == 127 || a >= 224 ||
                (a == 100 && (64..=127).contains(&b)) ||
                (a == 169 && b == 254) || (a == 172 && (16..=31).contains(&b)) ||
                (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99))) ||
                (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100))) ||
                (a == 203 && b == 0 && c == 113))
        }
        // Restrict IPv6 to globally routed unicast; mapped IPv4 addresses are rejected.
        std::net::IpAddr::V6(ip) => (ip.segments()[0] & 0xe000) == 0x2000 &&
            !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8),
    }
}

fn strip_html_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut in_script = false;

    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();

    while i < bytes.len() {
        if !in_tag && bytes[i] == b'<' {
            in_tag = true;
            if lower[i..].starts_with("<script") || lower[i..].starts_with("<style") {
                in_script = true;
            }
            i += 1;
            continue;
        }

        if in_tag && bytes[i] == b'>' {
            in_tag = false;
            if in_script && (lower[..i].ends_with("</script") || lower[..i].ends_with("</style")) {
                in_script = false;
            }
            i += 1;
            continue;
        }

        if !in_tag && !in_script {
            let ch = html[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }

    // Collapse multi-newlines and spaces
    let mut final_text = String::new();
    let mut last_space = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !last_space {
                final_text.push(' ');
                last_space = true;
            }
        } else {
            final_text.push(c);
            last_space = false;
        }
    }
    final_text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{strip_html_tags, is_public_address};
    #[test]
    fn preserves_unicode() {
        assert_eq!(strip_html_tags("<p>Привет, 世界!</p><script>ignored</script>"), "Привет, 世界!");
    }
    #[test]
    fn blocks_internal_addresses() {
        for ip in ["127.0.0.1", "10.0.0.1", "169.254.169.254", "::1", "::ffff:127.0.0.1"] {
            assert!(!is_public_address(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_address("8.8.8.8".parse().unwrap()));
    }
}
