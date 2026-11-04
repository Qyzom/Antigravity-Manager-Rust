use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogItem {
    pub id: String,
    pub time: String,
    pub method: String,
    pub path: String,
    pub model: String,
    pub status: u16,
    pub tokens: u64,
    pub latency_ms: u64,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub request_headers: String,
    #[serde(default)]
    pub request_body: String,
    #[serde(default)]
    pub response_body: String,
    #[serde(default)]
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistentStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub error_requests: u64,
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub thinking_tokens: u64,
    pub total_latency_ms: u64,
    pub max_latency_ms: u64,
}

pub struct ProxyStats {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub error_requests: AtomicU64,
    pub total_tokens: AtomicU64,
    pub prompt_tokens: AtomicU64,
    pub completion_tokens: AtomicU64,
    pub thinking_tokens: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub max_latency_ms: AtomicU64,
    pub logging_enabled: AtomicBool,
    pub max_log_capacity: AtomicUsize,
    pub recent_logs: RwLock<VecDeque<LogItem>>,
}

impl ProxyStats {
    pub fn new() -> Self {
        let stats = Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            error_requests: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            prompt_tokens: AtomicU64::new(0),
            completion_tokens: AtomicU64::new(0),
            thinking_tokens: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            max_latency_ms: AtomicU64::new(0),
            logging_enabled: AtomicBool::new(true),
            max_log_capacity: AtomicUsize::new(100),
            recent_logs: RwLock::new(VecDeque::with_capacity(500)),
        };

        // Try load previous stats from stats.json
        if let Some(loaded) = Self::load_persistent_stats() {
            stats.total_requests.store(loaded.total_requests, Ordering::Relaxed);
            stats.successful_requests.store(loaded.successful_requests, Ordering::Relaxed);
            stats.error_requests.store(loaded.error_requests, Ordering::Relaxed);
            stats.total_tokens.store(loaded.total_tokens, Ordering::Relaxed);
            stats.prompt_tokens.store(loaded.prompt_tokens, Ordering::Relaxed);
            stats.completion_tokens.store(loaded.completion_tokens, Ordering::Relaxed);
            stats.thinking_tokens.store(loaded.thinking_tokens, Ordering::Relaxed);
            stats.total_latency_ms.store(loaded.total_latency_ms, Ordering::Relaxed);
            stats.max_latency_ms.store(loaded.max_latency_ms, Ordering::Relaxed);
        }

