use crate::{Engine, FrozenSpec, Run};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const TICKET_INSTRUCTIONS: &str = "Adapted to-tickets: decompose approved frozen specs into complete independently verifiable behavior slices. Associate concrete acceptance criterion IDs, include cross-spec prerequisites, provide observable acceptance criteria. Do not seek approval of each decomposition. Return structured tickets; do not implement them.";
pub const VERIFIER_INSTRUCTIONS: &str = "Independently compare tickets with frozen specs and all extracted requirements. Check missing coverage, inaccurate associations, dependency correctness and verifiable slice granularity. Return verified, failed or unable-to-verify and inspectable findings. Favorable opinion cannot override structural failures.";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub id: String,
    pub spec_path: String,
    pub content_sha256: String,
    pub criterion: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: String,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub covers: Vec<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub outcome: String,
    pub findings: Vec<Finding>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub requirements: Vec<Requirement>,
    pub tickets: Vec<Ticket>,
    pub findings: Vec<Finding>,
    pub dependency_graph: BTreeMap<String, Vec<String>>,
    pub executable: bool,
    pub generation_context: String,
    pub verification_context: String,
    pub verification: Verification,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlanningRequest {
    pub context_id: String,
    pub instructions: String,
    pub specs: Vec<FrozenSpec>,
    pub requirements: Vec<Requirement>,
}
#[derive(Debug, Clone, Serialize)]
pub struct VerificationRequest {
    pub context_id: String,
    pub instructions: String,
    pub specs: Vec<FrozenSpec>,
    pub requirements: Vec<Requirement>,
    pub tickets: Vec<Ticket>,
}
/// System boundary: implementations launch fresh independent sessions for each request.
pub trait PlanningAgent {
    fn generate(&self, request: &PlanningRequest) -> Result<Vec<Ticket>>;
    fn verify(&self, request: &VerificationRequest) -> Result<Verification>;
}
#[derive(Debug, Clone, Serialize)]
pub struct AcceptanceCriteriaInferenceRequest {
    pub issue: crate::import::ImportedIssue,
}
#[derive(Debug, Clone, Deserialize)]
pub struct AcceptanceCriteriaInference {
    pub acceptance_criteria: Vec<String>,
}
/// Fresh planning context that turns issue-only prose into observable behavior.
pub trait AcceptanceCriteriaInferenceAgent {
    fn infer(&self, request: &AcceptanceCriteriaInferenceRequest) -> Result<Vec<String>>;
}
#[derive(Deserialize)]
pub struct FixtureAgent {
    tickets: Vec<Ticket>,
    verification: Verification,
    #[serde(default)]
    inferred_requirements: Vec<String>,
    #[serde(default)]
    inference_by_issue: BTreeMap<String, Vec<String>>,
}
impl FixtureAgent {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&std::fs::read(path)?)
            .context("invalid deterministic planning fixture")
    }
}
impl PlanningAgent for FixtureAgent {
    fn generate(&self, _: &PlanningRequest) -> Result<Vec<Ticket>> {
        Ok(self.tickets.clone())
    }
    fn verify(&self, _: &VerificationRequest) -> Result<Verification> {
        Ok(self.verification.clone())
    }
}
impl AcceptanceCriteriaInferenceAgent for FixtureAgent {
    fn infer(&self, request: &AcceptanceCriteriaInferenceRequest) -> Result<Vec<String>> {
        let identity = format!("{}", request.issue.number);
        let canonical = request
            .issue
            .url
            .strip_prefix("https://github.com/")
            .and_then(|url| url.split_once("/issues/"))
            .map(|(repository, number)| format!("github:{repository}#{number}"));
        if let Some(criteria) = self.inference_by_issue.get(&identity).or_else(|| {
            canonical
                .as_ref()
                .and_then(|key| self.inference_by_issue.get(key))
        }) {
            return Ok(criteria.clone());
        }
        if self.inferred_requirements.is_empty() {
            bail!("planning fixture has no inferred_requirements for an issue without explicit acceptance bullets");
        }
        Ok(self.inferred_requirements.clone())
    }
}
/// Requirement identity comes only from the frozen Markdown, never agent output.
/// Bullets under Acceptance criteria/Acceptance Criteria headings are numbered in source order.
pub fn requirements(specs: &[FrozenSpec]) -> Vec<Requirement> {
    let mut result = Vec::new();
    for spec in specs {
        let mut active = false;
        let mut index = 0;
        for line in spec.content.lines() {
            let text = line.trim();
            if text.starts_with("#") {
                active = text
                    .trim_start_matches("#")
                    .trim()
                    .eq_ignore_ascii_case("acceptance criteria");
                continue;
            }
            if active {
                let bullet = text
                    .strip_prefix("- ")
                    .or_else(|| text.strip_prefix("* "))
                    .or_else(|| {
                        text.split_once(". ")
                            .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
                            .map(|(_, v)| v)
                    });
                if let Some(value) = bullet {
                    let value = value
                        .trim_start_matches("[ ] ")
                        .trim_start_matches("[x] ")
                        .trim();
                    if !value.is_empty() {
                        index += 1;
                        result.push(Requirement {
                            id: format!("{}#ac-{index}", spec.path),
                            spec_path: spec.path.clone(),
                            content_sha256: spec.content_sha256.clone(),
                            criterion: value.into(),
                        });
                    }
                }
            }
        }
    }
    result
}
impl Engine {
    pub fn plan(&self, id: &str, agent: &dyn PlanningAgent) -> Result<Run> {
        let mut run = self.inspect(id)?;
        if run.status != "prepared" && run.status != "plan_rejected" {
            bail!("planning requires a prepared or rejected run");
        }
        let requirements = requirements(&run.specs);
        let generation_context = format!("{}-generation", run.id);
        let verification_context = format!("{}-verification", run.id);
        let (tickets, verification) = self.with_agent_log(id, None, "plan", || -> Result<_> {
            let tickets = agent.generate(&PlanningRequest {
                context_id: generation_context.clone(),
                instructions: TICKET_INSTRUCTIONS.into(),
                specs: run.specs.clone(),
                requirements: requirements.clone(),
            })?;
            let verification = agent.verify(&VerificationRequest {
                context_id: verification_context.clone(),
                instructions: VERIFIER_INSTRUCTIONS.into(),
                specs: run.specs.clone(),
                requirements: requirements.clone(),
                tickets: tickets.clone(),
            })?;
            Ok((tickets, verification))
        })?;
        let mut plan = validate_plan(
            &run.specs,
            tickets,
            verification,
            generation_context,
            verification_context,
        );
        plan.requirements = requirements;
        run.status = if plan.executable {
            "planned"
        } else {
            "plan_rejected"
        }
        .into();
        run.plan = Some(plan);
        self.save(&run)?;
        Ok(run)
    }
}
pub fn validate_plan(
    specs: &[FrozenSpec],
    tickets: Vec<Ticket>,
    verification: Verification,
    generation_context: String,
    verification_context: String,
) -> Plan {
    let requirements = requirements(specs);
    let mut findings = verification.findings.clone();
    let mut add = |code: &str, message: String| {
        findings.push(Finding {
            code: code.into(),
            message,
        })
    };
    for spec in specs {
        if !requirements.iter().any(|r| r.spec_path == spec.path) {
            add(
                "missing_requirements",
                format!(
                    "{} needs an Acceptance criteria heading with explicit bullets",
                    spec.path
                ),
            );
        }
    }
    let mut ids = BTreeSet::new();
    let known: BTreeSet<_> = requirements.iter().map(|r| r.id.as_str()).collect();
    let mut covered = BTreeSet::new();
    let mut graph = BTreeMap::new();
    if tickets.is_empty() {
        add("empty_plan", "No tickets generated".into());
    }
    for t in &tickets {
        if t.id.trim().is_empty() || !ids.insert(t.id.clone()) {
            add(
                "invalid_ticket_id",
                format!("Duplicate or empty ticket identity: {}", t.id),
            );
        }
        if t.title.trim().is_empty()
            || t.description.trim().is_empty()
            || t.acceptance_criteria.is_empty()
            || t.acceptance_criteria.iter().any(|c| c.trim().is_empty())
            || t.covers.is_empty()
        {
            add("granularity",format!("Ticket {} must describe a complete verifiable behavior with criteria and requirement associations",t.id));
        }
        for c in &t.covers {
            if !known.contains(c.as_str()) {
                add(
                    "unknown_requirement",
                    format!("Ticket {} references unknown criterion {c}", t.id),
                );
            } else {
                covered.insert(c.as_str());
            }
        }
        graph.insert(t.id.clone(), t.blocked_by.clone());
    }
    for r in &requirements {
        if !covered.contains(r.id.as_str()) {
            add(
                "missing_coverage",
                format!("Uncovered criterion {}: {}", r.id, r.criterion),
            );
        }
    }
    for t in &tickets {
        for b in &t.blocked_by {
            if !ids.contains(b) {
                add(
                    "unknown_blocker",
                    format!("Ticket {} has unknown blocker {b}", t.id),
                );
            }
        }
    }
    let mut remaining = ids.clone();
    loop {
        let ready: Vec<_> = remaining
            .iter()
            .filter(|id| graph[*id].iter().all(|b| !remaining.contains(b)))
            .cloned()
            .collect();
        if ready.is_empty() {
            break;
        }
        for id in ready {
            remaining.remove(&id);
        }
    }
    if !remaining.is_empty() {
        add(
            "cycle",
            format!(
                "Dependency cycle includes {}",
                remaining.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }
    if verification.outcome != "verified" {
        add(
            "verification_not_verified",
            format!("Independent verifier outcome: {}", verification.outcome),
        );
    }
    if generation_context == verification_context {
        add(
            "shared_context",
            "Generation and verification must use distinct contexts".into(),
        );
    }
    Plan {
        requirements,
        tickets,
        executable: findings.is_empty(),
        findings,
        dependency_graph: graph,
        generation_context,
        verification_context,
        verification,
    }
}
