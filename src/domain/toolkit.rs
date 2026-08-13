use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_CATALOG_ROWS: usize = 10_000;

fn default_limit() -> u16 {
    100
}

fn default_true() -> bool {
    true
}

fn default_include_partial() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConceptKind {
    Domain,
    Role,
    Capability,
    Standard,
    Attribute,
    Dimension,
    Descriptor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Boolean,
    Integer,
    Number,
    Text,
    Enum,
    Date,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    One,
    Many,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConceptStatus {
    Active,
    Deprecated,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub rubric: String,
    pub version: String,
    pub anchors: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddConcept {
    pub community_id: String,
    pub idempotency_key: String,
    pub key: String,
    pub name: String,
    pub kind: ConceptKind,
    pub definition: String,
    pub aliases: Vec<String>,
    pub value_type: ValueType,
    pub cardinality: Cardinality,
    pub applicable_categories: Vec<String>,
    pub allowed_values: Option<Vec<String>>,
    pub scale: Option<Scale>,
    pub parent: Option<String>,
    pub status: ConceptStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConceptDefinition {
    pub key: String,
    pub name: String,
    pub kind: ConceptKind,
    pub definition: String,
    pub aliases: Vec<String>,
    pub value_type: ValueType,
    pub cardinality: Cardinality,
    pub applicable_categories: Vec<String>,
    pub allowed_values: Option<Vec<String>>,
    pub scale: Option<Scale>,
    pub parent: Option<String>,
    pub status: ConceptStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitConcept {
    #[serde(flatten)]
    pub definition: ConceptDefinition,
    pub revision: String,
    pub source_operation_hash: String,
    pub active_revision: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListConcepts {
    pub community_id: String,
    #[serde(default = "default_true")]
    pub active_only: bool,
    #[serde(default = "default_limit")]
    pub limit: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowConcept {
    pub community_id: String,
    pub key: String,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddTool {
    pub community_id: String,
    pub idempotency_key: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default = "default_tool_status")]
    pub status: String,
}

fn default_tool_status() -> String {
    "active".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitTool {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub status: String,
    pub source_operation_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionOrigin {
    Human,
    Ai,
    Fixture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Proposed,
    Verified,
    Rejected,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertToolkitValue {
    pub community_id: String,
    pub idempotency_key: String,
    pub assertion_id: String,
    pub tool: String,
    pub concept: String,
    #[serde(default)]
    pub concept_revision: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    pub origin: AssertionOrigin,
    #[serde(default = "default_proposed")]
    pub verification_state: VerificationState,
    pub source: String,
    pub evidence: String,
    pub as_of: String,
    #[serde(default)]
    pub rubric: Option<String>,
    #[serde(default)]
    pub rubric_version: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub evaluator_type: Option<String>,
    #[serde(default)]
    pub evaluation_date: Option<String>,
}

fn default_proposed() -> VerificationState {
    VerificationState::Proposed
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyToolkitAssertion {
    pub community_id: String,
    pub idempotency_key: String,
    pub review_id: String,
    pub assertion_id: String,
    pub state: VerificationState,
    pub reviewer: String,
    pub rationale: String,
    pub review_date: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitAssertion {
    pub assertion_id: String,
    pub tool: String,
    pub concept: String,
    pub concept_revision: String,
    pub value: Option<Value>,
    pub origin: AssertionOrigin,
    pub verification_state: VerificationState,
    pub source: String,
    pub evidence: String,
    pub as_of: String,
    pub rubric: Option<String>,
    pub rubric_version: Option<String>,
    pub rationale: Option<String>,
    pub evaluator_type: Option<String>,
    pub evaluation_date: Option<String>,
    pub source_operation_hash: String,
    pub review_operation_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitReview {
    pub review_id: String,
    pub assertion_id: String,
    pub state: VerificationState,
    pub reviewer: String,
    pub rationale: String,
    pub review_date: String,
    pub source_operation_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOperator {
    Eq,
    Ne,
    In,
    Exists,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolkitRequirement {
    pub concept: String,
    #[serde(default)]
    pub concept_revision: Option<String>,
    pub op: QueryOperator,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolkitQueryPlan {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub requirements: Vec<ToolkitRequirement>,
    #[serde(default)]
    pub mandatory: Vec<ToolkitRequirement>,
    #[serde(default)]
    pub optional: Vec<ToolkitRequirement>,
    #[serde(default = "default_include_partial")]
    pub include_partial: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryToolkit {
    pub community_id: String,
    #[serde(flatten)]
    pub plan: ToolkitQueryPlan,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainToolkit {
    pub community_id: String,
    pub tool: String,
    pub query: ToolkitQueryPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementState {
    Satisfied,
    Unsatisfied,
    Unknown,
    Missing,
    Proposed,
    Rejected,
    Conflict,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequirementExplanation {
    pub concept: String,
    pub concept_revision: String,
    pub op: QueryOperator,
    pub required: bool,
    pub state: RequirementState,
    pub assertions: Vec<ToolkitAssertion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitQueryResult {
    pub tool: String,
    pub name: String,
    #[serde(rename = "match")]
    pub match_state: String,
    pub satisfied_requirements: usize,
    pub requirements: Vec<RequirementExplanation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitQueryResponse {
    pub exact_count: usize,
    pub results: Vec<ToolkitQueryResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportToolkit {
    pub community_id: String,
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "default_export_limit")]
    pub limit: u16,
}

fn default_export_limit() -> u16 {
    500
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolkitCatalog {
    pub concepts: Vec<ToolkitConcept>,
    pub tools: Vec<ToolkitTool>,
    pub assertions: Vec<ToolkitAssertion>,
    pub reviews: Vec<ToolkitReview>,
}

#[derive(Clone, Debug)]
pub enum ToolkitProjection {
    Concept(ToolkitConcept),
    Tool(ToolkitTool),
    Assertion(ToolkitAssertion),
    Review(ToolkitReview),
}
