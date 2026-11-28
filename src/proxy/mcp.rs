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

async fn execute_mcp_fetch_url(_state: &AppState, url_str: &str) -> String {
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

    let parsed = match url::Url::parse(&target_url) {
        Ok(u) => u,
        Err(_) => {
            return format!(
                "Invalid URL syntax for '{}'. Please provide a valid URL (e.g. https://example.com).",
                target_url
            );
        }
    };

    if !matches!(parsed.scheme(), "http" | "https") || !parsed.username().is_empty() || parsed.password().is_some() {
        return format!("Invalid or unsupported URL scheme/credentials for '{}'.", target_url);
    }

    let mut current_url = parsed;
    let mut final_resp = None;

    for hop in 0..=5 {
        if !matches!(current_url.scheme(), "http" | "https")
            || !current_url.username().is_empty()
            || current_url.password().is_some()
        {
            return format!(
                "Invalid or unsupported URL scheme/credentials for '{}'.",
                current_url
            );
        }

        let Some(host_str) = current_url.host_str() else {
            return format!("URL has no host: '{}'", current_url);
        };
        let host = host_str.to_string();
        if host.eq_ignore_ascii_case("localhost")
            || host.ends_with(".local")
            || host.ends_with(".internal")
        {
            return format!(
                "Security error: URL '{}' targets a local/internal host.",
                current_url
            );
        }
        let Some(port) = current_url.port_or_known_default() else {
            return format!("URL has no port: '{}'", current_url);
        };

        // Async DNS resolution + IP pinning per hop to prevent SSRF & DNS-rebinding TOCTOU
        let addresses: Vec<_> = match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(addrs) => addrs.collect(),
            Err(e) => return format!("Could not resolve URL host '{}': {}", host, e),
        };

        if addresses.is_empty() || addresses.iter().any(|a| !is_public_address(a.ip())) {
            return format!(
                "Security error: URL '{}' resolves to a private, loopback, or non-public address.",
                current_url
            );
        }

        let client = match wreq::Client::builder()
            .emulation(wreq_util::Emulation::Chrome136)
            .no_proxy()
            .resolve_to_addrs(host.clone(), addresses)
            .redirect(wreq::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return format!(
                    "Security error: failed to initialize isolated HTTP client: {}",
                    e
                )
            }
        };

        match client
            .get(current_url.as_str())
            .header(
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.8")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_redirection() => {
                if hop == 5 {
                    return format!("Error fetching '{}': Too many redirects", target_url);
                }
                let Some(loc) = resp.headers().get(wreq::header::LOCATION).and_then(|v| v.to_str().ok()) else {
                    return format!("Error fetching '{}': Redirect missing Location header", target_url);
                };
                current_url = match current_url.join(loc) {
                    Ok(u) => u,
                    Err(e) => return format!("Invalid redirect URL '{}': {}", loc, e),
                };
            }
            Ok(resp) => {
                final_resp = Some(resp);
                break;
            }
            Err(e) => {
                return format!("Failed to reach '{}': {}", target_url, e);
            }
        }
    }

    match final_resp {
        Some(resp) => {
            if resp.status().is_success() {
                use futures_util::StreamExt;
                const MAX_BYTES: usize = 2 * 1024 * 1024; // 2 MiB limit
                let mut body = Vec::new();
                let mut stream = resp.bytes_stream();
                while let Some(chunk_res) = stream.next().await {
                    match chunk_res {
                        Ok(chunk) => {
                            if body.len() + chunk.len() > MAX_BYTES {
                                return format!(
                                    "Error: Web page content at '{}' exceeds the 2 MiB size limit.",
                                    target_url
                                );
                            }
                            body.extend_from_slice(&chunk);
                        }
                        Err(e) => {
                            return format!("Error reading response from '{}': {}", target_url, e);
                        }
                    }
                }
                let html = String::from_utf8_lossy(&body);
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
        None => format!("Error connecting to URL {}: no response received", target_url),
    }
}

pub fn is_public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        // Restrict IPv6 to globally routed unicast (2000::/3); mapped IPv4 addresses and documentation are rejected.
        std::net::IpAddr::V6(ip) => {
            (ip.segments()[0] & 0xe000) == 0x2000
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        }
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

#[cfg(test)]
mod tests {
    use super::{is_public_address, strip_html_tags};

    #[test]
    fn test_strip_html_tags_preserves_unicode() {
        assert_eq!(
            strip_html_tags("<p>Привет, 世界! 🦀</p><script>ignored()</script>"),
            "Привет, 世界! 🦀"
        );
    }

    #[test]
    fn test_blocks_internal_and_private_addresses() {
        let blocked = [
            "127.0.0.1",
            "127.0.1.1",
            "10.0.0.1",
            "10.254.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "fe80::1",
            "fc00::1",
        ];
        for ip in blocked {
            let parsed = ip.parse().unwrap();
            assert!(!is_public_address(parsed), "Expected {} to be blocked", ip);
        }

        let allowed = [
            "8.8.8.8",
            "1.1.1.1",
            "142.250.190.46",
            "2607:f8b0:4005:805::200e",
        ];
        for ip in allowed {
            let parsed = ip.parse().unwrap();
            assert!(is_public_address(parsed), "Expected {} to be allowed", ip);
        }
    }
}
