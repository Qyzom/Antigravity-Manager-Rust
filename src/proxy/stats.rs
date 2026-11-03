use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
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
    pub logging_enabled: std::sync::atomic::AtomicBool,
    pub max_log_capacity: std::sync::atomic::AtomicUsize,
    pub recent_logs: RwLock<VecDeque<LogItem>>,
}

impl ProxyStats {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            error_requests: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            prompt_tokens: AtomicU64::new(0),
            completion_tokens: AtomicU64::new(0),
            thinking_tokens: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            max_latency_ms: AtomicU64::new(0),
            logging_enabled: std::sync::atomic::AtomicBool::new(true),
            max_log_capacity: std::sync::atomic::AtomicUsize::new(100),
            recent_logs: RwLock::new(VecDeque::with_capacity(500)),
        }
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
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if (200..300).contains(&status) {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.error_requests.fetch_add(1, Ordering::Relaxed);
        }

        self.total_tokens.fetch_add(tokens, Ordering::Relaxed);
        // Distribute token estimates if exact details not broken down
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
            };

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
    }
}
