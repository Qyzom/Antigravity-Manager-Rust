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
    AllCoolingDown { retry_after_secs: u64 },
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
            Self::AllCoolingDown { retry_after_secs } => {
                if *retry_after_secs > 0 {
                    write!(
                        f,
                        "All upstream accounts are currently cooling down after rate limits (retry in {}s)",
                        retry_after_secs
                    )
                } else {
                    write!(
                        f,
                        "All upstream accounts are currently cooling down after rate limits"
                    )
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStage {
    Healthy,    // Level 0: 0 failures, fully active
    SoftPause,  // Level 1: 1 failure, ~3s pause (±15% jitter)
    Backoff,    // Level 2: 2 failures, ~15s pause (±15% jitter)
    Cooldown,   // Level 3: 3 failures, ~60s pause (±15% jitter)
    Quarantine, // Level 4+: 4+ failures, ~300s quarantine (±15% jitter)
    Probing,    // Timer expired, in trial probe mode (auto-heal on next success)
}

#[derive(Debug, Clone)]
pub struct AccountHealthRecord {
    pub failure_count: u32,
    pub cooldown_until: Option<Instant>,
    pub stage: HealthStage,
    pub total_failures: u64,
    pub total_successes: u64,
    pub last_failure_time: Option<Instant>,
    pub last_status_code: Option<u16>,
    pub last_failure_reason: Option<String>,
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
            last_status_code: None,
            last_failure_reason: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetailedHealthStatus {
    pub stage: HealthStage,
    pub stage_level: u32,
    pub failure_count: u32,
    pub remaining_secs: Option<u64>,
    pub last_status_code: Option<u16>,
    pub last_failure_reason: Option<String>,
    pub total_failures: u64,
    pub total_successes: u64,
    pub is_cooling: bool,
}

pub struct LoadBalancer {
    /// Active session pin mappings: session_id -> (account_id, last_access_time)
    session_bindings: Arc<RwLock<HashMap<String, (String, Instant)>>>,
    /// Cooldown tracker: account_id -> cooldown_expiry
    account_cooldowns: Arc<RwLock<HashMap<String, Instant>>>,
    /// Account health and adaptive exponential backoff tracker: account_id -> AccountHealthRecord
    account_health: Arc<RwLock<HashMap<String, AccountHealthRecord>>>,
    /// Single-probe guard for Probing accounts to prevent thundering herd on cooldown expiry
    probing_in_flight: Arc<RwLock<HashMap<String, Instant>>>,
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
            probing_in_flight: Arc::new(RwLock::new(HashMap::new())),
            rr_counter: AtomicU64::new(0),
            total_routed: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            failover_rotations: AtomicU64::new(0),
        }
    }

    /// Extract or compute a deterministic session fingerprint across multi-turn interactions
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
    /// Advances through the adaptive exponential backoff ladder (3s -> 15s -> 60s -> quarantine)
    /// with randomized jitter (default ±15%) to prevent synthetic machine periodicities.
    pub fn record_failure(
        &self,
        account_id: &str,
        status_code: u16,
        max_quarantine_secs: u64,
        jitter_pct: u8,
    ) {
        self.record_failure_with_reason(
            account_id,
            status_code,
            max_quarantine_secs,
            jitter_pct,
            None,
        );
    }

    /// Record a failure with exact status code and descriptive reason.
    pub fn record_failure_with_reason(
        &self,
        account_id: &str,
        status_code: u16,
        max_quarantine_secs: u64,
        jitter_pct: u8,
        reason: Option<&str>,
    ) {
        self.record_failure_internal(
            account_id,
            status_code,
            max_quarantine_secs,
            jitter_pct,
            reason,
            None,
            false,
        );
    }

    /// Burst-safe rate limit recorder for live proxy requests:
    /// If the account is already cooling down (e.g. concurrent in-flight requests from the same burst),
    /// does NOT escalate `failure_count` further, and honors short upstream `retry_after_secs` hints.
    pub fn record_rate_limit_hit(
        &self,
        account_id: &str,
        status_code: u16,
        max_quarantine_secs: u64,
        jitter_pct: u8,
        reason: Option<&str>,
        retry_after_secs: Option<u64>,
    ) {
        self.record_failure_internal(
            account_id,
            status_code,
            max_quarantine_secs,
            jitter_pct,
            reason,
            retry_after_secs,
            true,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_failure_internal(
        &self,
        account_id: &str,
        status_code: u16,
        max_quarantine_secs: u64,
        jitter_pct: u8,
        reason: Option<&str>,
        retry_after_secs: Option<u64>,
        coalesce_burst: bool,
    ) {
        let now = Instant::now();
        let (expiry, _stage, _count) = {
            let mut health_map = self.account_health.write();
            let record = health_map.entry(account_id.to_string()).or_default();

            if coalesce_burst
                && record
                    .cooldown_until
                    .is_some_and(|until| until > now)
            {
                record.last_status_code = Some(status_code);
                if let Some(r) = reason {
                    record.last_failure_reason = Some(r.to_string());
                }
                return;
            }

            record.failure_count += 1;
            record.total_failures += 1;
            record.last_failure_time = Some(now);
            record.last_status_code = Some(status_code);
            if let Some(r) = reason {
                record.last_failure_reason = Some(r.to_string());
            } else if record.last_failure_reason.is_none() {
                record.last_failure_reason = Some(format!("HTTP {} error", status_code));
            }

            // Exponential Backoff Ladder:
            // Failure 1: ~3s (SoftPause)
            // Failure 2: ~15s (Backoff)
            // Failure 3: ~60s (Cooldown)
            // Failure 4+: max_quarantine_secs (Quarantine, clamped to min 30s)
            let (mut base_secs, mut stage) = match record.failure_count {
                1 => (3.0f64, HealthStage::SoftPause),
                2 => (15.0f64, HealthStage::Backoff),
                3 => (60.0f64, HealthStage::Cooldown),
                _ => (max_quarantine_secs.max(30) as f64, HealthStage::Quarantine),
            };

            // If upstream gave a short explicit retry hint (e.g. 2s-20s), honor it instead of locking 5m
            if let Some(hint) = retry_after_secs {
                let capped = hint.clamp(1, max_quarantine_secs.max(30)) as f64;
                if capped < base_secs {
                    base_secs = capped;
                    if capped <= 5.0 {
                        stage = HealthStage::SoftPause;
                    } else if capped <= 20.0 {
                        stage = HealthStage::Backoff;
                    } else if capped <= 90.0 {
                        stage = HealthStage::Cooldown;
                    }
                }
            }

            // Apply randomized jitter (e.g. ±15%)
            let jitter_ratio = (jitter_pct as f64).clamp(0.0, 50.0) / 100.0;
            let factor = 1.0 + (fastrand::f64() * 2.0 - 1.0) * jitter_ratio;
            let final_secs = (base_secs * factor).max(1.0);
            let expiry = now + std::time::Duration::from_secs_f64(final_secs);

            record.stage = stage;
            record.cooldown_until = Some(expiry);

            log::warn!(
                "[LoadBalancer] Account {} failure #{} (status: {}). Entering {:?} for {:.2}s (base: {:.0}s, jitter ±{}%) - Reason: {:?}",
                account_id, record.failure_count, status_code, stage, final_secs, base_secs, jitter_pct, record.last_failure_reason
            );

            (expiry, stage, record.failure_count)
        };

        // Mirror in legacy account_cooldowns for fast lookups
        {
            let mut cds = self.account_cooldowns.write();
            cds.insert(account_id.to_string(), expiry);
        }

        // Immediately unbind all sessions pinned to this account so next requests failover without delay
        self.clear_session_bindings(account_id);
        self.probing_in_flight.write().remove(account_id);

        self.failover_rotations.fetch_add(1, Ordering::Relaxed);
    }

    /// Update the failure reason on an account (e.g. once detailed upstream error body is parsed)
    pub fn update_failure_reason(&self, account_id: &str, status_code: u16, reason: &str) {
        let mut health_map = self.account_health.write();
        if let Some(record) = health_map.get_mut(account_id) {
            record.last_status_code = Some(status_code);
            record.last_failure_reason = Some(reason.to_string());
        }
    }

    /// Release a single-probe reservation when a trial probe finishes with a non-429/non-5xx error (e.g. network/401/403/400).
    pub fn clear_probing(&self, account_id: &str) {
        self.probing_in_flight.write().remove(account_id);
    }

    /// Clear all pinned sessions pointing to a specific account (e.g. on 401/403).
    pub fn clear_session_bindings(&self, account_id: &str) {
        let mut bindings = self.session_bindings.write();
        bindings.retain(|_, (acc_id, _)| acc_id != account_id);
    }

    /// Manually or automatically reset an account from quarantine/cooldown back to Healthy
    pub fn reset_health(&self, account_id: &str) {
        {
            let mut health_map = self.account_health.write();
            if let Some(record) = health_map.get_mut(account_id) {
                record.failure_count = 0;
                record.stage = HealthStage::Healthy;
                record.cooldown_until = None;
                record.last_failure_reason = None;
                record.last_status_code = None;
            }
        }
        let mut cds = self.account_cooldowns.write();
        cds.remove(account_id);
        self.probing_in_flight.write().remove(account_id);
        log::info!(
            "[LoadBalancer] Health manually reset for account {}. Cooldown cleared.",
            account_id
        );
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
                record.last_failure_reason = None;
                record.last_status_code = None;
            }
        }
        let mut cds = self.account_cooldowns.write();
        cds.remove(account_id);
        self.probing_in_flight.write().remove(account_id);
    }

    /// Mark account in temporary cooldown on rate-limit (429) or transient 5xx error
    pub fn mark_cooldown(&self, account_id: &str, duration_secs: u64) {
        self.record_failure_with_reason(
            account_id,
            429,
            duration_secs,
            15,
            Some("Manual Cooldown (429)"),
        );
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

    /// Retrieve comprehensive health details for UI modals and diagnostics
    pub fn get_detailed_health(&self, account_id: &str) -> DetailedHealthStatus {
        let now = Instant::now();
        let health_map = self.account_health.read();
        if let Some(record) = health_map.get(account_id) {
            let remaining = record.cooldown_until.and_then(|until| {
                if until > now {
                    Some(until.duration_since(now).as_secs().max(1))
                } else {
                    None
                }
            });
            let is_cooling = remaining.is_some();
            let effective_stage = if is_cooling {
                record.stage
            } else if record.failure_count > 0 {
                HealthStage::Probing
            } else {
                HealthStage::Healthy
            };
            let stage_level = match effective_stage {
                HealthStage::Healthy => 0,
                HealthStage::SoftPause => 1,
                HealthStage::Backoff => 2,
                HealthStage::Cooldown => 3,
                HealthStage::Quarantine => 4,
                HealthStage::Probing => 0,
            };
            DetailedHealthStatus {
                stage: effective_stage,
                stage_level,
                failure_count: record.failure_count,
                remaining_secs: remaining,
                last_status_code: record.last_status_code,
                last_failure_reason: record.last_failure_reason.clone(),
                total_failures: record.total_failures,
                total_successes: record.total_successes,
                is_cooling,
            }
        } else {
            DetailedHealthStatus {
                stage: HealthStage::Healthy,
                stage_level: 0,
                failure_count: 0,
                remaining_secs: None,
                last_status_code: None,
                last_failure_reason: None,
                total_failures: 0,
                total_successes: 0,
                is_cooling: false,
            }
        }
    }

    /// Extract short retry delay (in seconds) from upstream 429 error bodies
    /// (e.g. "retryDelay": "4s", "reset after 3s", "retry in 5s").
    pub fn parse_retry_delay_secs(body: &str) -> Option<u64> {
        let lower = body.to_lowercase();
        for marker in [
            "\"retrydelay\":",
            "\"retry_delay\":",
            "reset after ",
            "retry after ",
            "retry in ",
            "try again in ",
        ] {
            if let Some(pos) = lower.find(marker) {
                let rest = &lower[pos + marker.len()..];
                let trimmed = rest.trim_start_matches([' ', '"', ':']);
                let num_str: String = trimmed
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                if let Ok(val) = num_str.parse::<f64>() {
                    if val > 0.0 {
                        return Some(val.ceil() as u64);
                    }
                }
            }
        }
        None
    }

    /// Select optimal upstream account with cache affinity, quota balancing, model capability, and failover
    pub fn select_account(
        &self,
        config: &AppConfig,
        target_model: &str,
        session_id: Option<&str>,
    ) -> Result<UpstreamAccount, RoutingError> {
        self.select_account_excluding(config, target_model, session_id, &[])
    }

    /// Select optimal upstream account while excluding already-attempted account IDs for failover.
    pub fn select_account_excluding(
        &self,
        config: &AppConfig,
        target_model: &str,
        session_id: Option<&str>,
        exclude_ids: &[String],
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

        let candidate_accounts: Vec<UpstreamAccount> = if exclude_ids.is_empty() {
            active_accounts.clone()
        } else {
            active_accounts
                .iter()
                .filter(|a| !exclude_ids.iter().any(|ex| ex == &a.id))
                .cloned()
                .collect()
        };

        if candidate_accounts.is_empty() {
            return Err(RoutingError::AllCoolingDown {
                retry_after_secs: 0,
            });
        }

        let family = ModelFamily::from_model(target_model);
        let target_canon = AppConfig::canonical_catalog_id(target_model);
        let is_virtual = matches!(
            target_model,
            "web-search" | "gemini-3.5-transcribe" | "whisper-1"
        );

        let family_has_quota = |a: &UpstreamAccount| -> bool {
            match family {
                ModelFamily::ClaudeOrGpt => a.claude_percent > 0 && a.claude_weekly_percent > 0,
                ModelFamily::Gemini => a.gemini_percent > 0 && a.gemini_weekly_percent > 0,
            }
        };

        // 1. Check per-model quota when available_models is populated; fall back to family quota otherwise.
        // This prevents 0% on one model (e.g. gemini-pro or opus) from blocking other models (flash/sonnet) that still have quota.
        let quota_eligible: Vec<UpstreamAccount> = candidate_accounts
            .iter()
            .filter(|a| {
                if is_virtual || a.available_models.is_empty() {
                    return family_has_quota(a);
                }

                let (resolved_target, _) =
                    crate::proxy::router::resolve_model_with_effort(target_model, a, None);
                let resolved_canon = AppConfig::canonical_catalog_id(&resolved_target);

                let matching: Vec<_> = a
                    .available_models
                    .iter()
                    .filter(|m| {
                        let m_canon = AppConfig::canonical_catalog_id(&m.id);
                        m.id.eq_ignore_ascii_case(target_model)
                            || m_canon.eq_ignore_ascii_case(&target_canon)
                            || m.id.eq_ignore_ascii_case(&resolved_target)
                            || m_canon.eq_ignore_ascii_case(&resolved_canon)
                    })
                    .collect();

                if !matching.is_empty() {
                    matching
                        .iter()
                        .any(|m| !m.disabled && m.remaining_fraction > 0.0)
                } else {
                    family_has_quota(a)
                }
            })
            .cloned()
            .collect();

        if quota_eligible.is_empty() {
            return match family {
                ModelFamily::ClaudeOrGpt => {
                    let reset = candidate_accounts
                        .iter()
                        .map(|a| a.claude_reset.as_str())
                        .next()
                        .unwrap_or("in 4h 00m");
                    Err(RoutingError::ClaudeQuotaExhausted {
                        reset: reset.to_string(),
                    })
                }
                ModelFamily::Gemini => {
                    let reset = candidate_accounts
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

        let model_eligible = quota_eligible;

        // 2. Evaluate Account Health & Probing Eligibility
        let (healthy_zero_fail, probing_accounts, min_cd_secs) = {
            let health_map = self.account_health.read();
            let cds = self.account_cooldowns.read();
            let mut zero_fail = Vec::new();
            let mut probing = Vec::new();
            let mut min_wait: Option<u64> = None;

            for acc in model_eligible {
                if !acc.is_active {
                    continue;
                }
                // Check if account is currently cooling down in ladder
                if let Some(exp) = cds.get(&acc.id) {
                    if *exp > now {
                        let rem = (*exp - now).as_secs().max(1);
                        min_wait = Some(min_wait.map_or(rem, |w| w.min(rem)));
                        continue; // Active backoff ladder cooldown: skip to protect account!
                    }
                }
                // If cooldown expired, check failure count and single-probe guard:
                if let Some(rec) = health_map.get(&acc.id) {
                    if rec.failure_count > 0 {
                        let in_flight = self
                            .probing_in_flight
                            .read()
                            .get(&acc.id)
                            .is_some_and(|t| now.duration_since(*t).as_secs() < 30);
                        if in_flight {
                            min_wait = Some(min_wait.map_or(1, |w| w.min(1)));
                            continue;
                        }
                        probing.push(acc);
                        continue;
                    }
                }
                zero_fail.push(acc);
            }
            (zero_fail, probing, min_wait.unwrap_or(0))
        };

        // Include expired-cooldown probing accounts alongside zero-fail accounts so they can
        // actually receive a trial request and auto-heal instead of starving forever.
        let probing_ids: std::collections::HashSet<String> =
            probing_accounts.iter().map(|a| a.id.clone()).collect();
        let mut healthy_accounts = healthy_zero_fail;
        if !probing_accounts.is_empty() {
            if healthy_accounts.is_empty() {
                log::info!("[LoadBalancer] All active accounts have prior failures; dispatching to trial probing account to test auto-heal");
            }
            healthy_accounts.extend(probing_accounts);
        }

        if healthy_accounts.is_empty() {
            return Err(RoutingError::AllCoolingDown {
                retry_after_secs: min_cd_secs,
            });
        }

        let fraction_to_pct = |f: f64| -> i32 {
            if f <= 0.0 {
                0
            } else {
                (f * 100.0).round().max(1.0) as i32
            }
        };

        let account_model_pct = |a: &UpstreamAccount| -> i32 {
            if !is_virtual && !a.available_models.is_empty() {
                let (resolved_target, _) =
                    crate::proxy::router::resolve_model_with_effort(target_model, a, None);
                let resolved_canon = AppConfig::canonical_catalog_id(&resolved_target);
                if let Some(best) = a
                    .available_models
                    .iter()
                    .filter(|m| {
                        let m_canon = AppConfig::canonical_catalog_id(&m.id);
                        !m.disabled
                            && (m.id.eq_ignore_ascii_case(target_model)
                                || m_canon.eq_ignore_ascii_case(&target_canon)
                                || m.id.eq_ignore_ascii_case(&resolved_target)
                                || m_canon.eq_ignore_ascii_case(&resolved_canon))
                    })
                    .map(|m| fraction_to_pct(m.remaining_fraction))
                    .max()
                {
                    return best;
                }
            }
            match family {
                ModelFamily::ClaudeOrGpt => a.claude_percent.min(a.claude_weekly_percent),
                ModelFamily::Gemini => a.gemini_percent.min(a.gemini_weekly_percent),
            }
        };

        // Apply Quota Protection Threshold:
        // Accounts with remaining quota <= threshold (default 10%) are held in reserve
        let threshold = config.quota_protection_threshold as i32;
        let above_threshold: Vec<UpstreamAccount> = healthy_accounts
            .iter()
            .filter(|a| account_model_pct(a) > threshold)
            .cloned()
            .collect();

        let pool = if !above_threshold.is_empty() {
            above_threshold
        } else {
            healthy_accounts
        };

        // 3. Unified selection across Failover, Cache Affinity, and Fallback so single-probe & pin rules apply consistently
        let ttl_secs = config.session_sticky_ttl_secs;
        let mode = config.cache_affinity_mode;
        let mut chosen_acc: Option<UpstreamAccount> = None;
        let mut skip_pin_update = false;

        if config.routing_strategy == RoutingStrategy::Failover {
            if let Some(active_id) = &config.active_account_id {
                chosen_acc = pool.iter().find(|a| &a.id == active_id).cloned();
            }
            if chosen_acc.is_none() {
                chosen_acc = pool.first().cloned();
            }
            skip_pin_update = true;
        } else if let Some(sid) = session_id {
            // Check existing session pin
            let existing_pin = {
                let bindings = self.session_bindings.read();
                bindings.get(sid).cloned()
            };

            if let Some((bound_id, last_access)) = existing_pin {
                let elapsed_secs = last_access.elapsed().as_secs();

                if elapsed_secs < ttl_secs {
                    if let Some(bound_acc) = pool.iter().find(|a| a.id == bound_id) {
                        let bound_rem = account_model_pct(bound_acc).max(0) as u64;

                        match mode {
                            CacheAffinityMode::CacheFirst => {
                                if bound_rem > 0 {
                                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                                    chosen_acc = Some(bound_acc.clone());
                                }
                            }
                            CacheAffinityMode::Balanced => {
                                if bound_rem > 0 {
                                    let max_other_rem = pool
                                        .iter()
                                        .filter(|a| a.id != bound_id)
                                        .map(|a| account_model_pct(a).max(0) as u64)
                                        .max()
                                        .unwrap_or(0);

                                    let delta = max_other_rem.saturating_sub(bound_rem);
                                    if (delta as u32) < config.quota_imbalance_threshold {
                                        self.cache_hits.fetch_add(1, Ordering::Relaxed);
                                        chosen_acc = Some(bound_acc.clone());
                                    } else {
                                        self.failover_rotations.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            CacheAffinityMode::AggressiveSpread => {}
                        }
                    }
                }
            }
        }

        // 5. Fallback selection when no pin exists or pin was rotated
        let chosen_acc = match chosen_acc {
            Some(acc) => acc,
            None => match config.routing_strategy {
                RoutingStrategy::RoundRobin => {
                    let idx = (self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize) % pool.len();
                    pool[idx].clone()
                }
                RoutingStrategy::QuotaAware | RoutingStrategy::Failover => pool
                    .iter()
                    .max_by_key(|a| account_model_pct(a))
                    .cloned()
                    .ok_or(RoutingError::NoAccounts)?,
            },
        };

        // Atomically claim single-probe slot if this account is under trial probe (prevents TOCTOU race)
        let chosen_acc = if probing_ids.contains(&chosen_acc.id) {
            let mut in_flight = self.probing_in_flight.write();
            let already_taken = in_flight
                .get(&chosen_acc.id)
                .is_some_and(|t| now.duration_since(*t).as_secs() < 30);
            if already_taken {
                if let Some(alt) = pool.into_iter().find(|a| {
                    a.id != chosen_acc.id
                        && (!probing_ids.contains(&a.id)
                            || !in_flight
                                .get(&a.id)
                                .is_some_and(|t| now.duration_since(*t).as_secs() < 30))
                }) {
                    if probing_ids.contains(&alt.id) {
                        in_flight.insert(alt.id.clone(), now);
                    }
                    alt
                } else {
                    return Err(RoutingError::AllCoolingDown {
                        retry_after_secs: 1,
                    });
                }
            } else {
                in_flight.insert(chosen_acc.id.clone(), now);
                chosen_acc
            }
        } else {
            chosen_acc
        };

        // Pin session to chosen account if session_id is present and not AggressiveSpread
        if !skip_pin_update {
            if let Some(sid) = session_id {
                if mode != CacheAffinityMode::AggressiveSpread {
                    let mut bindings = self.session_bindings.write();
                    bindings.insert(sid.to_string(), (chosen_acc.id.clone(), now));
                }
            }
        }

        Ok(chosen_acc)
    }

    /// Cleanup old expired sessions periodically
    pub fn cleanup_expired_sessions(&self, ttl_secs: u64) {
        let now = Instant::now();
        let mut bindings = self.session_bindings.write();
        bindings
            .retain(|_, (_, last_access)| now.duration_since(*last_access).as_secs() < ttl_secs);
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
