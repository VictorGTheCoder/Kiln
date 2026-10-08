//! Implementation session boundary. Git isolation is not an operating-system sandbox.
use crate::{planning::Ticket, Engine, FrozenSpec, Run};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub passed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationSession {
    pub id: String,
    pub ticket_id: String,
    pub context_id: String,
    pub branch: String,
    pub worktree: String,
    pub base_commit: String,
    pub status: String,
    pub diff: String,
    pub commit: Option<String>,
    pub agent_outcome: Option<String>,
    pub agent_log: String,
    pub checks: Vec<CheckResult>,
    pub verification_passed: bool,
    pub failure: Option<String>,
    /// Input version (spec revision) the session was produced under; 0 = frozen specs.
    #[serde(default)]
    pub input_version: u32,
    /// The provider's usage or rate limit cut this attempt short: it is
    /// resumable run-wide exhaustion, not a ticket failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_limit: Option<crate::limits::ProviderLimit>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ImplementationRequest {
    pub context_id: String,
    pub isolation: crate::sandbox::IsolationPolicy,
    pub instructions: String,
    pub ticket: Ticket,
    pub specs: Vec<FrozenSpec>,
    pub repository_instructions: String,
    pub prerequisites: Vec<ImplementationSession>,
    pub worktree: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub outcome: String,
    pub log: String,
}
/// Trusted provider boundary: subprocess adapters MUST launch with request.isolation
/// through Sandbox. FixtureImplementationAgent is an in-process deterministic adapter
/// with path-checked writes, not an untrusted executable.
pub trait ImplementationAgent {
    /// Initialize provider credential redaction before recording any context.
    fn prepare_redaction(&self) -> Result<()> {
        Ok(())
    }

