#[cfg(test)]
mod tests {
    use aroxy::config::UpstreamAccount;
    use aroxy::proxy::fingerprint::apply_fingerprint;
    use aroxy::proxy::mappers::claude::{ClaudeContentBlock, ClaudeMessageResponse, ClaudeUsage};
    use aroxy::proxy::mappers::openai::{create_stream_chunk, OpenAIChatRequest, OpenAIMessage};
    use aroxy::proxy::router::{convert_claude_response_to_openai, convert_openai_to_claude, resolve_model};
    use aroxy::proxy::stats::ProxyStats;
    use reqwest::header::HeaderMap;
    use serde_json::Value;

    #[test]
    fn test_model_resolution() {
        let account = UpstreamAccount {
            id: "1".into(),
            name: "test".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "test".into(),
            fingerprint: "Claude Code 2.x".into(),
            is_active: true,
        };

        assert_eq!(resolve_model("claude-3-7-sonnet", &account), "claude-3-7-sonnet-20250219");
        assert_eq!(resolve_model("sonnet-3.7", &account), "claude-3-7-sonnet-20250219");
        assert_eq!(resolve_model("claude-3-5-sonnet", &account), "claude-3-5-sonnet-20241022");
    }

    #[test]
    fn test_convert_openai_to_claude() {
        let req = OpenAIChatRequest {
            model: "claude-3-7-sonnet".into(),
            messages: vec![
                OpenAIMessage {
                    role: "system".into(),
                    content: Value::String("You are a helpful coding assistant".into()),
                    reasoning_content: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Hello world".into()),
                    reasoning_content: None,
                    name: None,
                },
            ],
            stream: true,
            temperature: Some(0.7),
            max_tokens: Some(4096),
            stream_options: None,
            extra: Default::default(),
        };

        let claude_req = convert_openai_to_claude(&req, "claude-3-7-sonnet-20250219");
        assert_eq!(claude_req.model, "claude-3-7-sonnet-20250219");
        assert_eq!(claude_req.messages.len(), 1);
        assert!(claude_req.thinking.is_some());
        assert_eq!(claude_req.thinking.unwrap().budget_tokens, 2048);
    }

    #[test]
    fn test_convert_claude_response_to_openai() {
        let resp = ClaudeMessageResponse {
            id: "msg-123".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            model: "claude-3-7-sonnet-20250219".into(),
            content: vec![
                ClaudeContentBlock::Thinking {
                    thinking: "Let me consider this step by step...".into(),
                    signature: None,
                },
                ClaudeContentBlock::Text {
                    text: "Here is the solution.".into(),
                },
            ],
            stop_reason: Some("end_turn".into()),
            usage: ClaudeUsage {
                input_tokens: 50,
                output_tokens: 120,
            },
        };

        let oai = convert_claude_response_to_openai(&resp);
        assert_eq!(oai.id, "msg-123");
        assert_eq!(oai.choices.len(), 1);
        assert_eq!(
            oai.choices[0].message.reasoning_content.as_deref(),
            Some("Let me consider this step by step...")
        );
        assert_eq!(
            oai.choices[0].message.content,
            Value::String("Here is the solution.".into())
        );
        assert_eq!(oai.usage.unwrap().total_tokens, 170);
    }

    #[test]
    fn test_create_stream_chunk() {
        let chunk = create_stream_chunk("chatcmpl-test", "claude-3-7-sonnet", Some("chunk text"), None, None);
        assert!(chunk.starts_with("data: "));
        assert!(chunk.contains("chunk text"));
        assert!(chunk.ends_with("\n\n"));
    }

    #[test]
    fn test_fingerprint_headers() {
        let mut headers = HeaderMap::new();
        apply_fingerprint(&mut headers, "Claude Code 2.x");

        assert!(headers.contains_key("user-agent"));
        assert!(headers.get("user-agent").unwrap().to_str().unwrap().contains("claude-code"));
        assert_eq!(headers.get("anthropic-version").unwrap().to_str().unwrap(), "2023-06-01");
    }

    #[test]
    fn test_proxy_stats() {
        let stats = ProxyStats::new();
        stats.record_request("POST", "/v1/chat/completions", "gpt-4o", 200, 100, 25);
        stats.record_request("POST", "/v1/chat/completions", "gpt-4o", 200, 200, 75);

        let (reqs, tokens, avg_lat) = stats.get_metrics();
        assert_eq!(reqs, 2);
        assert_eq!(tokens, 300);
        assert_eq!(avg_lat, 50);

        let logs = stats.get_recent_logs();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].tokens, 200);
    }
}
