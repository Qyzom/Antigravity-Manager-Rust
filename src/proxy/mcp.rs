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
    let query = payload.get("query").and_then(|q| q.as_str()).unwrap_or("").trim();
    if query.is_empty() {
        return "Search query is empty.".to_string();
    }

    let max_results = state.config.read().mcp_search_max_results.clamp(1, 25) as usize;
    let target_model = crate::proxy::audio::pick_dynamic_gemini_model(state, "search", None);
    let res = crate::proxy::audio::execute_generate_content(state, &target_model, &target_model, |acc| {
        crate::proxy::antigravity::build_web_search_payload(query, None, &target_model, acc.project_id.as_deref())
    })
    .await;

    match res {
        Ok((val, _account_name)) => {
            let actual = val.get("response").unwrap_or(&val);
            let mut text = String::new();
            if let Some(cands) = actual.get("candidates").and_then(|c| c.as_array()) {
                for cand in cands {
                    if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                        for part in parts {
                            if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                                continue;
                            }
                            if let Some(t) = part.get("text").and_then(|txt| txt.as_str()) {
                                text.push_str(t);
                            }
                        }
                    }
                }
            }
            let (_queries, results, _sources_md) = crate::proxy::antigravity::extract_grounding_info(actual);
            if !results.is_empty() {
                text.push_str("\n\n### Sources\n");
                for (i, item) in results.iter().take(max_results).enumerate() {
                    let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let snippet = item.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
                    if !snippet.is_empty() {
                        text.push_str(&format!("{}. [{}]({})\n   > {}\n", i + 1, title, url, snippet));
                    } else {
                        text.push_str(&format!("{}. [{}]({})\n", i + 1, title, url));
                    }
                }
            }
            if !text.is_empty() {
                return text;
            }
            format!("Search query '{}' executed, but no grounding text was returned.", query)
        }
        Err((status, err)) => format!("Search failed (HTTP {}): {}", status, err),
    }
}

async fn execute_mcp_fetch_url(state: &AppState, url_str: &str) -> String {
    let clean_url = url_str.trim();
    if clean_url.is_empty() {
        return "Empty URL provided.".to_string();
    }

    let target_url = if clean_url.starts_with("http://") || clean_url.starts_with("https://") {
        clean_url.to_string()
    } else if !clean_url.contains(' ') && clean_url.contains('.') {
        format!("https://{}", clean_url)
    } else {
        return format!(
            "Invalid URL: '{}'. fetch_web_page requires a full URL (e.g. https://example.com). For web search queries, please use the 'google_web_search' tool.",
            clean_url
        );
    };

    let parsed = match reqwest::Url::parse(&target_url) {
        Ok(u) => u,
        Err(_) => {
            return format!(
                "Invalid URL syntax for '{}'. Please provide a valid URL (e.g. https://example.com).",
                target_url
            );
        }
    };

    // Direct fetch using proxy client with natural user agent
    let client = &state.http_client;
    match client
        .get(parsed)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36",
        )
        .header("Accept", "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                let html = resp.text().await.unwrap_or_default();
                // Strip tags for clean markdown/text content preserving all UTF-8 characters
                let clean = strip_html_tags(&html);
                if clean.chars().count() > 30000 {
                    let truncated: String = clean.chars().take(30000).collect();
                    format!("{}\n\n[Content truncated at 30,000 characters]", truncated)
                } else {
                    clean
                }
            } else {
                format!("Failed to fetch URL {}: HTTP {}", target_url, resp.status())
            }
        }
        Err(e) => format!("Error connecting to URL {}: {}", target_url, e),
    }
}

pub fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script_or_style = false;
    let mut tag_buffer = String::new();

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_buffer.clear();
            continue;
        }

        if in_tag {
            if c == '>' {
                in_tag = false;
                let tag_lower = tag_buffer.trim().to_lowercase();
                if tag_lower.starts_with("script") || tag_lower.starts_with("style") {
                    in_script_or_style = true;
                } else if tag_lower.starts_with("/script") || tag_lower.starts_with("/style") {
                    in_script_or_style = false;
                } else {
                    let bare_tag = tag_lower.strip_prefix('/').unwrap_or(&tag_lower);
                    let tag_name = bare_tag.split_whitespace().next().unwrap_or("");
                    if matches!(
                        tag_name,
                        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                            | "li" | "tr" | "br" | "hr" | "header" | "footer"
                            | "article" | "section" | "blockquote"
                    ) {
                        out.push(' ');
                    }
                }
                tag_buffer.clear();
            } else if tag_buffer.len() < 32 {
                tag_buffer.push(c);
            }
            continue;
        }

        if !in_script_or_style {
            out.push(c);
        }
    }

    // Collapse multi-newlines and spaces preserving UTF-8 cleanly
    let mut final_text = String::with_capacity(out.len());
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
