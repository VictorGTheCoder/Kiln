//! Explicit replanning when approved specs change. External edits never reach a
//! run's frozen inputs; `Engine::replan_specs` captures the revised specs as a new
//! input version (versioned `SpecRevision`s), asks a fresh session to revise the
//! affected tickets, independently reverifies the whole plan, and only then
//! invalidates affected work. Unaffected integrated work is retained with its
//! provenance; dependents of affected tickets keep their work but must have their
//! evidence revalidated against the re-integrated prerequisites before reuse.
use crate::{
    execution::{git, CheckResult},
    planning::{requirements, Finding, Plan, PlanningAgent, Requirement, Ticket},
    Engine, FrozenSpec, Run, SpecRevision,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub const SPEC_REPLANNING_INSTRUCTIONS: &str = "Approved specs changed. Revise the plan for the new input version: return complete revised tickets (same id) for every affected ticket and any new tickets the changed requirements need. Leave unaffected tickets out. Cover every changed requirement with exact requirement IDs. Do not implement and do not ask the developer.";

#[derive(Debug, Clone, Serialize)]
pub struct SpecReplanRequest {
    pub context_id: String,
    pub instructions: String,
    /// Effective specs of the new input version.
    pub specs: Vec<FrozenSpec>,
    /// Requirements whose criterion changed or that were added.
    pub changed_requirements: Vec<Requirement>,
    /// Requirement identities no longer present in the revised specs.
    pub removed_requirements: Vec<String>,
    pub affected_tickets: Vec<String>,
    pub tickets: Vec<Ticket>,
}
/// System boundary: a fresh revision session; verification runs in its own context.
pub trait SpecReplanningAgent {
    fn revise(&self, request: &SpecReplanRequest) -> Result<Vec<Ticket>>;
}
pub trait SpecReplanner: SpecReplanningAgent + PlanningAgent {}
impl<T: SpecReplanningAgent + PlanningAgent> SpecReplanner for T {}

/// Deterministic revision session and independent verifier.
#[derive(Deserialize)]
pub struct FixtureSpecReplanning {
    tickets: Vec<Ticket>,
    verification: crate::planning::Verification,
}
impl FixtureSpecReplanning {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&fs::read(path)?).context("invalid deterministic spec replanning fixture")
    }
}
impl SpecReplanningAgent for FixtureSpecReplanning {
    fn revise(&self, _: &SpecReplanRequest) -> Result<Vec<Ticket>> {
        Ok(self.tickets.clone())
    }
}
impl PlanningAgent for FixtureSpecReplanning {
    fn generate(&self, _: &crate::planning::PlanningRequest) -> Result<Vec<Ticket>> {
        bail!("spec replanning fixtures do not generate plans")
    }
    fn verify(&self, _: &crate::planning::VerificationRequest) -> Result<crate::planning::Verification> {
        Ok(self.verification.clone())
    }
}

/// Integrated work kept across a replan, with where it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetainedWork {
    pub ticket_id: String,
    pub session_id: String,
    pub commit: Option<String>,
    /// Input version the work was produced under.
    pub input_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecReplan {
    pub id: String,
    pub context_id: String,
    pub previous_input_version: u32,
    /// Input version this replan establishes (when `replanned`).
    pub input_version: u32,
    /// Spec revision versions captured by this replan.
    pub revisions: Vec<u32>,
    /// Changed, added and removed requirement identities.
    pub changed_requirements: Vec<String>,
    /// Tickets whose requirements or definition changed: re-executed.
    pub affected_tickets: Vec<String>,
    /// Descendants of affected tickets: evidence revalidated before reuse.
    pub dependent_tickets: Vec<String>,
    pub revised_tickets: Vec<Ticket>,
    pub previous_plan: Option<Plan>,
    /// Independent reverification of the revised plan.
    pub verification: Option<Plan>,
    pub findings: Vec<Finding>,
    /// replanned | rejected | failed
    pub outcome: String,
    pub retained: Vec<RetainedWork>,
    pub invalidated_sessions: Vec<String>,
    pub invalidated_integrations: Vec<String>,
    pub invalidated_validation_reports: Vec<String>,
    /// Dependent evidence revalidated against re-integrated prerequisites.
    #[serde(default)]
    pub revalidations: Vec<Revalidation>,
}

