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
            Self::NoAccounts => write!(f, "No active upstream accounts configured in Aroxy"),
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
            .filter(|a| a.is_active && a.status.to_uppercase() == "ACTIVE" && !a.api_key.is_empty())
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
                    a.claude_gpt_percent > 0 && a.claude_gpt_weekly_percent > 0 && a.tier.to_uppercase() != "FREE"
                }
                ModelFamily::Gemini => a.gemini_percent > 0 && a.gemini_weekly_percent > 0,
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

        // 2. Filter healthy accounts (not in cooldown, active, not in ERROR)
        let mut healthy_accounts: Vec<UpstreamAccount> = {
            let cds = self.account_cooldowns.read();
            quota_eligible
                .iter()
                .filter(|a| {
                    if !a.status.eq_ignore_ascii_case("ACTIVE") || !a.is_active {
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

        // If all eligible accounts are in cooldown, clear expired cooldowns and take any eligible
        if healthy_accounts.is_empty() {
            let mut cds = self.account_cooldowns.write();
            cds.clear();
            healthy_accounts = quota_eligible;
        }

        if healthy_accounts.is_empty() {
            return Err(RoutingError::AllCoolingDown);
        }

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
