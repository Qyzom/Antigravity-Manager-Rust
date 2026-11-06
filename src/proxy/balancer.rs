use crate::config::{AppConfig, CacheAffinityMode, RoutingStrategy, UpstreamAccount};
use crate::proxy::mappers::openai::OpenAIChatRequest;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    ClaudeOrGpt,
    Gemini,
}

impl ModelFamily {
    pub fn from_model(model: &str) -> Self {
        let m = model.to_lowercase();
        if m.contains("claude") || m.contains("gpt") || m.contains("o1") || m.contains("o3") {
            ModelFamily::ClaudeOrGpt
        } else {
            ModelFamily::Gemini
        }
    }
}

#[derive(Debug, Clone)]
pub enum RoutingError {
    NoAccounts,
    ClaudeQuotaExhausted { reset: String },
    GeminiQuotaExhausted { reset: String },
    AllCoolingDown,
}

impl std::fmt::Display for RoutingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAccounts => write!(f, "No active upstream accounts configured in AMR"),
            Self::ClaudeQuotaExhausted { reset } => write!(
                f,
                "Claude/GPT quota exhausted across all connected accounts (resets {})",
                reset
            ),
            Self::GeminiQuotaExhausted { reset } => write!(
                f,
                "Gemini quota exhausted across all connected accounts (resets {})",
                reset
            ),
            Self::AllCoolingDown => write!(f, "All upstream accounts are currently cooling down after rate limits"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStage {
    Healthy,       // Level 0: 0 failures, fully active
    SoftPause,     // Level 1: 1 failure, ~3s pause (±15% jitter)
    Backoff,       // Level 2: 2 failures, ~15s pause (±15% jitter)
    Cooldown,      // Level 3: 3 failures, ~60s pause (±15% jitter)
    Quarantine,    // Level 4+: 4+ failures, ~300s quarantine (±15% jitter)
    Probing,       // Timer expired, in trial probe mode (auto-heal on next success)
}

#[derive(Debug, Clone)]
pub struct AccountHealthRecord {
    pub failure_count: u32,
    pub cooldown_until: Option<Instant>,
    pub stage: HealthStage,
    pub total_failures: u64,
    pub total_successes: u64,
    pub last_failure_time: Option<Instant>,
}

impl Default for AccountHealthRecord {
    fn default() -> Self {
        Self {
            failure_count: 0,
            cooldown_until: None,
            stage: HealthStage::Healthy,
            total_failures: 0,
            total_successes: 0,
            last_failure_time: None,
        }
    }
}

pub struct LoadBalancer {
    /// Active session pin mappings: session_id -> (account_id, last_access_time)
    session_bindings: Arc<RwLock<HashMap<String, (String, Instant)>>>,
    /// Cooldown tracker: account_id -> cooldown_expiry
    account_cooldowns: Arc<RwLock<HashMap<String, Instant>>>,
    /// Account health and adaptive exponential backoff tracker: account_id -> AccountHealthRecord
    account_health: Arc<RwLock<HashMap<String, AccountHealthRecord>>>,
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
            account_health: Arc::new(RwLock::new(HashMap::new())),
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

    /// Record a failure on an account (e.g. 429 Too Many Requests or 5xx).
    /// Advances through the adaptive exponential backoff ladder (3s -> 15s -> 60s -> 300s quarantine)
    /// with randomized jitter (default ±15%) to prevent synthetic machine periodicities.
    pub fn record_failure(&self, account_id: &str, status_code: u16, max_quarantine_secs: u64, jitter_pct: u8) {
        let now = Instant::now();
        let (expiry, _stage, _count) = {
            let mut health_map = self.account_health.write();
            let record = health_map.entry(account_id.to_string()).or_default();
            record.failure_count += 1;
            record.total_failures += 1;
            record.last_failure_time = Some(now);

            // Exponential Backoff Ladder:
            // Failure 1: ~3s
            // Failure 2: ~15s
            // Failure 3: ~60s
            // Failure 4+: max_quarantine_secs (default 300s)
            let (base_secs, stage) = match record.failure_count {
                1 => (3.0f64, HealthStage::SoftPause),
                2 => (15.0f64, HealthStage::Backoff),
                3 => (60.0f64, HealthStage::Cooldown),
                _ => (max_quarantine_secs.max(120) as f64, HealthStage::Quarantine),
            };

            // Apply randomized jitter (e.g. ±15%)
            let jitter_ratio = (jitter_pct as f64).clamp(0.0, 50.0) / 100.0;
            let factor = 1.0 + (fastrand::f64() * 2.0 - 1.0) * jitter_ratio;
            let final_secs = (base_secs * factor).max(1.0);
            let expiry = now + std::time::Duration::from_secs_f64(final_secs);

            record.stage = stage;
            record.cooldown_until = Some(expiry);

            log::warn!(
                "[LoadBalancer] Account {} failure #{} (status: {}). Entering {:?} for {:.2}s (base: {:.0}s, jitter ±{}%)",
                account_id, record.failure_count, status_code, stage, final_secs, base_secs, jitter_pct
            );

            (expiry, stage, record.failure_count)
        };

        // Mirror in legacy account_cooldowns for fast lookups
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

    /// Record successful response on an account.
    /// Auto-heals the account: resets the failure count and restores to Healthy stage.
    pub fn record_success(&self, account_id: &str) {
        let mut health_map = self.account_health.write();
        if let Some(record) = health_map.get_mut(account_id) {
            record.total_successes += 1;
            if record.failure_count > 0 || record.stage != HealthStage::Healthy {
                log::info!(
                    "[LoadBalancer] Auto-Heal triggered for account {}: trial probe succeeded! Resetting failure counter (was {}) to 0. Restored to Healthy.",
                    account_id, record.failure_count
                );
                record.failure_count = 0;
                record.stage = HealthStage::Healthy;
                record.cooldown_until = None;
            }
        }
        let mut cds = self.account_cooldowns.write();
        cds.remove(account_id);
    }

    /// Mark account in temporary cooldown on rate-limit (429) or transient 5xx error
    pub fn mark_cooldown(&self, account_id: &str, duration_secs: u64) {
        self.record_failure(account_id, 429, duration_secs, 15);
    }

    /// Retrieve live health status of an account: (HealthStage, failure_count, remaining_cooldown_secs)
    pub fn get_health_status(&self, account_id: &str) -> (HealthStage, u32, Option<u64>) {
        let now = Instant::now();
        let health_map = self.account_health.read();
        if let Some(record) = health_map.get(account_id) {
            if let Some(until) = record.cooldown_until {
                if until > now {
                    let remaining = until.duration_since(now).as_secs();
                    return (record.stage, record.failure_count, Some(remaining));
                } else if record.failure_count > 0 {
                    // Timer has expired: account is in Probing stage (ready for 1 trial request)
                    return (HealthStage::Probing, record.failure_count, None);
                }
            }
        }
        (HealthStage::Healthy, 0, None)
    }

    /// Select optimal upstream account with cache affinity, quota balancing, model capability, and failover
    pub fn select_account(
        &self,
        config: &AppConfig,
        target_model: &str,
        session_id: Option<&str>,
    ) -> Result<UpstreamAccount, RoutingError> {
        let now = Instant::now();
        self.total_routed.fetch_add(1, Ordering::Relaxed);

        let active_accounts: Vec<UpstreamAccount> = config
            .accounts
            .iter()
            .filter(|a| a.is_active && !a.api_key.is_empty())
            .cloned()
            .collect();

        if active_accounts.is_empty() {
            return Err(RoutingError::NoAccounts);
        }

        let family = ModelFamily::from_model(target_model);

        // 1. Filter accounts by quota eligibility for requested model family
        let quota_eligible: Vec<UpstreamAccount> = active_accounts
            .iter()
            .filter(|a| match family {
                ModelFamily::ClaudeOrGpt => {
                    a.claude_gpt_percent > 0 && a.claude_gpt_weekly_percent > 0
                }
                ModelFamily::Gemini => {
                    (a.gemini_percent > 0 && a.gemini_weekly_percent > 0)
                        || (a.claude_gpt_percent > 0 && a.claude_gpt_weekly_percent > 0)
                }
            })
            .cloned()
            .collect();

        if quota_eligible.is_empty() {
            return match family {
                ModelFamily::ClaudeOrGpt => {
                    let reset = active_accounts
                        .iter()
                        .map(|a| a.claude_gpt_reset.as_str())
                        .next()
                        .unwrap_or("in 4h 00m");
                    Err(RoutingError::ClaudeQuotaExhausted {
                        reset: reset.to_string(),
                    })
                }
                ModelFamily::Gemini => {
                    let reset = active_accounts
                        .iter()
                        .map(|a| a.gemini_reset.as_str())
                        .next()
                        .unwrap_or("in 4h 00m");
                    Err(RoutingError::GeminiQuotaExhausted {
                        reset: reset.to_string(),
                    })
                }
            };
        }

        // 2. Evaluate Account Health & Probing Eligibility
        let (healthy_zero_fail, probing_accounts) = {
            let health_map = self.account_health.read();
            let cds = self.account_cooldowns.read();
            let mut zero_fail = Vec::new();
            let mut probing = Vec::new();

            for acc in quota_eligible {
                if !acc.is_active {
                    continue;
                }
                // Check if account is currently cooling down in ladder
                if let Some(exp) = cds.get(&acc.id) {
                    if *exp > now {
                        continue; // Active backoff ladder cooldown: skip to protect account!
                    }
                }
                // If cooldown expired, check failure count:
                if let Some(rec) = health_map.get(&acc.id) {
                    if rec.failure_count > 0 {
                        probing.push(acc);
                        continue;
                    }
                }
                zero_fail.push(acc);
            }
            (zero_fail, probing)
        };

        // Prefer clean healthy accounts first. If none available, dispatch to probing account for trial recovery.
        let healthy_accounts = if !healthy_zero_fail.is_empty() {
            healthy_zero_fail
        } else if !probing_accounts.is_empty() {
            log::info!("[LoadBalancer] All active accounts have prior failures; dispatching to trial probing account to test auto-heal");
            probing_accounts
        } else {
            return Err(RoutingError::AllCoolingDown);
        };

        // Apply Quota Protection Threshold:
        // Accounts with remaining quota <= threshold (default 5%) are held in reserve
        let threshold = config.quota_protection_threshold as i32;
        let above_threshold: Vec<UpstreamAccount> = healthy_accounts
            .iter()
            .filter(|a| match family {
                ModelFamily::ClaudeOrGpt => a.claude_gpt_percent > threshold && a.claude_gpt_weekly_percent > threshold,
                ModelFamily::Gemini => a.gemini_percent > threshold && a.gemini_weekly_percent > threshold,
            })
            .cloned()
            .collect();

        let pool = if !above_threshold.is_empty() {
            above_threshold
        } else {
            healthy_accounts.clone()
        };

        // 3. Routing strategy overrides (e.g. strict Failover)
        if config.routing_strategy == RoutingStrategy::Failover {
            if let Some(active_id) = &config.active_account_id {
                if let Some(acc) = healthy_accounts.iter().find(|a| &a.id == active_id) {
                    return Ok(acc.clone());
                }
            }
            return Ok(healthy_accounts.first().cloned().unwrap());
        }

        // 4. Cache Affinity Evaluation
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
                        // Account is alive and eligible. Now check mode rules:
                        let bound_rem = match family {
                            ModelFamily::ClaudeOrGpt => bound_acc.claude_gpt_percent as u64,
                            ModelFamily::Gemini => bound_acc.gemini_percent as u64,
                        };

                        match mode {
                            CacheAffinityMode::CacheFirst => {
                                // Strictly stick to the same account as long as it has remaining quota
                                if bound_rem > 0 {
                                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                                    let mut bindings = self.session_bindings.write();
                                    bindings.insert(sid.to_string(), (bound_id, now));
                                    return Ok(bound_acc.clone());
                                }
                            }
                            CacheAffinityMode::Balanced => {
                                // Sticky to session, but check if another account has significantly higher quota
                                let max_other_rem = healthy_accounts
                                    .iter()
                                    .filter(|a| a.id != bound_id)
                                    .map(|a| match family {
                                        ModelFamily::ClaudeOrGpt => a.claude_gpt_percent as u64,
                                        ModelFamily::Gemini => a.gemini_percent as u64,
                                    })
                                    .max()
                                    .unwrap_or(0);

                                let delta = max_other_rem.saturating_sub(bound_rem);
                                if (delta as u32) < config.quota_imbalance_threshold {
                                    // Within threshold: stick to account to maximize prompt caching!
                                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                                    let mut bindings = self.session_bindings.write();
                                    bindings.insert(sid.to_string(), (bound_id, now));
                                    return Ok(bound_acc.clone());
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

        // 5. Fallback selection when no pin exists or pin was rotated
        let chosen_acc = match config.routing_strategy {
            RoutingStrategy::RoundRobin => {
                let idx = (self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize)
                    % pool.len();
                pool[idx].clone()
            }
            RoutingStrategy::QuotaAware | RoutingStrategy::Failover => {
                // Select account with maximum remaining percentage for this family
                pool
                    .into_iter()
                    .max_by_key(|a| match family {
                        ModelFamily::ClaudeOrGpt => a.claude_gpt_percent.min(a.claude_gpt_weekly_percent),
                        ModelFamily::Gemini => a.gemini_percent.min(a.gemini_weekly_percent),
                    })
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

        Ok(chosen_acc)
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