/// Fresh verification of a dependent's retained work in the combined result of
/// the new input version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revalidation {
    pub ticket_id: String,
    pub session_id: String,
    pub input_version: u32,
    /// Integration branch commit the configured checks ran against.
    pub verified_commit: Option<String>,
    pub checks: Vec<CheckResult>,
    /// revalidated | failed
    pub outcome: String,
    pub failure: Option<String>,
}

/// The replan whose dependent evidence for `ticket` still awaits revalidation.
pub(crate) fn pending_revalidation<'a>(run: &'a Run, ticket: &str) -> Option<&'a SpecReplan> {
    let session = run
        .sessions
        .iter()
        .rev()
        .find(|s| s.ticket_id == ticket && s.status == "integrated")?;
    run.spec_replans.iter().rev().find(|r| {
        r.outcome == "replanned"
            && r.dependent_tickets.iter().any(|t| t == ticket)
            && session.input_version < r.input_version
            && !r
                .revalidations
                .iter()
                .any(|v| v.ticket_id == ticket && v.outcome == "revalidated")
    })
}

fn sha(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

impl Engine {
    /// Read the current approved content of frozen specs named by `paths`.
    fn read_approved(&self, run: &Run, paths: &[PathBuf]) -> Result<Vec<(String, String)>> {
        if paths.is_empty() {
            bail!("name at least one approved spec to replan from");
        }
        let mut result: Vec<(String, String)> = Vec::new();
        for path in paths {
            let source = fs::canonicalize(self.repository.join(path))
                .with_context(|| format!("read spec {}", path.display()))?;
            let relative = source
                .strip_prefix(&self.repository)
                .with_context(|| format!("spec {} must be inside the repository", path.display()))?
                .to_string_lossy()
                .into_owned();
            if !run.specs.iter().any(|s| s.path == relative) {
                bail!("spec {relative} is not an approved input of run {}", run.id);
            }
            if result.iter().any(|(p, _)| *p == relative) {
                bail!("duplicate spec {relative}");
            }
            let content = fs::read_to_string(&source).context("spec must be readable UTF-8 Markdown")?;
            if content.trim().is_empty() {
                bail!("spec {relative} is empty");
            }
            if run.config.isolation.redact(&content) != content {
                bail!("approved spec contains a registered secret value; remove it before replanning");
            }
            result.push((relative, content));
        }
        Ok(result)
    }

    /// Apply revised approved specs to a run through an explicit replanning operation.
    pub fn replan_specs(&self, id: &str, paths: &[PathBuf], agent: &dyn SpecReplanner) -> Result<Run> {
        let _owner = self.own_run(id)?;
        let run = self.inspect(id)?;
        let current = run
            .plan
            .clone()
            .filter(|p| p.executable)
            .context("replanning requires an independently verified executable plan")?;
        let previous_input_version = run.input_version();
        let mut specs = run.effective_specs();
        let replan_id = format!("{}-spec-replan-{}", run.id, run.spec_replans.len() + 1);
        let context_id = format!("{replan_id}-context");
        let mut revisions = Vec::new();
        for (path, content) in self.read_approved(&run, paths)? {
            let spec = specs.iter_mut().find(|s| s.path == path).context("frozen spec")?;
            if spec.content == content {
                continue;
            }
            revisions.push(SpecRevision {
                version: (run.spec_revisions.len() + revisions.len()) as u32 + 1,
                path: path.clone(),
                base_sha256: spec.content_sha256.clone(),
                content_sha256: sha(&content),
                content: content.clone(),
                decision_id: String::new(),
                replan_id: Some(replan_id.clone()),
                status: String::new(),
            });
            spec.content_sha256 = sha(&content);
            spec.content = content;
        }
        if revisions.is_empty() {
            bail!("no approved spec changed since input version {previous_input_version}; nothing to replan");
        }
        let old = requirements(&run.effective_specs());
        let new = requirements(&specs);
        let changed: Vec<Requirement> = new
            .iter()
            .filter(|r| !old.iter().any(|o| o.id == r.id && o.criterion == r.criterion))
            .cloned()
            .collect();
        let removed: Vec<String> = old
            .iter()
            .filter(|o| !new.iter().any(|r| r.id == o.id))
            .map(|o| o.id.clone())
            .collect();
        let changed_ids: BTreeSet<&str> = changed
            .iter()
            .map(|r| r.id.as_str())
            .chain(removed.iter().map(String::as_str))
            .collect();
        let covering: Vec<String> = current
            .tickets
            .iter()
            .filter(|t| t.covers.iter().any(|c| changed_ids.contains(c.as_str())))
            .map(|t| t.id.clone())
            .collect();
        let mut record = SpecReplan {
            id: replan_id.clone(),
            context_id: context_id.clone(),
            previous_input_version,
            input_version: previous_input_version,
            revisions: revisions.iter().map(|r| r.version).collect(),
            changed_requirements: changed_ids.iter().map(|s| s.to_string()).collect(),
            affected_tickets: covering.clone(),
            dependent_tickets: Vec::new(),
            revised_tickets: Vec::new(),
            previous_plan: Some(current.clone()),
            verification: None,
            findings: Vec::new(),
            outcome: "failed".into(),
            retained: Vec::new(),
            invalidated_sessions: Vec::new(),
            invalidated_integrations: Vec::new(),
            invalidated_validation_reports: Vec::new(),
            revalidations: Vec::new(),
        };
        let revised = agent.revise(&SpecReplanRequest {
            context_id: context_id.clone(),
            instructions: SPEC_REPLANNING_INSTRUCTIONS.into(),
            specs: specs.clone(),
            changed_requirements: changed,
            removed_requirements: removed,
            affected_tickets: covering.clone(),
            tickets: current.tickets.clone(),
        });
        let mut adopted = None;
        match revised {
            Err(e) => record.findings.push(Finding {
                code: "replanning_failed".into(),
                message: format!("{e:#}"),
            }),
            Ok(revised) => {
                let plan = crate::decision::reverify(&run, &specs, &revised, agent, &replan_id, &context_id)?;
                record.findings = plan.findings.clone();
                record.outcome = if plan.executable { "replanned" } else { "rejected" }.into();
                if plan.executable {
                    adopted = Some(plan.clone());
                }
                record.revised_tickets = revised;
                record.verification = Some(plan);
            }
        }
        let status = if adopted.is_some() { "verified" } else { "rejected" };
        for r in &mut revisions {
            r.status = status.into();
        }
        if let Some(plan) = &adopted {
            record.input_version = revisions.iter().map(|r| r.version).max().unwrap_or(previous_input_version);
            let mut affected: Vec<String> = Vec::new();
            for t in &plan.tickets {
                let revised = record.revised_tickets.iter().any(|r| r.id == t.id)
                    && current.tickets.iter().any(|c| c.id == t.id);
                if covering.contains(&t.id) || revised {
                    affected.push(t.id.clone());
                }
            }
            record.dependent_tickets = descendants(&plan.tickets, &affected);
            record.affected_tickets = affected;
        }
        self.transact(id, |run| {
            run.spec_revisions.extend(revisions);
            if let Some(plan) = adopted {
                run.plan = Some(plan);
                run.status = "replanned".into();
                invalidate(run, &mut record);
            }
            run.spec_replans.push(record);
            Ok(run.clone())
        })
    }

    /// Revalidate a dependent's retained work: its exact commit must still be part
    /// of the integration branch, and the configured build and test checks must pass
    /// on the current combined result (which includes re-integrated prerequisites).
    pub fn revalidate_ticket(&self, id: &str, ticket: &str) -> Result<Revalidation> {
        let run = self.inspect(id)?;
        let replan = pending_revalidation(&run, ticket)
            .with_context(|| format!("ticket {ticket} has no dependent evidence awaiting revalidation"))?;
        let replan_id = replan.id.clone();
        let input_version = replan.input_version;
        let session = run
            .sessions
            .iter()
            .rev()
            .find(|s| s.ticket_id == ticket && s.status == "integrated")
            .context("retained work is missing")?
            .clone();
        let mut revalidation = Revalidation {
            ticket_id: ticket.into(),
            session_id: session.id.clone(),
            input_version,
            verified_commit: None,
            checks: Vec::new(),
            outcome: "failed".into(),
            failure: None,
        };
        let worktree = self
            .repository
            .join(".kiln/worktrees")
            .join(format!("{replan_id}-revalidate-{ticket}-{}", replan.revalidations.len() + 1));
        let result = (|| -> Result<()> {
            let branch = run.integration_branch.clone().context("missing integration branch")?;
            let tip = git(&self.repository, &["rev-parse", &branch])?;
            revalidation.verified_commit = Some(tip.clone());
            let commit = session.commit.clone().context("retained work has no commit")?;
            git(&self.repository, &["merge-base", "--is-ancestor", &commit, &tip])
                .context("retained work is no longer part of the integration branch")?;
            {
                let _git = self.lock_run(id, "git")?;
                git(
                    &self.repository,
                    &["worktree", "add", "--detach", worktree.to_str().context("non UTF-8 worktree")?, &tip],
                )?;
            }
            for (name, argv) in [("build", &run.config.build), ("test", &run.config.test)] {
                revalidation.checks.push(check(&run, &worktree, name, argv));
            }
            if revalidation.checks.iter().any(|c| !c.passed) {
                bail!("configured verification failed on the combined result");
            }
            Ok(())
        })();
        if worktree.exists() {
            let _git = self.lock_run(id, "git")?;
            let _ = git(&self.repository, &["worktree", "remove", "--force", worktree.to_str().unwrap_or_default()]);
        }
        match result {
            Ok(()) => revalidation.outcome = "revalidated".into(),
            Err(e) => revalidation.failure = Some(run.config.isolation.redact(&format!("{e:#}"))),
        }
        for c in &mut revalidation.checks {
            c.stdout = run.config.isolation.redact(&c.stdout);
            c.stderr = run.config.isolation.redact(&c.stderr);
        }
        self.transact(id, |run| {
            let record = run
                .spec_replans
                .iter_mut()
                .find(|r| r.id == replan_id)
                .context("spec replan record is missing")?;
            record.revalidations.push(revalidation.clone());
            Ok(())
        })?;
        Ok(revalidation)
    }
}

fn check(run: &Run, worktree: &Path, name: &str, argv: &[String]) -> CheckResult {
    match crate::sandbox::Sandbox::command(&run.config.isolation, worktree, name, argv, &[])
        .and_then(|mut command| Ok(command.output()?))
    {
        Ok(output) => CheckResult {
            name: name.into(),
            command: argv.to_vec(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            passed: output.status.success(),
        },
        Err(error) => CheckResult {
            name: name.into(),
            command: argv.to_vec(),
            exit_code: None,
            stdout: String::new(),
            stderr: error.to_string(),
            passed: false,
        },
    }
}

/// Transitive descendants of `roots` in plan order, excluding the roots.
fn descendants(tickets: &[Ticket], roots: &[String]) -> Vec<String> {
    let mut found: BTreeSet<String> = roots.iter().cloned().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for t in tickets {
            if !found.contains(&t.id) && t.blocked_by.iter().any(|b| found.contains(b)) {
                found.insert(t.id.clone());
                changed = true;
            }
        }
    }
    tickets
        .iter()
        .filter(|t| found.contains(&t.id) && !roots.contains(&t.id))
        .map(|t| t.id.clone())
        .collect()
}

/// Invalidate affected work and evidence; record retained work with its provenance.
fn invalidate(run: &mut Run, record: &mut SpecReplan) {
    let affected = &record.affected_tickets;
    for s in run.sessions.iter_mut().filter(|s| affected.contains(&s.ticket_id)) {
        if matches!(s.status.as_str(), "running" | "implemented" | "integrated") {
            s.status = "superseded".into();
            record.invalidated_sessions.push(s.id.clone());
        }
    }
    for i in run.integrations.iter_mut().filter(|i| affected.contains(&i.ticket_id)) {
        if i.status == "integrated" {
            i.status = "superseded".into();
            record.invalidated_integrations.push(i.id.clone());
        }
    }
    record.invalidated_validation_reports = run.validation_reports.iter().map(|r| r.id.clone()).collect();
    for s in run.sessions.iter().filter(|s| s.status == "integrated" && !affected.contains(&s.ticket_id)) {
        record.retained.push(RetainedWork {
            ticket_id: s.ticket_id.clone(),
            session_id: s.id.clone(),
            commit: s.commit.clone(),
            input_version: s.input_version,
        });
    }
    if let Some(scheduler) = &mut run.scheduler {
        for t in scheduler.tickets.iter_mut().filter(|t| {
            affected.contains(&t.id) || record.dependent_tickets.contains(&t.id)
        }) {
            t.state = "waiting".into();
            t.blocker = None;
            t.exhaustion = None;
        }
    }
}
