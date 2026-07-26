use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningStatus {
    Planned,
    Active,
    Completed,
    Cancelled,
    Archived,
}

impl PlanningStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecurrenceRule {
    pub frequency: RecurrenceFrequency,
    #[serde(default = "default_interval")]
    pub interval: u16,
    #[serde(default)]
    pub count: Option<u16>,
    #[serde(default)]
    pub until_ms: Option<i64>,
    #[serde(default)]
    pub by_weekday: Vec<Weekday>,
}

const fn default_interval() -> u16 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventTiming {
    AllDay {
        start_date: String,
        end_date_exclusive: String,
    },
    Utc {
        start_at_ms: i64,
        end_at_ms: i64,
        time_zone: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutProject {
    pub community_id: String,
    pub idempotency_key: String,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub status: PlanningStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutTask {
    pub community_id: String,
    pub idempotency_key: String,
    pub task_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub status: PlanningStatus,
    #[serde(default)]
    pub start_at_ms: Option<i64>,
    #[serde(default)]
    pub due_at_ms: Option<i64>,
    #[serde(default)]
    pub progress_percent: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutEvent {
    pub community_id: String,
    pub idempotency_key: String,
    pub event_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub status: PlanningStatus,
    pub timing: EventTiming,
    #[serde(default)]
    pub recurrence: Option<RecurrenceRule>,
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    FinishToStart,
    StartToStart,
    FinishToFinish,
    StartToFinish,
}

impl DependencyKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::FinishToStart => "finish_to_start",
            Self::StartToStart => "start_to_start",
            Self::FinishToFinish => "finish_to_finish",
            Self::StartToFinish => "start_to_finish",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutDependency {
    pub community_id: String,
    pub idempotency_key: String,
    pub dependency_id: String,
    pub predecessor_task_id: String,
    pub successor_task_id: String,
    pub kind: DependencyKind,
    #[serde(default)]
    pub lag_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningViewRequest {
    pub community_id: String,
    #[serde(default = "default_limit")]
    pub limit: u16,
}

const fn default_limit() -> u16 {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningProjectRecord {
    pub project_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanningStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningTaskRecord {
    pub task_id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanningStatus,
    pub start_at_ms: Option<i64>,
    pub due_at_ms: Option<i64>,
    pub progress_percent: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningEventRecord {
    pub event_id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanningStatus,
    pub timing: EventTiming,
    pub recurrence: Option<RecurrenceRule>,
    pub location: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningDependencyRecord {
    pub dependency_id: String,
    pub predecessor_task_id: String,
    pub successor_task_id: String,
    pub kind: DependencyKind,
    pub lag_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningProject {
    pub project_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanningStatus,
    pub submitted_by_app_id: String,
    pub source_operation_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningTask {
    pub task_id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanningStatus,
    pub start_at_ms: Option<i64>,
    pub due_at_ms: Option<i64>,
    pub progress_percent: u8,
    pub submitted_by_app_id: String,
    pub source_operation_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningEvent {
    pub event_id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: PlanningStatus,
    pub timing: EventTiming,
    pub recurrence: Option<RecurrenceRule>,
    pub location: Option<String>,
    pub submitted_by_app_id: String,
    pub source_operation_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningDependency {
    pub dependency_id: String,
    pub predecessor_task_id: String,
    pub successor_task_id: String,
    pub kind: DependencyKind,
    pub lag_ms: i64,
    pub submitted_by_app_id: String,
    pub source_operation_hash: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlanningCatalog {
    pub version: u64,
    pub projects: Vec<PlanningProject>,
    pub tasks: Vec<PlanningTask>,
    pub events: Vec<PlanningEvent>,
    pub dependencies: Vec<PlanningDependency>,
}

#[derive(Clone, Debug)]
pub struct PlanningProjectionWrite {
    pub profile_digest: String,
    pub authorized_app_id: String,
    pub profile_id: String,
    pub expected_community_version: u64,
    pub projection: PlanningProjection,
}

#[derive(Clone, Debug)]
pub enum PlanningProjection {
    Project(PlanningProject),
    Task(PlanningTask),
    Event(PlanningEvent),
    Dependency(PlanningDependency),
}
