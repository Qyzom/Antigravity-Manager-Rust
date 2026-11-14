#[cfg(test)]
mod tests {
    use amr::config::{AppConfig, UpstreamAccount};
    use amr::proxy::fingerprint::apply_fingerprint;
    use amr::proxy::mappers::claude::{ClaudeContentBlock, ClaudeMessageResponse, ClaudeUsage};
    use amr::proxy::mappers::openai::{create_stream_chunk, OpenAIChatRequest, OpenAIMessage};
    use amr::proxy::router::{
        convert_claude_response_to_openai, convert_openai_to_claude, parse_model_and_reasoning,
        resolve_model, ReasoningEffort,
    };
    use amr::proxy::stats::ProxyStats;
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
            claude_percent: 100,
            claude_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
        };

        assert_eq!(
            resolve_model("claude-3-7-sonnet", &account),
            "claude-3-7-sonnet-20250219"
        );
        assert_eq!(
            resolve_model("claude-3-7-sonnet (low)", &account),
            "claude-3-7-sonnet-20250219"
        );
        assert_eq!(
            resolve_model("sonnet-3.7 (high)", &account),
            "claude-3-7-sonnet-20250219"
        );
        assert_eq!(
            resolve_model("claude-3-5-sonnet", &account),
            "claude-3-5-sonnet-20241022"
        );
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

        let (base_off, eff_off) =
            parse_model_and_reasoning("claude-3-7-sonnet (no-thinking)", None);
        assert_eq!(base_off, "claude-3-7-sonnet");
        assert_eq!(eff_off, Some(ReasoningEffort::Off));

        let (base_none, eff_none) = parse_model_and_reasoning("claude-opus-4-6 (none)", None);
        assert_eq!(base_none, "claude-opus-4-6");
        assert_eq!(eff_none, Some(ReasoningEffort::Off));

        let (base_none2, eff_none2) = parse_model_and_reasoning("claude-opus-4-6-none", None);
        assert_eq!(base_none2, "claude-opus-4-6");
        assert_eq!(eff_none2, Some(ReasoningEffort::Off));

        // Native protocol parameter detection
        let (base_nat, eff_nat) = parse_model_and_reasoning("claude-3-7-sonnet", Some("high"));
        assert_eq!(base_nat, "claude-3-7-sonnet");
        assert_eq!(eff_nat, Some(ReasoningEffort::High));
    }

    #[test]
    fn test_broadcast_models_generation() {
        use amr::config::{AccountModelStatus, UpstreamAccount};

        let mut cfg = AppConfig::default();
        cfg.accounts.push(UpstreamAccount {
            id: "acc_broadcast".into(),
            api_key: "test_token".into(),
            is_active: true,
            available_models: vec![
                AccountModelStatus {
                    id: "claude-sonnet-4-6".into(),
                    display_name: "Claude Sonnet 4.6".into(),
                    provider: "claude".into(),
                    supports_thinking: true,
                    remaining_fraction: 1.0,
                    modality: "text".into(),
                    is_recommended_agent: true,
                    sort_order: 0,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-3.8-flash-low".into(),
                    display_name: "Gemini 3.8 Flash (Low)".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 1.0,
                    modality: "text".into(),
                    is_recommended_agent: true,
                    sort_order: 1,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-3.8-flash-medium".into(),
                    display_name: "Gemini 3.8 Flash (Medium)".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 1.0,
                    modality: "text".into(),
                    is_recommended_agent: true,
                    sort_order: 2,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-3.8-flash-high".into(),
                    display_name: "Gemini 3.8 Flash (High)".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 1.0,
                    modality: "text".into(),
                    is_recommended_agent: true,
                    sort_order: 3,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        cfg.group_reasoning_clones = false;
        cfg.expose_reasoning_variants = true;
        cfg.rebuild_models_from_accounts();

        let broadcasted = cfg.get_broadcast_models();
        assert!(broadcasted.iter().any(|m| m["id"] == "claude-sonnet-4-6"));
        assert!(broadcasted
            .iter()
            .any(|m| m["id"] == "gemini-3.8-flash-low"));
        assert!(broadcasted
            .iter()
            .any(|m| m["id"] == "gemini-3.8-flash-medium"));
        assert!(broadcasted
            .iter()
            .any(|m| m["id"] == "gemini-3.8-flash-high"));

        // When group_reasoning_clones is enabled, effort variants collapse into canonical IDs
        cfg.group_reasoning_clones = true;
        cfg.rebuild_models_from_accounts();
        let grouped = cfg.get_broadcast_models();
        assert!(grouped.iter().any(|m| m["id"] == "claude-sonnet-4-6"));
        assert!(grouped
            .iter()
            .any(|m| m["id"] == "gemini-3.8-flash"
                && m["reasoning"]["supported_efforts"].is_array()));
        assert!(!grouped.iter().any(|m| m["id"] == "gemini-3.8-flash-low"));
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
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Hello world".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            stream: true,
            temperature: Some(0.7),
            max_tokens: Some(4096),
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };

        let claude_req = convert_openai_to_claude(
            &req,
            "claude-3-7-sonnet-20250219",
            Some(ReasoningEffort::Low),
        );
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
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
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
        let chunk = create_stream_chunk(
            "chatcmpl-test",
            "claude-3-7-sonnet",
            Some("chunk text"),
            None,
            None,
        );
        assert!(chunk.starts_with("data: "));
        assert!(chunk.contains("chunk text"));
        assert!(chunk.ends_with("\n\n"));
    }

    #[test]
    fn test_fingerprint_headers() {
        let mut headers = HeaderMap::new();
        apply_fingerprint(&mut headers, "Claude Code 2.x");

        assert!(headers.contains_key("user-agent"));
        assert!(headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("claude-code"));
        assert_eq!(
            headers.get("anthropic-version").unwrap().to_str().unwrap(),
            "2023-06-01"
        );
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
        use amr::proxy::antigravity::{
            build_antigravity_payload, convert_antigravity_response_to_openai,
        };

        let req = OpenAIChatRequest {
            model: "claude-3-7-sonnet".into(),
            messages: vec![
                OpenAIMessage {
                    role: "system".into(),
                    content: Value::String("You are Antigravity system.".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Write a test".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            stream: false,
            temperature: Some(0.2),
            max_tokens: Some(2048),
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };

        let payload_46 = build_antigravity_payload(
            &req,
            "claude-sonnet-4-6",
            Some("test-project"),
            Some(ReasoningEffort::Medium),
        );
        assert_eq!(payload_46["model"], "claude-sonnet-4-6");

        let payload = build_antigravity_payload(
            &req,
            "claude-sonnet-5-5",
            Some("test-project"),
            Some(ReasoningEffort::Medium),
        );
        assert_eq!(payload["model"], "claude-sonnet-5-5-high");
        assert_eq!(payload["project"], "test-project");
        assert_eq!(payload["userAgent"], "antigravity");
        assert!(payload["request"]["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Antigravity"));
        assert_eq!(payload["request"]["contents"][0]["role"], "user");
        assert_eq!(
            payload["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            4096
        );

        // Test Antigravity Response conversion with functionCall
        let raw_upstream_tools = serde_json::json!({
            "response": {
                "candidates": [{
                    "finishReason": "STOP",
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "cordis-plugin-development:skill",
                                    "args": { "param1": "val1" },
                                    "id": "call_123"
                                }
                            }
                        ]
                    }
                }]
            }
        });

        let oai_tools =
            convert_antigravity_response_to_openai(&raw_upstream_tools, "gemini-3.8-flash");
        assert_eq!(
            oai_tools.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
        let tc = oai_tools.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0]["id"], "call_123");
        assert_eq!(tc[0]["function"]["name"], "cordis-plugin-development:skill");

        // Test Antigravity request with tools mapping
        let req_with_tools = OpenAIChatRequest {
            model: "gemini-3.8-flash".into(),
            messages: vec![OpenAIMessage {
                role: "user".into(),
                content: Value::String("hi".into()),
                reasoning_content: None,
                refusal: None,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: true,
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: Some(vec![serde_json::json!({
                "type": "function",
                "function": {
                    "name": "search",
                    "description": "search tool",
                    "parameters": { "type": "object" }
                }
            })]),
            tool_choice: None,
            extra: Default::default(),
        };
        let payload_with_tools =
            build_antigravity_payload(&req_with_tools, "gemini-3.8-flash", None, None);
        assert!(
            payload_with_tools["request"]["tools"][0]["functionDeclarations"]
                .as_array()
                .is_some()
        );
        assert_eq!(
            payload_with_tools["request"]["tools"][0]["functionDeclarations"][0]["name"],
            "search"
        );
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
        assert_eq!(
            oai.choices[0].message.reasoning_content.as_deref(),
            Some("Deep thinking step...")
        );
        assert_eq!(
            oai.choices[0].message.content,
            Value::String("Code output here.".into())
        );
        assert_eq!(oai.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_thought_signature_cache_and_turn2_handling() {
        use amr::proxy::antigravity::{
            build_antigravity_payload, get_thought_signature, record_thought_signature,
        };

        // 1. Record thought signature for a tool call
        let call_id = "call_signed_12345";
        let sig = "EJgCGI4EGgBteWNyeXB0b3NpZw==";
        record_thought_signature(call_id, "search_web", sig);

        assert_eq!(get_thought_signature(call_id).as_deref(), Some(sig));
        assert_eq!(get_thought_signature("call_unknown"), None);

        // 2. Build Turn 2 payload with one signed call and one unsigned call
        let turn2_req = OpenAIChatRequest {
            model: "gemini-3.8-flash".into(),
            messages: vec![
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Please search for weather".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".into(),
                    content: Value::Null,
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: Some(vec![
                        serde_json::json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": "search_web",
                                "arguments": "{\"query\":\"weather\"}"
                            }
                        }),
                        serde_json::json!({
                            "id": "call_unsigned_67890",
                            "type": "function",
                            "function": {
                                "name": "calc",
                                "arguments": "{\"expr\":\"2+2\"}"
                            }
                        }),
                    ]),
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "tool".into(),
                    content: Value::String("Sunny, 20C".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: Some(call_id.into()),
                },
                OpenAIMessage {
                    role: "tool".into(),
                    content: Value::String("4".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: Some("call_unsigned_67890".into()),
                },
            ],
            stream: true,
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };

        let payload = build_antigravity_payload(&turn2_req, "gemini-3.8-flash", None, None);
        let contents = payload["request"]["contents"].as_array().expect("contents array");

        // Contents structure:
        // [0]: user ("Please search for weather")
        // [1]: model with parts:
        //      part 0: functionCall with thoughtSignature
        //      part 1: text fallback for unsigned call
        // [2]: user with merged tool responses

        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[2]["role"], "user");

        let model_parts = contents[1]["parts"].as_array().expect("model parts");
        assert_eq!(model_parts[0]["thoughtSignature"], sig);
        assert_eq!(model_parts[0]["functionCall"]["name"], "search_web");
        assert!(model_parts[1]["text"].as_str().unwrap().contains("[Called tool calc"));

        let user_tool_parts = contents[2]["parts"].as_array().expect("user parts");
        // tool responses are merged into the same user message
        assert_eq!(user_tool_parts.len(), 2);
        assert_eq!(user_tool_parts[0]["functionResponse"]["name"], "search_web");
        assert!(user_tool_parts[1]["text"].as_str().unwrap().contains("[Tool result for"));
    }

    #[test]
    fn test_balancer_cache_affinity_and_failover() {
        use amr::config::{CacheAffinityMode, RoutingStrategy};
        use amr::proxy::balancer::LoadBalancer;

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
            claude_percent: 80,
            claude_reset: "in 3h".into(),
            gemini_percent: 100,
            gemini_reset: "in 3h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
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
            claude_percent: 50,
            claude_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
        };

        cfg.accounts = vec![acc_a.clone(), acc_b.clone()];

        // Request 1 in conversation session "chat-1": should pick acc_a (80% remaining > 50% remaining)
        let chosen1 = balancer
            .select_account(&cfg, "claude-sonnet-4-6", Some("chat-1"))
            .expect("must pick account");
        assert_eq!(chosen1.id, "acc-a");

        // Request 2 in same session: should stick to acc_a for prompt caching (Cache hit!)
        let chosen2 = balancer
            .select_account(&cfg, "claude-sonnet-4-6", Some("chat-1"))
            .expect("must pick account");
        assert_eq!(chosen2.id, "acc-a");

        let (_, hits, _, _, active_pins) = balancer.get_metrics();
        assert_eq!(hits, 1);
        assert_eq!(active_pins, 1);

        // Test Cooldown Failover: mark acc-a as rate-limited (429)
        balancer.mark_cooldown("acc-a", 30);
        let chosen3 = balancer
            .select_account(&cfg, "claude-sonnet-4-6", Some("chat-1"))
            .expect("must failover");
        assert_eq!(chosen3.id, "acc-b"); // Automatically routed to acc-b!
    }

    #[test]
    fn test_quota_aware_model_family_filtering() {
        use amr::config::{AppConfig, CacheAffinityMode, RoutingStrategy, UpstreamAccount};
        use amr::proxy::balancer::LoadBalancer;

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
            claude_percent: 0,
            claude_reset: "in 4h".into(),
            gemini_percent: 100,
            gemini_reset: "in 4h".into(),
            claude_weekly_percent: 0,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
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
            claude_percent: 60,
            claude_reset: "in 2h".into(),
            gemini_percent: 90,
            gemini_reset: "in 2h".into(),
            claude_weekly_percent: 60,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
        };

        cfg.accounts = vec![acc_free.clone(), acc_pro.clone()];

        // 1. Claude request: Free account MUST NOT be selected; must route to acc-pro!
        let claude_chosen = balancer
            .select_account(&cfg, "claude-3-7-sonnet", None)
            .expect("must route to pro");
        assert_eq!(claude_chosen.id, "acc-pro");

        // 2. Gemini request: Both accounts are eligible, Free account has higher Gemini quota (100% > 90%), so selects acc-free!
        let gemini_chosen = balancer
            .select_account(&cfg, "gemini-3.8-flash", None)
            .expect("must route to free");
        assert_eq!(gemini_chosen.id, "acc-free");

        // 3. If acc-pro is disabled or exhausted, Claude requests return clean quota exhausted error!
        let mut cfg_only_free = cfg.clone();
        cfg_only_free.accounts = vec![acc_free.clone()];
        let err = balancer
            .select_account(&cfg_only_free, "claude-opus-4-6", None)
            .unwrap_err();
        match err {
            amr::proxy::balancer::RoutingError::ClaudeQuotaExhausted { reset } => {
                assert!(reset.contains("4h"));
            }
            other => panic!("expected ClaudeQuotaExhausted, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_session_fingerprint_multiturn() {
        use amr::proxy::balancer::LoadBalancer;

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
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Refactor this function to be lock-free".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            stream: true,
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
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
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Refactor this function to be lock-free".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "assistant".into(),
                    content: Value::String("Here is the lock-free implementation...".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                OpenAIMessage {
                    role: "user".into(),
                    content: Value::String("Can you add unit tests for it?".into()),
                    reasoning_content: None,
                    refusal: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            stream: true,
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };

        let sid1 = LoadBalancer::extract_session_fingerprint(&req1, None);
        let sid2 = LoadBalancer::extract_session_fingerprint(&req2, None);

        // Turn 1 and Turn 2 MUST have the exact same fingerprint to ensure prompt cache hit!
        assert_eq!(sid1, sid2);
    }

    #[test]
    fn test_multimodal_vision_conversion() {
        use amr::proxy::antigravity::{
            build_antigravity_payload, build_antigravity_payload_from_claude,
        };
        use amr::proxy::mappers::openai::{OpenAIChatRequest, OpenAIMessage};
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

        let pa_payload1 =
            build_antigravity_payload_from_claude(&anthropic_img_req, "gemini-2.5-pro", None, None);
        let contents1 = pa_payload1["request"]["contents"]
            .as_array()
            .expect("must have contents array");
        let parts1 = contents1[0]["parts"]
            .as_array()
            .expect("must have parts array");
        assert_eq!(parts1.len(), 2);
        assert_eq!(parts1[0]["text"], "What is in this screenshot?");
        assert_eq!(parts1[1]["inlineData"]["mimeType"], "image/png");
        assert!(parts1[1]["inlineData"]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBORw"));

        // 2. OpenAI-style image_url data URI
        let openai_img_req = OpenAIChatRequest {
            model: "gpt-4o".into(),
            messages: vec![OpenAIMessage {
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
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: false,
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };

        let pa_payload2 = build_antigravity_payload(&openai_img_req, "gemini-2.5-pro", None, None);
        let contents2 = pa_payload2["request"]["contents"]
            .as_array()
            .expect("must have contents array");
        let parts2 = contents2[0]["parts"]
            .as_array()
            .expect("must have parts array");
        assert_eq!(parts2.len(), 2);
        assert_eq!(parts2[0]["text"], "Inspect this image");
        assert_eq!(parts2[1]["inlineData"]["mimeType"], "image/jpeg");
        assert_eq!(
            parts2[1]["inlineData"]["data"],
            "/9j/4AAQSkZJRgABAQEASABIAAD/"
        );
    }

    #[test]
    fn test_quota_protection_threshold_and_error_exclusion() {
        use amr::config::{AppConfig, RoutingStrategy, UpstreamAccount};
        use amr::proxy::balancer::LoadBalancer;

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
            claude_percent: 5, // 5% < 10% threshold!
            claude_reset: "in 1h".into(),
            gemini_percent: 5,
            gemini_reset: "in 1h".into(),
            claude_weekly_percent: 5,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
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
            claude_percent: 50,
            claude_reset: "in 2h".into(),
            gemini_percent: 50,
            gemini_reset: "in 2h".into(),
            claude_weekly_percent: 50,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
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
            claude_percent: 100, // high quota, but status ERROR
            claude_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
        };

        cfg.accounts = vec![acc_low.clone(), acc_healthy.clone(), acc_error.clone()];

        // acc_error must be completely ignored.
        // acc_low is under threshold (5% < 10%), so balancer routes to acc-healthy.
        let chosen = balancer
            .select_account(&cfg, "claude-sonnet-4-6", None)
            .expect("must select account");
        assert_eq!(chosen.id, "acc-healthy");

        // If only acc_low and acc_error exist, balancer falls back gracefully to acc_low rather than failing
        cfg.accounts = vec![acc_low.clone(), acc_error.clone()];
        let fallback = balancer
            .select_account(&cfg, "claude-sonnet-4-6", None)
            .expect("must select fallback");
        assert_eq!(fallback.id, "acc-low");
    }

    #[test]
    fn test_jwt_email_extraction() {
        use amr::proxy::antigravity::extract_email_from_jwt;

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

    #[test]
    fn test_stream_usage_chunk_and_smoothing() {
        use amr::proxy::mappers::openai::{
            create_stream_chunk_with_usage, create_stream_usage_chunk, OpenAIUsage,
        };
        use amr::proxy::stream::emit_smooth_chunks;

        let usage = OpenAIUsage {
            prompt_tokens: 15,
            completion_tokens: 42,
            total_tokens: 57,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
        };

        // 1. Verify usage chunk format for DeepSeek Harness / OpenAI standard
        let chunk_with_usage = create_stream_chunk_with_usage(
            "chatcmpl-123",
            "gemini-3.8-flash",
            None,
            None,
            Some("stop"),
            Some(usage.clone()),
        );
        assert!(chunk_with_usage.contains("\"prompt_tokens\":15"));
        assert!(chunk_with_usage.contains("\"completion_tokens\":42"));
        assert!(chunk_with_usage.contains("\"finish_reason\":\"stop\""));

        let final_usage_chunk =
            create_stream_usage_chunk("chatcmpl-123", "gemini-3.8-flash", usage);
        assert!(final_usage_chunk.contains("\"prompt_tokens\":15"));
        assert!(final_usage_chunk.contains("\"choices\":[]"));

        let cached_usage = OpenAIUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            prompt_tokens_details: Some(amr::proxy::mappers::openai::PromptTokensDetails {
                cached_tokens: 80,
                audio_tokens: 0,
            }),
            completion_tokens_details: None,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
        };
        let cached_chunk = create_stream_usage_chunk("chatcmpl-123", "gemini-3.8-flash", cached_usage);
        assert!(cached_chunk.contains("\"cached_tokens\":80"));
        assert!(cached_chunk.contains("\"prompt_cache_hit_tokens\":80"));
        assert!(cached_chunk.contains("\"prompt_cache_miss_tokens\":20"));

        // 2. Verify smooth micro-chunking for natural streaming UI rendering
        let text = "fn calculate_fast_matrix_multiplication(a: &[f64], b: &[f64]) -> Vec<f64> {";
        let smooth = emit_smooth_chunks(text);
        assert!(smooth.len() >= 3);
        assert_eq!(smooth.concat(), text);
    }

    #[test]
    fn test_model_deduplication_and_grouping() {
        use amr::config::{AccountModelStatus, AppConfig, ModelCatalogItem, UpstreamAccount};

        let mut cfg = AppConfig::default();
        // Simulate corrupted config with internal models and duplicates
        cfg.models.push(ModelCatalogItem {
            id: "chat_20706".into(),
            display_name: "Internal Chat".into(),
            provider: "gemini".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: false,
            ..Default::default()
        });
        cfg.models.push(ModelCatalogItem {
            id: "gemini-3.8-flash".into(),
            display_name: "Gemini 3.8 Flash".into(),
            provider: "gemini".into(),
            tier: "PRO".into(),
            is_enabled: true,
            supports_thinking: true,
            ..Default::default()
        });

        cfg.sanitize_models();
        assert_eq!(cfg.models.len(), 1);
        assert!(cfg.models.iter().any(|m| m.id == "gemini-3.8-flash"));
        assert!(!cfg.models.iter().any(|m| m.id == "chat_20706"));

        // Verify dynamic per-account rebuild collapses effort variants & keeps disabled status
        cfg.accounts.push(UpstreamAccount {
            id: "acc1".into(),
            is_active: true,
            available_models: vec![
                AccountModelStatus {
                    id: "gemini-3.8-flash-low".into(),
                    display_name: "Gemini 3.8 Flash (Low)".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 0.9,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-3.8-flash-high".into(),
                    display_name: "Gemini 3.8 Flash (High)".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 0.9,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-4-argon-disabled".into(),
                    display_name: "Gemini 4 Argon".into(),
                    provider: "gemini".into(),
                    disabled: true,
                    remaining_fraction: 0.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        cfg.rebuild_models_from_accounts();
        assert!(cfg.models.iter().any(|m| m.id == "gemini-3.8-flash" && m.availability_status == "available"));
        assert!(cfg.models.iter().any(|m| m.id == "gemini-4-argon" && m.availability_status == "disabled"));
        assert!(cfg.models.iter().any(|m| m.id == "tts-1"));
        assert!(!cfg.models.iter().any(|m| m.id == "gemini-3.8-flash-medium"));
        let flash = cfg
            .models
            .iter()
            .find(|m| m.id == "gemini-3.8-flash")
            .unwrap();
        assert!(flash.supports_thinking);
    }

    #[tokio::test]
    async fn test_billing_and_usage_emulation() {
        use amr::config::AppConfig;
        use amr::proxy::server::{create_proxy_router, AppState};
        use amr::proxy::stats::ProxyStats;
        use axum::http::{Request, StatusCode};
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        use tower::ServiceExt;

        let stats = Arc::new(ProxyStats::new());
        stats.total_tokens.store(500_000, Ordering::Relaxed);
        let config = Arc::new(parking_lot::RwLock::new(AppConfig::default()));
        let balancer = Arc::new(amr::proxy::balancer::LoadBalancer::new());
        let http_client = reqwest::Client::new();
        let cfg_version = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let proxy_clients = Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new()));

        let state = AppState {
            config: config.clone(),
            stats: stats.clone(),
            balancer,
            http_client,
            cfg_version,
            proxy_clients,
        };

        let app = create_proxy_router(state);

        // 1. Subscription query
        let req_sub = Request::builder()
            .uri("/v1/dashboard/billing/subscription")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_sub = app.clone().oneshot(req_sub).await.unwrap();
        assert_eq!(resp_sub.status(), StatusCode::OK);

        // 2. Usage query
        let req_usage = Request::builder()
            .uri("/v1/dashboard/billing/usage")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_usage = app.clone().oneshot(req_usage).await.unwrap();
        assert_eq!(resp_usage.status(), StatusCode::OK);

        // 3. When disabled, returns 404
        config.write().enable_billing_emulation = false;
        let req_off = Request::builder()
            .uri("/v1/dashboard/billing/subscription")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp_off = app.oneshot(req_off).await.unwrap();
        assert_eq!(resp_off.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_exponential_ladder_backoff_and_auto_heal() {
        use amr::proxy::balancer::{HealthStage, LoadBalancer};

        let balancer = LoadBalancer::new();
        let acc_id = "test-acc-ladder";

        // Initial state: Healthy
        let (stage, count, rem) = balancer.get_health_status(acc_id);
        assert_eq!(stage, HealthStage::Healthy);
        assert_eq!(count, 0);
        assert!(rem.is_none());

        // Step 1: First 429 -> SoftPause (~3s)
        balancer.record_failure(acc_id, 429, 300, 15);
        let (stage, count, rem) = balancer.get_health_status(acc_id);
        assert_eq!(stage, HealthStage::SoftPause);
        assert_eq!(count, 1);
        assert!(rem.is_some());
        assert!(rem.unwrap() <= 5);

        // Step 2: Second 429 -> Backoff (~15s)
        balancer.record_failure(acc_id, 429, 300, 15);
        let (stage, count, rem) = balancer.get_health_status(acc_id);
        assert_eq!(stage, HealthStage::Backoff);
        assert_eq!(count, 2);
        assert!(rem.is_some());
        assert!(rem.unwrap() >= 10 && rem.unwrap() <= 20);

        // Step 3: Third 429 -> Cooldown (~60s)
        balancer.record_failure(acc_id, 429, 300, 15);
        let (stage, count, rem) = balancer.get_health_status(acc_id);
        assert_eq!(stage, HealthStage::Cooldown);
        assert_eq!(count, 3);
        assert!(rem.is_some());
        assert!(rem.unwrap() >= 45 && rem.unwrap() <= 75);

        // Step 4: Fourth 429 -> Quarantine (~300s)
        balancer.record_failure(acc_id, 429, 300, 15);
        let (stage, count, rem) = balancer.get_health_status(acc_id);
        assert_eq!(stage, HealthStage::Quarantine);
        assert_eq!(count, 4);
        assert!(rem.is_some());
        assert!(rem.unwrap() >= 240 && rem.unwrap() <= 360);

        // Step 5: Successful response -> Auto-Heal resets everything back to Healthy!
        balancer.record_success(acc_id);
        let (stage, count, rem) = balancer.get_health_status(acc_id);
        assert_eq!(stage, HealthStage::Healthy);
        assert_eq!(count, 0);
        assert!(rem.is_none());
    }

    #[test]
    fn test_detailed_health_and_manual_reset() {
        use amr::proxy::balancer::{HealthStage, LoadBalancer};

        let balancer = LoadBalancer::new();
        let acc_id = "test-detailed-acc";

        // Initial check
        let initial = balancer.get_detailed_health(acc_id);
        assert_eq!(initial.stage, HealthStage::Healthy);
        assert_eq!(initial.stage_level, 0);
        assert_eq!(initial.failure_count, 0);
        assert!(!initial.is_cooling);
        assert!(initial.remaining_secs.is_none());

        // Record failure with specific reason
        balancer.record_failure_with_reason(
            acc_id,
            429,
            90,
            0,
            Some("HTTP 429: Resource exhausted"),
        );
        let step1 = balancer.get_detailed_health(acc_id);
        assert_eq!(step1.stage, HealthStage::SoftPause);
        assert_eq!(step1.stage_level, 1);
        assert_eq!(step1.failure_count, 1);
        assert!(step1.is_cooling);
        assert_eq!(step1.last_status_code, Some(429));
        assert_eq!(
            step1.last_failure_reason.as_deref(),
            Some("HTTP 429: Resource exhausted")
        );

        // Update reason
        balancer.update_failure_reason(acc_id, 429, "HTTP 429: Updated quota details");
        let updated = balancer.get_detailed_health(acc_id);
        assert_eq!(
            updated.last_failure_reason.as_deref(),
            Some("HTTP 429: Updated quota details")
        );

        // Manual reset back to healthy
        balancer.reset_health(acc_id);
        let reset = balancer.get_detailed_health(acc_id);
        assert_eq!(reset.stage, HealthStage::Healthy);
        assert_eq!(reset.stage_level, 0);
        assert_eq!(reset.failure_count, 0);
        assert!(!reset.is_cooling);
        assert!(reset.remaining_secs.is_none());
        assert!(reset.last_failure_reason.is_none());
    }

    #[test]
    fn test_upstream_spoofing_headers_cleanliness_and_resilience() {
        use amr::proxy::antigravity::{
            get_official_hub_user_agent, FETCH_MODELS_ENDPOINTS, GENERATE_ENDPOINTS,
            LOAD_PROJECT_ENDPOINTS, QUOTA_SUMMARY_ENDPOINTS, STREAM_ENDPOINTS,
        };
        use amr::proxy::server::{build_upstream_headers, build_upstream_headers_json};

        // 1. Verify all endpoints never contain public unauthenticated/broken prod endpoint
        for ep in QUOTA_SUMMARY_ENDPOINTS
            .iter()
            .chain(STREAM_ENDPOINTS.iter())
            .chain(GENERATE_ENDPOINTS.iter())
            .chain(LOAD_PROJECT_ENDPOINTS.iter())
            .chain(FETCH_MODELS_ENDPOINTS.iter())
        {
            assert!(
                !ep.starts_with("https://cloudcode-pa.googleapis.com"),
                "Endpoint {} should not be bare prod endpoint",
                ep
            );
            assert!(
                ep.contains("daily-cloudcode-pa"),
                "Endpoint {} must target daily-cloudcode-pa",
                ep
            );
        }

        // 2. Verify Claude upstream headers
        let headers_claude = build_upstream_headers(
            " ya29.test_token_with_whitespace \n",
            "dummy_client_id",
            "dummy_machine_id",
            "LINUX",
            "2.29.1",
            "x86_64",
            "stable",
            true,
            true,
            "claude-sonnet-4-6",
        );

        assert_eq!(
            headers_claude.get("authorization").unwrap(),
            "Bearer ya29.test_token_with_whitespace"
        );
        assert_eq!(headers_claude.get("accept").unwrap(), "text/event-stream");
        assert_eq!(
            headers_claude.get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(
            headers_claude.get("anthropic-beta").unwrap(),
            "claude-code-20250219"
        );
        let ua = headers_claude.get("user-agent").unwrap().to_str().unwrap();
        assert!(ua.starts_with("antigravity/hub/2.29.1"));
        assert!(ua.contains("cl=1015941558"));
        assert!(ua.contains("os_type=linux"));
        assert!(ua.contains("arch=x86_64"));

        // Critical anti-leak check: ensure synthetic headers are NEVER attached to upstream requests
        assert!(!headers_claude.contains_key("x-goog-api-client"));
        assert!(!headers_claude.contains_key("x-client-id"));
        assert!(!headers_claude.contains_key("x-machine-id"));
        assert!(!headers_claude.contains_key("x-client-channel"));
        assert!(!headers_claude.contains_key("x-session-id"));
        assert!(!headers_claude.contains_key("x-api-key"));

        // 3. Verify Gemini upstream headers (must not attach anthropic-beta) & JSON non-stream accept
        let headers_gemini = build_upstream_headers_json(
            "ya29.gemini_token",
            "dummy_client_id",
            "dummy_machine_id",
            "WINDOWS",
            "2.29.1",
            "arm64",
            "stable",
            false,
            false,
            "gemini-3.8-flash-medium",
        );
        assert!(!headers_gemini.contains_key("anthropic-beta"));
        assert_eq!(headers_gemini.get("accept").unwrap(), "application/json");
        let ua_win = headers_gemini.get("user-agent").unwrap().to_str().unwrap();
        assert!(ua_win.contains("os_type=windows"));
        assert!(ua_win.contains("arch=arm64"));

        // 4. Verify get_official_hub_user_agent
        let official_ua = get_official_hub_user_agent();
        assert!(official_ua.starts_with("antigravity/hub/2.29.1"));
        assert!(official_ua.contains("cl=1015941558"));
    }

    #[test]
    fn test_audio_mime_detection_and_image_route() {
        use amr::proxy::antigravity::build_antigravity_payload;
        use amr::proxy::audio::{detect_audio_mime, normalize_audio_mime};
        use amr::proxy::router::resolve_model;

        // Test audio MIME detection
        assert_eq!(detect_audio_mime("recording.mp3"), "audio/mp3");
        assert_eq!(detect_audio_mime("voice.wav"), "audio/wav");
        assert_eq!(detect_audio_mime("audio.m4a"), "audio/aac");
        assert_eq!(detect_audio_mime("speech.ogg"), "audio/ogg");
        assert_eq!(detect_audio_mime("track.flac"), "audio/flac");
        assert_eq!(detect_audio_mime("clip.webm"), "audio/webm");

        assert_eq!(normalize_audio_mime("audio/wav"), "audio/wav");
        assert_eq!(normalize_audio_mime("mp3"), "audio/mp3");

        // Test image model resolution to gemini-3.1-flash-image
        let account = UpstreamAccount {
            id: "1".into(),
            name: "test".into(),
            provider: "google".into(),
            base_url: "https://cloudcode-pa.googleapis.com".into(),
            api_key: "test".into(),
            fingerprint: "Antigravity".into(),
            tier: "Pro".into(),
            quota_used: 0,
            quota_limit: 100000,
            quota_reset: "in 2h".into(),
            claude_percent: 100,
            claude_reset: "in 2h".into(),
            gemini_percent: 100,
            gemini_reset: "in 2h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
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
            available_models: Vec::new(),
        };

        assert_eq!(resolve_model("gemini-3.1-flash-image", &account), "gemini-3.1-flash-image");

        // Verify image generation payload conforms to official Antigravity schema:
        // requestType: "image_gen", no thinkingConfig, no systemInstruction, no tools
        let img_req = OpenAIChatRequest {
            model: "gemini-3.1-flash-image".into(),
            messages: vec![OpenAIMessage {
                role: "user".into(),
                content: serde_json::Value::String("A cybernetic cityscape".into()),
                reasoning_content: None,
                refusal: None,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: false,
            temperature: Some(0.7),
            max_tokens: Some(2048),
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };

        let payload = build_antigravity_payload(&img_req, "gemini-3.1-flash-image", Some("test-proj"), None);
        assert_eq!(payload["model"], "gemini-3.1-flash-image");
        assert_eq!(payload["requestType"], "image_gen");
        assert!(payload["request"].get("thinkingConfig").is_none());
        assert!(payload["request"].get("systemInstruction").is_none());
        assert!(payload["request"].get("tools").is_none());
    }

    #[test]
    fn test_deepseek_harness_reasoning_metadata_and_dispatch() {
        use amr::config::{AccountModelStatus, UpstreamAccount};

        let mut cfg = AppConfig::default();
        cfg.group_reasoning_clones = true;
        cfg.accounts.push(UpstreamAccount {
            id: "acc_ds".into(),
            api_key: "test_token".into(),
            is_active: true,
            available_models: vec![
                AccountModelStatus {
                    id: "gemini-3.8-flash-high".into(),
                    display_name: "Gemini 3.8 Flash (High)".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 1.0,
                    modality: "text".into(),
                    is_recommended_agent: true,
                    sort_order: 0,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-3.1-flash-image".into(),
                    display_name: "Gemini 3.1 Flash Image".into(),
                    provider: "gemini".into(),
                    supports_thinking: false,
                    remaining_fraction: 1.0,
                    modality: "image".into(),
                    is_recommended_agent: false,
                    sort_order: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        cfg.rebuild_models_from_accounts();

        let models = cfg.get_broadcast_models();
        let gemini = models.iter().find(|m| m["id"] == "gemini-3.8-flash").expect("gemini-3.8-flash not found");

        // Verify DeepSeek Harness & OpenAI standard capability and reasoning metadata
        assert_eq!(gemini["owned_by"], "google");
        assert_eq!(gemini["capabilities"]["reasoning_effort"], true);
        assert_eq!(gemini["capabilities"]["reasoningEffort"], true);
        assert_eq!(gemini["capabilities"]["thinking_budget"], true);
        assert_eq!(gemini["max_thinking_budget"], 32768);
        assert_eq!(gemini["min_thinking_budget"], 0);
        assert_eq!(gemini["thinking"]["budget_tokens"], 4000);
        assert!(gemini["supported_parameters"].as_array().unwrap().iter().any(|p| p == "reasoningEffort"));
        assert!(gemini["supported_parameters"].as_array().unwrap().iter().any(|p| p == "thinking_budget"));
        assert!(gemini["supported_reasoning_efforts"].as_array().unwrap().iter().any(|e| e == "high"));
        assert!(gemini["reasoning_efforts"].as_array().unwrap().iter().any(|e| e == "low"));
        assert_eq!(gemini["reasoningEfforts"]["high"], "high");
        assert_eq!(gemini["reasoning"]["default_effort"], "medium");

        // Test OpenAIChatRequest deserialization with camelCase reasoningEffort
        let json_camel = r#"{
            "model": "gemini-3.8-flash",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoningEffort": "high"
        }"#;
        let req_camel: OpenAIChatRequest = serde_json::from_str(json_camel).unwrap();
        assert_eq!(req_camel.reasoning_effort.as_deref(), Some("high"));

        let json_kebab = r#"{
            "model": "gemini-3.8-flash",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoning-effort": "low"
        }"#;
        let req_kebab: OpenAIChatRequest = serde_json::from_str(json_kebab).unwrap();
        assert_eq!(req_kebab.reasoning_effort.as_deref(), Some("low"));

        // Test router resolution for all effort tiers
        use amr::proxy::router::resolve_model_with_effort;
        let acc = UpstreamAccount {
            id: "acc_1".into(),
            name: "test".into(),
            provider: "antigravity".into(),
            base_url: "https://api.test".into(),
            api_key: "key".into(),
            fingerprint: "Default".into(),
            tier: "PRO".into(),
            quota_used: 0,
            quota_limit: 100,
            quota_reset: "in 2h".into(),
            claude_percent: 100,
            claude_reset: "in 4h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
            gemini_percent: 100,
            gemini_reset: "in 4h".into(),
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
            available_models: Vec::new(),
        };

        let (m_low, eff_low) = resolve_model_with_effort("gemini-3.8-flash", &acc, Some(ReasoningEffort::Low));
        assert_eq!(m_low, "gemini-3.8-flash-low");
        assert_eq!(eff_low, Some(ReasoningEffort::Low));

        let (m_med, eff_med) = resolve_model_with_effort("gemini-3.8-flash", &acc, Some(ReasoningEffort::Medium));
        assert_eq!(m_med, "gemini-3.8-flash-medium");
        assert_eq!(eff_med, Some(ReasoningEffort::Medium));

        let (m_high, eff_high) = resolve_model_with_effort("gemini-3.8-flash", &acc, Some(ReasoningEffort::High));
        assert_eq!(m_high, "gemini-3.8-flash-high");
        assert_eq!(eff_high, Some(ReasoningEffort::High));

        let (m_off, eff_off) = resolve_model_with_effort("gemini-3.8-flash", &acc, Some(ReasoningEffort::Off));
        assert_eq!(m_off, "gemini-3.8-flash-low");
        assert_eq!(eff_off, Some(ReasoningEffort::Off));

        // Verify real upstream models are preserved directly instead of being hijacked
        let (m_25, _) = resolve_model_with_effort("gemini-2.5-flash", &acc, None);
        assert_eq!(m_25, "gemini-2.5-flash");
        let (m_3f, _) = resolve_model_with_effort("gemini-3-flash", &acc, None);
        assert_eq!(m_3f, "gemini-3-flash");
        let (m_oss, _) = resolve_model_with_effort("gpt-oss-120b-medium", &acc, None);
        assert_eq!(m_oss, "gpt-oss-120b-medium");

        // Test sanitize_models preservation of utility modality models
        cfg.sanitize_models();
        assert!(cfg.models.iter().any(|m| m.id == "gemini-3.1-flash-image"));
        assert!(cfg.models.iter().any(|m| m.id == "whisper-1"));
        assert!(cfg.models.iter().any(|m| m.id == "tts-1"));
    }

    #[test]
    fn test_multimodal_capabilities_and_web_search_grounding() {
        use amr::proxy::antigravity::{
            aspect_ratio_from_size_or_hint, build_antigravity_payload, build_web_search_payload,
            convert_antigravity_response_to_openai, extract_grounding_info,
        };
        use amr::proxy::router::resolve_model;

        use amr::config::AccountModelStatus;
        let mut cfg = AppConfig::default();
        cfg.accounts.push(UpstreamAccount {
            id: "acc_mm".into(),
            is_active: true,
            available_models: vec![
                AccountModelStatus {
                    id: "gemini-3.8-flash".into(),
                    display_name: "Gemini 3.8 Flash".into(),
                    provider: "gemini".into(),
                    supports_thinking: true,
                    remaining_fraction: 1.0,
                    modality: "text".into(),
                    is_recommended_agent: true,
                    sort_order: 0,
                    ..Default::default()
                },
                AccountModelStatus {
                    id: "gemini-3.1-flash-image".into(),
                    display_name: "Gemini 3.1 Flash Image".into(),
                    provider: "gemini".into(),
                    supports_thinking: false,
                    remaining_fraction: 1.0,
                    modality: "image".into(),
                    is_recommended_agent: false,
                    sort_order: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        cfg.rebuild_models_from_accounts();
        let broadcasted = cfg.get_broadcast_models();

        let img = broadcasted.iter().find(|m| m["id"] == "gemini-3.1-flash-image").unwrap();
        assert_eq!(img["capabilities"]["image_generation"], true);
        assert_eq!(img["capabilities"]["reasoning"], false);
        assert_eq!(img["output_modalities"], serde_json::json!(["image", "text"]));

        let stt = broadcasted.iter().find(|m| m["id"] == "whisper-1").unwrap();
        assert_eq!(stt["capabilities"]["audio_transcription"], true);
        assert_eq!(stt["input_modalities"], serde_json::json!(["audio"]));

        let tts = broadcasted.iter().find(|m| m["id"] == "tts-1").unwrap();
        assert_eq!(tts["capabilities"]["audio_speech"], true);
        assert_eq!(tts["output_modalities"], serde_json::json!(["audio"]));

        let acc = UpstreamAccount {
            id: "acc_1".into(),
            name: "test".into(),
            provider: "antigravity".into(),
            base_url: "https://api.test".into(),
            api_key: "key".into(),
            fingerprint: "Default".into(),
            tier: "PRO".into(),
            quota_used: 0,
            quota_limit: 100,
            quota_reset: "in 2h".into(),
            claude_percent: 100,
            claude_reset: "in 4h".into(),
            claude_weekly_percent: 100,
            claude_weekly_reset: "in 7d".into(),
            gemini_percent: 100,
            gemini_reset: "in 4h".into(),
            gemini_weekly_percent: 100,
            gemini_weekly_reset: "in 7d".into(),
            status: "Active".into(),
            error_message: None,
            refresh_token: None,
            project_id: Some("proj-1".into()),
            is_active: true,
            added_at: None,
            device_profile: None,
            proxy_url: None,
            available_models: Vec::new(),
        };
        assert_eq!(resolve_model("whisper-1", &acc), "whisper-1");
        assert_eq!(resolve_model("tts-1", &acc), "tts-1");
        assert_eq!(resolve_model("web-search", &acc), "web-search");

        // Verify native Google Gemini model list builder and payload
        let gemini_models = amr::proxy::mappers::gemini::build_gemini_models_list(&cfg.models);
        assert!(gemini_models["models"].as_array().unwrap().iter().any(|m| m["name"] == "models/gemini-3.8-flash"));
        let gem_req = serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "Hello Gemini"}]}],
            "generationConfig": {"temperature": 0.5}
        });
        let gem_payload = amr::proxy::mappers::gemini::build_antigravity_from_gemini(&gem_req, "gemini-3.8-flash", Some("proj-1"));
        assert_eq!(gem_payload["project"], "proj-1");
        assert_eq!(gem_payload["request"]["contents"][0]["parts"][0]["text"], "Hello Gemini");

        // Verify MCP protocol tools structure
        let mcp_tools = serde_json::json!({
            "tools": [
                { "name": "google_web_search" },
                { "name": "fetch_web_page" }
            ]
        });
        assert!(mcp_tools["tools"].as_array().unwrap().iter().any(|t| t["name"] == "google_web_search"));
        assert!(mcp_tools["tools"].as_array().unwrap().iter().any(|t| t["name"] == "fetch_web_page"));

        assert_eq!(aspect_ratio_from_size_or_hint(None, Some("1792x1024")), Some("16:9"));
        assert_eq!(aspect_ratio_from_size_or_hint(Some("3:4"), None), Some("3:4"));

        // Direct web search payload schema (GetWebSearchResults)
        let ws_payload = build_web_search_payload("rust release", Some("rust-lang.org"), "web-search", Some("proj-1"));
        assert_eq!(ws_payload["requestType"], "web_search");
        assert_eq!(ws_payload["model"], "gemini-2.5-flash");
        assert!(ws_payload["request"]["contents"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("site:rust-lang.org"));
        assert!(ws_payload["request"]["tools"][0].get("googleSearch").is_some());

        // Chat completion with web_search tool maps to googleSearch
        let req = OpenAIChatRequest {
            model: "gemini-3.8-flash".into(),
            messages: vec![OpenAIMessage {
                role: "user".into(),
                content: Value::String("Latest Rust version?".into()),
                reasoning_content: None,
                refusal: None,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: false,
            temperature: None,
            max_tokens: None,
            max_completion_tokens: None,
            reasoning_effort: None,
            stream_options: None,
            tools: Some(vec![serde_json::json!({ "type": "web_search" })]),
            tool_choice: None,
            extra: Default::default(),
        };
        let chat_payload = build_antigravity_payload(&req, "gemini-3.8-flash", Some("proj-1"), None);
        assert!(chat_payload["request"]["tools"][0].get("googleSearch").is_some());

        // Grounding metadata parsing & citation rendering
        let fake_raw = serde_json::json!({
            "response": {
                "candidates": [{
                    "content": { "parts": [{ "text": "Rust 1.85 is released." }] },
                    "groundingMetadata": {
                        "webSearchQueries": ["rust latest version"],
                        "groundingChunks": [{ "web": { "uri": "https://blog.rust-lang.org/", "title": "Rust Blog" } }],
                        "groundingSupports": [{ "segment": { "text": "Rust 1.85 is released." }, "groundingChunkIndices": [0] }]
                    }
                }]
            }
        });
        let (queries, results, md) = extract_grounding_info(fake_raw.get("response").unwrap());
        assert_eq!(queries, vec!["rust latest version"]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Rust Blog");
        assert!(md.contains("https://blog.rust-lang.org/"));

        let oai = convert_antigravity_response_to_openai(&fake_raw, "web-search");
        assert!(oai.choices[0].message.content.as_str().unwrap().contains("### Sources"));
    }
}
