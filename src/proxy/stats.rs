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
    pub total_tokens: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub recent_logs: RwLock<VecDeque<LogItem>>,
}

impl ProxyStats {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            recent_logs: RwLock::new(VecDeque::with_capacity(100)),
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
        self.total_tokens.fetch_add(tokens, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);

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
            if logs.len() >= 100 {
                logs.pop_back();
            }
            logs.push_front(item);
        }
    }

    pub fn get_metrics(&self) -> (u64, u64, u64) {
        let reqs = self.total_requests.load(Ordering::Relaxed);
        let tokens = self.total_tokens.load(Ordering::Relaxed);
        let total_lat = self.total_latency_ms.load(Ordering::Relaxed);
        let avg_lat = if reqs > 0 { total_lat / reqs } else { 0 };
        (reqs, tokens, avg_lat)
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
