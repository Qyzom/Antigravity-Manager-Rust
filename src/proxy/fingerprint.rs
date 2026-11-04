use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FingerprintProfile {
    pub name: &'static str,
    pub user_agent: &'static str,
    pub extra_headers: Vec<(&'static str, &'static str)>,
}

pub fn get_profile(name: &str) -> FingerprintProfile {
    match name {
        "Google Antigravity Client (1.107.0)" | "Antigravity IDE" | "Antigravity CLI" => FingerprintProfile {
            name: "Google Antigravity Client (1.107.0)",
            user_agent: "antigravity/1.107.0 linux/x64",
            extra_headers: vec![
                ("x-goog-api-client", "gl-go/1.21.0 gdcl/0.1.0"),
                ("x-client-vendor", "Google-Antigravity"),
            ],
        },
        "Claude Code 2.x" => FingerprintProfile {
            name: "Claude Code 2.x",
            user_agent: "claude-code/0.2.29 (darwin-arm64)",
            extra_headers: vec![
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "prompt-caching-2024-07-31,thinking-2025-01-24"),
                ("x-client-version", "0.2.29"),
            ],
        },
        "Cursor Pro" => FingerprintProfile {
            name: "Cursor Pro",
            user_agent: "Cursor/0.46.10 Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            extra_headers: vec![
                ("sec-ch-ua", "\"Not A(Brand\";v=\"8\", \"Chromium\";v=\"132\", \"Cursor\";v=\"132\""),
                ("sec-ch-ua-platform", "\"Windows\""),
                ("sec-fetch-site", "cross-site"),
                ("sec-fetch-mode", "cors"),
            ],
        },
        "OpenCode" => FingerprintProfile {
            name: "OpenCode",
            user_agent: "OpenCode/1.1.0",
            extra_headers: vec![
                ("x-app-name", "opencode"),
            ],
        },
        _ => FingerprintProfile {
            name: "Google Antigravity Client (1.107.0)",
            user_agent: "antigravity/1.107.0 linux/x64",
            extra_headers: vec![
                ("x-goog-api-client", "gl-go/1.21.0 gdcl/0.1.0"),
            ],
        },
    }
}

pub fn apply_fingerprint(headers: &mut HeaderMap, profile_name: &str) {
    let profile = get_profile(profile_name);
    if let Ok(ua_val) = HeaderValue::from_str(profile.user_agent) {
        headers.insert(reqwest::header::USER_AGENT, ua_val);
    }
    for (k, v) in profile.extra_headers {
        if let (Ok(h_key), Ok(h_val)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            headers.insert(h_key, h_val);
        }
    }
}
