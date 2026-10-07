//! Run limits: per-ticket correction cycles and run-wide duration, usage and
//! cost ceilings. Run-wide exhaustion stops further work while preserving
//! resumable state; it is recorded separately from ticket correction exhaustion.
//!
//! Usage and cost are accounted only from provider observations recorded in run
//! state (`{"usage": {...}, "cost": number|null, "cost_estimate": number}` logs).
//! Missing monetary cost is reported as unavailable, never as zero, and estimates
//! are reported but never enforced.
use crate::{ProjectConfig, Run};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

pub const DEFAULT_CORRECTION_CYCLES: u64 = 3;
/// Replanning attempts per ticket after its correction cycles are exhausted.
pub const DEFAULT_REPLANNING_ATTEMPTS: u64 = 1;
/// Let in-flight provider invocations finish, then start nothing further.
pub const SETTLE: &str = "settle";
/// Cancel in-flight provider invocations through their stop handles.
pub const STOP: &str = "stop";

/// Configured limits, presented in scheduler state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLimits {
    pub correction_cycles: u64,
    /// Bounded replanning attempts per ticket after correction exhaustion (0 disables).
    #[serde(default = "default_replanning_attempts")]
    pub replanning_attempts: u64,
    /// Wall-clock budget of one `kiln run` invocation.
    pub duration_limit_seconds: Option<u64>,
    /// Cumulative provider tokens observed across the run.
    pub usage_token_limit: Option<u64>,
    /// No default monetary ceiling exists; enforced only against measured cost.
    pub cost_limit_usd: Option<f64>,
    /// `settle` (default) or `stop`: what happens to active sessions on exhaustion.
    pub limit_policy: String,
}
impl RunLimits {
    pub fn from_config(config: &ProjectConfig) -> Result<Self> {
        let get = |name: &str| config.extensions.get(name);
        let positive = |name: &str| -> Result<Option<u64>> {
            match get(name) {
                None => Ok(None),
                Some(v) => match v.as_u64() {
                    Some(n) if n > 0 => Ok(Some(n)),
                    _ => bail!("{name} must be a positive integer"),
                },
            }
        };
        let cost_limit_usd = match get("cost_limit_usd") {
            None => None,
            Some(v) => match v.as_f64() {
                Some(n) if n > 0.0 && n.is_finite() => Some(n),
                _ => bail!("cost_limit_usd must be a positive number"),
            },
        };
        let limit_policy = match get("limit_policy") {
            None => SETTLE.into(),
            Some(Value::String(p)) if p == SETTLE || p == STOP => p.clone(),
            Some(_) => bail!("limit_policy must be \"settle\" or \"stop\""),
        };
        let replanning_attempts = match get("replanning_attempts") {
            None => DEFAULT_REPLANNING_ATTEMPTS,
            Some(v) => v
                .as_u64()
                .context("replanning_attempts must be a non-negative integer")?,
        };
        Ok(Self {
            correction_cycles: positive("correction_cycles")?.unwrap_or(DEFAULT_CORRECTION_CYCLES),
            replanning_attempts,
            duration_limit_seconds: positive("duration_limit_seconds")?,
            usage_token_limit: positive("usage_token_limit")?,
            cost_limit_usd,
            limit_policy,
        })
    }
    pub fn duration(&self) -> Option<Duration> {
        self.duration_limit_seconds.map(Duration::from_secs)
    }
    /// First exhausted run-wide limit, if any.
    pub fn evaluate(&self, usage: &UsageAccount, elapsed: Duration) -> Option<LimitExhaustion> {
        if let Some(limit) = self.duration_limit_seconds {
            if elapsed >= Duration::from_secs(limit) {
                return Some(LimitExhaustion::new(
                    "duration",
                    format!(
                        "duration_limit_seconds {limit} reached after {:.1}s",
                        elapsed.as_secs_f64()
                    ),
                ));
            }
        }
        if let Some(limit) = self.usage_token_limit {
            if usage.tokens >= limit {
                return Some(LimitExhaustion::new(
                    "usage_tokens",
                    format!(
                        "usage_token_limit {limit} reached: {} provider tokens observed",
                        usage.tokens
                    ),
                ));
            }
        }
        if let (Some(limit), Some(cost)) = (self.cost_limit_usd, usage.measured_cost_usd) {
            if cost >= limit {
                return Some(LimitExhaustion::new(
                    "cost",
                    format!(
                        "cost_limit_usd {limit} reached: {cost} USD measured ({} cost data)",
                        usage.cost_data
                    ),
                ));
            }
        }
        None
    }
    /// How the configured cost ceiling relates to the available cost data.
    pub fn cost_ceiling(&self, usage: &UsageAccount) -> String {
        match (self.cost_limit_usd, usage.cost_data.as_str()) {
            (None, _) => "not_configured",
            (Some(_), "measured") => "enforced",
            (Some(_), "partial") => "enforced_on_measured_subset",
            (Some(_), "estimated") => "not_enforced_estimate_only",
            (Some(_), _) => "not_enforced_cost_unavailable",
        }
        .into()
    }
}

