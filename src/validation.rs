//! Global validation of the integrated revision against acceptance workflows
//! derived from the frozen specs. Every acceptance criterion ends `verified`,
//! `failed`, or `unable-to-verify`; missing evidence is never a pass.
use crate::{
    execution::{git, CheckResult},
    planning::requirements,
    sandbox::{IsolationPolicy, Sandbox},
    Engine, ProjectConfig,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const VERIFIED: &str = "verified";
pub const FAILED: &str = "failed";
pub const UNABLE: &str = "unable-to-verify";
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// An argv check that supplies evidence for one frozen acceptance criterion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCheck {
    pub criterion: String,
    pub command: Vec<String>,
}
/// The optional `validation` section of the project configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSettings {
    #[serde(default)]
    pub startup_probe: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub workflows: Vec<AcceptanceCheck>,
}
impl ValidationSettings {
    pub fn from_config(config: &ProjectConfig) -> Result<Self> {
        let settings: Self = match config.extensions.get("validation") {
            Some(value) => serde_json::from_value(value.clone()).context(
                "validation must contain startup_probe, timeout_ms and workflows [{criterion, command}]",
            )?,
            None => Self::default(),
        };
        if settings.timeout_ms == Some(0) {
            bail!("validation.timeout_ms must be a positive integer");
        }
        let commands = settings
            .workflows
            .iter()
            .map(|w| &w.command)
            .chain(settings.startup_probe.iter());
        for argv in commands {
            if !config.isolation.commands.contains(argv) {
                bail!("isolation denied unauthorized validation command");
            }
        }
        Ok(settings)
    }
    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS))
    }
}
/// Acceptance tests an independent verifier derived from the specs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierChecks {
    pub acceptance_checks: Vec<AcceptanceCheck>,
}
impl VerifierChecks {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(
            &std::fs::read(path)
                .with_context(|| format!("read verifier checks {}", path.display()))?,
        )
        .context("verifier checks must be JSON {acceptance_checks:[{criterion, command}]}")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// `configured` (project workflow) or `verifier` (independently derived test).
    pub source: String,
    /// False when the check could not execute, e.g. an unauthorized command.
    pub available: bool,
    pub check: CheckResult,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionResult {
    pub id: String,
    pub spec_path: String,
    pub content_sha256: String,
    pub source_revision: Option<String>,
    pub criterion: String,
    pub outcome: String,
    pub evidence: Vec<Evidence>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub id: String,
    pub integration_branch: Option<String>,
    pub integrated_commit: Option<String>,
    pub worktree: Option<String>,
    pub outcome: String,
    pub checks: Vec<CheckResult>,
    pub criteria: Vec<CriterionResult>,
    pub failure: Option<String>,
    /// Input version (spec revision) the report validated; 0 = frozen specs.
    #[serde(default)]
    pub input_version: u32,
}
impl ValidationReport {
    /// Criteria grouped by outcome, with their supporting evidence.
    pub fn summary(&self, run_id: &str) -> serde_json::Value {
        let by = |outcome: &str| -> Vec<&CriterionResult> {
            self.criteria
                .iter()
                .filter(|c| c.outcome == outcome)
                .collect()
        };
        serde_json::json!({
            "run_id": run_id,
            "id": self.id,
            "outcome": self.outcome,
            "integration_branch": self.integration_branch,
            "integrated_commit": self.integrated_commit,
            "failure": self.failure,
            "checks": self.checks,
            "verified": by(VERIFIED),
            "failed": by(FAILED),
            "unable_to_verify": by(UNABLE),
        })
    }
}

struct Executed {
    check: CheckResult,
    available: bool,
}
impl Executed {
    fn failed(&self) -> bool {
        self.available && !self.check.passed
    }
}
fn unavailable(name: &str, argv: &[String], error: String) -> Executed {
    Executed {
        check: CheckResult {
            name: name.into(),
            command: argv.into(),
            exit_code: None,
            stdout: String::new(),
            stderr: error,
            passed: false,
        },
        available: false,
    }
}
fn capture(child: &mut Child) -> (thread::JoinHandle<String>, thread::JoinHandle<String>) {
    fn reader<R: Read + Send + 'static>(stream: Option<R>) -> thread::JoinHandle<String> {
        thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut buffer);
            }
            String::from_utf8_lossy(&buffer).into_owned()
        })
    }
    (reader(child.stdout.take()), reader(child.stderr.take()))
}
fn spawn(policy: &IsolationPolicy, worktree: &Path, role: &str, argv: &[String]) -> Result<Child> {
    let mut command: Command = Sandbox::command(policy, worktree, role, argv, &[])?;
    Ok(command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}
/// Run a bounded sandboxed check. Commands that cannot start are unavailable
/// evidence, distinct from commands that ran and failed.
fn execute(
    policy: &IsolationPolicy,
    worktree: &Path,
    name: &str,
    role: &str,
    argv: &[String],
    timeout: Duration,
) -> Executed {
    let mut child = match spawn(policy, worktree, role, argv) {
        Ok(child) => child,
        Err(e) => return unavailable(name, argv, policy.redact(&format!("{e:#}"))),
    };
    let (out, err) = capture(&mut child);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = out.join().unwrap_or_default();
    let mut stderr = err.join().unwrap_or_default();
    if status.is_none() {
        stderr.push_str(&format!(
            "\nKiln: timed out after {} ms",
            timeout.as_millis()
        ));
    }
    Executed {
        check: CheckResult {
            name: name.into(),
            command: argv.into(),
            exit_code: status.and_then(|s| s.code()),
            stdout: policy.redact(&stdout),
            stderr: policy.redact(&stderr),
            passed: status.is_some_and(|s| s.success()),
        },
        available: true,
    }
}
/// A started application stops when dropped; the sandbox dies with its parent.
struct Application {
    child: Child,
    output: Option<(thread::JoinHandle<String>, thread::JoinHandle<String>)>,
}
impl Application {
    /// Stop the application and return its captured stdout and stderr.
    fn stop(mut self) -> (String, String) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (out, err) = self.output.take().expect("output captured once");
        (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        )
    }
}
impl Drop for Application {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
/// Start the long-lived application and wait for readiness. Without a probe, a
/// process still running (or exited successfully) after a grace period is ready.
/// Returns the startup check, the last probe check and the running application.
fn start(
    policy: &IsolationPolicy,
    worktree: &Path,
    argv: &[String],
    settings: &ValidationSettings,
) -> (Executed, Option<Executed>, Option<Application>) {
    let mut child = match spawn(policy, worktree, "startup", argv) {
        Ok(child) => child,
        Err(e) => {
            let error = policy.redact(&format!("{e:#}"));
            return (unavailable("startup", argv, error), None, None);
        }
    };
    let output = Some(capture(&mut child));
    let mut app = Application { child, output };
    let timeout = settings.timeout();
    let deadline = Instant::now() + timeout;
    let grace = Instant::now() + timeout.min(Duration::from_millis(500));
    let mut probe: Option<Executed> = None;
    let (ready, exit) = loop {
        let exited = app.child.try_wait().ok().flatten();
        if let Some(status) = exited {
            if !status.success() || settings.startup_probe.is_some() {
                break (false, Some(status));
            }
        }
        if let Some(probe_argv) = &settings.startup_probe {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = execute(
                policy,
                worktree,
                "startup-probe",
                "test",
                probe_argv,
                remaining,
            );
            let stop = result.check.passed || !result.available;
            probe = Some(result);
            if stop {
                break (probe.as_ref().is_some_and(|p| p.check.passed), None);
            }
        } else if Instant::now() >= grace {
            break (true, exited);
        }
        if Instant::now() >= deadline {
            break (false, None);
        }
        thread::sleep(Duration::from_millis(50));
    };
    let available = probe.as_ref().is_none_or(|p| p.available);
    let mut check = CheckResult {
        name: "startup".into(),
        command: argv.into(),
        exit_code: exit.and_then(|s| s.code()),
        stdout: String::new(),
        stderr: String::new(),
        passed: ready,
    };
    if ready {
        return (Executed { check, available }, probe, Some(app));
    }
    let (stdout, mut stderr) = app.stop();
    if exit.is_none() && available {
        stderr.push_str(&format!(
            "\nKiln: application not ready within {} ms",
            timeout.as_millis()
        ));
    }
    check.stdout = policy.redact(&stdout);
    check.stderr = policy.redact(&stderr);
    (Executed { check, available }, probe, None)
}

impl Engine {
    /// Validate the current integrated revision. Configured workflows and
    /// verifier-derived tests supply evidence per frozen acceptance criterion.
    pub fn validate(&self, id: &str, verifier: &VerifierChecks) -> Result<crate::Run> {
        let _owner = self.own_run(id)?;
        let run = self.inspect(id)?;
        let settings = ValidationSettings::from_config(&run.config)?;
        // Validate against the current input version (frozen specs plus verified revisions).
        let specs = run.effective_specs();
        let requirements = requirements(&specs);
        for check in settings.workflows.iter().chain(&verifier.acceptance_checks) {
            if !requirements.iter().any(|r| r.id == check.criterion) {
                bail!(
                    "acceptance check references unknown criterion {}",
                    check.criterion
                );
            }
            if check.command.is_empty() {
                bail!(
                    "acceptance check for {} has an empty command",
                    check.criterion
                );
            }
        }
        let report_id = format!("{}-validation-{}", run.id, run.validation_reports.len() + 1);
        let branch = run.integration_branch.clone();
        let commit = branch.as_ref().and_then(|b| {
            git(
                &self.repository,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("refs/heads/{b}^{{commit}}"),
                ],
            )
            .ok()
        });
        let mut report = ValidationReport {
            id: report_id.clone(),
            integration_branch: branch,
            integrated_commit: commit.clone(),
            worktree: None,
            outcome: UNABLE.into(),
            checks: vec![],
            criteria: vec![],
            failure: None,
            input_version: run.input_version(),
        };
        let mut evidence: Vec<(String, Evidence)> = Vec::new();
        let mut global = Vec::new();
        match commit {
            None => {
                report.failure = Some(
                    "no integrated revision exists; integrate tickets before validation".into(),
                )
            }
            Some(commit) => {
                let path = self.repository.join(".kiln/worktrees").join(&report_id);
                report.worktree = Some(path.to_string_lossy().into());
                git(
                    &self.repository,
                    &[
                        "worktree",
                        "add",
                        "--detach",
                        path.to_str().context("worktree UTF-8")?,
                        &commit,
                    ],
                )?;
                let policy = &run.config.isolation;
                let timeout = settings.timeout();
                for (name, argv) in [("build", &run.config.build), ("test", &run.config.test)] {
                    global.push(execute(policy, &path, name, name, argv, timeout));
                }
                let mut app = None;
                if global.iter().all(|c| c.check.passed) {
                    let (startup, probe, running) =
                        start(policy, &path, &run.config.startup, &settings);
                    global.push(startup);
                    global.extend(probe);
                    app = running;
                }
                if app.is_some() {
                    let checks = settings
                        .workflows
                        .iter()
                        .map(|c| ("configured", c))
                        .chain(verifier.acceptance_checks.iter().map(|c| ("verifier", c)));
                    for (source, check) in checks {
                        let executed =
                            execute(policy, &path, "acceptance", "test", &check.command, timeout);
                        evidence.push((
                            check.criterion.clone(),
                            Evidence {
                                source: source.into(),
                                available: executed.available,
                                check: executed.check,
                            },
                        ));
                    }
                } else {
                    report.failure = Some(
                        "global checks did not pass; acceptance workflows were not run".into(),
                    );
                }
                if let Some(app) = app {
                    let (stdout, stderr) = app.stop();
                    if let Some(startup) = global.iter_mut().find(|c| c.check.name == "startup") {
                        startup.check.stdout = policy.redact(&stdout);
                        startup.check.stderr = policy.redact(&stderr);
                    }
                }
            }
        }
        for requirement in requirements {
            let spec = specs.iter().find(|s| s.path == requirement.spec_path);
            let items: Vec<Evidence> = evidence
                .iter()
                .filter(|(id, _)| *id == requirement.id)
                .map(|(_, e)| e.clone())
                .collect();
            let outcome = if items.iter().any(|e| e.available && !e.check.passed) {
                FAILED
            } else if items.is_empty() || items.iter().any(|e| !e.available) {
                UNABLE
            } else {
                VERIFIED
            };
            report.criteria.push(CriterionResult {
                id: requirement.id,
                spec_path: requirement.spec_path,
                content_sha256: requirement.content_sha256,
                source_revision: spec.and_then(|s| s.source_revision.clone()),
                criterion: requirement.criterion,
                outcome: outcome.into(),
                evidence: items,
            });
        }
        report.outcome = if global.iter().any(Executed::failed)
            || report.criteria.iter().any(|c| c.outcome == FAILED)
        {
            FAILED
        } else if report.integrated_commit.is_none()
            || global.iter().any(|c| !c.available)
            || report.criteria.iter().any(|c| c.outcome != VERIFIED)
        {
            UNABLE
        } else {
            VERIFIED
        }
        .into();
        report.checks = global.into_iter().map(|e| e.check).collect();
        self.transact(id, |latest| {
            let current_tip = latest.integration_branch.as_ref().and_then(|branch| {
                git(
                    &self.repository,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("refs/heads/{branch}^{{commit}}"),
                    ],
                )
                .ok()
            });
            if latest.input_version() != report.input_version
                || current_tip != report.integrated_commit
            {
                report.outcome = "stale".into();
                report.failure = Some(
                    "approved input version or integration branch changed during validation; rerun validation against the current revision".into(),
                );
            }
            latest.validation_reports.push(report);
            Ok(latest.clone())
        })
    }
}
