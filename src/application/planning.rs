use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use super::{CommunityCore, validate_idempotency_key, validate_object_id};
use crate::domain::{
    DocumentKey, EventTiming, GetProfile, GrantProfile, Mutation, PLANNING_NAMESPACE,
    PLANNING_PROFILE_DIGEST, PLANNING_PROFILE_ID, PLANNING_PROFILE_VERSION, PlanningCatalog,
    PlanningDependency, PlanningDependencyRecord, PlanningEvent, PlanningEventRecord,
    PlanningProject, PlanningProjectRecord, PlanningProjection, PlanningProjectionWrite,
    PlanningTask, PlanningTaskRecord, PlanningViewRequest, PrimitiveValue, PrincipalRole,
    ProfileAccess, ProfileDescription, ProfileManifest, ProjectionWrite, PutDependency, PutEvent,
    PutProject, PutTask, RecurrenceFrequency, RecurrenceRule,
};

const MAX_PROFILE_ROWS: u16 = 10_001;
const MAX_PROFILE_RECORDS: usize = 10_000;

#[derive(Serialize)]
struct SignedPlanningRecord<'a, T> {
    profile_id: &'static str,
    profile_version: u32,
    profile_digest: &'a str,
    submitted_by_app_id: &'a str,
    record: &'a T,
}

impl CommunityCore {
    pub(super) fn profile_list() -> Result<Value> {
        Ok(serde_json::to_value(vec![ProfileDescription {
            manifest: planning_manifest()?,
            access: ProfileAccess::default(),
        }])?)
    }

    pub(super) async fn profile_get(&self, app_id: &str, request: GetProfile) -> Result<Value> {
        validate_profile_reference(&request.profile_id, request.profile_version)?;
        let access = if let Some(community_id) = request.community_id.as_deref() {
            validate_object_id(community_id, "community_id")?;
            self.repository
                .profile_access(app_id, PLANNING_PROFILE_ID, community_id)
                .await?
        } else {
            ProfileAccess::default()
        };
        Ok(serde_json::to_value(ProfileDescription {
            manifest: planning_manifest()?,
            access,
        })?)
    }

    pub(super) async fn profile_grant(&self, request: GrantProfile) -> Result<Value> {
        validate_object_id(&request.app_id, "app_id")?;
        validate_object_id(&request.community_id, "community_id")?;
        validate_profile_reference(&request.profile_id, None)?;
        let target = self
            .repository
            .principal_by_id(&request.app_id)
            .await?
            .context("profile grant target is not an enabled local principal")?;
        if target.role != PrincipalRole::App {
            bail!("profile grants target APP principals only");
        }
        if request.can_write && !request.can_read {
            bail!("profile write access requires read access");
        }
        let access = ProfileAccess {
            can_read: request.can_read,
            can_write: request.can_write,
        };
        self.repository
            .grant_profile(
                &request.app_id,
                &request.profile_id,
                &request.community_id,
                access,
            )
            .await?;
        Ok(
            json!({"app_id":request.app_id,"profile_id":request.profile_id,"community_id":request.community_id,"access":access}),
        )
    }

    pub(super) async fn planning_project_put(
        &self,
        app_id: &str,
        request: PutProject,
    ) -> Result<Value> {
        self.require_profile_write(app_id, &request.community_id)
            .await?;
        validate_common_write(
            &request.community_id,
            &request.idempotency_key,
            &request.project_id,
            "project_id",
            &request.title,
            request.description.as_deref(),
        )?;
        let profile_lock = self
            .document_lock(&planning_lock_key(&request.community_id))
            .await;
        let _profile_guard = profile_lock.lock().await;
        let request_hash = self.hash_request(&request)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let catalog = self
            .repository
            .planning_catalog(PLANNING_NAMESPACE, &request.community_id, MAX_PROFILE_ROWS)
            .await?;
        validate_catalog_bound(&catalog)?;
        if catalog.projects.len() == MAX_PROFILE_RECORDS
            && !catalog
                .projects
                .iter()
                .any(|project| project.project_id == request.project_id)
        {
            bail!("planning project limit reached");
        }
        let record = PlanningProjectRecord {
            project_id: request.project_id,
            title: request.title,
            description: request.description,
            status: request.status,
        };
        let projection = PlanningProject {
            project_id: record.project_id.clone(),
            title: record.title.clone(),
            description: record.description.clone(),
            status: record.status.clone(),
            submitted_by_app_id: app_id.into(),
            source_operation_hash: String::new(),
        };
        self.commit_planning_record(
            app_id,
            request.community_id,
            format!("planning/projects/{}", record.project_id),
            request.idempotency_key,
            request_hash,
            catalog.version,
            &record,
            PlanningProjection::Project(projection),
        )
        .await
    }

