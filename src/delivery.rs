//! Dependency-aware delivery units for whole-snapshot backlog runs.
//!
//! A unit is a weakly connected component of the frozen ticket graph. Keeping
//! every prerequisite in the same unit prevents publishing a dependent ticket
//! without the work it relies on, while independent components can progress
//! and be delivered separately.
use crate::Run;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryGroup {
    pub id: String,
    pub tickets: Vec<String>,
    #[serde(default)]
    pub dependency_edges: Vec<DeliveryDependency>,
    /// Prerequisite group IDs, retained for explicit stacked-delivery support.
    #[serde(default)]
    pub prerequisites: Vec<String>,
    /// pending | integrated | blocked | failed
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<DeliveryValidation>,
    #[serde(default)]
    pub reviews: Vec<crate::review::ReviewSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<crate::publication::PullRequest>,
    #[serde(default)]
    pub ci_attempts: Vec<CiAttempt>,
    #[serde(default)]
    pub repair_attempts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryDependency {
    pub ticket: String,
    pub prerequisite: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryValidation {
    pub commit: String,
    pub outcome: String,
    pub checked_unix_ms: u128,
    pub checks: Vec<crate::execution::CheckResult>,
    #[serde(default)]
    pub criteria: Vec<GroupCriterionEvidence>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupCriterionEvidence {
    pub requirement: String,
    pub criterion: String,
    pub check: crate::execution::CheckResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiAttempt {
    pub id: String,
    pub pull_request: u64,
    pub commit: String,
    pub observed_unix_ms: u128,
    pub status: String,
    pub checks: Vec<crate::publication::RequiredCheck>,
    pub failure: Option<String>,
    #[serde(default)]
    pub observations: Vec<CiObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiObservation {
    pub observed_unix_ms: u128,
    pub status: String,
    pub checks: Vec<crate::publication::RequiredCheck>,
    pub failure: Option<String>,
}

fn identity(tickets: &[String]) -> String {
    let mut hash = Sha256::new();
    for ticket in tickets {
        hash.update(ticket.as_bytes());
        hash.update([0]);
    }
    format!("group-{}", &format!("{:x}", hash.finalize())[..12])
}

/// Build durable groups from the verified plan and current recorded outcomes.
/// Edges are treated as undirected for membership, so a shared prerequisite is
/// represented once and can never be duplicated across PR groups.
pub fn derive(run: &Run) -> Vec<DeliveryGroup> {
    let Some(plan) = run.plan.as_ref() else {
        return Vec::new();
    };
    let ids: BTreeSet<_> = plan
        .tickets
        .iter()
        .map(|ticket| ticket.id.clone())
        .collect();
    let mut adjacent: BTreeMap<String, BTreeSet<String>> =
        ids.iter().map(|id| (id.clone(), BTreeSet::new())).collect();
    for ticket in &plan.tickets {
        for prerequisite in &ticket.blocked_by {
            if ids.contains(prerequisite) {
                adjacent
                    .entry(ticket.id.clone())
                    .or_default()
                    .insert(prerequisite.clone());
                adjacent
                    .entry(prerequisite.clone())
                    .or_default()
                    .insert(ticket.id.clone());
            }
        }
    }

    let dispositions: BTreeMap<_, _> = run
        .backlog
        .as_ref()
        .map(|backlog| {
            backlog
                .dispositions
                .iter()
                .map(|d| (d.issue.as_str(), d))
                .collect()
        })
        .unwrap_or_default();
    let mut remaining = ids;
    let mut groups = Vec::new();
    while let Some(seed) = remaining.iter().next().cloned() {
        let mut stack = vec![seed];
        let mut members = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !members.insert(id.clone()) {
                continue;
            }
            remaining.remove(&id);
            if let Some(neighbors) = adjacent.get(&id) {
                stack.extend(neighbors.iter().filter(|n| !members.contains(*n)).cloned());
            }
        }
        let tickets: Vec<_> = members.into_iter().collect();
        let dependency_edges = plan
            .tickets
            .iter()
            .filter(|ticket| tickets.contains(&ticket.id))
            .flat_map(|ticket| {
                ticket
                    .blocked_by
                    .iter()
                    .filter(|prerequisite| tickets.contains(prerequisite))
                    .map(|prerequisite| DeliveryDependency {
                        ticket: ticket.id.clone(),
                        prerequisite: prerequisite.clone(),
                    })
            })
            .collect();
        let outcomes: Vec<_> = tickets
            .iter()
            .filter_map(|ticket| dispositions.get(ticket.as_str()).copied())
            .collect();
        let status =
            if outcomes.len() != tickets.len() || outcomes.iter().any(|d| d.status == "eligible") {
                "pending"
            } else if outcomes.iter().all(|d| d.status == "completed") {
                "integrated"
            } else if outcomes.iter().any(|d| d.status == "failed") {
                "failed"
            } else {
                "blocked"
            };
        let reason = match status {
            "failed" | "blocked" => outcomes
                .iter()
                .filter(|d| d.status == "failed" || d.status == "blocked")
                .map(|d| format!("{}: {}", d.issue, d.reason))
                .next(),
            _ => None,
        };
        groups.push(DeliveryGroup {
            id: identity(&tickets),
            tickets,
            dependency_edges,
            prerequisites: Vec::new(),
            status: status.into(),
            reason,
            branch: None,
            commit: None,
            validation: None,
            reviews: Vec::new(),
            pull_request: None,
            ci_attempts: Vec::new(),
            repair_attempts: 0,
        });
    }
    groups.sort_by(|a, b| a.tickets.cmp(&b.tickets));
    groups
}

/// Rebuild one dependency-closed delivery branch from the exact reviewed ticket
/// commits. The original integration branch remains untouched.
pub fn prepare_group_branch(
    engine: &crate::Engine,
    run: &Run,
    group: &DeliveryGroup,
) -> Result<(String, String)> {
    if !matches!(
        group.status.as_str(),
        "integrated"
            | "validating"
            | "validation-failed"
            | "validated"
            | "awaiting-ci"
            | "ci-failed"
            | "ci-pending"
            | "ci-unavailable"
            | "verified"
    ) {
        bail!("delivery group {} is not fully integrated", group.id);
    }
    let base = run
        .integrations
        .iter()
        .find(|attempt| attempt.status == "integrated")
        .map(|attempt| attempt.base_commit.clone())
        .context("backlog run has no recorded integration base")?;
    let branch = format!("kiln/{}/groups/{}", run.id, group.id);
    let path = engine
        .repository
        .join(".kiln/worktrees")
        .join(format!("{}-rebuild", group.id));
    if let Some(commit) = group.commit.as_deref() {
        let existing = crate::execution::git(
            &engine.repository,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{branch}^{{commit}}"),
            ],
        )
        .ok();
        if existing.as_deref() == Some(commit) {
            return Ok((branch, commit.to_owned()));
        }
    }
    let _ = engine.remove_worktree(&path);
    {
        let _git_admin = engine.lock_git_admin()?;
        if crate::execution::git(
            &engine.repository,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
        {
            crate::execution::git(&engine.repository, &["branch", "-D", &branch])?;
        }
        crate::execution::git(
            &engine.repository,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().context("delivery worktree UTF-8")?,
                &base,
            ],
        )?;
    }
    let result = (|| -> Result<String> {
        let ticket_ids: BTreeSet<_> = group.tickets.iter().map(String::as_str).collect();
        let integrated: Vec<_> = run
            .integrations
            .iter()
            .filter(|attempt| {
                attempt.status == "integrated" && ticket_ids.contains(attempt.ticket_id.as_str())
            })
            .collect();
        if integrated.len() != group.tickets.len() {
            bail!("delivery group is missing one or more integrated ticket commits");
        }
        for attempt in integrated {
            let output = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Kiln",
                    "-c",
                    "user.email=kiln@localhost",
                    "cherry-pick",
                    &attempt.reviewed_commit,
                ])
                .current_dir(&path)
                .output()?;
            if !output.status.success() {
                bail!(
                    "cannot reconstruct dependency group {} from reviewed ticket commit: {}",
                    group.id,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        crate::execution::git(&path, &["branch", "-f", &branch, "HEAD"])?;
        crate::execution::git(&path, &["rev-parse", "HEAD"])
    })();
    let cleanup = engine.remove_worktree(&path);
    let commit = result?;
    cleanup?;
    Ok((branch, commit))
}

/// Validate only the acceptance requirements covered by this dependency group.
/// A missing configured workflow is explicit unable-to-verify evidence.
pub fn validate_group(
    engine: &crate::Engine,
    run: &Run,
    group: &DeliveryGroup,
    branch: &str,
    commit: &str,
) -> Result<DeliveryValidation> {
    let actual = crate::execution::git(
        &engine.repository,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )?;
    if actual != commit {
        bail!("delivery branch moved after group preparation; rebuild before validation");
    }
    let settings = crate::validation::ValidationSettings::from_config(&run.config)?;
    let plan = run
        .plan
        .as_ref()
        .context("backlog delivery requires a verified plan")?;
    let covered: BTreeSet<_> = plan
        .tickets
        .iter()
        .filter(|ticket| group.tickets.contains(&ticket.id))
        .flat_map(|ticket| ticket.covers.iter().cloned())
        .collect();
    let requirements: Vec<_> = plan
        .requirements
        .iter()
        .filter(|requirement| covered.contains(&requirement.id))
        .collect();
    if requirements.is_empty() {
        bail!(
            "delivery group {} covers no acceptance requirements",
            group.id
        );
    }
    let path = engine
        .repository
        .join(".kiln/worktrees")
        .join(format!("{}-validate", group.id));
    let _ = engine.remove_worktree(&path);
    {
        let _git_admin = engine.lock_git_admin()?;
        crate::execution::git(
            &engine.repository,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str()
                    .context("delivery validation worktree UTF-8")?,
                commit,
            ],
        )?;
    }
    let mut checks = Vec::new();
    for (name, command) in [("build", &run.config.build), ("test", &run.config.test)] {
        checks.push(crate::sandbox::Sandbox::check(
            &run.config.isolation,
            &path,
            name,
            command,
        ));
    }
    let mut criteria = Vec::new();
    let mut missing = Vec::new();
    for requirement in requirements {
        let Some(workflow) = settings
            .workflows
            .iter()
            .find(|workflow| workflow.criterion == requirement.id)
        else {
            missing.push(requirement.id.clone());
            continue;
        };
        criteria.push(GroupCriterionEvidence {
            requirement: requirement.id.clone(),
            criterion: requirement.criterion.clone(),
            check: crate::sandbox::Sandbox::check(
                &run.config.isolation,
                &path,
                &format!("acceptance:{}", requirement.id),
                &workflow.command,
            ),
        });
    }
    let cleanup = engine.remove_worktree(&path);
    cleanup?;
    let failed = checks.iter().any(|check| !check.passed)
        || criteria.iter().any(|evidence| !evidence.check.passed);
    let outcome = if failed {
        "failed"
    } else if !missing.is_empty() {
        "unable-to-verify"
    } else {
        "verified"
    };
    let failure = if !missing.is_empty() {
        Some(format!(
            "no configured validation workflow covers group requirements: {}",
            missing.join(", ")
        ))
    } else if failed {
        Some("configured group validation check failed".into())
    } else {
        None
    };
    Ok(DeliveryValidation {
        commit: commit.into(),
        outcome: outcome.into(),
        checked_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
        checks,
        criteria,
        failure,
    })
}

fn group_ticket(run: &Run, group: &DeliveryGroup) -> Result<crate::planning::Ticket> {
    let plan = run
        .plan
        .as_ref()
        .context("backlog delivery requires a plan")?;
    let tickets: Vec<_> = plan
        .tickets
        .iter()
        .filter(|ticket| group.tickets.contains(&ticket.id))
        .collect();
    Ok(crate::planning::Ticket {
        id: group.id.clone(),
        title: format!("Dependency delivery group {}", group.id),
        description: tickets
            .iter()
            .map(|ticket| format!("{}: {}", ticket.id, ticket.description))
            .collect::<Vec<_>>()
            .join("\n"),
        acceptance_criteria: tickets
            .iter()
            .flat_map(|ticket| ticket.acceptance_criteria.clone())
            .collect(),
        covers: tickets
            .iter()
            .flat_map(|ticket| ticket.covers.clone())
            .collect(),
        blocked_by: Vec::new(),
    })
}

/// Fresh, independent Standards and Spec review of the exact combined group
/// commit. Both axes receive the local checks and group-scoped specs.
pub fn review_group(
    engine: &crate::Engine,
    run: &Run,
    group: &DeliveryGroup,
    commit: &str,
    reviewer: &dyn crate::review::ReviewAgent,
) -> Result<crate::review::ReviewSession> {
    use crate::review::{AxisReview, ReviewRequest, ReviewSession};
    reviewer.prepare_redaction()?;
    let ticket = group_ticket(run, group)?;
    let plan = run.plan.as_ref().context("delivery plan missing")?;
    let specs: Vec<_> = run
        .effective_specs()
        .into_iter()
        .filter(|spec| {
            plan.requirements.iter().any(|requirement| {
                requirement.spec_path == spec.path && ticket.covers.contains(&requirement.id)
            })
        })
        .collect();
    let base = run
        .integrations
        .iter()
        .find(|attempt| attempt.status == "integrated")
        .map(|attempt| attempt.base_commit.as_str())
        .context("delivery base commit missing")?;
    let standard_paths =
        crate::execution::git(&engine.repository, &["ls-tree", "-r", "--name-only", base])?;
    let mut repository_standards = String::new();
    for standard_path in standard_paths.lines().filter(|path| {
        matches!(
            Path::new(path).file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md" | "CLAUDE.md" | "CONTRIBUTING.md" | "CODE_STYLE.md" | "standards.md")
        )
    }) {
        repository_standards.push_str(&format!("\n--- {standard_path} at {base} ---\n"));
        repository_standards.push_str(&crate::execution::git(
            &engine.repository,
            &["show", &format!("{base}:{standard_path}")],
        )?);
    }
    let path = engine
        .repository
        .join(".kiln/worktrees")
        .join(format!("{}-review", group.id));
    let _ = engine.remove_worktree(&path);
    {
        let _git_admin = engine.lock_git_admin()?;
        crate::execution::git(
            &engine.repository,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().context("delivery review worktree UTF-8")?,
                commit,
            ],
        )?;
    }
    let result = (|| -> Result<ReviewSession> {
        let diff = crate::execution::git(&path, &["diff", "--binary", base, commit])?;
        let checks = ["build", "test"]
            .into_iter()
            .map(|name| {
                crate::sandbox::Sandbox::check(
                    &run.config.isolation,
                    &path,
                    name,
                    if name == "build" {
                        &run.config.build
                    } else {
                        &run.config.test
                    },
                )
            })
            .collect::<Vec<_>>();
        let session_id = format!("{}-review-{}", group.id, commit);
        let review_axis = |axis: &str| -> Result<AxisReview> {
            let request = ReviewRequest {
                context_id: format!("{session_id}-{axis}"),
                axis: axis.into(),
                author_context_id: format!("{}-implementation", group.id),
                instructions: format!("Independently review this exact dependency-group commit on the {axis} axis. Do not edit files. The current PR head is {commit}."),
                isolation: run.config.isolation.clone(),
                ticket: ticket.clone(),
                specs: specs.clone(),
                repository_standards: repository_standards.clone(),
                commit: commit.into(),
                diff: diff.clone(),
                implementation_checks: checks.clone(),
                worktree: path.clone(),
            };
            let mut result =
                engine.with_agent_log(&run.id, Some(&group.id), "delivery-review", || {
                    reviewer.review(&request)
                })?;
            let redact = |value: &str| run.config.isolation.redact(&reviewer.redact_output(value));
            result.evidence = redact(&result.evidence);
            result.log = redact(&result.log);
            for finding in &mut result.findings {
                finding.message = redact(&finding.message);
                finding.evidence = redact(&finding.evidence);
            }
            Ok(AxisReview {
                axis: axis.into(),
                context_id: request.context_id,
                result,
                checks: checks.clone(),
                failure: None,
                provider_limit: None,
            })
        };
        let standards = review_axis("standards")?;
        let spec = review_axis("spec")?;
        let passed = standards.verified() && spec.verified();
        Ok(ReviewSession {
            id: session_id.clone(),
            session_id,
            ticket_id: group.id.clone(),
            commit: commit.into(),
            diff,
            standards,
            spec,
            passed,
        })
    })();
    let cleanup = engine.remove_worktree(&path);
    let result = result?;
    cleanup?;
    Ok(result)
}

/// Apply one bounded CI repair to a group branch, leaving review and validation
/// to their independent gates. This creates a new exact commit; old check runs
/// can never authorize the new head.
pub fn repair_group(
    engine: &crate::Engine,
    run: &Run,
    group: &DeliveryGroup,
    branch: &str,
    commit: &str,
    findings: &[String],
    corrector: &dyn crate::correction::CorrectionAgent,
) -> Result<String> {
    use crate::correction::CorrectionRequest;
    corrector.prepare_redaction()?;
    let ticket = group_ticket(run, group)?;
    let plan = run.plan.as_ref().context("delivery plan missing")?;
    let specs: Vec<_> = run
        .effective_specs()
        .into_iter()
        .filter(|spec| {
            plan.requirements.iter().any(|requirement| {
                requirement.spec_path == spec.path && ticket.covers.contains(&requirement.id)
            })
        })
        .collect();
    let base = run
        .integrations
        .iter()
        .find(|attempt| attempt.status == "integrated")
        .map(|attempt| attempt.base_commit.clone())
        .context("delivery base commit missing")?;
    let path = engine
        .repository
        .join(".kiln/worktrees")
        .join(format!("{}-repair", group.id));
    let _ = engine.remove_worktree(&path);
    {
        let _git_admin = engine.lock_git_admin()?;
        crate::execution::git(
            &engine.repository,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().context("delivery repair worktree UTF-8")?,
                commit,
            ],
        )?;
    }
    let result = (|| -> Result<String> {
        let diff = crate::execution::git(&path, &["diff", "--binary", &base, commit])?;
        let instructions = format!("Repair only this dependency group's reported required CI failures using TDD vertical slices. Preserve all included tickets and acceptance criteria. Do not stage, commit, integrate, modify Git metadata, merge, or deploy; Kiln owns commit and gates. Current exact PR head: {commit}. Findings:\n{}", findings.join("\n"));
        let request = CorrectionRequest {
            context_id: format!("{}-ci-repair-{}", group.id, group.repair_attempts + 1),
            instructions,
            ticket,
            specs,
            diff,
            findings: findings.to_vec(),
            checks: Vec::new(),
            repository_instructions: String::new(),
            isolation: run.config.isolation.clone(),
            worktree: path.clone(),
        };
        let result = engine.with_agent_log(&run.id, Some(&group.id), "repair", || {
            corrector.correct(&request)
        })?;
        if result.outcome != "completed" {
            bail!("CI repair agent did not complete: {}", result.outcome);
        }
        crate::execution::git(&path, &["add", "-A"])?;
        crate::execution::git(&path, &["diff", "--cached", "--check"])?;
        if !crate::execution::git(&path, &["diff", "--name-only", "--diff-filter=U"])?.is_empty() {
            bail!("CI repair left unresolved merge conflicts");
        }
        if crate::execution::git(&path, &["diff", "--cached", "--quiet"]).is_ok() {
            bail!("CI repair produced no changes");
        }
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Kiln",
                "-c",
                "user.email=kiln@localhost",
                "commit",
                "-m",
                &format!("Kiln CI repair for {}", group.id),
            ])
            .current_dir(&path)
            .output()?;
        if !output.status.success() {
            bail!(
                "commit CI repair: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let repaired = crate::execution::git(&path, &["rev-parse", "HEAD"])?;
        crate::execution::git(&engine.repository, &["branch", "-f", branch, &repaired])?;
        Ok(repaired)
    })();
    let cleanup = engine.remove_worktree(&path);
    let commit = result?;
    cleanup?;
    Ok(commit)
}
