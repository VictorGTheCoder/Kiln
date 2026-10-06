//! Independent review gate bound to the exact implementation commit.
use crate::{
    execution::{git, CheckResult, ImplementationSession},
    planning::Ticket,
    Engine, FrozenSpec, Run,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub code: String,
    pub message: String,
    pub evidence: String,
    #[serde(default = "required")]
    pub required: bool,
}
fn required() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub outcome: String,
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
    pub evidence: String,
    #[serde(default)]
    pub log: String,
    #[serde(default)]
    pub acceptance_checks: Vec<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisReview {
    pub axis: String,
    pub context_id: String,
    pub result: ReviewResult,
    pub checks: Vec<CheckResult>,
    pub failure: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSession {
    pub id: String,
    pub session_id: String,
    pub ticket_id: String,
    pub commit: String,
    pub diff: String,
    pub standards: AxisReview,
    pub spec: AxisReview,
    pub passed: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ReviewRequest {
    pub context_id: String,
    pub axis: String,
    pub author_context_id: String,
    pub instructions: String,
    pub isolation: crate::sandbox::IsolationPolicy,
    pub ticket: Ticket,
    pub specs: Vec<FrozenSpec>,
    pub repository_standards: String,
    pub commit: String,
    pub diff: String,
    pub implementation_checks: Vec<CheckResult>,
    pub worktree: PathBuf,
}
pub trait ReviewAgent {
    fn review(&self, request: &ReviewRequest) -> Result<ReviewResult>;
    fn redact_output(&self, text: &str) -> String {
        text.into()
    }
}
#[derive(Deserialize)]
pub struct FixtureReviewAgent {
    standards: ReviewResult,
    spec: ReviewResult,
}
impl FixtureReviewAgent {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
}
impl ReviewAgent for FixtureReviewAgent {
    fn review(&self, request: &ReviewRequest) -> Result<ReviewResult> {
        Ok(if request.axis == "standards" {
            &self.standards
        } else {
            &self.spec
        }
        .clone())
    }
}
fn standards(repository: &Path, base: &str) -> Result<String> {
    let paths = git(repository, &["ls-tree", "-r", "--name-only", base])?;
    let mut text = String::new();
    for path in paths.lines().filter(|p| {
        let name = Path::new(p)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        matches!(
            name,
            "AGENTS.md" | "CLAUDE.md" | "CONTRIBUTING.md" | "CODE_STYLE.md" | "standards.md"
        )
    }) {
        text.push_str(&format!("\n--- {path} at {base} ---\n"));
        text.push_str(&git(repository, &["show", &format!("{base}:{path}")])?);
    }
    Ok(text)
}
fn clean(repo: &Path) -> Result<bool> {
    Ok(git(repo, &["status", "--porcelain", "--untracked-files=all"])?.is_empty())
}
fn check(run: &Run, repo: &Path, name: &str, argv: &[String]) -> CheckResult {
    let result = crate::sandbox::Sandbox::command(&run.config.isolation, repo, name, argv, &[])
        .and_then(|mut c| Ok(c.output()?));
    match result {
        Ok(r) => CheckResult {
            name: name.into(),
            command: argv.into(),
            exit_code: r.status.code(),
            stdout: String::from_utf8_lossy(&r.stdout).into(),
            stderr: String::from_utf8_lossy(&r.stderr).into(),
            passed: r.status.success(),
        },
        Err(e) => CheckResult {
            name: name.into(),
            command: argv.into(),
            exit_code: None,
            stdout: String::new(),
            stderr: e.to_string(),
            passed: false,
        },
    }
}
impl AxisReview {
    pub fn verified(&self) -> bool {
        self.failure.is_none()
            && self.result.outcome == "approved"
            && !self.result.evidence.trim().is_empty()
            && self
                .result
                .findings
                .iter()
                .all(|f| !f.required && !f.evidence.trim().is_empty())
            && ["build", "test"]
                .iter()
                .all(|name| self.checks.iter().any(|c| c.name == *name && c.passed))
            && self.checks.iter().all(|c| c.passed)
    }
}
impl ReviewSession {
    /// Consumers must also check the current worktree identity via Engine::review_gate.
    pub fn approves(&self, session: &ImplementationSession) -> bool {
        self.passed
            && self.standards.verified()
            && self.spec.verified()
            && session.verification_passed
            && session.commit.as_deref() == Some(&self.commit)
            && session.id == self.session_id
    }
}
impl Engine {
    pub fn review_gate(&self, run: &Run, session: &ImplementationSession) -> Result<bool> {
        let path = Path::new(&session.worktree);
        Ok(run
            .reviews
            .iter()
            .rev()
            .find(|r| r.session_id == session.id)
            .is_some_and(|r| r.approves(session))
            && git(path, &["rev-parse", "HEAD"])? == session.commit.as_deref().unwrap_or("")
            && clean(path)?)
    }
    pub fn review_ticket(&self, id: &str, ticket_id: &str, agent: &dyn ReviewAgent) -> Result<Run> {
        let mut run = self.inspect(id)?;
        let session = run
            .sessions
            .iter()
            .rev()
            .find(|s| s.ticket_id == ticket_id && s.status == "implemented")
            .context("review requires implemented ticket")?
            .clone();
        let commit = session
            .commit
            .clone()
            .context("missing implementation commit")?;
        let plan = run.plan.as_ref().context("missing plan")?;
        let ticket = plan
            .tickets
            .iter()
            .find(|t| t.id == ticket_id)
            .context("unknown ticket")?
            .clone();
        let diff = git(
            Path::new(&session.worktree),
            &["diff", "--binary", &session.base_commit, &commit],
        )?;
        let specs = run
            .specs
            .iter()
            .filter(|s| {
                plan.requirements
                    .iter()
                    .any(|r| r.spec_path == s.path && ticket.covers.contains(&r.id))
            })
            .cloned()
            .collect::<Vec<_>>();
        let review_id = format!("{}-review-{}", run.id, run.reviews.len() + 1);
        let mut axes = Vec::new();
        for axis in ["standards", "spec"] {
            let context_id = format!("{review_id}-{axis}");
            let temp = tempfile::tempdir()?;
            let repo = temp.path().join("repository");
            git(
                &self.repository,
                &[
                    "clone",
                    "--no-hardlinks",
                    "--quiet",
                    self.repository.to_str().context("repository UTF-8")?,
                    repo.to_str().context("review path UTF-8")?,
                ],
            )?;
            git(&repo, &["checkout", "--detach", &commit])?;
            let request = ReviewRequest {
                context_id: context_id.clone(),
                axis: axis.into(),
                author_context_id: session.context_id.clone(),
                instructions: format!("Independently review {axis} at the exact supplied commit. Observe concrete changes and relevant behavior. Report actionable findings with evidence. Do not edit files, Git metadata, or integrate. Approval cannot substitute for executable evidence. If uncertain return unable-to-verify."),
                isolation: run.config.isolation.clone(),
                ticket: ticket.clone(),
                specs: specs.clone(),
                repository_standards: standards(&repo, &session.base_commit)?,
                commit: commit.clone(),
                diff: diff.clone(),
                implementation_checks: session.checks.clone(),
                worktree: repo.clone(),
            };
            fs::create_dir_all(self.repository.join(".kiln/contexts"))?;
            let context_path = self
                .repository
                .join(".kiln/contexts")
                .join(format!("{context_id}.json"));
            let context_json = serde_json::to_string_pretty(&request)?;
            fs::write(
                &context_path,
                agent.redact_output(&run.config.isolation.redact(&context_json)),
            )?;
            let mut failure = None;
            let mut result = match agent.review(&request) {
                Ok(r) => r,
                Err(e) => {
                    failure = Some(format!("{e:#}"));
                    ReviewResult {
                        outcome: "unable-to-verify".into(),
                        findings: vec![],
                        evidence: String::new(),
                        log: String::new(),
                        acceptance_checks: vec![],
                    }
                }
            };
            // Credentials discovered during the provider invocation must also sanitize inputs.
            fs::write(
                &context_path,
                agent.redact_output(&run.config.isolation.redact(&context_json)),
            )?;
            let mut checks = vec![
                check(&run, &repo, "build", &run.config.build),
                check(&run, &repo, "test", &run.config.test),
            ];
            for argv in &result.acceptance_checks {
                checks.push(check(&run, &repo, "test", argv));
            }
            if !clean(&repo)? || git(&repo, &["rev-parse", "HEAD"])? != commit {
                failure=Some("review or verification changed commit content; requires new implementation and review".into());
            }
            if !session.verification_passed
                || ![("build", &run.config.build), ("test", &run.config.test)]
                    .iter()
                    .all(|(name, argv)| {
                        session
                            .checks
                            .iter()
                            .any(|c| c.name == *name && c.command == **argv && c.passed)
                    })
                || session.checks.iter().any(|c| !c.passed)
                || checks.iter().any(|c| !c.passed)
            {
                failure = Some("required executable checks failed or missing".into());
            }
            if result.evidence.trim().is_empty()
                || result
                    .findings
                    .iter()
                    .any(|f| f.evidence.trim().is_empty() || f.message.trim().is_empty())
            {
                failure = Some("missing observed review evidence".into());
            }
            if !["approved", "rejected", "unable-to-verify"].contains(&result.outcome.as_str()) {
                failure = Some("invalid review outcome".into());
            }
            let redact = |s: &str| agent.redact_output(&run.config.isolation.redact(s));
            result.outcome = redact(&result.outcome);
            result.evidence = redact(&result.evidence);
            result.log = redact(&result.log);
            for f in &mut result.findings {
                f.code = redact(&f.code);
                f.message = redact(&f.message);
                f.evidence = redact(&f.evidence);
            }
            for c in &mut checks {
                c.stdout = redact(&c.stdout);
                c.stderr = redact(&c.stderr);
            }
            axes.push(AxisReview {
                axis: axis.into(),
                context_id,
                result,
                checks,
                failure: failure.map(|f| redact(&f)),
            });
        }
        let spec = axes.pop().unwrap();
        let standards = axes.pop().unwrap();
        let passed = [&spec, &standards].iter().all(|a| a.verified())
            && git(Path::new(&session.worktree), &["rev-parse", "HEAD"])? == commit
            && clean(Path::new(&session.worktree))?;
        run.reviews.push(ReviewSession {
            id: review_id,
            session_id: session.id,
            ticket_id: ticket_id.into(),
            commit,
            diff: agent.redact_output(&run.config.isolation.redact(&diff)),
            standards,
            spec,
            passed,
        });
        self.save(&run)?;
        Ok(run)
    }
}