    pub(super) async fn planning_task_put(&self, app_id: &str, request: PutTask) -> Result<Value> {
        self.require_profile_write(app_id, &request.community_id)
            .await?;
        validate_common_write(
            &request.community_id,
            &request.idempotency_key,
            &request.task_id,
            "task_id",
            &request.title,
            request.description.as_deref(),
        )?;
        validate_optional_id(request.project_id.as_deref(), "project_id")?;
        if request.progress_percent > 100 {
            bail!("progress_percent must be between 0 and 100");
        }
        if request
            .start_at_ms
            .zip(request.due_at_ms)
            .is_some_and(|(start, due)| due < start)
        {
            bail!("due_at_ms must not precede start_at_ms");
        }
        let profile_lock = self
            .document_lock(&planning_lock_key(&request.community_id))
            .await;
        let _profile_guard = profile_lock.lock().await;
        let request_hash = self.hash_request(&request)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let catalog = self
            .repository
            .planning_catalog(PLANNING_NAMESPACE, &request.community_id, MAX_PROFILE_ROWS)
            .await?;
        validate_catalog_bound(&catalog)?;
        require_project(request.project_id.as_deref(), &catalog)?;
        if catalog.tasks.len() == MAX_PROFILE_RECORDS
            && !catalog
                .tasks
                .iter()
                .any(|task| task.task_id == request.task_id)
        {
            bail!("planning task limit reached");
        }
        let record = PlanningTaskRecord {
            task_id: request.task_id,
            project_id: request.project_id,
            title: request.title,
            description: request.description,
            status: request.status,
            start_at_ms: request.start_at_ms,
            due_at_ms: request.due_at_ms,
            progress_percent: request.progress_percent,
        };
        let projection = PlanningTask {
            task_id: record.task_id.clone(),
            project_id: record.project_id.clone(),
            title: record.title.clone(),
            description: record.description.clone(),
            status: record.status.clone(),
            start_at_ms: record.start_at_ms,
            due_at_ms: record.due_at_ms,
            progress_percent: record.progress_percent,
            submitted_by_app_id: app_id.into(),
            source_operation_hash: String::new(),
        };
        self.commit_planning_record(
            app_id,
            request.community_id,
            format!("planning/tasks/{}", record.task_id),
            request.idempotency_key,
            request_hash,
            catalog.version,
            &record,
            PlanningProjection::Task(projection),
        )
        .await
    }

    pub(super) async fn planning_event_put(
        &self,
        app_id: &str,
        request: PutEvent,
    ) -> Result<Value> {
        self.require_profile_write(app_id, &request.community_id)
            .await?;
        validate_common_write(
            &request.community_id,
            &request.idempotency_key,
            &request.event_id,
            "event_id",
            &request.title,
            request.description.as_deref(),
        )?;
        validate_optional_id(request.project_id.as_deref(), "project_id")?;
        validate_optional_text(request.location.as_deref(), "location", 1_024)?;
        validate_timing(&request.timing)?;
        if let Some(recurrence) = &request.recurrence {
            validate_recurrence(recurrence, &request.timing)?;
        }
        let profile_lock = self
            .document_lock(&planning_lock_key(&request.community_id))
            .await;
        let _profile_guard = profile_lock.lock().await;
        let request_hash = self.hash_request(&request)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let catalog = self
            .repository
            .planning_catalog(PLANNING_NAMESPACE, &request.community_id, MAX_PROFILE_ROWS)
            .await?;
        validate_catalog_bound(&catalog)?;
        require_project(request.project_id.as_deref(), &catalog)?;
        if catalog.events.len() == MAX_PROFILE_RECORDS
            && !catalog
                .events
                .iter()
                .any(|event| event.event_id == request.event_id)
        {
            bail!("planning event limit reached");
        }
        let record = PlanningEventRecord {
            event_id: request.event_id,
            project_id: request.project_id,
            title: request.title,
            description: request.description,
            status: request.status,
            timing: request.timing,
            recurrence: request.recurrence,
            location: request.location,
        };
        let projection = PlanningEvent {
            event_id: record.event_id.clone(),
            project_id: record.project_id.clone(),
            title: record.title.clone(),
            description: record.description.clone(),
            status: record.status.clone(),
            timing: record.timing.clone(),
            recurrence: record.recurrence.clone(),
            location: record.location.clone(),
            submitted_by_app_id: app_id.into(),
            source_operation_hash: String::new(),
        };
        self.commit_planning_record(
            app_id,
            request.community_id,
            format!("planning/events/{}", record.event_id),
            request.idempotency_key,
            request_hash,
            catalog.version,
            &record,
            PlanningProjection::Event(projection),
        )
        .await
    }

