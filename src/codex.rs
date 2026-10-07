//! Fresh Codex contexts. Workflow progression and verification remain engine-owned.
use crate::{
    execution::{AgentResult, ImplementationAgent, ImplementationRequest},
    sandbox::{IsolationPolicy, Mount, Sandbox},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexConfig {
    pub installation: PathBuf,
    pub auth: PathBuf,
    #[serde(default)]
    pub model: Option<String>,
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
        argv.push("-".into());
        argv
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub thread_id: Option<String>,
    pub message: String,
    pub log: String,
    pub usage: Option<Value>,
    pub cost: Option<f64>,
}
pub struct CodexAdapter {
    pub config: CodexConfig,
    stopped: Arc<AtomicBool>,
    credentials: Mutex<Vec<String>>,
}
impl CodexAdapter {
    pub fn new(config: CodexConfig) -> Self {
        Self {
            config,
            stopped: Arc::new(AtomicBool::new(false)),
            credentials: Mutex::new(Vec::new()),
        }
    }
    /// Eagerly load scoped authentication redactors before durable input recording.
    pub fn prepare_redaction(&self) -> Result<()> {
        let auth = std::fs::read(&self.config.auth)
            .context("Codex authentication unavailable: configure an authenticated auth.json")?;
        let value: Value = serde_json::from_slice(&auth)
            .map_err(|_| anyhow::anyhow!("Codex authentication file is invalid"))?;
        collect_strings(&value, &mut self.credentials.lock().unwrap());
        Ok(())
    }
    /// Clonable cancellation handle; stopping kills the sandbox and descendants.
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stopped.clone()
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
    /// Structured response at the caller's exact, isolated repository snapshot.
    pub fn structured_at<T: serde::de::DeserializeOwned>(
        &self,
        worktree: &Path,
        policy: &IsolationPolicy,
        prompt: &str,
    ) -> Result<(T, Observation)> {
        let observation = self.invoke(worktree, policy, prompt)?;
        let value = serde_json::from_str(&observation.message).with_context(|| {
            format!(
                "Codex final response must be the requested JSON value; observed response: {:?}",
                observation.message
            )
        })?;
        Ok((value, observation))
    }
    pub fn invoke(
        &self,
        worktree: &Path,
        policy: &IsolationPolicy,
        prompt: &str,
    ) -> Result<Observation> {
        if self.stopped.load(Ordering::SeqCst) {
            bail!("Codex session stopped");
        }
        if policy.network != "allow-all" {
            bail!("Codex requires explicit isolation.network=allow-all model endpoint access");
        }
        if self.config.timeout_seconds == 0 {
            bail!("Codex timeout must be positive");
        }
        let executable = std::fs::canonicalize(&self.config.installation)
            .context("configured Codex installation is unavailable")?;
        let auth = std::fs::read(&self.config.auth)
            .context("Codex authentication unavailable: configure an authenticated auth.json")?;
        let auth_json: Value =
            serde_json::from_slice(&auth).context("Codex authentication file is invalid")?;
        let mut credentials = Vec::new();
        collect_strings(&auth_json, &mut credentials);
        self.credentials.lock().unwrap().extend(credentials.clone());
        let redact = |input: &str| {
            let mut value = policy.redact(input);
            for credential in &credentials {
                if !credential.is_empty() {
                    value = value.replace(credential, "[REDACTED]");
                    let escaped = serde_json::to_string(credential).unwrap();
                    value = value.replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
                }
            }
            value
        };
        let private = tempfile::tempdir()?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(private.path().join("auth.json"), auth)?;
        std::fs::set_permissions(
            private.path().join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )?;
        let companion = executable.parent().unwrap().join("codex-code-mode-host");
        let mut mounts = vec![
            Mount {
                source: executable,
                destination: "/codex/codex".into(),
                writable: false,
            },
            Mount {
                source: private.path().into(),
                destination: "/home/kiln/.codex".into(),
                writable: true,
            },
        ];
        if companion.is_file() {
            mounts.push(Mount {
                source: companion,
                destination: "/codex/codex-code-mode-host".into(),
                writable: false,
            });
        }
        let mut command =
            Sandbox::supervised_command(policy, worktree, "agent", &self.config.argv(), &mounts)?;
        use std::os::unix::process::CommandExt;
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("launch Codex isolated process")?;
        let pid = child.id();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let errors = thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        });
        let mut stdin = child.stdin.take().unwrap();
        let prompt = prompt.to_owned();
        let writer = thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
        });
        let mut result = Observation {
            thread_id: None,
            message: String::new(),
            log: String::new(),
            usage: None,
            cost: None,
        };
        let mut failure = None;
        let mut limit: Option<crate::limits::ProviderLimit> = None;
        let start = Instant::now();
        let mut status = None;
        let mut completed = false;
        while status.is_none() {
            while let Ok(line) = rx.try_recv() {
                match line {
                    Ok(line) => {
                        let safe = redact(&line);
                        result.log.push_str(&safe);
                        result.log.push('\n');
                        match serde_json::from_str::<Value>(&safe) {
                            Ok(event) => match event["type"].as_str().unwrap_or("") {
                                "thread.started" => {
                                    result.thread_id =
                                        event["thread_id"].as_str().map(str::to_owned)
                                }
                                "turn.completed" => {
                                    completed = true;
                                    result.usage = event.get("usage").cloned();
                                }
                                "error" | "turn.failed" => {
                                    limit = limit.or_else(|| usage_limit(&event));
                                    failure = Some(format!("Codex provider failure: {event}"))
                                }
                                "item.completed" if event["item"]["type"] == "error" => {
                                    failure = Some(format!(
                                        "Codex capability failure: {}",
                                        event["item"]["message"]
                                    ))
                                }
                                "item.completed" if event["item"]["type"] == "agent_message" => {
                                    result.message =
                                        event["item"]["text"].as_str().unwrap_or("").into()
                                }
                                _ => (),
                            },
                            Err(_) => failure = Some("Codex emitted malformed JSONL event".into()),
                        }
                    }
                    Err(error) => failure = Some(format!("observe Codex stdout: {error}")),
                }
            }
            if failure.is_some()
                || self.stopped.load(Ordering::SeqCst)
                || start.elapsed() > Duration::from_secs(self.config.timeout_seconds)
            {
                if failure.is_none() {
                    failure = Some(
                        if self.stopped.load(Ordering::SeqCst) {
                            "Codex session stopped"
                        } else {
                            "Codex session timed out"
                        }
                        .into(),
                    );
                }
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }
                thread::sleep(Duration::from_millis(150));
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
                status = Some(child.wait()?);
            } else {
                status = child.try_wait()?;
                thread::sleep(Duration::from_millis(10));
            }
        }
        // Reader may have delivered final events just before exit; drain after its join.
        let _ = reader.join();
        let _ = writer.join();
        for line in rx {
            match line {
                Ok(line) => {
                    let safe = redact(&line);
                    result.log.push_str(&safe);
                    result.log.push('\n');
                    match serde_json::from_str::<Value>(&safe) {
                        Ok(event) => match event["type"].as_str().unwrap_or("") {
                            "thread.started" => {
                                result.thread_id = event["thread_id"].as_str().map(str::to_owned)
                            }
                            "turn.completed" => {
                                completed = true;
                                result.usage = event.get("usage").cloned();
                            }
                            "item.completed" if event["item"]["type"] == "error" => {
                                failure = Some(format!(
                                    "Codex capability failure: {}",
                                    event["item"]["message"]
                                ))
                            }
                            "item.completed" if event["item"]["type"] == "agent_message" => {
                                result.message = event["item"]["text"].as_str().unwrap_or("").into()
                            }
                            "error" | "turn.failed" => {
                                limit = limit.or_else(|| usage_limit(&event));
                                failure = Some(format!("Codex provider failure: {event}"))
                            }
                            _ => (),
                        },
                        Err(_) => failure = Some("Codex emitted malformed JSONL event".into()),
                    }
                }
                Err(e) => failure = Some(e.to_string()),
            }
        }
        if let Ok(auth) = std::fs::read(private.path().join("auth.json")) {
            if let Ok(auth) = serde_json::from_slice::<Value>(&auth) {
                collect_strings(&auth, &mut self.credentials.lock().unwrap());
            }
        }
        result.message = self.redact_output(&result.message);
        result.log = self.redact_output(&result.log);
        let stderr = self.redact_output(&redact(&errors.join().unwrap_or_default()));
        result.log.push_str(&stderr);
        if let Some(error) = failure {
            let detail = format!("{}; {}", self.redact_output(&error), result.log);
            if let Some(mut limit) = limit {
                limit.message = self.redact_output(&limit.message);
                return Err(anyhow::Error::new(limit).context(detail));
            }
            bail!("{detail}");
        }
        if !status.unwrap().success() {
            bail!("Codex exited unsuccessfully: {}", result.log);
        }
        if !completed {
            bail!("Codex exited without a completed turn: {}", result.log);
        }
        Ok(result)
    }
}
/// Feed only the provider's own error message to the shared limit classifier.
fn usage_limit(event: &Value) -> Option<crate::limits::ProviderLimit> {
    let message = [
        &event["message"],
        &event["error"]["message"],
        &event["error"],
    ]
    .into_iter()
    .find_map(Value::as_str)
    .map_or_else(|| event.to_string(), str::to_owned);
    crate::limits::ProviderLimit::detect("codex", &message)
}
fn collect_strings(value: &Value, result: &mut Vec<String>) {
    match value {
        Value::String(s) => result.push(s.clone()),
        Value::Object(o) => {
            for v in o.values() {
                collect_strings(v, result)
            }
        }
        Value::Array(a) => {
            for v in a {
                collect_strings(v, result)
            }
        }
        _ => {}
    }
}
impl ImplementationAgent for CodexAdapter {
    fn prepare_redaction(&self) -> Result<()> {
        CodexAdapter::prepare_redaction(self)
    }

