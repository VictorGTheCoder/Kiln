//! Codex provider for fresh agent contexts (see `agent`).
use crate::{
    agent::{Provider, Signal},
    sandbox::Mount,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub use crate::agent::Observation;
pub type CodexAdapter = crate::agent::Adapter<CodexConfig>;
pub type CodexPlanningAgent = crate::agent::PlanningContexts<CodexConfig>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexConfig {
    pub installation: PathBuf,
    pub auth: PathBuf,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    1800
}
impl CodexConfig {
    pub fn argv(&self) -> Vec<String> {
        let mut argv: Vec<String> = [
            "/codex/codex",
            "exec",
            "--json",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--color",
            "never",
            "--sandbox",
            "danger-full-access",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "features.code_mode=false",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(model) = &self.model {
            argv.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &self.reasoning_effort {
            argv.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
        }
        argv.push("-".into());
        argv
    }
    pub fn from_project(
        config: &crate::ProjectConfig,
        installation: Option<PathBuf>,
    ) -> Result<Self> {
        crate::agent::from_project(config, installation)
    }
}
impl Provider for CodexConfig {
    const NAME: &'static str = "Codex";
    const SECTION: &'static str = "codex";
    const EXECUTABLE: &'static str = "/codex/codex";
    const CREDENTIAL_HOME: &'static str = "/home/kiln/.codex";
    const CREDENTIAL_FILE: &'static str = "auth.json";
    const CREDENTIAL_HINT: &'static str = "auth";
    fn installation(&self) -> &Path {
        &self.installation
    }
    fn set_installation(&mut self, path: PathBuf) {
        self.installation = path;
    }
    fn credentials(&self) -> &Path {
        &self.auth
    }
    fn secret_fields() -> &'static [&'static str] {
        &["access_token", "refresh_token", "id_token"]
    }
    fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }
    fn argv(&self) -> Vec<String> {
        CodexConfig::argv(self)
    }
    /// Recent installations require their companion host even with code_mode disabled.
    fn extra_mounts(&self, executable: &Path) -> Vec<Mount> {
        let companion = executable.parent().unwrap().join("codex-code-mode-host");
        if !companion.is_file() {
            return Vec::new();
        }
        vec![Mount {
            source: companion,
            destination: "/codex/codex-code-mode-host".into(),
            writable: false,
        }]
    }
    fn observe(event: &Value, result: &mut Observation) -> Signal {
        match event["type"].as_str().unwrap_or("") {
            "thread.started" => result.thread_id = event["thread_id"].as_str().map(str::to_owned),
            "turn.completed" => {
                result.usage = event.get("usage").cloned();
                return Signal::Completed;
            }
            "error" | "turn.failed" => {
                return Signal::Failure(format!("Codex provider failure: {event}"))
            }
            "item.completed" if event["item"]["type"] == "error" => {
                return Signal::Failure(format!(
                    "Codex capability failure: {}",
                    event["item"]["message"]
                ))
            }
            "item.completed" if event["item"]["type"] == "agent_message" => {
                result.message = event["item"]["text"].as_str().unwrap_or("").into()
            }
            _ => (),
        }
        Signal::Continue
    }
}
