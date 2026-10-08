use crate::ProjectConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenSpec {
    pub path: String,
    pub content: String,
    pub content_sha256: String,
    pub source_revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    #[serde(default)]
    pub scheduler: Option<crate::scheduler::SchedulerState>,
    pub schema_version: u32,
    pub id: String,
    pub repository: String,
    pub created_unix_ms: u128,
    pub status: String,
    pub config: ProjectConfig,
    pub specs: Vec<FrozenSpec>,
    /// Frozen open-issue inputs and derived criteria for a one-command backlog run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backlog: Option<BacklogRun>,
    /// Dependency-connected delivery units for whole-snapshot backlog runs.
    /// Group membership is frozen from the verified plan; outcomes track only
    /// the durable scheduler dispositions for those tickets.
    #[serde(default)]
    pub delivery_groups: Vec<crate::delivery::DeliveryGroup>,
    #[serde(default)]
    pub plan: Option<crate::planning::Plan>,
    #[serde(default)]
    pub imported_issues: Vec<crate::import::ImportedIssue>,
    #[serde(default)]
    pub integration_branch: Option<String>,
    #[serde(default)]
    pub reviews: Vec<crate::review::ReviewSession>,
    #[serde(default)]
    pub sessions: Vec<crate::execution::ImplementationSession>,
    #[serde(default)]
    pub corrections: Vec<crate::correction::CorrectionCycle>,
    #[serde(default)]
    pub integrations: Vec<crate::integration::IntegrationAttempt>,
    #[serde(default)]
    pub validation_reports: Vec<crate::validation::ValidationReport>,
    /// Remote branch and pull request identity of the verified delivery.
    #[serde(default)]
    pub publication: Option<crate::publication::Publication>,
    /// Autonomous decision sessions and their preserved rationale.
    #[serde(default)]
    pub decisions: Vec<crate::decision::Decision>,
    /// Versioned spec revisions made during the run. Frozen `specs` never change.
    #[serde(default)]
    pub spec_revisions: Vec<SpecRevision>,
    /// Bounded replanning attempts after exhausted correction cycles.
    #[serde(default)]
    pub replans: Vec<crate::replanning::Replanning>,
    /// Explicit replanning operations that applied revised approved specs.
    #[serde(default)]
    pub spec_replans: Vec<crate::spec_replanning::SpecReplan>,
    /// Decisions made each time the run was resumed after an interruption.
    #[serde(default)]
    pub recoveries: Vec<crate::recovery::Recovery>,
    /// Progress comments reflected back to imported GitHub issues.
    #[serde(default)]
    pub synchronization: Option<crate::synchronization::Synchronization>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogRun {
    pub github_repository: String,
    /// single-issue compatibility mode or whole-snapshot dependency-graph mode.
    #[serde(default = "single_backlog_mode")]
    pub mode: String,
    pub selected_issue: u64,
    pub snapshot_unix_ms: u128,
    pub issue_snapshot: Vec<crate::import::ImportedIssue>,
    /// A durable classification for every issue captured at run start. This is
    /// separate from the executable plan: containers and issues outside the
    /// selected one-issue workflow still receive an explicit disposition.
    #[serde(default)]
    pub dispositions: Vec<BacklogIssueDisposition>,
    #[serde(default)]
    pub inferred_requirements: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    pub skill_version: String,
    pub outcome: String,
}
fn single_backlog_mode() -> String {
    "single-issue".into()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogIssueDisposition {
    pub issue: String,
    /// actionable | container
    #[serde(default = "actionable_issue_kind")]
    pub kind: String,
    /// Whether this issue was selected by the command that created the run.
    #[serde(default)]
    pub selected: bool,
    /// Whether this actionable issue has enough criteria and actionable
    /// prerequisites to produce an executable plan ticket.
    #[serde(default)]
    pub plan_candidate: bool,
    /// eligible | blocked | skipped | completed | failed | unable-to-verify
    pub status: String,
    pub dependencies: Vec<String>,
    pub reason: String,
    #[serde(default)]
    pub inferred_criteria: Vec<String>,
}
fn actionable_issue_kind() -> String {
    "actionable".into()
}
/// A spec revision produced by a decision: an explicit, versioned run artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecRevision {
    /// Run-wide revision number, starting at 1.
    pub version: u32,
    pub path: String,
    /// Content hash of the effective spec this revision replaces.
    pub base_sha256: String,
    pub content: String,
    pub content_sha256: String,
    /// Decision that produced the revision (empty for explicit spec replanning).
    #[serde(default)]
    pub decision_id: String,
    /// Explicit spec replanning that captured the revision, if any.
    #[serde(default)]
    pub replan_id: Option<String>,
    /// verified (effective) | rejected (failed independent reverification)
    pub status: String,
}
impl Run {
    /// Frozen specs with the latest verified revision of each applied.
    pub fn effective_specs(&self) -> Vec<FrozenSpec> {
        self.specs
            .iter()
            .map(|spec| {
                match self
                    .spec_revisions
                    .iter()
                    .rev()
                    .find(|r| r.path == spec.path && r.status == "verified")
                {
                    Some(r) => FrozenSpec {
                        content: r.content.clone(),
                        content_sha256: r.content_sha256.clone(),
                        ..spec.clone()
                    },
                    None => spec.clone(),
                }
            })
            .collect()
    }
    /// Current input version: the latest verified spec revision (0 = frozen specs).
    pub fn input_version(&self) -> u32 {
        self.spec_revisions
            .iter()
            .filter(|r| r.status == "verified")
            .map(|r| r.version)
            .max()
            .unwrap_or(0)
    }
    /// The delivery group that delivers `ticket`, if one was formed.
    pub fn delivery_group_of(&self, ticket: &str) -> Option<&crate::delivery::DeliveryGroup> {
        self.delivery_groups
            .iter()
            .find(|g| g.tickets.iter().any(|t| t == ticket))
    }
    /// Where `ticket` is in the pipeline, from recorded state only. Every view
    /// of ticket stages (`kiln status`, the dashboard kanban) derives from it.
    pub fn ticket_progress(&self, ticket: &str) -> TicketProgress {
        let state = self
            .scheduler
            .iter()
            .flat_map(|s| &s.tickets)
            .find(|t| t.id == ticket)
            .map_or("planned", |t| t.state.as_str());
        let stage = match state {
            "planned" | "waiting" => Stage::Planned,
            "implementing" => match self.sessions.iter().rev().find(|s| s.ticket_id == ticket) {
                // The latest implementation finished: the ticket is in review.
                Some(s) if s.status == "implemented" => Stage::Review,
                _ => Stage::Implementing,
            },
            "awaiting_integration" | "integrating" => Stage::Integrating,
            "integrated" => {
                let group = self.delivery_group_of(ticket);
                // Single-issue runs record one publication for the run.
                let pull_request = match group {
                    Some(g) => g.pull_request.as_ref(),
                    None => self
                        .publication
                        .as_ref()
                        .and_then(|p| p.pull_request.as_ref()),
                };
                match (group, pull_request) {
                    (Some(g), _) if g.status == "verified" => Stage::Delivered,
                    (_, None) => Stage::Integrated,
                    (Some(g), Some(_))
                        if !g.ci_attempts.is_empty()
                            || matches!(
                                g.status.as_str(),
                                "awaiting-ci" | "ci-pending" | "ci-failed"
                            ) =>
                    {
                        Stage::Ci
                    }
                    _ => Stage::PullRequest,
                }
            }
            // Blocked or stopped tickets stay where their evidence ends.
            _ => {
                if self.integrations.iter().any(|i| i.ticket_id == ticket) {
                    Stage::Integrating
                } else if self.reviews.iter().any(|r| r.ticket_id == ticket) {
                    Stage::Review
                } else if self.sessions.iter().any(|s| s.ticket_id == ticket) {
                    Stage::Implementing
                } else {
                    Stage::Planned
                }
            }
        };
        TicketProgress {
            state: state.to_owned(),
            stage,
        }
    }
}

/// A ticket's recorded scheduler state and the furthest pipeline stage it
/// reached (see [`Run::ticket_progress`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketProgress {
    /// Scheduler state (`waiting`, `implementing`, `integrated`, `blocked`, …),
    /// or `planned` before scheduling.
    pub state: String,
    pub stage: Stage,
}

/// Pipeline stages of a ticket, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Planned,
    Implementing,
    Review,
    Integrating,
    /// Integrated; no pull request yet.
    Integrated,
    /// Its pull request is open; CI not observed yet.
    PullRequest,
    /// Its pull request is awaiting, pending or failing CI.
    Ci,
    /// Its delivery group is verified.
    Delivered,
}
