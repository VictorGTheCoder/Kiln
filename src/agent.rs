//! Provider-neutral fresh agent contexts. A [`Provider`] (Codex, Claude Code)
//! supplies its argv, credential layout and event vocabulary; launching,
//! isolation, observation, redaction, cancellation and every agent seam are shared.
//! Workflow progression and verification remain engine-owned.
use crate::{
    execution::{AgentResult, ImplementationAgent, ImplementationRequest},
    sandbox::{IsolationPolicy, Mount, Sandbox},
};
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
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

/// Observable result of one provider context, recorded as session/review logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub thread_id: Option<String>,
    pub message: String,
    pub log: String,
    pub usage: Option<Value>,
    /// Exact monetary cost; never claimed for subscription providers.
    pub cost: Option<f64>,
    /// Provider-reported cost figure, informational only (see `limits`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_estimate: Option<f64>,
}

/// What one provider event means to the shared observation loop.
pub enum Signal {
    Continue,
    Completed,
    Failure(String),
}

/// A provider CLI driven non-interactively in the configured isolation.
pub trait Provider: Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Human name used in every failure message ("Codex", "Claude Code").
    const NAME: &'static str;
    /// Project configuration section.
    const SECTION: &'static str;
    /// Executable path inside the namespace.
    const EXECUTABLE: &'static str;
    /// Private directory mounted for the credential copy, and its file name.
    const CREDENTIAL_HOME: &'static str;
    const CREDENTIAL_FILE: &'static str;
    const CREDENTIAL_HINT: &'static str;
    fn installation(&self) -> &Path;
    fn set_installation(&mut self, path: PathBuf);
    fn credentials(&self) -> &Path;
    fn timeout_seconds(&self) -> u64;
    /// Exact argv to authorize in `isolation.commands`; prompts travel on stdin.
    fn argv(&self) -> Vec<String>;
    /// The credential content copied into the session (default: unchanged).
    fn private_credentials(&self, credentials: &Value) -> Value {
        credentials.clone()
    }
    /// Further scoped read-only runtime files next to the executable.
    fn extra_mounts(&self, _executable: &Path) -> Vec<Mount> {
        Vec::new()
    }
    /// Interpret one (already redacted) stdout event.
    fn observe(event: &Value, observation: &mut Observation) -> Signal;
}

/// Read a provider section of the project configuration; the CLI installation path wins.
pub fn from_project<P: Provider>(
    config: &crate::ProjectConfig,
    installation: Option<PathBuf>,
) -> Result<P> {
    let mut value: P = serde_json::from_value(
        config
            .extensions
            .get(P::SECTION)
            .cloned()
            .with_context(|| {
                format!(
                    "configure {} installation, {} and timeout_seconds in project configuration",
                    P::SECTION,
                    P::CREDENTIAL_HINT
                )
            })?,
    )?;
    if let Some(path) = installation {
        value.set_installation(path);
    }
    Ok(value)
}

