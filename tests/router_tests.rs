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
            tier: "Pro".into(),
            quota_used: 1000,
            quota_limit: 100000,
            quota_reset: "in 2h".into(),
            claude_gpt_percent: 100,
            claude_gpt_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_gpt_weekly_percent: 100,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d".into(),
            status: "Active".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
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
        cfg.group_reasoning_clones = false;
        cfg.expose_reasoning_variants = true;

        let broadcasted = cfg.get_broadcast_models();
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-sonnet-4-6"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-sonnet-4-6 (low)"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-sonnet-4-6 (medium)"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-sonnet-4-6 (high)"));
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-sonnet-4-6 (no-thinking)"));

        // When group_reasoning_clones is enabled, clones should NOT be present
        cfg.group_reasoning_clones = true;
        let grouped = cfg.get_broadcast_models();
        assert!(grouped.iter().any(|m| m["id"] == "claude-sonnet-4-6"));
        assert!(grouped.iter().any(|m| m["id"] == "claude-sonnet-4-6" && m["reasoning"]["supported_efforts"].is_array()));
        assert!(!grouped.iter().any(|m| m["id"] == "claude-sonnet-4-6 (low)"));
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
                    refusal: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Hello world".into()),
                    reasoning_content: None,
                    refusal: None,
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
        stats.reset_stats();
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

    #[test]
    fn test_antigravity_payload_conversion() {
        use aroxy::proxy::antigravity::{build_antigravity_payload, convert_antigravity_response_to_openai};

        let req = OpenAIChatRequest {
            model: "claude-3-7-sonnet".into(),
            messages: vec![
                OpenAIMessage {
                    role: "system".into(),
                    content: Value::String("You are Antigravity system.".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Write a test".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
            ],
            stream: false,
            temperature: Some(0.2),
            max_tokens: Some(2048),
            reasoning_effort: None,
            stream_options: None,
            extra: Default::default(),
        };

        let payload_46 = build_antigravity_payload(&req, "claude-sonnet-4-6", Some("test-project"), Some(ReasoningEffort::Medium));
        assert_eq!(payload_46["model"], "claude-sonnet-4-6");

        let payload = build_antigravity_payload(&req, "claude-sonnet-5-5", Some("test-project"), Some(ReasoningEffort::Medium));
        assert_eq!(payload["model"], "claude-sonnet-5-5-high");
        assert_eq!(payload["project"], "test-project");
        assert_eq!(payload["userAgent"], "antigravity");
        assert!(payload["request"]["systemInstruction"]["parts"][0]["text"].as_str().unwrap().contains("Antigravity"));
        assert_eq!(payload["request"]["contents"][0]["role"], "user");
        assert_eq!(payload["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"], 4096);

        // Test Antigravity Response conversion
        let raw_upstream = serde_json::json!({
            "response": {
                "candidates": [{
                    "finishReason": "STOP",
                    "content": {
                        "parts": [
                            { "thought": true, "text": "Deep thinking step..." },
                            { "text": "Code output here." }
                        ]
                    }
                }]
            }
        });

        let oai = convert_antigravity_response_to_openai(&raw_upstream, "claude-3-7-sonnet");
        assert_eq!(oai.choices[0].message.reasoning_content.as_deref(), Some("Deep thinking step..."));
        assert_eq!(oai.choices[0].message.content, Value::String("Code output here.".into()));
        assert_eq!(oai.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_balancer_cache_affinity_and_failover() {
        use aroxy::config::{CacheAffinityMode, RoutingStrategy};
        use aroxy::proxy::balancer::LoadBalancer;

        let balancer = LoadBalancer::new();
        let mut cfg = AppConfig::default();
        cfg.routing_strategy = RoutingStrategy::QuotaAware;
        cfg.cache_affinity_mode = CacheAffinityMode::CacheFirst;
        cfg.session_sticky_ttl_secs = 600;
        cfg.quota_imbalance_threshold = 25;

        let acc_a = UpstreamAccount {
            id: "acc-a".into(),
            name: "account.a@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "token-a".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "PRO".into(),
            quota_used: 20000,
            quota_limit: 100000,
            quota_reset: "in 3h".into(),
            claude_gpt_percent: 80,
            claude_gpt_reset: "in 3h".into(),
            gemini_percent: 100,
            gemini_reset: "in 3h".into(),
            claude_gpt_weekly_percent: 100,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d".into(),
            status: "Active".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        let acc_b = UpstreamAccount {
            id: "acc-b".into(),
            name: "account.b@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "token-b".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "PRO".into(),
            quota_used: 50000,
            quota_limit: 100000,
            quota_reset: "in 2h".into(),
            claude_gpt_percent: 50,
            claude_gpt_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_gpt_weekly_percent: 100,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d".into(),
            status: "Active".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        cfg.accounts = vec![acc_a.clone(), acc_b.clone()];

        // Request 1 in conversation session "chat-1": should pick acc_a (80% remaining > 50% remaining)
        let chosen1 = balancer.select_account(&cfg, "claude-sonnet-4-6", Some("chat-1")).expect("must pick account");
        assert_eq!(chosen1.id, "acc-a");

        // Request 2 in same session: should stick to acc_a for prompt caching (Cache hit!)
        let chosen2 = balancer.select_account(&cfg, "claude-sonnet-4-6", Some("chat-1")).expect("must pick account");
        assert_eq!(chosen2.id, "acc-a");

        let (_, hits, _, _, active_pins) = balancer.get_metrics();
        assert_eq!(hits, 1);
        assert_eq!(active_pins, 1);

        // Test Cooldown Failover: mark acc-a as rate-limited (429)
        balancer.mark_cooldown("acc-a", 30);
        let chosen3 = balancer.select_account(&cfg, "claude-sonnet-4-6", Some("chat-1")).expect("must failover");
        assert_eq!(chosen3.id, "acc-b"); // Automatically routed to acc-b!
    }

    #[test]
    fn test_quota_aware_model_family_filtering() {
        use aroxy::config::{AppConfig, CacheAffinityMode, RoutingStrategy, UpstreamAccount};
        use aroxy::proxy::balancer::LoadBalancer;

        let balancer = LoadBalancer::new();
        let mut cfg = AppConfig::default();
        cfg.routing_strategy = RoutingStrategy::QuotaAware;
        cfg.cache_affinity_mode = CacheAffinityMode::Balanced;

        // Account 1: Free tier, Claude quota 0%, Gemini 100%
        let acc_free = UpstreamAccount {
            id: "acc-free".into(),
            name: "free.user@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "token-free".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "FREE".into(),
            quota_used: 0,
            quota_limit: 100000,
            quota_reset: "in 4h".into(),
            claude_gpt_percent: 0,
            claude_gpt_reset: "in 4h".into(),
            gemini_percent: 100,
            gemini_reset: "in 4h".into(),
            claude_gpt_weekly_percent: 0,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d".into(),
            status: "Active".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        // Account 2: Pro tier, Claude quota 60%, Gemini 90%
        let acc_pro = UpstreamAccount {
            id: "acc-pro".into(),
            name: "pro.user@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "token-pro".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "PRO".into(),
            quota_used: 40000,
            quota_limit: 100000,
            quota_reset: "in 2h".into(),
            claude_gpt_percent: 60,
            claude_gpt_reset: "in 2h".into(),
            gemini_percent: 90,
            gemini_reset: "in 2h".into(),
            claude_gpt_weekly_percent: 60,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 90,
            gemini_weekly_reset: "in 7d".into(),
            status: "Active".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        cfg.accounts = vec![acc_free.clone(), acc_pro.clone()];

        // 1. Claude request: Free account MUST NOT be selected; must route to acc-pro!
        let claude_chosen = balancer.select_account(&cfg, "claude-3-7-sonnet", None).expect("must route to pro");
        assert_eq!(claude_chosen.id, "acc-pro");

        // 2. Gemini request: Both accounts are eligible, Free account has higher Gemini quota (100% > 90%), so selects acc-free!
        let gemini_chosen = balancer.select_account(&cfg, "gemini-3.8-flash", None).expect("must route to free");
        assert_eq!(gemini_chosen.id, "acc-free");

        // 3. If acc-pro is disabled or exhausted, Claude requests return clean quota exhausted error!
        let mut cfg_only_free = cfg.clone();
        cfg_only_free.accounts = vec![acc_free.clone()];
        let err = balancer.select_account(&cfg_only_free, "claude-opus-4-6", None).unwrap_err();
        match err {
            aroxy::proxy::balancer::RoutingError::ClaudeQuotaExhausted { reset } => {
                assert!(reset.contains("4h"));
            }
            other => panic!("expected ClaudeQuotaExhausted, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_session_fingerprint_multiturn() {
        use aroxy::proxy::balancer::LoadBalancer;

        // Turn 1
        let req1 = OpenAIChatRequest {
            model: "claude-3-7-sonnet".into(),
            messages: vec![
                OpenAIMessage {
                    role: "system".into(),
                    content: Value::String("You are a helpful coding assistant".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Refactor this function to be lock-free".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
            ],
            stream: true,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            extra: Default::default(),
        };

        // Turn 2 of the SAME conversation (assistant reply + next user follow-up)
        let req2 = OpenAIChatRequest {
            model: "claude-3-7-sonnet".into(),
            messages: vec![
                OpenAIMessage {
                    role: "system".into(),
                    content: Value::String("You are a helpful coding assistant".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Refactor this function to be lock-free".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "assistant".into(),
                    content: Value::String("Here is the lock-free implementation...".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Can you add unit tests for it?".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                },
            ],
            stream: true,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            extra: Default::default(),
        };

        let sid1 = LoadBalancer::extract_session_fingerprint(&req1, None);
        let sid2 = LoadBalancer::extract_session_fingerprint(&req2, None);

        // Turn 1 and Turn 2 MUST have the exact same fingerprint to ensure prompt cache hit!
        assert_eq!(sid1, sid2);
    }

    #[test]
    fn test_multimodal_vision_conversion() {
        use aroxy::proxy::antigravity::{build_antigravity_payload, build_antigravity_payload_from_claude};
        use aroxy::proxy::mappers::openai::{OpenAIChatRequest, OpenAIMessage};
        use serde_json::json;

        // 1. Anthropic-style base64 image block
        let anthropic_img_req = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "What is in this screenshot?" },
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
                            }
                        }
                    ]
                }
            ]
        });

        let pa_payload1 = build_antigravity_payload_from_claude(&anthropic_img_req, "gemini-2.5-pro", None, None);
        let contents1 = pa_payload1["request"]["contents"].as_array().expect("must have contents array");
        let parts1 = contents1[0]["parts"].as_array().expect("must have parts array");
        assert_eq!(parts1.len(), 2);
        assert_eq!(parts1[0]["text"], "What is in this screenshot?");
        assert_eq!(parts1[1]["inlineData"]["mimeType"], "image/png");
        assert!(parts1[1]["inlineData"]["data"].as_str().unwrap().starts_with("iVBORw"));

        // 2. OpenAI-style image_url data URI
        let openai_img_req = OpenAIChatRequest {
            model: "gpt-4o".into(),
            messages: vec![
                OpenAIMessage {
                    role: "user".into(),
                    content: json!([
                        { "type": "text", "text": "Inspect this image" },
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEASABIAAD/"
                            }
                        }
                    ]),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                }
            ],
            stream: false,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            extra: Default::default(),
        };

        let pa_payload2 = build_antigravity_payload(&openai_img_req, "gemini-2.5-pro", None, None);
        let contents2 = pa_payload2["request"]["contents"].as_array().expect("must have contents array");
        let parts2 = contents2[0]["parts"].as_array().expect("must have parts array");
        assert_eq!(parts2.len(), 2);
        assert_eq!(parts2[0]["text"], "Inspect this image");
        assert_eq!(parts2[1]["inlineData"]["mimeType"], "image/jpeg");
        assert_eq!(parts2[1]["inlineData"]["data"], "/9j/4AAQSkZJRgABAQEASABIAAD/");
    }

    #[test]
    fn test_quota_protection_threshold_and_error_exclusion() {
        use aroxy::config::{AppConfig, RoutingStrategy, UpstreamAccount};
        use aroxy::proxy::balancer::LoadBalancer;

        let balancer = LoadBalancer::new();
        let mut cfg = AppConfig::default();
        cfg.routing_strategy = RoutingStrategy::QuotaAware;
        cfg.quota_protection_threshold = 10; // Protect accounts under 10%

        let acc_low = UpstreamAccount {
            id: "acc-low".into(),
            name: "low@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "tok-low".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "PRO".into(),
            quota_used: 95000,
            quota_limit: 100000,
            quota_reset: "in 1h".into(),
            claude_gpt_percent: 5, // 5% < 10% threshold!
            claude_gpt_reset: "in 1h".into(),
            gemini_percent: 5,
            gemini_reset: "in 1h".into(),
            claude_gpt_weekly_percent: 5,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 5,
            gemini_weekly_reset: "in 7d".into(),
            status: "ACTIVE".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        let acc_healthy = UpstreamAccount {
            id: "acc-healthy".into(),
            name: "healthy@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "tok-healthy".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "PRO".into(),
            quota_used: 50000,
            quota_limit: 100000,
            quota_reset: "in 2h".into(),
            claude_gpt_percent: 50,
            claude_gpt_reset: "in 2h".into(),
            gemini_percent: 50,
            gemini_reset: "in 2h".into(),
            claude_gpt_weekly_percent: 50,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 50,
            gemini_weekly_reset: "in 7d".into(),
            status: "ACTIVE".into(),
            error_message: None,
            refresh_token: None,
            project_id: None,
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        let acc_error = UpstreamAccount {
            id: "acc-error".into(),
            name: "error@gmail.com".into(),
            provider: "antigravity".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: "tok-error".into(),
            fingerprint: "Claude Code 2.x".into(),
            tier: "PRO".into(),
            quota_used: 0,
            quota_limit: 100000,
            quota_reset: "in 2h".into(),
            claude_gpt_percent: 100, // high quota, but status ERROR
            claude_gpt_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_gpt_weekly_percent: 100,
            claude_gpt_weekly_reset: "in 7d".into(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d".into(),
            status: "ERROR".into(),
            error_message: Some("Invalid credentials".into()),
            refresh_token: None,
            project_id: None,
            is_active: false,
            added_at: None,
            device_profile: None,
            proxy_url: None,
        };

        cfg.accounts = vec![acc_low.clone(), acc_healthy.clone(), acc_error.clone()];

        // acc_error must be completely ignored.
        // acc_low is under threshold (5% < 10%), so balancer routes to acc-healthy.
        let chosen = balancer.select_account(&cfg, "claude-sonnet-4-6", None).expect("must select account");
        assert_eq!(chosen.id, "acc-healthy");

        // If only acc_low and acc_error exist, balancer falls back gracefully to acc_low rather than failing
        cfg.accounts = vec![acc_low.clone(), acc_error.clone()];
        let fallback = balancer.select_account(&cfg, "claude-sonnet-4-6", None).expect("must select fallback");
        assert_eq!(fallback.id, "acc-low");
    }

    #[test]
    fn test_jwt_email_extraction() {
        use aroxy::proxy::antigravity::extract_email_from_jwt;

        // Valid JWT structure with header.payload.signature
        // payload: {"email":"developer@antigravity.test","sub":"12345"}
        // base64url of {"email":"developer@antigravity.test","sub":"12345"}:
        // eyJlbWFpbCI6ImRldmVsb3BlckBhbnRpZ3Jhdml0eS50ZXN0Iiwic3ViIjoiMTIzNDUifQ
        let fake_jwt = "eyJhbGciOiJSUzI1NiJ9.eyJlbWFpbCI6ImRldmVsb3BlckBhbnRpZ3Jhdml0eS50ZXN0Iiwic3ViIjoiMTIzNDUifQ.signaturesig";
        let email = extract_email_from_jwt(fake_jwt);
        assert_eq!(email, Some("developer@antigravity.test".to_string()));

        // Invalid JWT
        assert_eq!(extract_email_from_jwt("random-junk"), None);
    }
}
