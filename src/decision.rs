//! Autonomous decision sessions. Conflicting or ambiguous requirements are resolved
//! by the decision hierarchy: product objectives first, architectural decisions
//! second, specs third. The engine (not the agent) ranks the positions and rejects
//! a proposal that is not governed by the highest-ranked source present.
use crate::{planning::Finding, Engine, Run, SpecRevision};
use sha2::{Digest, Sha256};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const PRODUCT_OBJECTIVE: &str = "product_objective";
pub const ARCHITECTURAL_DECISION: &str = "architectural_decision";
pub const SPEC: &str = "spec";
pub const DECISION_INSTRUCTIONS: &str = "Resolve this ambiguity autonomously without asking the developer. Apply the decision hierarchy: product objectives first, architectural decisions second, specs third. Name the governing position's reference, the resolution, the rationale and concrete evidence. Propose a spec revision and revised tickets only when the resolution changes requirements.";

/// Hierarchy rank of a source kind (lower governs).
pub fn rank(source: &str) -> Option<usize> {
    [PRODUCT_OBJECTIVE, ARCHITECTURAL_DECISION, SPEC]
        .iter()
        .position(|s| *s == source)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// product_objective | architectural_decision | spec
    pub source: String,
    /// Objective or decision identity, frozen spec path or requirement identity.
    pub reference: String,
    pub statement: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ambiguity {
    pub id: String,
    pub question: String,
    pub positions: Vec<Position>,
}
impl Ambiguity {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&std::fs::read(path)?).context("invalid ambiguity description")
    }
}
/// A position as recorded: ranked, with the authoritative statement of its source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedPosition {
    pub rank: usize,
    pub source: String,
    pub reference: String,
    pub statement: String,
    pub source_statement: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct DecisionRequest {
    pub context_id: String,
    pub instructions: String,
    pub question: String,
    pub positions: Vec<RankedPosition>,
    pub specs: Vec<crate::FrozenSpec>,
    pub tickets: Vec<crate::planning::Ticket>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Reference of the governing position.
    pub governing: String,
    pub resolution: String,
    pub rationale: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Revised content of one frozen spec, when the resolution changes requirements.
    #[serde(default)]
    pub spec_revision: Option<ProposedRevision>,
    /// Complete revised tickets replacing (by identity) or extending the plan.
    #[serde(default)]
    pub tickets: Vec<crate::planning::Ticket>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedRevision {
    pub path: String,
    pub content: String,
}
/// System boundary: each call is a fresh independent session.
pub trait DecisionAgent {
    fn decide(&self, request: &DecisionRequest) -> Result<Proposal>;
}
/// Deterministic decision session and independent plan verifier.
#[derive(Deserialize)]
pub struct FixtureDecisionAgent {
    proposal: Proposal,
    verification: crate::planning::Verification,
}
impl FixtureDecisionAgent {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&std::fs::read(path)?).context("invalid deterministic decision fixture")
    }
}
impl DecisionAgent for FixtureDecisionAgent {
    fn decide(&self, _: &DecisionRequest) -> Result<Proposal> {
        Ok(self.proposal.clone())
    }
}
impl crate::planning::PlanningAgent for FixtureDecisionAgent {
    fn generate(&self, _: &crate::planning::PlanningRequest) -> Result<Vec<crate::planning::Ticket>> {
        bail!("decision fixtures do not generate plans")
    }
    fn verify(&self, _: &crate::planning::VerificationRequest) -> Result<crate::planning::Verification> {
        Ok(self.verification.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub ambiguity_id: String,
    pub context_id: String,
    pub question: String,
    pub positions: Vec<RankedPosition>,
    pub governing: Option<RankedPosition>,
    pub resolution: String,
    pub rationale: String,
    pub evidence: Vec<String>,
    /// resolved | rejected
    pub outcome: String,
    pub findings: Vec<Finding>,
    /// Version of the spec revision this decision produced, if any.
    #[serde(default)]
    pub spec_revision: Option<u32>,
    /// Independent reverification of affected tickets, when requirements or tickets changed.
    #[serde(default)]
    pub reverification: Option<crate::planning::Plan>,
}

fn configured_statement(run: &Run, key: &str, reference: &str) -> Option<String> {
    run.config.extensions.get(key)?.as_array()?.iter().find_map(|entry| {
        (entry.get("id")?.as_str()? == reference)
            .then(|| entry.get("statement")?.as_str().map(Into::into))?
    })
}
fn source_statement(run: &Run, position: &Position) -> Result<String> {
    let found = match position.source.as_str() {
        PRODUCT_OBJECTIVE => configured_statement(run, "product_objectives", &position.reference),
        ARCHITECTURAL_DECISION => {
            configured_statement(run, "architectural_decisions", &position.reference)
        }
        SPEC => {
            let requirements = crate::planning::requirements(&run.specs);
            run.specs
                .iter()
                .find(|s| s.path == position.reference)
                .map(|s| s.content.clone())
                .or_else(|| {
                    requirements
                        .into_iter()
                        .find(|r| r.id == position.reference)
                        .map(|r| r.criterion)
                })
        }
        other => bail!("unknown decision source {other}; use product_objective, architectural_decision or spec"),
    };
    found.with_context(|| {
        format!(
            "{} {} is not recorded in this run's configuration or frozen specs",
            position.source, position.reference
        )
    })
}

impl Engine {
    /// Resolve one ambiguity in a fresh decision context and record the decision.
    /// `verifier` independently reverifies affected tickets in a separate context.
    pub fn decide(
        &self,
        id: &str,
        ambiguity: &Ambiguity,
        agent: &dyn DecisionAgent,
        verifier: &dyn crate::planning::PlanningAgent,
    ) -> Result<Run> {
        let run = self.inspect(id)?;
        if ambiguity.positions.is_empty() {
            bail!("an ambiguity needs at least one position");
        }
        let mut positions = Vec::new();
        for p in &ambiguity.positions {
            positions.push(RankedPosition {
                rank: rank(&p.source).with_context(|| format!("unknown decision source {}", p.source))?,
                source: p.source.clone(),
                reference: p.reference.clone(),
                statement: p.statement.clone(),
                source_statement: source_statement(&run, p)?,
            });
        }
        positions.sort_by_key(|p| p.rank);
        let number = run.decisions.len() + 1;
        let decision_id = format!("{}-decision-{number}", run.id);
        let context_id = format!("{decision_id}-context");
        let proposal = agent.decide(&DecisionRequest {
            context_id: context_id.clone(),
            instructions: DECISION_INSTRUCTIONS.into(),
            question: ambiguity.question.clone(),
            positions: positions.clone(),
            specs: run.effective_specs(),
            tickets: run.plan.as_ref().map(|p| p.tickets.clone()).unwrap_or_default(),
        })?;
        let mut findings = Vec::new();
        let top = positions[0].rank;
        let governing = positions
            .iter()
            .find(|p| p.reference == proposal.governing)
            .cloned();
        match &governing {
            None => findings.push(Finding {
                code: "unknown_governing_position".into(),
                message: format!("{} is not one of the recorded positions", proposal.governing),
            }),
            Some(g) if g.rank != top => findings.push(Finding {
                code: "hierarchy_violation".into(),
                message: format!(
                    "{} {} cannot govern while a {} position is present",
                    g.source, g.reference, positions[0].source
                ),
            }),
            Some(_) => {}
        }
        if proposal.rationale.trim().is_empty() || proposal.evidence.iter().all(|e| e.trim().is_empty()) {
            findings.push(Finding {
                code: "missing_rationale".into(),
                message: "a decision must preserve its rationale and evidence".into(),
            });
        }
        let mut revision = None;
        let mut reverification = None;
        let mut adopted_plan = None;
        if findings.is_empty() && (proposal.spec_revision.is_some() || !proposal.tickets.is_empty()) {
            let mut specs = run.effective_specs();
            if let Some(proposed) = &proposal.spec_revision {
                let spec = specs
                    .iter_mut()
                    .find(|s| s.path == proposed.path)
                    .with_context(|| format!("spec revision targets {}, which is not a frozen spec of this run", proposed.path))?;
                let sha = format!("{:x}", Sha256::digest(proposed.content.as_bytes()));
                revision = Some(SpecRevision {
                    version: run.spec_revisions.len() as u32 + 1,
                    path: proposed.path.clone(),
                    base_sha256: spec.content_sha256.clone(),
                    content: proposed.content.clone(),
                    content_sha256: sha.clone(),
                    decision_id: decision_id.clone(),
                    replan_id: None,
                    status: String::new(),
                });
                spec.content = proposed.content.clone();
                spec.content_sha256 = sha;
            }
            let plan = reverify(&run, &specs, &proposal.tickets, verifier, &decision_id, &context_id)?;
            findings.extend(plan.findings.iter().cloned());
            if let Some(r) = &mut revision {
                r.status = if plan.executable { "verified" } else { "rejected" }.into();
            }
            if plan.executable {
                adopted_plan = Some(plan.clone());
            }
            reverification = Some(plan);
        }
        let decision = Decision {
            id: decision_id,
            ambiguity_id: ambiguity.id.clone(),
            context_id,
            question: ambiguity.question.clone(),
            positions,
            governing,
            resolution: proposal.resolution,
            rationale: proposal.rationale,
            evidence: proposal.evidence,
            outcome: if findings.is_empty() { "resolved" } else { "rejected" }.into(),
            findings,
            spec_revision: revision.as_ref().map(|r| r.version),
            reverification,
        };
        self.transact(id, |run| {
            if let Some(r) = revision {
                run.spec_revisions.push(r);
            }
            if let Some(plan) = adopted_plan {
                run.plan = Some(plan);
            }
            run.decisions.push(decision);
            Ok(run.clone())
        })
    }
}

/// Independently reverify the plan with `revised` tickets against `specs` in a
/// fresh verification context: coverage, dependencies and acceptance checks.
pub(crate) fn reverify(
    run: &Run,
    specs: &[crate::FrozenSpec],
    revised: &[crate::planning::Ticket],
    verifier: &dyn crate::planning::PlanningAgent,
    origin: &str,
    origin_context: &str,
) -> Result<crate::planning::Plan> {
    let mut tickets = run
        .plan
        .as_ref()
        .filter(|p| p.executable)
        .context("revising work requires an independently verified executable plan")?
        .tickets
        .clone();
    for t in revised {
        match tickets.iter_mut().find(|x| x.id == t.id) {
            Some(existing) => *existing = t.clone(),
            None => tickets.push(t.clone()),
        }
    }
    let requirements = crate::planning::requirements(specs);
    let verification_context = format!("{origin}-verification");
    let verification = verifier.verify(&crate::planning::VerificationRequest {
        context_id: verification_context.clone(),
        instructions: crate::planning::VERIFIER_INSTRUCTIONS.into(),
        specs: specs.to_vec(),
        requirements,
        tickets: tickets.clone(),
    })?;
    Ok(crate::planning::validate_plan(
        specs,
        tickets,
        verification,
        origin_context.into(),
        verification_context,
    ))
}
