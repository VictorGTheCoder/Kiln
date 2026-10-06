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
}