    pub(super) async fn planning_dependency_put(
        &self,
        app_id: &str,
        request: PutDependency,
    ) -> Result<Value> {
        self.require_profile_write(app_id, &request.community_id)
            .await?;
        validate_object_id(&request.community_id, "community_id")?;
        validate_idempotency_key(&request.idempotency_key)?;
        validate_object_id(&request.dependency_id, "dependency_id")?;
        validate_object_id(&request.predecessor_task_id, "predecessor_task_id")?;
        validate_object_id(&request.successor_task_id, "successor_task_id")?;
        if request.predecessor_task_id == request.successor_task_id {
            bail!("a task cannot depend on itself");
        }
        if request.lag_ms.unsigned_abs() > 31_536_000_000 {
            bail!("dependency lag must not exceed one year");
        }
        let profile_lock = self
            .document_lock(&planning_lock_key(&request.community_id))
            .await;
        let _profile_guard = profile_lock.lock().await;
        let request_hash = self.hash_request(&request)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let catalog = self
            .repository
            .planning_catalog(PLANNING_NAMESPACE, &request.community_id, MAX_PROFILE_ROWS)
            .await?;
        validate_catalog_bound(&catalog)?;
        if catalog.dependencies.len() == MAX_PROFILE_RECORDS
            && !catalog
                .dependencies
                .iter()
                .any(|dependency| dependency.dependency_id == request.dependency_id)
        {
            bail!("planning dependency limit reached");
        }
        for task_id in [&request.predecessor_task_id, &request.successor_task_id] {
            if !catalog.tasks.iter().any(|task| task.task_id == *task_id) {
                bail!("dependency references an unknown task");
            }
        }
        reject_dependency_cycle(&request, &catalog)?;
        let record = PlanningDependencyRecord {
            dependency_id: request.dependency_id,
            predecessor_task_id: request.predecessor_task_id,
            successor_task_id: request.successor_task_id,
            kind: request.kind,
            lag_ms: request.lag_ms,
        };
        let projection = PlanningDependency {
            dependency_id: record.dependency_id.clone(),
            predecessor_task_id: record.predecessor_task_id.clone(),
            successor_task_id: record.successor_task_id.clone(),
            kind: record.kind.clone(),
            lag_ms: record.lag_ms,
            submitted_by_app_id: app_id.into(),
            source_operation_hash: String::new(),
        };
        self.commit_planning_record(
            app_id,
            request.community_id,
            format!("planning/dependencies/{}", record.dependency_id),
            request.idempotency_key,
            request_hash,
            catalog.version,
            &record,
            PlanningProjection::Dependency(projection),
        )
        .await
    }

    pub(super) async fn planning_calendar_list(
        &self,
        app_id: &str,
        request: PlanningViewRequest,
    ) -> Result<Value> {
        self.require_profile_read(app_id, &request.community_id)
            .await?;
        validate_view(&request)?;
        let catalog = self
            .repository
            .planning_catalog(PLANNING_NAMESPACE, &request.community_id, request.limit)
            .await?;
        Ok(json!({
            "profile": planning_manifest()?,
            "events": catalog.events,
            "resolved_truth": false
        }))
    }

    pub(super) async fn planning_gantt_get(
        &self,
        app_id: &str,
        request: PlanningViewRequest,
    ) -> Result<Value> {
        self.require_profile_read(app_id, &request.community_id)
            .await?;
        validate_view(&request)?;
        let catalog = self
            .repository
            .planning_catalog(PLANNING_NAMESPACE, &request.community_id, request.limit)
            .await?;
        Ok(json!({
            "profile": planning_manifest()?,
            "projects": catalog.projects,
            "tasks": catalog.tasks,
            "dependencies": catalog.dependencies,
            "resolved_truth": false
        }))
    }

    async fn require_profile_read(&self, app_id: &str, community_id: &str) -> Result<()> {
        if !self
            .repository
            .profile_access(app_id, PLANNING_PROFILE_ID, community_id)
            .await?
            .can_read
        {
            bail!("application is not granted read access to community.planning");
        }
        Ok(())
    }

