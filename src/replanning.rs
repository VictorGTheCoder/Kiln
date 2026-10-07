//! Bounded autonomous replanning. A ticket that exhausts its correction cycles
//! receives at most `replanning_attempts` (default one) replanning attempts while
//! run-wide limits allow. The revised ticket is independently reverified (coverage,
//! dependencies, acceptance checks) before any revised work starts; persistent
//! failure afterwards blocks the ticket without asking the developer to intervene.
use crate::{
    planning::{Finding, PlanningAgent, Ticket},
    Engine, FrozenSpec, Run,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const REPLANNING_INSTRUCTIONS: &str = "Correction cycles for this ticket are exhausted. Replan it autonomously: return one complete revised ticket with the same id that addresses the unresolved findings while covering the same approved requirements. Apply product objectives first, architectural decisions second, specs third. Do not implement it and do not ask the developer.";

#[derive(Debug, Clone, Serialize)]
pub struct ReplanRequest {
    pub context_id: String,
    pub instructions: String,
    pub ticket: Ticket,
    pub failures: Vec<String>,
    pub specs: Vec<FrozenSpec>,
    pub tickets: Vec<Ticket>,
}
/// System boundary: fresh replanning session; `verify` is a separate fresh context.
pub trait ReplanningAgent {
    fn replan(&self, request: &ReplanRequest) -> Result<Ticket>;
}
pub trait Replanner: ReplanningAgent + PlanningAgent {}
impl<T: ReplanningAgent + PlanningAgent> Replanner for T {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Replanning {
    pub id: String,
    pub ticket_id: String,
    /// 1-based attempt number for this ticket.
    pub attempt: u64,
    pub context_id: String,
    /// Unresolved findings that triggered the attempt.
    pub failures: Vec<String>,
    pub previous: Ticket,
    pub revised: Option<Ticket>,
    /// Independent reverification of the plan containing the revised ticket.
    pub verification: Option<crate::planning::Plan>,
    pub findings: Vec<Finding>,
    /// replanned (verified, revised work proceeds) | rejected | failed
    pub outcome: String,
    /// Outcome of the revised work: integrated | blocked.
    #[serde(default)]
    pub result: Option<String>,
}

/// Unresolved findings of a ticket's latest correction and session.
fn failures(run: &Run, ticket: &str) -> Vec<String> {
    let mut failures: Vec<String> = run
        .corrections
        .iter()
        .rev()
        .find(|c| c.ticket_id == ticket)
        .map(|c| c.findings.clone())
        .unwrap_or_default();
    if let Some(f) = run
        .sessions
        .iter()
        .rev()
        .find(|s| s.ticket_id == ticket)
        .and_then(|s| s.failure.clone())
    {
        failures.push(f);
    }
    failures
}

impl Engine {
    /// Replanning attempts already made for `ticket` in this run.
    /// Attempts cut short by a provider usage limit are not charged.
    pub fn replanning_attempts(&self, run: &Run, ticket: &str) -> u64 {
        run.replans
            .iter()
            .filter(|r| r.ticket_id == ticket && r.outcome != crate::correction::PROVIDER_LIMIT)
            .count() as u64
    }
    /// One replanning attempt. Returns the recorded attempt; when `replanned`, the
    /// plan holds the verified revision and the ticket's earlier sessions are superseded.
    pub fn replan_ticket(
        &self,
        id: &str,
        ticket: &str,
        agent: &dyn Replanner,
    ) -> Result<Replanning> {
        let run = self.inspect(id)?;
        let previous = run
            .plan
            .as_ref()
            .and_then(|p| p.tickets.iter().find(|t| t.id == ticket))
            .context("unknown ticket")?
            .clone();
        let attempt = self.replanning_attempts(&run, ticket) + 1;
        let recorded = run.replans.iter().filter(|r| r.ticket_id == ticket).count() + 1;
        let replan_id = format!("{}-replan-{ticket}-{recorded}", run.id);
        let context_id = format!("{replan_id}-context");
        let failures = failures(&run, ticket);
        let specs = run.effective_specs();
        let mut record = Replanning {
            id: replan_id.clone(),
            ticket_id: ticket.into(),
            attempt,
            context_id: context_id.clone(),
            failures: failures.clone(),
            previous: previous.clone(),
            revised: None,
            verification: None,
            findings: Vec::new(),
            outcome: "failed".into(),
            result: None,
        };
        let revised = agent.replan(&ReplanRequest {
            context_id: context_id.clone(),
            instructions: REPLANNING_INSTRUCTIONS.into(),
            ticket: previous,
            failures,
            specs: specs.clone(),
            tickets: run
                .plan
                .as_ref()
                .map(|p| p.tickets.clone())
                .unwrap_or_default(),
        });
        let mut adopted = None;
        let mut provider_limit = None;
        match revised {
            Err(e) if crate::limits::ProviderLimit::in_error(&e).is_some() => {
                provider_limit = crate::limits::ProviderLimit::in_error(&e).cloned();
                record.outcome = crate::correction::PROVIDER_LIMIT.into();
                record.findings.push(Finding {
                    code: "provider_limit".into(),
                    message: format!("{e:#}"),
                });
            }
            Err(e) => record.findings.push(Finding {
                code: "replanning_failed".into(),
                message: format!("{e:#}"),
            }),
            Ok(revised) if revised.id != ticket => record.findings.push(Finding {
                code: "ticket_identity_changed".into(),
                message: format!("replanning of {ticket} returned ticket {}", revised.id),
            }),
            Ok(revised) => {
                let plan = crate::decision::reverify(
                    &run,
                    &specs,
                    std::slice::from_ref(&revised),
                    &[],
                    agent,
                    &replan_id,
                    &context_id,
                )?;
                record.findings = plan.findings.clone();
                record.outcome = if plan.executable {
                    "replanned"
                } else {
                    "rejected"
                }
                .into();
                if plan.executable {
                    adopted = Some(plan.clone());
                }
                record.revised = Some(revised);
                record.verification = Some(plan);
            }
        }
        self.transact(id, |run| {
            if let Some(plan) = adopted {
                run.plan = Some(plan);
                for s in run
                    .sessions
                    .iter_mut()
                    .filter(|s| s.ticket_id == ticket && s.status != "integrated")
                {
                    s.status = "superseded".into();
                }
            }
            run.replans.push(record.clone());
            Ok(())
        })?;
        match provider_limit {
            // Recorded first; the caller stops the run on the typed limit.
            Some(limit) => Err(limit.into()),
            None => Ok(record),
        }
    }
    /// Record what became of the revised work of the ticket's latest attempt.
    pub(crate) fn settle_replanning(&self, id: &str, ticket: &str, result: &str) -> Result<()> {
        self.transact(id, |run| {
            if let Some(r) = run
                .replans
                .iter_mut()
                .rev()
                .find(|r| r.ticket_id == ticket && r.result.is_none())
            {
                r.result = Some(result.into());
            }
            Ok(())
        })
    }
}
