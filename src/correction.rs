//! Bounded autonomous correction, with durable evidence for every attempted cycle.
use crate::{
    execution::{git, AgentResult, CheckResult, ImplementationSession},
    planning::Ticket,
    review::{ReviewAgent, ReviewSession},
    Engine, FrozenSpec, Run,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionCycle {
    pub id: String,
    pub ticket_id: String,
    pub before: ImplementationSession,
    pub findings: Vec<String>,
    pub after: ImplementationSession,
    pub review: Option<ReviewSession>,
    pub outcome: String,
    pub failure: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CorrectionRequest {
    pub context_id: String,
    pub ticket: Ticket,
    pub specs: Vec<FrozenSpec>,
    pub diff: String,
    pub findings: Vec<String>,
    pub checks: Vec<CheckResult>,
    pub repository_instructions: String,
    pub isolation: crate::sandbox::IsolationPolicy,
    pub worktree: PathBuf,
}
pub trait CorrectionAgent {
    fn prepare_redaction(&self) -> Result<()> {
        Ok(())
    }
    fn correct(&self, request: &CorrectionRequest) -> Result<AgentResult>;
    fn redact_output(&self, s: &str) -> String {
        s.into()
    }
}
#[derive(Deserialize)]
pub struct FixtureCorrectionAgent {
    corrections: Vec<crate::execution::FixtureImplementationAgent>,
    reviews: Vec<crate::review::FixtureReviewAgent>,
    #[serde(skip)]
    correction_index: Cell<usize>,
    #[serde(skip)]
    review_index: Cell<usize>,
}
impl FixtureCorrectionAgent {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
}
impl CorrectionAgent for FixtureCorrectionAgent {
    fn correct(&self, r: &CorrectionRequest) -> Result<AgentResult> {
        let i = self.correction_index.get();
        self.correction_index.set(i + 1);
        let agent = self
            .corrections
            .get(i)
            .context("correction fixture sequence exhausted")?;
        crate::execution::ImplementationAgent::implement(
            agent,
            &crate::execution::ImplementationRequest {
                context_id: r.context_id.clone(),
                isolation: r.isolation.clone(),
                instructions: "Correct unresolved findings".into(),
                ticket: r.ticket.clone(),
                specs: r.specs.clone(),
                repository_instructions: r.repository_instructions.clone(),
                prerequisites: vec![],
                worktree: r.worktree.clone(),
            },
        )
    }
}
impl ReviewAgent for FixtureCorrectionAgent {
    fn review(&self, r: &crate::review::ReviewRequest) -> Result<crate::review::ReviewResult> {
        let i = self.review_index.get();
        let agent = self
            .reviews
            .get(i)
            .context("review fixture sequence exhausted")?;
        let result = agent.review(r);
        if r.axis == "spec" {
            self.review_index.set(i + 1);
        }
        result
    }
}
impl Engine {
    /// Reusable scheduler entry. Never integrates; consumers must use review_gate.
    pub fn correct_ticket(
        &self,
        id: &str,
        ticket_id: &str,
        agent: &dyn CorrectionAgent,
        reviewer: &dyn ReviewAgent,
    ) -> Result<Run> {
        self.correct_ticket_within(id, ticket_id, agent, reviewer, &|| true)
    }
    /// As `correct_ticket`, but starts another cycle only while `resources_remain`
    /// (run-wide limits). A cycle not started is not counted against the ticket.
    pub fn correct_ticket_within(
        &self,
        id: &str,
        ticket_id: &str,
        agent: &dyn CorrectionAgent,
        reviewer: &dyn ReviewAgent,
        resources_remain: &dyn Fn() -> bool,
    ) -> Result<Run> {
        agent.prepare_redaction()?;
        reviewer.prepare_redaction()?;
        let mut run = self.inspect(id)?;
        let limit = crate::limits::RunLimits::from_config(&run.config)?.correction_cycles;
        let used = run
            .corrections
            .iter()
            .filter(|c| c.ticket_id == ticket_id)
            .count() as u64;
        let mut index = run
            .sessions
            .iter()
            .rposition(|s| {
                s.ticket_id == ticket_id && ["failed", "implemented"].contains(&s.status.as_str())
            })
            .context("correction requires attempted implementation")?;
        if run.sessions[index].status == "implemented"
            && !run
                .reviews
                .iter()
                .any(|r| r.session_id == run.sessions[index].id)
        {
            run = self.review_ticket(id, ticket_id, reviewer)?;
        }
        if self.review_gate(&run, &run.sessions[index])? {
            return Ok(run);
        }
        if run
            .corrections
            .iter()
            .rev()
            .find(|c| c.ticket_id == ticket_id)
            .is_some_and(|c| matches!(c.outcome.as_str(), "no-progress" | "exhausted"))
        {
            return Ok(run);
        }
        for number in used..limit {
            if !resources_remain() {
                break;
            }
            let before = run.sessions[index].clone();
            let worktree = PathBuf::from(&before.worktree);
            let mut findings = vec![];
            if let Some(f) = &before.failure {
                findings.push(f.clone());
            }
            if let Some(r) = run.reviews.iter().rev().find(|r| r.session_id == before.id) {
                for axis in [&r.standards, &r.spec] {
                    for f in &axis.result.findings {
                        if f.required {
                            findings.push(format!(
                                "{} {}: {} ({})",
                                axis.axis, f.code, f.message, f.evidence
                            ));
                        }
                    }
                    if let Some(f) = &axis.failure {
                        findings.push(format!("{}: {}", axis.axis, f));
                    }
                    if !axis.verified() {
                        findings.push(format!(
                            "{} outcome: {} evidence: {}",
                            axis.axis, axis.result.outcome, axis.result.evidence
                        ));
                    }
                }
            }
            let plan = run.plan.as_ref().context("missing plan")?;
            let ticket = plan
                .tickets
                .iter()
                .find(|t| t.id == ticket_id)
                .context("unknown ticket")?
                .clone();
            let specs = run
                .effective_specs()
                .into_iter()
                .filter(|s| {
                    plan.requirements
                        .iter()
                        .any(|r| r.spec_path == s.path && ticket.covers.contains(&r.id))
                })
                .collect();
            // Session-scoped identity: concurrent tickets correct from independent snapshots.
            let cycle_id = format!("{}-correction-{}", before.id, number + 1);
            let request = CorrectionRequest {
                context_id: cycle_id.clone(),
                ticket,
                specs,
                diff: git(&worktree, &["diff", "--binary", &before.base_commit])?,
                findings: findings.clone(),
                checks: before.checks.clone(),
                repository_instructions: fs::read_to_string(worktree.join("AGENTS.md"))
                    .unwrap_or_default(),
                isolation: run.config.isolation.clone(),
                worktree: worktree.clone(),
            };
            let redact = |s: &str| {
                reviewer.redact_output(&agent.redact_output(&run.config.isolation.redact(s)))
            };
            let context_path = self
                .repository
                .join(".kiln/contexts")
                .join(format!("{cycle_id}.json"));
            fs::write(
                &context_path,
                redact(&serde_json::to_string_pretty(&request)?),
            )?;
            let mut after = before.clone();
            after.context_id = cycle_id.clone();
            after.verification_passed = false;
            after.commit = None;
            after.checks.clear();
            after.failure = None;
            after.status = "failed".into();
            let attempt = (|| -> Result<()> {
                let result = agent.correct(&request);
                git(&worktree, &["add", "-A", "--", "."])?;
                after.diff = git(
                    &worktree,
                    &["diff", "--cached", "--binary", &before.base_commit],
                )?;
                let result = result?;
                if after.diff.is_empty() {
                    bail!("correction produced no usable Git change");
                }
                after.agent_outcome = Some(result.outcome.clone());
                after.agent_log = result.log;
                if result.outcome != "completed" {
                    bail!("correction outcome: {}", result.outcome);
                }
                for (name, argv) in [("build", &run.config.build), ("test", &run.config.test)] {
                    let output = crate::sandbox::Sandbox::command(
                        &run.config.isolation,
                        &worktree,
                        name,
                        argv,
                        &[],
                    )
                    .and_then(|mut c| Ok(c.output()?));
                    after.checks.push(match output {
                        Ok(o) => CheckResult {
                            name: name.into(),
                            command: argv.clone(),
                            exit_code: o.status.code(),
                            stdout: String::from_utf8_lossy(&o.stdout).into(),
                            stderr: String::from_utf8_lossy(&o.stderr).into(),
                            passed: o.status.success(),
                        },
                        Err(e) => CheckResult {
                            name: name.into(),
                            command: argv.clone(),
                            exit_code: None,
                            stdout: String::new(),
                            stderr: e.to_string(),
                            passed: false,
                        },
                    });
                }
                if after.checks.iter().any(|c| !c.passed) {
                    bail!("configured verification failed");
                }
                if !git(&worktree, &["diff", "--name-only"])?.is_empty()
                    || !git(&worktree, &["ls-files", "--others", "--exclude-standard"])?.is_empty()
                    || git(
                        &worktree,
                        &["diff", "--cached", "--binary", &before.base_commit],
                    )? != after.diff
                {
                    bail!("verification changed repository content");
                }
                if !git(&worktree, &["diff", "--cached", "--name-only"])?.is_empty() {
                    git(
                        &worktree,
                        &[
                            "-c",
                            "user.name=Kiln",
                            "-c",
                            "user.email=kiln@localhost",
                            "commit",
                            "-m",
                            &format!("Correct ticket {ticket_id}"),
                        ],
                    )?;
                }
                after.commit = Some(git(&worktree, &["rev-parse", "HEAD"])?);
                after.verification_passed = true;
                after.status = "implemented".into();
                Ok(())
            })();
            if let Err(e) = attempt {
                after.failure = Some(format!("{e:#}"));
            }
            fs::write(
                &context_path,
                redact(&serde_json::to_string_pretty(&request)?),
            )?;
            after = serde_json::from_str(&redact(&serde_json::to_string(&after)?))?;
            let no_progress = after.diff == before.diff;
            run.sessions[index] = after.clone();
            run = self.save_ticket(&run, ticket_id)?;
            let review = if after.verification_passed {
                run = self.review_ticket(id, ticket_id, reviewer)?;
                run.reviews.last().cloned()
            } else {
                None
            };
            let approved = self.review_gate(&run, &run.sessions[index])?;
            let outcome = if approved {
                "approved"
            } else if no_progress {
                "no-progress"
            } else if number + 1 == limit {
                "exhausted"
            } else {
                "retry"
            };
            let cycle = CorrectionCycle {
                id: cycle_id,
                ticket_id: ticket_id.into(),
                before,
                findings,
                failure: after.failure.clone(),
                after,
                review,
                outcome: outcome.into(),
            };
            let safe = reviewer.redact_output(
                &agent.redact_output(&run.config.isolation.redact(&serde_json::to_string(&cycle)?)),
            );
            run.corrections.push(serde_json::from_str(&safe)?);
            run = self.save_ticket(&run, ticket_id)?;
            if approved || no_progress {
                return Ok(run);
            }
            index = run
                .sessions
                .iter()
                .rposition(|s| s.ticket_id == ticket_id)
                .unwrap();
        }
        Ok(run)
    }
}