    async fn require_profile_write(&self, app_id: &str, community_id: &str) -> Result<()> {
        let access = self
            .repository
            .profile_access(app_id, PLANNING_PROFILE_ID, community_id)
            .await?;
        if !access.can_read || !access.can_write {
            bail!("application is not granted write access to community.planning");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_planning_record<T: Serialize>(
        &self,
        app_id: &str,
        community_id: String,
        document_id: String,
        idempotency_key: String,
        request_hash: [u8; 32],
        expected_community_version: u64,
        record: &T,
        projection: PlanningProjection,
    ) -> Result<Value> {
        let manifest = planning_manifest()?;
        let signed = SignedPlanningRecord {
            profile_id: PLANNING_PROFILE_ID,
            profile_version: PLANNING_PROFILE_VERSION,
            profile_digest: &manifest.profile_digest,
            submitted_by_app_id: app_id,
            record,
        };
        let canonical_json = serde_json::to_string(&signed)?;
        if canonical_json.len() > 65_536 {
            bail!("planning record exceeds 65536 bytes");
        }
        self.mutate_in_namespace(
            app_id,
            PLANNING_NAMESPACE,
            crate::domain::MutateDocument {
                community_id,
                document_id,
                idempotency_key,
                schema_version: PLANNING_PROFILE_VERSION,
                mutations: vec![
                    planning_map("canonical_json", canonical_json),
                    planning_map("profile_id", PLANNING_PROFILE_ID.into()),
                    planning_map("profile_digest", manifest.profile_digest.clone()),
                    planning_map("submitted_by_app_id", app_id.into()),
                    Mutation::MapSet {
                        container: "record".into(),
                        key: "profile_version".into(),
                        value: PrimitiveValue::Integer(i64::from(PLANNING_PROFILE_VERSION)),
                    },
                ],
            },
            request_hash,
            vec![ProjectionWrite::Planning(Box::new(
                PlanningProjectionWrite {
                    profile_digest: signed.profile_digest.into(),
                    authorized_app_id: app_id.into(),
                    profile_id: PLANNING_PROFILE_ID.into(),
                    expected_community_version,
                    projection,
                },
            ))],
        )
        .await
    }
}

fn planning_lock_key(community_id: &str) -> DocumentKey {
    DocumentKey {
        app_id: PLANNING_NAMESPACE.into(),
        community_id: community_id.into(),
        document_id: "planning/__profile_lock".into(),
    }
}

fn planning_map(key: &str, value: String) -> Mutation {
    Mutation::MapSet {
        container: "record".into(),
        key: key.into(),
        value: PrimitiveValue::String(value),
    }
}

pub(super) fn planning_manifest() -> Result<ProfileManifest> {
    let definition: Value = serde_json::from_str(include_str!(
        "../../protocol/profiles/community-planning-v1.json"
    ))?;
    Ok(ProfileManifest {
        profile_id: PLANNING_PROFILE_ID.into(),
        profile_version: PLANNING_PROFILE_VERSION,
        profile_digest: PLANNING_PROFILE_DIGEST.into(),
        data_namespace: PLANNING_NAMESPACE.into(),
        title: "Community Planning Interoperability Profile".into(),
        record_types: vec![
            "dependency".into(),
            "event".into(),
            "project".into(),
            "task".into(),
        ],
        extension_policy: definition["extension_policy"]
            .as_str()
            .unwrap_or_default()
            .into(),
        temporal_policy: definition["temporal_policy"].clone(),
        contract: definition,
        native: true,
    })
}

fn validate_profile_reference(profile_id: &str, version: Option<u32>) -> Result<()> {
    if profile_id != PLANNING_PROFILE_ID
        || version.is_some_and(|version| version != PLANNING_PROFILE_VERSION)
    {
        bail!("profile is not supported");
    }
    Ok(())
}

fn validate_common_write(
    community_id: &str,
    idempotency_key: &str,
    record_id: &str,
    record_field: &str,
    title: &str,
    description: Option<&str>,
) -> Result<()> {
    validate_object_id(community_id, "community_id")?;
    validate_idempotency_key(idempotency_key)?;
    validate_object_id(record_id, record_field)?;
    validate_text(title, "title", 512)?;
    validate_optional_text(description, "description", 8_192)
}

fn validate_text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        bail!("{field} must contain 1..={max} non-control bytes");
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, field: &str, max: usize) -> Result<()> {
    if let Some(value) = value {
        validate_text(value, field, max)?;
    }
    Ok(())
}

fn validate_optional_id(value: Option<&str>, field: &str) -> Result<()> {
    if let Some(value) = value {
        validate_object_id(value, field)?;
    }
    Ok(())
}

fn validate_timing(timing: &EventTiming) -> Result<()> {
    match timing {
        EventTiming::AllDay {
            start_date,
            end_date_exclusive,
        } => {
            validate_date(start_date)?;
            validate_date(end_date_exclusive)?;
            if end_date_exclusive <= start_date {
                bail!("all-day end_date_exclusive must follow start_date");
            }
        }
        EventTiming::Utc {
            start_at_ms,
            end_at_ms,
            time_zone,
        } => {
            if end_at_ms <= start_at_ms {
                bail!("timed event end_at_ms must follow start_at_ms");
            }
            if time_zone != "UTC"
                && (!time_zone.contains('/')
                    || time_zone.len() > 128
                    || !time_zone.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+')
                    }))
            {
                bail!("time_zone must be UTC or a bounded IANA-style identifier");
            }
        }
    }
    Ok(())
}

