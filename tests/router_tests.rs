#[cfg(test)]
mod tests {
    use aroxy::config::{AppConfig, UpstreamAccount};
    use aroxy::proxy::fingerprint::apply_fingerprint;
    use aroxy::proxy::mappers::claude::{ClaudeContentBlock, ClaudeMessageResponse, ClaudeUsage};
    use aroxy::proxy::mappers::openai::{create_stream_chunk, OpenAIChatRequest, OpenAIMessage};
    use aroxy::proxy::router::{
        convert_claude_response_to_openai, convert_openai_to_claude, parse_model_and_reasoning,
        resolve_model, ReasoningEffort,
    };
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
        assert_eq!(resolve_model("claude-3-7-sonnet (low)", &account), "claude-3-7-sonnet-20250219");
        assert_eq!(resolve_model("sonnet-3.7 (high)", &account), "claude-3-7-sonnet-20250219");
        assert_eq!(resolve_model("claude-3-5-sonnet", &account), "claude-3-5-sonnet-20241022");
    }

    #[test]
    fn test_parse_model_and_reasoning() {
        // Suffix-based clone detection
        let (base, eff) = parse_model_and_reasoning("claude-3-7-sonnet (low)", None);
        assert_eq!(base, "claude-3-7-sonnet");
        assert_eq!(eff, Some(ReasoningEffort::Low));
        assert_eq!(eff.unwrap().budget_tokens(), 1024);

        let (base_med, eff_med) = parse_model_and_reasoning("gemini-2.5-flash (medium)", None);
        assert_eq!(base_med, "gemini-2.5-flash");
        assert_eq!(eff_med, Some(ReasoningEffort::Medium));
        assert_eq!(eff_med.unwrap().budget_tokens(), 4096);

        let (base_high, eff_high) = parse_model_and_reasoning("o3-mini (high)", None);
        assert_eq!(base_high, "o3-mini");
        assert_eq!(eff_high, Some(ReasoningEffort::High));
        assert_eq!(eff_high.unwrap().budget_tokens(), 16384);

        let (base_off, eff_off) = parse_model_and_reasoning("claude-3-7-sonnet (no-thinking)", None);
        assert_eq!(base_off, "claude-3-7-sonnet");
        assert_eq!(eff_off, Some(ReasoningEffort::Off));

        // Native protocol parameter detection
        let (base_nat, eff_nat) = parse_model_and_reasoning("claude-3-7-sonnet", Some("high"));
        assert_eq!(base_nat, "claude-3-7-sonnet");
        assert_eq!(eff_nat, Some(ReasoningEffort::High));
    }

    #[test]
    fn test_broadcast_models_generation() {
        let mut cfg = AppConfig::default();
        cfg.expose_reasoning_variants = true;

        let broadcasted = cfg.get_broadcast_models();
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-3-7-sonnet"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-3-7-sonnet (low)"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-3-7-sonnet (medium)"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-3-7-sonnet (high)"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-3-7-sonnet (no-thinking)"));

        // If disabled, clones should not be present
        cfg.expose_reasoning_variants = false;
        let non_variant = cfg.get_broadcast_models();
        assert!(non_variant.iter().any(|m| m["id"] == "claude-3-7-sonnet"));
        assert!(!non_variant.iter().any(|m| m["id"] == "claude-3-7-sonnet (low)"));
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
            reasoning_effort: None,
            stream_options: None,
            extra: Default::default(),
        };

        let claude_req = convert_openai_to_claude(&req, "claude-3-7-sonnet-20250219", Some(ReasoningEffort::Low));
        assert_eq!(claude_req.model, "claude-3-7-sonnet-20250219");
        assert_eq!(claude_req.messages.len(), 1);
        assert!(claude_req.thinking.is_some());
        assert_eq!(claude_req.thinking.unwrap().budget_tokens, 1024);
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