pub struct Adapter<P: Provider> {
    pub config: P,
    stopped: Arc<AtomicBool>,
    credentials: Mutex<Vec<String>>,
}
impl<P: Provider> Adapter<P> {
    pub fn new(config: P) -> Self {
        Self {
            config,
            stopped: Arc::new(AtomicBool::new(false)),
            credentials: Mutex::new(Vec::new()),
        }
    }
    fn read_credentials(&self) -> Result<(Vec<u8>, Value)> {
        let raw = std::fs::read(self.config.credentials()).with_context(|| {
            format!(
                "{} authentication unavailable: configure an authenticated {}",
                P::NAME,
                P::CREDENTIAL_FILE
            )
        })?;
        let value = serde_json::from_slice(&raw)
            .map_err(|_| anyhow::anyhow!("{} authentication file is invalid", P::NAME))?;
        Ok((raw, value))
    }
    /// Eagerly load scoped authentication redactors before durable input recording.
    pub fn prepare_redaction(&self) -> Result<()> {
        let (_, value) = self.read_credentials()?;
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
    pub fn redact_output(&self, text: &str) -> String {
        redact_with(text, self.credentials.lock().unwrap().iter())
    }
    /// Structured response at the caller's exact, isolated repository snapshot.
    pub fn structured_at<T: DeserializeOwned>(
        &self,
        worktree: &Path,
        policy: &IsolationPolicy,
        prompt: &str,
    ) -> Result<(T, Observation)> {
        let observation = self.invoke(worktree, policy, prompt)?;
        let value = serde_json::from_str(unfence(&observation.message)).with_context(|| {
            format!(
                "{} final response must be the requested JSON value; observed response: {:?}",
                P::NAME,
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
        let name = P::NAME;
        if self.stopped.load(Ordering::SeqCst) {
            bail!("{name} session stopped");
        }
        if policy.network != "allow-all" {
            bail!("{name} requires explicit isolation.network=allow-all model endpoint access");
        }
        if self.config.timeout_seconds() == 0 {
            bail!("{name} timeout must be positive");
        }
        let executable = std::fs::canonicalize(self.config.installation())
            .with_context(|| format!("configured {name} installation is unavailable"))?;
        let (_, credentials_json) = self.read_credentials()?;
        let mut credentials = Vec::new();
        collect_strings(&credentials_json, &mut credentials);
        self.credentials.lock().unwrap().extend(credentials.clone());
        let redact = |input: &str| redact_with(&policy.redact(input), credentials.iter());
        let private = tempfile::tempdir()?;
        let copy = private.path().join(P::CREDENTIAL_FILE);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(
            &copy,
            serde_json::to_vec(&self.config.private_credentials(&credentials_json))?,
        )?;
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o600))?;
        let mut mounts = self.config.extra_mounts(&executable);
        mounts.splice(
            0..0,
            [
                Mount {
                    source: executable,
                    destination: P::EXECUTABLE.into(),
                    writable: false,
                },
                Mount {
                    source: private.path().into(),
                    destination: P::CREDENTIAL_HOME.into(),
                    writable: true,
                },
            ],
        );
        let mut command =
            Sandbox::supervised_command(policy, worktree, "agent", &self.config.argv(), &mounts)?;
        use std::os::unix::process::CommandExt;
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("launch {name} isolated process"))?;
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
            cost_estimate: None,
        };
        let mut failure = None;
        let mut completed = false;
        let mut observe_line =
            |line: std::io::Result<String>,
             result: &mut Observation,
             failure: &mut Option<String>| match line {
                Ok(line) => {
                    let safe = redact(&line);
                    result.log.push_str(&safe);
                    result.log.push('\n');
                    match serde_json::from_str::<Value>(&safe) {
                        Ok(event) => match P::observe(&event, result) {
                            Signal::Continue => (),
                            Signal::Completed => completed = true,
                            Signal::Failure(error) => *failure = Some(error),
                        },
                        Err(_) => *failure = Some(format!("{name} emitted malformed JSONL event")),
                    }
                }
                Err(error) => *failure = Some(format!("observe {name} stdout: {error}")),
            };
        let start = Instant::now();
        let mut status = None;
        while status.is_none() {
            while let Ok(line) = rx.try_recv() {
                observe_line(line, &mut result, &mut failure);
            }
            let stopped = self.stopped.load(Ordering::SeqCst);
            if failure.is_some()
                || stopped
                || start.elapsed() > Duration::from_secs(self.config.timeout_seconds())
            {
                if failure.is_none() {
                    failure = Some(if stopped {
                        format!("{name} session stopped")
                    } else {
                        format!("{name} session timed out")
                    });
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
            observe_line(line, &mut result, &mut failure);
        }
        // Refreshed credentials in the private copy are redacted too.
        if let Ok(copy) = std::fs::read(&copy) {
            if let Ok(copy) = serde_json::from_slice::<Value>(&copy) {
                collect_strings(&copy, &mut self.credentials.lock().unwrap());
            }
        }
        result.message = self.redact_output(&result.message);
        result.log = self.redact_output(&result.log);
        let stderr = self.redact_output(&redact(&errors.join().unwrap_or_default()));
        result.log.push_str(&stderr);
        if let Some(error) = failure {
            bail!("{}; {}", self.redact_output(&error), result.log);
        }
        if !status.unwrap().success() {
            bail!("{name} exited unsuccessfully: {}", result.log);
        }
        if !completed {
            bail!("{name} exited without a completed turn: {}", result.log);
        }
        Ok(result)
    }
}

fn redact_with<'a>(text: &str, credentials: impl Iterator<Item = &'a String>) -> String {
    let mut value = text.to_owned();
    for credential in credentials {
        if !credential.is_empty() {
            value = value.replace(credential, "[REDACTED]");
            let escaped = serde_json::to_string(credential).unwrap();
            value = value.replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
        }
    }
    value
}

/// Accept a single fenced Markdown code block around the requested JSON value.
fn unfence(message: &str) -> &str {
    let trimmed = message.trim();
    let Some(body) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(body) = body.strip_suffix("```") else {
        return trimmed;
    };
    match body.split_once('\n') {
        Some((info, rest)) if !info.trim_start().starts_with(['{', '[']) => rest.trim(),
        _ => body.trim(),
    }
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

impl<P: Provider> ImplementationAgent for Adapter<P> {
    fn prepare_redaction(&self) -> Result<()> {
        Adapter::prepare_redaction(self)
    }
    fn redact_output(&self, text: &str) -> String {
        Adapter::redact_output(self, text)
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

impl<P: Provider> crate::review::ReviewAgent for Adapter<P> {
    fn prepare_redaction(&self) -> Result<()> {
        Adapter::prepare_redaction(self)
    }
    fn redact_output(&self, text: &str) -> String {
        Adapter::redact_output(self, text)
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

impl<P: Provider> crate::correction::CorrectionAgent for Adapter<P> {
    fn prepare_redaction(&self) -> Result<()> {
        Adapter::prepare_redaction(self)
    }
    fn redact_output(&self, s: &str) -> String {
        Adapter::redact_output(self, s)
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

/// Planning runs in disposable clones, never in the user's working checkout.
pub struct PlanningContexts<P: Provider> {
    pub adapter: Adapter<P>,
    pub repository: PathBuf,
    pub isolation: IsolationPolicy,
}
impl<P: Provider> PlanningContexts<P> {
    pub fn structured<T: DeserializeOwned>(&self, prompt: &str) -> Result<T> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repository");
        let output = std::process::Command::new("git")
            .args(["clone", "--no-hardlinks", "--quiet"])
            .arg(&self.repository)
            .arg(&repo)
            .output()?;
        if !output.status.success() {
            bail!(
                "prepare fresh {} context: {}",
                P::NAME,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        self.adapter
            .structured_at(&repo, &self.isolation, prompt)
            .map(|(value, _)| value)
    }
}
impl<P: Provider> crate::planning::PlanningAgent for PlanningContexts<P> {
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
impl<P: Provider> crate::decision::DecisionAgent for PlanningContexts<P> {
    fn decide(
        &self,
        request: &crate::decision::DecisionRequest,
    ) -> Result<crate::decision::Proposal> {
        self.structured(&format!("{}\nReturn only a JSON object with governing (reference of the governing position), resolution, rationale, evidence (array of strings), optional spec_revision ({{path, content}}) and optional tickets (complete revised tickets). Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
impl<P: Provider> crate::replanning::ReplanningAgent for PlanningContexts<P> {
    fn replan(
        &self,
        request: &crate::replanning::ReplanRequest,
    ) -> Result<crate::planning::Ticket> {
        self.structured(&format!("{}\nReturn only one JSON ticket object with id (unchanged), title, description, acceptance_criteria, covers (exact requirement IDs), blocked_by. Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
impl<P: Provider> crate::spec_replanning::SpecReplanningAgent for PlanningContexts<P> {
    fn revise(
        &self,
        request: &crate::spec_replanning::SpecReplanRequest,
    ) -> Result<crate::spec_replanning::SpecReplanProposal> {
        self.structured(&format!("{}\nReturn only a JSON object with tickets (array of complete revised or new tickets, each with id, title, description, acceptance_criteria, covers (exact requirement IDs), blocked_by) and remove_ticket_ids (array of affected existing ticket IDs that are obsolete). Do not edit files.\n{}",request.instructions,serde_json::to_string_pretty(request)?))
    }
}