fn validate_date(value: &str) -> Result<()> {
    if !crate::domain::dates::is_valid_gregorian_date(value) {
        bail!("invalid date: expected a valid Gregorian YYYY-MM-DD date");
    }
    Ok(())
}

fn validate_recurrence(rule: &RecurrenceRule, timing: &EventTiming) -> Result<()> {
    if rule.interval == 0 || rule.interval > 365 {
        bail!("recurrence interval must be between 1 and 365");
    }
    if rule.count.is_some_and(|count| count == 0 || count > 10_000) {
        bail!("recurrence count must be between 1 and 10000");
    }
    if rule.count.is_some() && rule.until_ms.is_some() {
        bail!("recurrence may use count or until_ms, not both");
    }
    if rule.count.is_none() && rule.until_ms.is_none() {
        bail!("recurrence requires a bounded count or until_ms");
    }
    match (timing, rule.until_ms) {
        (EventTiming::AllDay { .. }, Some(_)) => {
            bail!("all-day recurrence must use count; date-based UNTIL is reserved for profile v2");
        }
        (EventTiming::Utc { start_at_ms, .. }, Some(until))
            if until < *start_at_ms || until.saturating_sub(*start_at_ms) > 315_360_000_000 =>
        {
            bail!("recurrence until_ms must be within ten years of its start");
        }
        _ => {}
    }
    if !rule.by_weekday.is_empty() && rule.frequency != RecurrenceFrequency::Weekly {
        bail!("by_weekday is supported only for weekly recurrence");
    }
    let distinct = rule.by_weekday.iter().collect::<HashSet<_>>();
    if distinct.len() != rule.by_weekday.len() || rule.by_weekday.len() > 7 {
        bail!("recurrence weekdays must be unique and bounded");
    }
    Ok(())
}

fn validate_catalog_bound(catalog: &PlanningCatalog) -> Result<()> {
    if [
        catalog.projects.len(),
        catalog.tasks.len(),
        catalog.events.len(),
        catalog.dependencies.len(),
    ]
    .into_iter()
    .any(|count| count > MAX_PROFILE_RECORDS)
    {
        bail!("planning catalog exceeds the validation bound");
    }
    Ok(())
}

fn require_project(project_id: Option<&str>, catalog: &PlanningCatalog) -> Result<()> {
    if let Some(project_id) = project_id
        && !catalog
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
    {
        bail!("project_id does not reference an existing project");
    }
    Ok(())
}

fn reject_dependency_cycle(request: &PutDependency, catalog: &PlanningCatalog) -> Result<()> {
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for dependency in &catalog.dependencies {
        if dependency.dependency_id != request.dependency_id {
            edges
                .entry(&dependency.predecessor_task_id)
                .or_default()
                .push(&dependency.successor_task_id);
        }
    }
    edges
        .entry(&request.predecessor_task_id)
        .or_default()
        .push(&request.successor_task_id);
    let mut pending = vec![request.successor_task_id.as_str()];
    let mut visited = HashSet::new();
    while let Some(task) = pending.pop() {
        if task == request.predecessor_task_id {
            bail!("dependency would create a cycle");
        }
        if visited.insert(task) {
            pending.extend(edges.get(task).into_iter().flatten().copied());
        }
    }
    Ok(())
}

fn validate_view(request: &PlanningViewRequest) -> Result<()> {
    validate_object_id(&request.community_id, "community_id")?;
    if request.limit == 0 || request.limit > 500 {
        bail!("limit must be between 1 and 500");
    }
    Ok(())
}