        stats
    }

    pub fn stats_file_path() -> PathBuf {
        let cfg_path = crate::config::AppConfig::config_path();
        cfg_path.parent().unwrap_or(&cfg_path).join("stats.json")
    }

    pub fn logs_file_path() -> PathBuf {
        let cfg_path = crate::config::AppConfig::config_path();
        cfg_path.parent().unwrap_or(&cfg_path).join("traffic.jsonl")
    }

    pub fn logs_folder_path() -> PathBuf {
        let cfg_path = crate::config::AppConfig::config_path();
        cfg_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
    }

    fn load_persistent_stats() -> Option<PersistentStats> {
        let path = Self::stats_file_path();
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                return serde_json::from_str(&content).ok();
            }
        }
        None
    }

    pub fn save_persistent_stats(&self) {
        let s = PersistentStats {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            successful_requests: self.successful_requests.load(Ordering::Relaxed),
            error_requests: self.error_requests.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            thinking_tokens: self.thinking_tokens.load(Ordering::Relaxed),
            total_latency_ms: self.total_latency_ms.load(Ordering::Relaxed),
            max_latency_ms: self.max_latency_ms.load(Ordering::Relaxed),
        };
        let path = Self::stats_file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&s) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub fn reset_stats(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.successful_requests.store(0, Ordering::Relaxed);
        self.error_requests.store(0, Ordering::Relaxed);
        self.total_tokens.store(0, Ordering::Relaxed);
        self.prompt_tokens.store(0, Ordering::Relaxed);
        self.completion_tokens.store(0, Ordering::Relaxed);
        self.thinking_tokens.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
        self.max_latency_ms.store(0, Ordering::Relaxed);
        self.save_persistent_stats();
    }

    pub fn record_request(
        &self,
        method: &str,
        path: &str,
        model: &str,
        status: u16,
        tokens: u64,
        latency_ms: u64,
    ) {
        self.record_detailed_request(
            method,
            path,
            model,
            status,
            tokens,
            latency_ms,
            "Auto-Selected",
            "",
            "",
            "",
            None,
        );
    }

    pub fn record_detailed_request(
        &self,
        method: &str,
        path: &str,
        model: &str,
        status: u16,
        tokens: u64,
        latency_ms: u64,
        account: &str,
        request_headers: &str,
        request_body: &str,
        response_body: &str,
        error_msg: Option<&str>,
    ) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if (200..300).contains(&status) {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.error_requests.fetch_add(1, Ordering::Relaxed);
        }

        self.total_tokens.fetch_add(tokens, Ordering::Relaxed);
        let prompt_est = (tokens as f64 * 0.45) as u64;
        let completion_est = (tokens as f64 * 0.40) as u64;
        let thinking_est = tokens.saturating_sub(prompt_est + completion_est);
        self.prompt_tokens.fetch_add(prompt_est, Ordering::Relaxed);
        self.completion_tokens.fetch_add(completion_est, Ordering::Relaxed);
        self.thinking_tokens.fetch_add(thinking_est, Ordering::Relaxed);

        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        let current_max = self.max_latency_ms.load(Ordering::Relaxed);
        if latency_ms > current_max {
            self.max_latency_ms.store(latency_ms, Ordering::Relaxed);
        }

        // Periodically save stats to stats.json
        if self.total_requests.load(Ordering::Relaxed) % 5 == 0 {
            self.save_persistent_stats();
        }

        if self.logging_enabled.load(Ordering::Relaxed) {
            let max_cap = self.max_log_capacity.load(Ordering::Relaxed).max(10);
            let item = LogItem {
                id: uuid::Uuid::new_v4().to_string(),
                time: Local::now().format("%H:%M:%S").to_string(),
                method: method.to_string(),
                path: path.to_string(),
                model: model.to_string(),
                status,
                tokens,
                latency_ms,
                account: account.to_string(),
                request_headers: request_headers.to_string(),
                request_body: request_body.to_string(),
                response_body: response_body.to_string(),
                error_msg: error_msg.map(|s| s.to_string()),
            };

            // Write to disk traffic.jsonl
            let log_path = Self::logs_file_path();
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
                if let Ok(line) = serde_json::to_string(&item) {
                    let _ = writeln!(file, "{}", line);
                }
            }

            // Write to in-memory ring buffer
            if let Ok(mut logs) = self.recent_logs.write() {
                while logs.len() >= max_cap {
                    logs.pop_back();
                }
                logs.push_front(item);
            }
        }
    }

    pub fn get_metrics(&self) -> (u64, u64, u64) {
        let reqs = self.total_requests.load(Ordering::Relaxed);
        let tokens = self.total_tokens.load(Ordering::Relaxed);
        let total_lat = self.total_latency_ms.load(Ordering::Relaxed);
        let avg_lat = if reqs > 0 { total_lat / reqs } else { 0 };
        (reqs, tokens, avg_lat)
    }

    pub fn get_detailed_stats(&self) -> (u64, u64, u64, u64, u64, u64, u64, u64) {
        let reqs = self.total_requests.load(Ordering::Relaxed);
        let success = self.successful_requests.load(Ordering::Relaxed);
        let errors = self.error_requests.load(Ordering::Relaxed);
        let prompt = self.prompt_tokens.load(Ordering::Relaxed);
        let completion = self.completion_tokens.load(Ordering::Relaxed);
        let thinking = self.thinking_tokens.load(Ordering::Relaxed);
        let total_lat = self.total_latency_ms.load(Ordering::Relaxed);
        let avg_lat = if reqs > 0 { total_lat / reqs } else { 0 };
        let max_lat = self.max_latency_ms.load(Ordering::Relaxed);
        (reqs, success, errors, prompt, completion, thinking, avg_lat, max_lat)
    }

    pub fn get_recent_logs(&self) -> Vec<LogItem> {
        if let Ok(logs) = self.recent_logs.read() {
            logs.iter().cloned().collect()
        } else {
            Vec::new()
        }
    }

    pub fn clear_logs(&self) {
        if let Ok(mut logs) = self.recent_logs.write() {
            logs.clear();
        }
        let log_path = Self::logs_file_path();
        let _ = std::fs::remove_file(log_path);
    }
}
