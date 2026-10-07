//! Claude Code provider for fresh agent contexts (see `agent`).
use crate::agent::{Observation, Provider, Signal};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub type ClaudeAdapter = crate::agent::Adapter<ClaudeConfig>;
pub type ClaudePlanningAgent = crate::agent::PlanningContexts<ClaudeConfig>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Pinned Claude Code executable (e.g. `~/.local/share/claude/versions/X.Y.Z`).
    pub installation: PathBuf,
    /// Authenticated `.credentials.json`; only a private copy reaches the session.
    pub credentials: PathBuf,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    1800
}
impl ClaudeConfig {
    pub fn argv(&self) -> Vec<String> {
        let mut argv: Vec<String> = [
            "/claude/claude",
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--permission-mode",
            "bypassPermissions",
            "--strict-mcp-config",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(model) = &self.model {
            argv.extend(["--model".into(), model.clone()]);
        }
        argv
    }
    pub fn from_project(
        config: &crate::ProjectConfig,
        installation: Option<PathBuf>,
    ) -> Result<Self> {
        crate::agent::from_project(config, installation)
    }
}
impl Provider for ClaudeConfig {
    const NAME: &'static str = "Claude Code";
    const SECTION: &'static str = "claude";
    const EXECUTABLE: &'static str = "/claude/claude";
    const CREDENTIAL_HOME: &'static str = "/home/kiln/.claude";
    const CREDENTIAL_FILE: &'static str = ".credentials.json";
    const CREDENTIAL_HINT: &'static str = "credentials";
    fn installation(&self) -> &Path {
        &self.installation
    }
    fn set_installation(&mut self, path: PathBuf) {
        self.installation = path;
    }
    fn credentials(&self) -> &Path {
        &self.credentials
    }
    fn secret_fields() -> &'static [&'static str] {
        &["accessToken", "refreshToken", "idToken"]
    }
    fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }
    fn argv(&self) -> Vec<String> {
        ClaudeConfig::argv(self)
    }
    /// Only the subscription login reaches the session, never unrelated MCP tokens.
    fn private_credentials(&self, credentials: &Value) -> Value {
        match credentials.get("claudeAiOauth") {
            Some(oauth) => serde_json::json!({ "claudeAiOauth": oauth }),
            None => credentials.clone(),
        }
    }
    fn observe(event: &Value, result: &mut Observation) -> Signal {
        match event["type"].as_str().unwrap_or("") {
            "system" if event["subtype"] == "init" => {
                result.thread_id = event["session_id"].as_str().map(str::to_owned)
            }
            "result" => {
                if result.thread_id.is_none() {
                    result.thread_id = event["session_id"].as_str().map(str::to_owned);
                }
                result.usage = event.get("usage").cloned();
                // The CLI's figure is an API-price estimate, never subscription cost.
                result.cost_estimate = event["total_cost_usd"].as_f64();
                let failed = event["is_error"].as_bool().unwrap_or(false)
                    || event["subtype"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("error"));
                if failed {
                    return Signal::Failure(format!("Claude Code provider failure: {event}"));
                }
                result.message = event["result"].as_str().unwrap_or("").into();
                return Signal::Completed;
            }
            _ => (),
        }
        Signal::Continue
    }
}