fn default_replanning_attempts() -> u64 {
    DEFAULT_REPLANNING_ATTEMPTS
}

/// Provider usage observed in recorded run state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageAccount {
    pub tokens: u64,
    /// Provider contexts (implementation, correction, review) accounted.
    pub contexts: usize,
    pub contexts_without_usage: usize,
    /// Sum of provider-reported monetary cost; null when none was reported.
    pub measured_cost_usd: Option<f64>,
    /// Sum of reported estimates; informational, never enforced.
    pub estimated_cost_usd: Option<f64>,
    /// measured | partial | estimated | unavailable
    pub cost_data: String,
}
pub fn account(run: &Run) -> UsageAccount {
    let mut logs: BTreeMap<&str, &str> = BTreeMap::new();
    for s in run
        .sessions
        .iter()
        .chain(run.corrections.iter().flat_map(|c| [&c.before, &c.after]))
    {
        logs.insert(&s.context_id, &s.agent_log);
    }
    for r in &run.reviews {
        for axis in [&r.standards, &r.spec] {
            logs.insert(&axis.context_id, &axis.result.log);
        }
    }
    let mut usage = UsageAccount {
        contexts: logs.len(),
        ..Default::default()
    };
    let mut measured = 0usize;
    for log in logs.values() {
        let observation: Value = serde_json::from_str(log).unwrap_or(Value::Null);
        match observation.get("usage").filter(|u| u.is_object()) {
            Some(u) => {
                let n = |k: &str| u.get(k).and_then(Value::as_u64);
                usage.tokens += n("total_tokens").unwrap_or_else(|| {
                    n("input_tokens").unwrap_or(0) + n("output_tokens").unwrap_or(0)
                });
            }
            None => usage.contexts_without_usage += 1,
        }
        if let Some(cost) = observation.get("cost").and_then(Value::as_f64) {
            measured += 1;
            *usage.measured_cost_usd.get_or_insert(0.0) += cost;
        }
        if let Some(estimate) = observation.get("cost_estimate").and_then(Value::as_f64) {
            *usage.estimated_cost_usd.get_or_insert(0.0) += estimate;
        }
    }
    usage.cost_data = if measured > 0 && measured == usage.contexts {
        "measured"
    } else if measured > 0 {
        "partial"
    } else if usage.estimated_cost_usd.is_some() {
        "estimated"
    } else {
        "unavailable"
    }
    .into();
    usage
}

