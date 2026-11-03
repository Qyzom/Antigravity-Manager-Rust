use crate::config::{AppConfig, CacheAffinityMode, RoutingStrategy, UpstreamAccount};
use crate::proxy::mappers::openai::OpenAIChatRequest;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub struct LoadBalancer {
    /// Active session pin mappings: session_id -> (account_id, last_access_time)
    session_bindings: Arc<RwLock<HashMap<String, (String, Instant)>>>,
    /// Cooldown tracker: account_id -> cooldown_expiry
    account_cooldowns: Arc<RwLock<HashMap<String, Instant>>>,
    /// Round-robin counter
    rr_counter: AtomicU64,
    /// Metrics
    total_routed: AtomicU64,
    cache_hits: AtomicU64,
    failover_rotations: AtomicU64,
}

impl Default for LoadBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer {
    pub fn new() -> Self {
        Self {
            session_bindings: Arc::new(RwLock::new(HashMap::new())),
            account_cooldowns: Arc::new(RwLock::new(HashMap::new())),
            rr_counter: AtomicU64::new(0),
            total_routed: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            failover_rotations: AtomicU64::new(0),
        }
    }

    /// Extract or compute a rock-solid, stable session fingerprint across multi-turn interactions
    pub fn extract_session_fingerprint(
        req: &OpenAIChatRequest,
        header_session_id: Option<&str>,
    ) -> String {
        // 1. Explicit header session ID
        if let Some(sid) = header_session_id {
            let trimmed = sid.trim();
            if !trimmed.is_empty() {
                return format!("hdr-{}", trimmed);
            }
        }

        // 2. Extra fields inside JSON request body
        if let Some(sid) = req.extra.get("session_id").and_then(|v| v.as_str()) {
            if !sid.trim().is_empty() {
                return format!("body-{}", sid.trim());
            }
        }
        if let Some(cid) = req.extra.get("conversation_id").and_then(|v| v.as_str()) {
            if !cid.trim().is_empty() {
                return format!("conv-{}", cid.trim());
            }
        }
        if let Some(user) = req.extra.get("user").and_then(|v| v.as_str()) {
            if !user.trim().is_empty() {
                return format!("usr-{}", user.trim());
            }
        }

        // 3. Multi-turn Conversation Hash:
        // Hash (System Prompt + First User Message). This guarantees that
        // Turn 1, Turn 2, ... Turn N of the same conversation produce the exact same session fingerprint!
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut found_user_msg = false;

        for msg in &req.messages {
            let content_str = match &msg.content {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let trimmed = content_str.trim();

            if msg.role == "system" || msg.role == "developer" {
                hasher.write(b"[sys]");
                hasher.write(trimmed.as_bytes());
            } else if msg.role == "user" && !found_user_msg && !trimmed.is_empty() {
                hasher.write(b"[user1]");
                hasher.write(trimmed.as_bytes());
                found_user_msg = true;
            }
        }

        if !found_user_msg {
            // Fallback: hash the last message
            if let Some(last) = req.messages.last() {
                let s = last.content.to_string();
                hasher.write(s.as_bytes());
            }
        }

        format!("ctx-{:016x}", hasher.finish())
    }

    /// Mark account in temporary cooldown on rate-limit (429) or transient 5xx error
    pub fn mark_cooldown(&self, account_id: &str, duration_secs: u64) {
        let expiry = Instant::now() + std::time::Duration::from_secs(duration_secs);
        {
            let mut cds = self.account_cooldowns.write();
            cds.insert(account_id.to_string(), expiry);
        }

        // Immediately unbind all sessions pinned to this account so next requests failover without delay
        {
            let mut bindings = self.session_bindings.write();
            bindings.retain(|_, (acc_id, _)| acc_id != account_id);
        }

        self.failover_rotations.fetch_add(1, Ordering::Relaxed);
    }

    /// Select optimal upstream account with cache affinity, quota balancing, and failover
    pub fn select_account(
        &self,
        config: &AppConfig,
        session_id: Option<&str>,
    ) -> Option<UpstreamAccount> {
        let now = Instant::now();
        self.total_routed.fetch_add(1, Ordering::Relaxed);

        // 1. Filter healthy accounts
        let mut healthy_accounts: Vec<UpstreamAccount> = {
            let cds = self.account_cooldowns.read();
            config
                .accounts
                .iter()
                .filter(|a| {
                    if !a.is_active || a.status != "Active" || a.api_key.is_empty() {
                        return false;
                    }
                    if let Some(exp) = cds.get(&a.id) {
                        if *exp > now {
                            return false; // Still cooling down
                        }
                    }
                    true
                })
                .cloned()
                .collect()
        };

        // If all accounts are in cooldown, clear expired cooldowns and take any active account
        if healthy_accounts.is_empty() {
            let mut cds = self.account_cooldowns.write();
            cds.clear();
            healthy_accounts = config
                .accounts
                .iter()
                .filter(|a| a.is_active && !a.api_key.is_empty())
                .cloned()
                .collect();
        }

        if healthy_accounts.is_empty() {
            return None;
        }

        // 2. Routing strategy overrides (e.g. strict Failover)
        if config.routing_strategy == RoutingStrategy::Failover {
            if let Some(active_id) = &config.active_account_id {
                if let Some(acc) = healthy_accounts.iter().find(|a| &a.id == active_id) {
                    return Some(acc.clone());
                }
            }
            return healthy_accounts.first().cloned();
        }

        // 3. Cache Affinity Evaluation
        let ttl_secs = config.session_sticky_ttl_secs;
        let mode = config.cache_affinity_mode;

        if let Some(sid) = session_id {
            // Check existing session pin
            let existing_pin = {
                let bindings = self.session_bindings.read();
                bindings.get(sid).cloned()
            };

            if let Some((bound_id, last_access)) = existing_pin {
                let elapsed_secs = last_access.elapsed().as_secs();

                if elapsed_secs < ttl_secs {
                    if let Some(bound_acc) = healthy_accounts.iter().find(|a| a.id == bound_id) {
                        // Account is alive. Now check mode rules:
                        let bound_rem = bound_acc.quota_limit.saturating_sub(bound_acc.quota_used);

                        match mode {
                            CacheAffinityMode::CacheFirst => {
                                // Strictly stick to the same account as long as it has remaining quota
                                if bound_rem > 0 || bound_acc.quota_limit == 0 {
                                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                                    let mut bindings = self.session_bindings.write();
                                    bindings.insert(sid.to_string(), (bound_id, now));
                                    return Some(bound_acc.clone());
                                }
                            }
                            CacheAffinityMode::Balanced => {
                                // Sticky to session, but check if another account has significantly higher quota
                                let max_other_rem = healthy_accounts
                                    .iter()
                                    .filter(|a| a.id != bound_id)
                                    .map(|a| a.quota_limit.saturating_sub(a.quota_used))
                                    .max()
                                    .unwrap_or(0);

                                let delta = max_other_rem.saturating_sub(bound_rem);
                                let base = bound_acc.quota_limit.max(1) as f64;
                                let imbalance_pct = (delta as f64 / base) * 100.0;

                                if (imbalance_pct as u32) < config.quota_imbalance_threshold {
                                    // Within threshold: stick to account to maximize prompt caching!
                                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                                    let mut bindings = self.session_bindings.write();
                                    bindings.insert(sid.to_string(), (bound_id, now));
                                    return Some(bound_acc.clone());
                                } else {
                                    // Imbalance exceeded: smoothly rotate to level out quota
                                    self.failover_rotations.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            CacheAffinityMode::AggressiveSpread => {
                                // Do not stick: level out on every request
                            }
                        }
                    }
                }
            }
        }

        // 4. Fallback selection when no pin exists or pin was rotated
        let chosen_acc = match config.routing_strategy {
            RoutingStrategy::RoundRobin => {
                let idx = (self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize)
                    % healthy_accounts.len();
                healthy_accounts[idx].clone()
            }
            RoutingStrategy::QuotaAware | RoutingStrategy::Failover => {
                // Select account with maximum remaining quota
                healthy_accounts
                    .into_iter()
                    .max_by_key(|a| a.quota_limit.saturating_sub(a.quota_used))
                    .unwrap()
            }
        };

        // Pin session to chosen account if session_id is present and not AggressiveSpread
        if let Some(sid) = session_id {
            if mode != CacheAffinityMode::AggressiveSpread {
                let mut bindings = self.session_bindings.write();
                bindings.insert(sid.to_string(), (chosen_acc.id.clone(), now));
            }
        }

        Some(chosen_acc)
    }

    /// Cleanup old expired sessions periodically
    pub fn cleanup_expired_sessions(&self, ttl_secs: u64) {
        let now = Instant::now();
        let mut bindings = self.session_bindings.write();
        bindings.retain(|_, (_, last_access)| now.duration_since(*last_access).as_secs() < ttl_secs);
    }

    /// Balancer Telemetry Metrics: (total_routed, cache_hits, failovers, hit_rate_pct, active_pins)
    pub fn get_metrics(&self) -> (u64, u64, u64, f64, usize) {
        let routed = self.total_routed.load(Ordering::Relaxed);
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let failovers = self.failover_rotations.load(Ordering::Relaxed);
        let hit_rate = if routed > 0 {
            (hits as f64 / routed as f64) * 100.0
        } else {
            100.0
        };
        let active_pins = self.session_bindings.read().len();
        (routed, hits, failovers, hit_rate, active_pins)
    }
}
