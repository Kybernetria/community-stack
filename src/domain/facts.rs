use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{PlanningProjectionWrite, ToolkitProjection};

pub const MAX_FACT_OBJECT_BYTES: usize = 65_536;
pub const MAX_FACT_HISTORY_PAGE: u16 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FactLifecycleStatus {
    Asserted,
    Superseded,
    Retracted,
    Disputed,
    Expired,
}

impl FactLifecycleStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asserted => "ASSERTED",
            Self::Superseded => "SUPERSEDED",
            Self::Retracted => "RETRACTED",
            Self::Disputed => "DISPUTED",
            Self::Expired => "EXPIRED",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SubmittedFactLifecycle {
    Asserted,
    Retracted,
    Disputed,
    Expired,
}

impl From<SubmittedFactLifecycle> for FactLifecycleStatus {
    fn from(value: SubmittedFactLifecycle) -> Self {
        match value {
            SubmittedFactLifecycle::Asserted => Self::Asserted,
            SubmittedFactLifecycle::Retracted => Self::Retracted,
            SubmittedFactLifecycle::Disputed => Self::Disputed,
            SubmittedFactLifecycle::Expired => Self::Expired,
        }
    }
}

const fn default_lifecycle() -> SubmittedFactLifecycle {
    SubmittedFactLifecycle::Asserted
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProvenancePolicy {
    None,
    Source,
    SourceDocument,
    SourceAndDocument,
}

impl ProvenancePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Source => "SOURCE",
            Self::SourceDocument => "SOURCE_DOCUMENT",
            Self::SourceAndDocument => "SOURCE_AND_DOCUMENT",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct FactLifecyclePolicy {
    #[serde(default = "yes")]
    pub allow_supersession: bool,
    #[serde(default = "yes")]
    pub allow_retraction: bool,
    #[serde(default)]
    pub allow_dispute: bool,
    #[serde(default)]
    pub allow_expiry: bool,
}

const fn yes() -> bool {
    true
}

impl Default for FactLifecyclePolicy {
    fn default() -> Self {
        Self {
            allow_supersession: true,
            allow_retraction: true,
            allow_dispute: false,
            allow_expiry: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterFactSchema {
    pub app_id: String,
    pub schema_id: String,
    pub schema_version: u32,
    pub predicate: String,
    pub object_schema: Value,
    #[serde(default)]
    pub multiple_active_claims: bool,
    pub provenance_policy: ProvenancePolicy,
    #[serde(default)]
    pub lifecycle_policy: FactLifecyclePolicy,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetFactSchema {
    pub app_id: String,
    pub schema_id: String,
    #[serde(default)]
    pub schema_version: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListFactSchemas {
    pub app_id: String,
    #[serde(default = "default_schema_limit")]
    pub limit: u16,
}

const fn default_schema_limit() -> u16 {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactSchema {
    pub app_id: String,
    pub schema_id: String,
    pub schema_version: u32,
    pub predicate: String,
    pub object_schema: Value,
    pub multiple_active_claims: bool,
    pub provenance_policy: ProvenancePolicy,
    pub lifecycle_policy: FactLifecyclePolicy,
    pub created_at_ms: i64,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertFact {
    pub community_id: String,
    pub claim_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: Value,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub source_document_id: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    pub schema_id: String,
    pub schema_version: u32,
    #[serde(default = "default_lifecycle")]
    pub lifecycle_status: SubmittedFactLifecycle,
    pub idempotency_key: String,
}

const fn default_confidence() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryFacts {
    pub community_id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub predicate: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
}

const fn default_limit() -> u16 {
    50
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InspectFact {
    pub community_id: String,
    pub claim_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactHistoryRequest {
    pub community_id: String,
    pub claim_id: String,
    #[serde(default = "default_history_limit")]
    pub limit: u16,
    #[serde(default)]
    pub cursor: Option<String>,
}

const fn default_history_limit() -> u16 {
    25
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactClaim {
    pub claim_id: String,
    pub community_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: Value,
    pub source: Option<String>,
    pub confidence: f64,
    pub source_operation_hash: String,
    pub updated_at_ms: i64,
    pub schema_id: String,
    pub schema_version: u32,
    pub lifecycle_status: FactLifecycleStatus,
    pub source_document_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactRevision {
    pub operation_hash: String,
    pub claim_id: String,
    pub community_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: Value,
    pub source: Option<String>,
    pub confidence: f64,
    pub author_key: String,
    pub schema_id: String,
    pub schema_version: u32,
    pub asserted_at_ms: i64,
    pub supersedes_operation_hash: Option<String>,
    pub lifecycle_status: FactLifecycleStatus,
    pub source_document_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactHistoryPage {
    pub revisions: Vec<FactRevision>,
    pub next_cursor: Option<String>,
    pub limit: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactInspection {
    pub current_claim: FactClaim,
    pub current_revision_operation_hash: String,
    pub author_key: String,
    pub semantic_source: Option<String>,
    pub source_document_id: Option<String>,
    pub schema_id: String,
    pub schema_version: u32,
    pub lifecycle_status: FactLifecycleStatus,
    pub loro_document_id: String,
    pub revision_count: u32,
    pub verification_status: String,
    pub apply_status: String,
    pub supersedes_operation_hash: Option<String>,
    pub superseded_by_operation_hash: Option<String>,
    pub projection_consistent: bool,
    pub consistency_issues: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FactCurrentRevision {
    pub operation_hash: String,
    pub subject: String,
    pub predicate: String,
    pub schema_id: String,
    pub schema_version: u32,
    pub lifecycle_status: FactLifecycleStatus,
}

#[derive(Clone, Debug)]
pub struct FactRevisionProjection {
    pub claim_id: String,
    pub community_id: String,
    pub subject: String,
    pub predicate: String,
    pub object_json: Vec<u8>,
    pub source: Option<String>,
    pub confidence: f64,
    pub schema_id: String,
    pub schema_version: u32,
    pub lifecycle_status: FactLifecycleStatus,
    pub source_document_id: Option<String>,
    pub multiple_active_claims: bool,
    pub expected_previous_operation_hash: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ProjectionWrite {
    FactRevision(Box<FactRevisionProjection>),
    Planning(Box<PlanningProjectionWrite>),
    Toolkit(Box<ToolkitProjection>),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub check_name: String,
    pub status: CheckStatus,
    pub count: u32,
    pub examples: Vec<String>,
    pub repairable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub mode: String,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Clone, Debug)]
pub struct DocumentReplayInput {
    pub identifier: String,
    pub updates: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectionCheck {
    pub projection: String,
    pub consistent: bool,
    pub current_count: u32,
    pub expected_count: u32,
    pub current_hash: String,
    pub expected_hash: String,
    pub mismatches: Vec<String>,
    pub rebuild_enabled: bool,
}
