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
    #[serde(deserialize_with = "text")]
    pub message: String,
    #[serde(deserialize_with = "text")]
    pub evidence: String,
    #[serde(default = "required")]
    pub required: bool,
}
fn required() -> bool {
    true
}
/// Accepts a string or a list of strings; Codex sometimes itemizes prose fields.
fn text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Text {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Text::deserialize(deserializer)? {
        Text::One(text) => text,
        Text::Many(lines) => lines.join("\n"),
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub outcome: String,
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
    #[serde(deserialize_with = "text")]
    pub evidence: String,
    #[serde(default, deserialize_with = "text")]
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
    /// The provider's usage or rate limit cut this axis short: no verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_limit: Option<crate::limits::ProviderLimit>,
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
    /// Runs before the first durable context is written, including failed invocations.
    fn prepare_redaction(&self) -> Result<()> {
        Ok(())
    }

    fn review(&self, request: &ReviewRequest) -> Result<ReviewResult>;
    fn redact_output(&self, text: &str) -> String {
        text.into()
    }
}
#[derive(Deserialize)]
pub struct FixtureReviewAgent {
    standards: ReviewResult,
    spec: ReviewResult,
    /// Provider failure text, classified through the shared provider-limit seam.
    #[serde(default)]
    provider_failure: Option<String>,
}
impl FixtureReviewAgent {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
}
impl ReviewAgent for FixtureReviewAgent {
    fn review(&self, request: &ReviewRequest) -> Result<ReviewResult> {
        if let Some(failure) = &self.provider_failure {
            return Err(crate::limits::provider_failure("fixture", failure));
        }
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
fn append_failure(failure: &mut Option<String>, message: impl Into<String>) {
    let message = message.into();
    match failure {
        Some(existing) if !existing.contains(&message) => {
            existing.push_str("; ");
            existing.push_str(&message);
        }
        Some(_) => {}
        None => *failure = Some(message),
    }
}
fn check(run: &Run, repo: &Path, name: &str, argv: &[String]) -> CheckResult {
    crate::sandbox::Sandbox::check(&run.config.isolation, repo, name, argv)
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
    /// A review cut short by a provider usage limit carries no verdict.
    pub fn provider_limited(&self) -> bool {
        self.standards.provider_limit.is_some() || self.spec.provider_limit.is_some()
    }
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
/// Whether the session has a review verdict (reviews cut short by a provider
/// usage limit do not count and are rerun).
pub fn reviewed(run: &Run, session_id: &str) -> bool {
    run.reviews
        .iter()
        .any(|r| r.session_id == session_id && !r.provider_limited())
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
        agent.prepare_redaction()?;
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
            .effective_specs()
            .into_iter()
            .filter(|s| {
                plan.requirements
                    .iter()
                    .any(|r| r.spec_path == s.path && ticket.covers.contains(&r.id))
            })
            .collect::<Vec<_>>();
        // Session-scoped identity: concurrent tickets review from independent snapshots.
        let review_id = format!(
            "{}-review-{}",
            session.id,
            run.reviews
                .iter()
                .filter(|r| r.session_id == session.id)
                .count()
                + 1
        );
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
                instructions: format!("Independently review {axis} at the exact supplied commit. Inspect the diff, changed files, and relevant documentation; report concrete observed behavior with file and line references. Report only actionable defects as findings. Do not edit files, modify Git metadata, or integrate. If you run project checks, wait for dependency installation to finish and run build, test, and typecheck commands serially; never launch wrappers that install dependencies concurrently in the same worktree. Kiln reruns configured checks itself. If unable to verify, return unable-to-verify with a concrete explanation. Approval cannot substitute for executable evidence."),
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
            let mut provider_limit = None;
            let mut result = match agent.review(&request) {
                Ok(r) => r,
                Err(e) => {
                    provider_limit = crate::limits::ProviderLimit::in_error(&e).cloned();
                    let provider_error = format!("review provider error: {e:#}");
                    failure = Some(provider_error.clone());
                    ReviewResult {
                        outcome: "unable-to-verify".into(),
                        findings: vec![],
                        evidence: String::new(),
                        log: provider_error,
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
                append_failure(
                    &mut failure,
                    "review or verification changed commit content; requires new implementation and review",
                );
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
                append_failure(&mut failure, "required executable checks failed or missing");
            }
            if result.evidence.trim().is_empty()
                || result
                    .findings
                    .iter()
                    .any(|f| f.evidence.trim().is_empty() || f.message.trim().is_empty())
            {
                append_failure(&mut failure, "missing observed review evidence");
            }
            if !["approved", "rejected", "unable-to-verify"].contains(&result.outcome.as_str()) {
                append_failure(&mut failure, "invalid review outcome");
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
                provider_limit: provider_limit.map(|mut l: crate::limits::ProviderLimit| {
                    l.message = redact(&l.message);
                    l
                }),
            });
            crate::recovery::fault("review.after_axis", ticket_id);
        }
        let spec = axes.pop().unwrap();
        let standards = axes.pop().unwrap();
        let passed = [&spec, &standards].iter().all(|a| a.verified())
            && git(Path::new(&session.worktree), &["rev-parse", "HEAD"])? == commit
            && clean(Path::new(&session.worktree))?;
        let review = ReviewSession {
            id: review_id,
            session_id: session.id,
            ticket_id: ticket_id.into(),
            commit,
            diff: agent.redact_output(&run.config.isolation.redact(&diff)),
            standards,
            spec,
            passed,
        };
        // Serialize once so future provider-added fields and argv cannot bypass redaction.
        let safe_review = agent.redact_output(
            &run.config
                .isolation
                .redact(&serde_json::to_string(&review)?),
        );
        let review: ReviewSession = serde_json::from_str(&safe_review)?;
        let limit = [&review.standards, &review.spec]
            .into_iter()
            .find_map(|a| a.provider_limit.clone());
        run.reviews.push(review);
        run = self.save_ticket(&run, ticket_id)?;
        match limit {
            // Recorded first; the caller stops the run on the typed limit.
            Some(limit) => Err(limit.into()),
            None => Ok(run),
        }
    }
}