/// Durable limit state of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitState {
    #[serde(flatten)]
    pub configured: RunLimits,
    #[serde(default)]
    pub usage: UsageAccount,
    /// not_configured | enforced | enforced_on_measured_subset |
    /// not_enforced_estimate_only | not_enforced_cost_unavailable
    #[serde(default)]
    pub cost_ceiling: String,
    /// Run-wide exhaustion that stopped further work, if any.
    pub exhausted: Option<LimitExhaustion>,
}
impl LimitState {
    pub fn new(configured: RunLimits) -> Self {
        Self {
            configured,
            usage: UsageAccount::default(),
            cost_ceiling: String::new(),
            exhausted: None,
        }
    }
    pub fn observe(&mut self, run: &Run) {
        self.usage = account(run);
        self.cost_ceiling = self.configured.cost_ceiling(&self.usage);
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitExhaustion {
    /// duration | usage_tokens | cost | provider_usage
    pub limit: String,
    pub reason: String,
    /// Provider whose account usage or rate limit was reached (`provider_usage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Reset time reported by the provider, verbatim, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}
impl LimitExhaustion {
    pub fn new(limit: &str, reason: String) -> Self {
        Self {
            limit: limit.into(),
            reason,
            provider: None,
            reset_at: None,
        }
    }
}

/// Run-wide limit name of an exhausted provider account usage or rate limit.
pub const PROVIDER_USAGE: &str = "provider_usage";

/// A provider session failed because the provider account's usage or rate limit
/// was reached. This is a run-wide resource exhaustion, never a ticket failure:
/// the run stops resumably and `kiln resume` retries the interrupted step.
///
/// Provider-agnostic seam: adapters (Codex, Claude Code, fixtures) feed the
/// provider's failure text to [`provider_failure`] and return the resulting error
/// from their agent methods; the engine recognises it through the error chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderLimit {
    pub provider: String,
    /// Provider failure message (bounded excerpt).
    pub message: String,
    /// Reset time reported by the provider, verbatim, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}
impl std::fmt::Display for ProviderLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} usage limit reached: {}", self.provider, self.message)?;
        if let Some(reset) = &self.reset_at {
            write!(f, " (resets: {reset})")?;
        }
        Ok(())
    }
}
impl std::error::Error for ProviderLimit {}
impl ProviderLimit {
    /// Classify provider failure text. Feed only the provider's own failure
    /// message, not agent transcripts: work about rate limiting must not match.
    pub fn detect(provider: &str, text: &str) -> Option<Self> {
        const SIGNS: [&str; 11] = [
            "usage limit",
            "rate limit",
            "rate-limit",
            "rate_limit",
            "ratelimit",
            "too many requests",
            "quota exceeded",
            "insufficient_quota",
            "hit your limit",
            "5-hour limit",
            "weekly limit",
        ];
        let lower = text.to_lowercase();
        if !SIGNS.iter().any(|s| lower.contains(s)) {
            return None;
        }
        let message: String = text.trim().chars().take(500).collect();
        Some(Self {
            provider: provider.into(),
            message,
            reset_at: reset_time(text),
        })
    }
    /// The provider limit carried by an error chain, if any.
    pub fn in_error(error: &anyhow::Error) -> Option<&Self> {
        error.chain().find_map(|e| e.downcast_ref::<Self>())
    }
    pub fn exhaustion(&self) -> LimitExhaustion {
        LimitExhaustion {
            limit: PROVIDER_USAGE.into(),
            reason: self.to_string(),
            provider: Some(self.provider.clone()),
            reset_at: self.reset_at.clone(),
        }
    }
}

/// Error for a failed provider session: a typed [`ProviderLimit`] when the text
/// reports an exhausted usage or rate limit, otherwise a plain failure.
pub fn provider_failure(provider: &str, message: &str) -> anyhow::Error {
    match ProviderLimit::detect(provider, message) {
        Some(limit) => limit.into(),
        None => anyhow::anyhow!("{message}"),
    }
}

/// Reset time as reported ("try again at 10:05 AM", "resets 3pm", "|1717000000").
fn reset_time(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for marker in [
        "try again at ",
        "try again in ",
        "try again after ",
        "resets at ",
        "reset at ",
        "resets in ",
        "resets ",
        "retry after ",
    ] {
        if let Some(start) = lower.find(marker).map(|i| i + marker.len()) {
            let rest = &text[start..];
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                let sentence_end = c == '.'
                    && rest[i + 1..]
                        .chars()
                        .next()
                        .is_none_or(|n| n.is_whitespace() || n == '"');
                if sentence_end || matches!(c, '"' | '\n' | ';' | '}' | '\\' | '|') {
                    end = i;
                    break;
                }
            }
            let reset = rest[..end].trim();
            if !reset.is_empty() {
                return Some(reset.chars().take(80).collect());
            }
        }
    }
    // Claude Code style: "usage limit reached|<unix seconds>".
    let (_, after) = text.split_once("limit reached|")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| format!("unix {digits}"))
}
