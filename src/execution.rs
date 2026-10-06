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
    fn implement(&self, request: &ImplementationRequest) -> Result<AgentResult>;
}
#[derive(Deserialize)]
pub struct FixtureImplementationAgent {
    #[serde(default)]
    files: BTreeMap<String, String>,
    outcome: String,
    #[serde(default)]
    log: String,
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
        let mut run = self.inspect(id)?;
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
                self.save(&run)?;
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
        fs::create_dir_all(worktree.parent().unwrap())?;
        git(
            &self.repository,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                worktree.to_str().context("non UTF-8 worktree")?,
                &base_commit,
            ],
        )?;
        let worktree = fs::canonicalize(worktree)?;
        let mut session = ImplementationSession {
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
        };
        run.sessions.push(session.clone());
        self.save(&run)?;
        let execution = (|| -> Result<()> {
            let specs = run
                .specs
                .iter()
                .filter(|s| {
                    plan.requirements
                        .iter()
                        .any(|r| r.spec_path == s.path && ticket.covers.contains(&r.id))
                })
                .cloned()
                .collect();
            let repository_instructions =
                fs::read_to_string(worktree.join("AGENTS.md")).unwrap_or_default();
            let request=ImplementationRequest { context_id:session_id.clone(),isolation:run.config.isolation.clone(),instructions:"Implement this ticket using its frozen specs and prerequisite context. Preserve repository standards. Return a concrete usable change; do not integrate or release dependent tickets.".into(),ticket,specs,repository_instructions,prerequisites,worktree:worktree.clone() };
            fs::create_dir_all(self.repository.join(".kiln/contexts"))?;
            fs::write(
                self.repository
                    .join(".kiln/contexts")
                    .join(format!("{session_id}.json")),
                serde_json::to_vec_pretty(&request)?,
            )?;
            let agent_result = agent.implement(&request);
            // Collect actual changes even when the provider reports failure.
            git(&worktree, &["add", "-A", "--", "."])?;
            session.diff = git(
                &worktree,
                &["diff", "--cached", "--binary", &session.base_commit],
            )?;
            let result = agent_result?;
            session.agent_outcome = Some(result.outcome.clone());
            session.agent_log = result.log;
            if result.outcome != "completed" {
                bail!("agent outcome: {}", result.outcome);
            }
            if session.diff.is_empty() {
                bail!("agent produced no usable Git change");
            }
            for (name, argv) in [("build", &run.config.build), ("test", &run.config.test)] {
                let result = crate::sandbox::Sandbox::command(
                    &run.config.isolation,
                    &worktree,
                    name,
                    argv,
                    &[],
                )
                .and_then(|mut command| Ok(command.output()?));
                let check = match result {
                    Ok(result) => CheckResult {
                        name: name.into(),
                        command: argv.clone(),
                        exit_code: result.status.code(),
                        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
                        passed: result.status.success(),
                    },
                    Err(error) => CheckResult {
                        name: name.into(),
                        command: argv.clone(),
                        exit_code: None,
                        stdout: String::new(),
                        stderr: error.to_string(),
                        passed: false,
                    },
                };
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
            session.commit = Some(git(&worktree, &["rev-parse", "HEAD"])?);
            session.verification_passed = true;
            session.status = "implemented".into();
            Ok(())
        })();
        if let Err(error) = execution {
            session.status = "failed".into();
            session.failure = Some(format!("{error:#}"));
        }
        session.agent_log = run.config.isolation.redact(&session.agent_log);
        session.diff = run.config.isolation.redact(&session.diff);
        session.failure = session.failure.map(|f| run.config.isolation.redact(&f));
        for check in &mut session.checks {
            check.stdout = run.config.isolation.redact(&check.stdout);
            check.stderr = run.config.isolation.redact(&check.stderr);
        }
        *run.sessions.last_mut().unwrap() = session;
        self.save(&run)?;
        Ok(run)
    }
}
