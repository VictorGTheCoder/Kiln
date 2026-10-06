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
    /// Decisions made each time the run was resumed after an interruption.
    #[serde(default)]
    pub recoveries: Vec<crate::recovery::Recovery>,
}
