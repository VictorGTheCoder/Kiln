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
    /// Decisions made each time the run was resumed after an interruption.
    #[serde(default)]
    pub recoveries: Vec<crate::recovery::Recovery>,
    /// Progress comments reflected back to imported GitHub issues.
    #[serde(default)]
    pub synchronization: Option<crate::synchronization::Synchronization>,
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
    pub decision_id: String,
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

}