    fn implement(&self, request: &ImplementationRequest) -> Result<AgentResult>;
    /// Provider credential redaction also applies to diffs, checks and failures.
    fn redact_output(&self, text: &str) -> String {
        text.to_owned()
    }
}
#[derive(Deserialize)]
pub struct FixtureImplementationAgent {
    #[serde(default)]
    files: BTreeMap<String, String>,
    outcome: String,
    #[serde(default)]
    log: String,
    /// Provider failure text reported after the files are written, classified
    /// through the shared provider-limit seam like a real adapter's failure.
    #[serde(default)]
    provider_failure: Option<String>,
}
impl FixtureImplementationAgent {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&fs::read(path)?).context("invalid implementation fixture")
    }
}
impl ImplementationAgent for FixtureImplementationAgent {
    fn implement(&self, request: &ImplementationRequest) -> Result<AgentResult> {
        for (name, content) in &self.files {
            let path = Path::new(name);
            if path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
                || name.is_empty()
                || path.starts_with(".git")
                || path.starts_with(".kiln")
            {
                bail!("fixture file must be a safe relative repository path");
            }
            let destination = request.worktree.join(path);
            let parent = destination.parent().context("missing parent")?;
            fs::create_dir_all(parent)?;
            if !fs::canonicalize(parent)?.starts_with(&request.worktree)
                || fs::symlink_metadata(&destination).is_ok_and(|m| m.file_type().is_symlink())
            {
                bail!("fixture path escapes worktree");
            }
            fs::write(destination, content)?;
        }
        if let Some(failure) = &self.provider_failure {
            return Err(crate::limits::provider_failure("fixture", failure));
        }
        Ok(AgentResult {
            outcome: self.outcome.clone(),
            log: self.log.clone(),
        })
    }
}
pub fn git(repository: &Path, args: &[&str]) -> Result<String> {
    let result = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()?;
    if !result.status.success() {
        bail!(
            "Git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout)
        .trim_end()
        .to_owned())
}
impl Engine {
    pub fn implement_ticket(
        &self,
        id: &str,
        ticket_id: &str,
        agent: &dyn ImplementationAgent,
    ) -> Result<Run> {
        agent.prepare_redaction()?;
        let (run, mut session, ticket, prerequisites) = self.transact(id, |run| {
            run.config.isolation.validate(&self.repository)?;
            let plan = run
                .plan
                .as_ref()
                .filter(|p| p.executable)
                .context("implementation requires an independently verified executable plan")?;
            let ticket = plan
                .tickets
                .iter()
                .find(|t| t.id == ticket_id)
                .context("unknown ticket")?
                .clone();
            if run.sessions.iter().any(|s| {
                s.ticket_id == ticket_id
                    && matches!(s.status.as_str(), "running" | "implemented" | "integrated")
            }) {
                bail!("ticket already has an active or completed implementation");
            }
            let mut prerequisites = Vec::new();
            for blocker in &ticket.blocked_by {
                prerequisites.push(
                    run.sessions
                        .iter()
                        .find(|s| {
                            s.ticket_id == *blocker
                                && s.status == "integrated"
                                && s.verification_passed
                                && s.commit.is_some()
                        })
                        .with_context(|| {
                            format!("prerequisite {blocker} is not integrated and verified")
                        })?
                        .clone(),
                );
            }
            let integration = match &run.integration_branch {
                Some(branch) => branch.clone(),
                None => {
                    let branch = format!("kiln/{}/integration", run.id);
                    git(&self.repository, &["branch", &branch, "HEAD"])?;
                    run.integration_branch = Some(branch.clone());
                    branch
                }
            };
            let base_commit = git(&self.repository, &["rev-parse", &integration])?;
            for prerequisite in &prerequisites {
                git(
                    &self.repository,
                    &[
                        "merge-base",
                        "--is-ancestor",
                        prerequisite.commit.as_deref().unwrap(),
                        &base_commit,
                    ],
                )
                .context("prerequisite commit is absent from integration branch")?;
            }
            let session_id = format!("{}-session-{}", run.id, run.sessions.len() + 1);
            let branch = format!("kiln/{}/session-{}", run.id, run.sessions.len() + 1);
            let worktree = self.repository.join(".kiln/worktrees").join(&session_id);
            let session = ImplementationSession {
                id: session_id.clone(),
                ticket_id: ticket_id.into(),
                context_id: session_id.clone(),
                branch,
                worktree: worktree.to_string_lossy().into_owned(),
                base_commit,
                status: "running".into(),
                diff: String::new(),
                commit: None,
                agent_outcome: None,
                agent_log: String::new(),
                checks: Vec::new(),
                verification_passed: false,
                failure: None,
                input_version: run.input_version(),
                provider_limit: None,
            };
            run.sessions.push(session.clone());
            Ok((run.clone(), session, ticket, prerequisites))
        })?;
        let session_id = session.id.clone();
        let worktree = PathBuf::from(&session.worktree);
        let plan = run.plan.as_ref().context("missing accepted plan")?;
        let execution = (|| -> Result<()> {
            fs::create_dir_all(worktree.parent().unwrap())?;
            {
                // Shared Git administrative state: serialize worktree creation.
                let _git = self.lock_git_admin()?;
                git(
                    &self.repository,
                    &[
                        "worktree",
                        "add",
                        "-b",
                        &session.branch,
                        worktree.to_str().context("non UTF-8 worktree")?,
                        &session.base_commit,
                    ],
                )?;
            }
            let specs = run
                .effective_specs()
                .into_iter()
                .filter(|s| {
                    plan.requirements
                        .iter()
                        .any(|r| r.spec_path == s.path && ticket.covers.contains(&r.id))
                })
                .collect();
            let repository_instructions =
                fs::read_to_string(worktree.join("AGENTS.md")).unwrap_or_default();
            let mut instructions = "Implement this ticket using its frozen specs and prerequisite context. Preserve repository standards. Return a concrete usable change; do not integrate or release dependent tickets.".to_owned();
            if let Some(backlog) = &run.backlog {
                instructions.push_str(&format!(
                    " Pinned Matt Pocock workflow version {}. {}",
                    backlog.skill_version,
                    crate::backlog::IMPLEMENTATION_SKILL_INSTRUCTIONS
                ));
            }
            let request = ImplementationRequest {
                context_id: session_id.clone(),
                isolation: run.config.isolation.clone(),
                instructions,
                ticket,
                specs,
                repository_instructions,
                prerequisites,
                worktree: worktree.clone(),
            };
            fs::create_dir_all(self.repository.join(".kiln/contexts"))?;
            fs::write(
                self.repository
                    .join(".kiln/contexts")
                    .join(format!("{session_id}.json")),
                agent.redact_output(
                    &run.config
                        .isolation
                        .redact(&serde_json::to_string_pretty(&request)?),
                ),
            )?;
            let agent_result = self.with_agent_log(id, Some(ticket_id), "implementation", || {
                agent.implement(&request)
            });
            crate::recovery::fault("implementation.after_agent", ticket_id);
            // Collect actual changes even when the provider reports failure.
            git(&worktree, &["add", "-A", "--", "."])?;
            session.diff = git(
                &worktree,
                &["diff", "--cached", "--binary", &session.base_commit],
            )?;
            let result = agent_result?;
            session.agent_outcome =
                Some(agent.redact_output(&run.config.isolation.redact(&result.outcome)));
            session.agent_log = result.log;
            if result.outcome != "completed" {
                bail!("agent outcome: {}", result.outcome);
            }
            if session.diff.is_empty() {
                bail!("agent produced no usable Git change");
            }
            for (name, argv) in [("build", &run.config.build), ("test", &run.config.test)] {
                let check =
                    crate::sandbox::Sandbox::check(&run.config.isolation, &worktree, name, argv);
                session.checks.push(check);
            }
            if session.checks.iter().any(|c| !c.passed) {
                bail!("configured verification failed");
            }
            // Verification may have changed files: reject rather than commit unverified output.
            if git(
                &worktree,
                &["diff", "--cached", "--binary", &session.base_commit],
            )? != session.diff
                || !git(&worktree, &["diff", "--name-only"])?.is_empty()
                || !git(&worktree, &["ls-files", "--others", "--exclude-standard"])?.is_empty()
            {
                bail!("verification changed repository content; result requires fresh checks");
            }
            git(
                &worktree,
                &[
                    "-c",
                    "user.name=Kiln",
                    "-c",
                    "user.email=kiln@localhost",
                    "commit",
                    "-m",
                    &format!("Implement ticket {ticket_id}"),
                ],
            )?;
            crate::recovery::fault("implementation.after_commit", ticket_id);
            session.commit = Some(git(&worktree, &["rev-parse", "HEAD"])?);
            session.verification_passed = true;
            session.status = "implemented".into();
            Ok(())
        })();
        if let Err(error) = execution {
            session.provider_limit = crate::limits::ProviderLimit::in_error(&error).cloned();
            session.status = if crate::sandbox::command_cancellation_active()
                || session.provider_limit.is_some()
            {
                "interrupted"
            } else {
                "failed"
            }
            .into();
            session.failure = Some(format!("{error:#}"));
        }
        session.agent_log = agent.redact_output(&run.config.isolation.redact(&session.agent_log));
        session.diff = agent.redact_output(&run.config.isolation.redact(&session.diff));
        session.failure = session
            .failure
            .map(|f| agent.redact_output(&run.config.isolation.redact(&f)));
        if let Some(limit) = &mut session.provider_limit {
            limit.message = agent.redact_output(&run.config.isolation.redact(&limit.message));
        }
        for check in &mut session.checks {
            check.stdout = agent.redact_output(&run.config.isolation.redact(&check.stdout));
            check.stderr = agent.redact_output(&run.config.isolation.redact(&check.stderr));
        }
        self.transact(id, |latest| {
            let target = latest
                .sessions
                .iter_mut()
                .find(|s| s.id == session.id)
                .context("reserved session is missing")?;
            // A spec replan invalidated this session while it ran: keep the late
            // result inspectable but never integrable.
            if target.status == "superseded" {
                session.status = "superseded".into();
            }
            *target = session.clone();
            Ok(latest.clone())
        })
        .and_then(|run| match session.provider_limit {
            // Recorded first; the caller stops the run on the typed limit.
            Some(limit) => Err(limit.into()),
            None => {
                if session.status == "integrated" {
                    self.remove_worktree(&worktree)?;
                }
                Ok(run)
            }
        })
    }
}