    fn redact_output(&self, text: &str) -> String {
        let mut result = text.to_owned();
        for credential in self.credentials.lock().unwrap().iter() {
            if !credential.is_empty() {
                result = result.replace(credential, "[REDACTED]");
                let escaped = serde_json::to_string(credential).unwrap();
                result = result.replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
            }
        }
        result
    }
    fn implement(&self, request: &ImplementationRequest) -> Result<AgentResult> {
        let prompt=format!("Adapted implement-spec and Matt Pocock TDD: work only on this assigned ticket. Use red-green vertical slices at the approved CLI temporary Git boundary or the ticket acceptance seam. Follow repository instructions. Do not stage, commit, integrate, release, or modify Git metadata; Rust owns progression. Do not invoke implement-spec for the full run. Preserve other work. Execute relevant verification and report evidence. The external sandbox authorizes system tools and the configured network policy. Return a concrete change.\n{}",serde_json::to_string_pretty(request)?);
        let result = self.invoke(&request.worktree, &request.isolation, &prompt)?;
        Ok(AgentResult {
            outcome: "completed".into(),
            log: serde_json::to_string(&result)?,
        })
    }
}

/// Planning runs in disposable clones, never in the user's working checkout.
pub struct CodexPlanningAgent {
    pub adapter: CodexAdapter,
    pub repository: PathBuf,
    pub isolation: IsolationPolicy,
}
impl CodexPlanningAgent {
    pub fn structured<T: serde::de::DeserializeOwned>(&self, prompt: &str) -> Result<T> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repository");
        let output = std::process::Command::new("git")
            .args(["clone", "--no-hardlinks", "--quiet"])
            .arg(&self.repository)
            .arg(&repo)
            .output()?;
        if !output.status.success() {
            bail!(
                "prepare fresh Codex context: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        self.adapter
            .structured_at(&repo, &self.isolation, prompt)
            .map(|(value, _)| value)
    }
}
impl crate::planning::PlanningAgent for CodexPlanningAgent {
    fn generate(
        &self,
        request: &crate::planning::PlanningRequest,
    ) -> Result<Vec<crate::planning::Ticket>> {
        self.structured(&format!("{}\nReturn only a JSON array of tickets, each with id, title, description, acceptance_criteria, covers (exact requirement IDs), blocked_by. Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
    fn verify(
        &self,
        request: &crate::planning::VerificationRequest,
    ) -> Result<crate::planning::Verification> {
        self.structured(&format!("{}\nReturn only a JSON object with outcome (verified, failed, unable-to-verify) and findings (array of objects with code and message). Use findings only for concrete blocking defects that make the outcome failed or unable-to-verify; positive confirmations belong nowhere in findings. A verified outcome must have an empty findings array. Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
impl crate::decision::DecisionAgent for CodexPlanningAgent {
    fn decide(
        &self,
        request: &crate::decision::DecisionRequest,
    ) -> Result<crate::decision::Proposal> {
        self.structured(&format!("{}\nReturn only a JSON object with governing (reference of the governing position), resolution, rationale, evidence (array of strings), optional spec_revision ({{path, content}}) and optional tickets (complete revised tickets). Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
impl crate::replanning::ReplanningAgent for CodexPlanningAgent {
    fn replan(
        &self,
        request: &crate::replanning::ReplanRequest,
    ) -> Result<crate::planning::Ticket> {
        self.structured(&format!("{}\nReturn only one JSON ticket object with id (unchanged), title, description, acceptance_criteria, covers (exact requirement IDs), blocked_by. Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
impl crate::spec_replanning::SpecReplanningAgent for CodexPlanningAgent {
    fn revise(
        &self,
        request: &crate::spec_replanning::SpecReplanRequest,
    ) -> Result<crate::spec_replanning::SpecReplanProposal> {
        self.structured(&format!("{}\nReturn only a JSON object with tickets (array of complete revised or new tickets, each with id, title, description, acceptance_criteria, covers (exact requirement IDs), blocked_by) and remove_ticket_ids (array of affected existing ticket IDs that are obsolete). Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
impl CodexConfig {
    pub fn from_project(
        config: &crate::ProjectConfig,
        installation: Option<PathBuf>,
    ) -> Result<Self> {
        let mut value: Self =
            serde_json::from_value(config.extensions.get("codex").cloned().context(
                "configure codex installation, auth and timeout_seconds in project configuration",
            )?)?;
        if let Some(path) = installation {
            value.installation = path;
        }
        Ok(value)
    }
}

impl crate::review::ReviewAgent for CodexAdapter {
    fn prepare_redaction(&self) -> Result<()> {
        CodexAdapter::prepare_redaction(self)
    }

    fn redact_output(&self, text: &str) -> String {
        ImplementationAgent::redact_output(self, text)
    }
    fn review(
        &self,
        request: &crate::review::ReviewRequest,
    ) -> Result<crate::review::ReviewResult> {
        let prompt = format!("{}\nReturn only one JSON object with exactly these field types: outcome is a string (approved, rejected, unable-to-verify); findings is an array of objects with code (string), message (string), evidence (string), and required (boolean); evidence MUST be a nonempty string describing concrete observed behavior and naming relevant files; log MUST be a string; acceptance_checks is an optional array of authorized argv arrays (arrays of strings) derived from acceptance criteria, or empty. Evidence, log, finding message, and finding evidence are strings, never arrays or objects. Findings contain only actionable defects. Do not put positive confirmations in findings. Do not claim passing evidence for checks you did not observe.\n{}",request.instructions,serde_json::to_string_pretty(request)?);
        let (mut review, result): (crate::review::ReviewResult, Observation) =
            self.structured_at(&request.worktree, &request.isolation, &prompt)?;
        review.log = serde_json::to_string(&result)?;
        Ok(review)
    }
}

impl crate::correction::CorrectionAgent for CodexAdapter {
    fn prepare_redaction(&self) -> Result<()> {
        CodexAdapter::prepare_redaction(self)
    }
    fn redact_output(&self, s: &str) -> String {
        ImplementationAgent::redact_output(self, s)
    }
    fn correct(&self, r: &crate::correction::CorrectionRequest) -> Result<AgentResult> {
        let prompt=format!("Correct only this ticket's unresolved findings and failing checks using the frozen specs. Use TDD vertical slices. Do not stage, commit, integrate or modify Git metadata. Rust owns checks and fresh review.\n{}",serde_json::to_string_pretty(r)?);
        let o = self.invoke(&r.worktree, &r.isolation, &prompt)?;
        Ok(AgentResult {
            outcome: "completed".into(),
            log: serde_json::to_string(&o)?,
        })
    }
}
