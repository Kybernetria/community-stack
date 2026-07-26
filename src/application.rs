//! Use-case orchestration.
//!
//! This is the only layer allowed to coordinate documents, secure logs,
//! persistence, projections, and outboxes. It depends exclusively on ports.

mod planning;
mod toolkit;

use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    domain::{
        AckOutbox, AssertFact, CheckStatus, ClaimOutbox, DoctorCheck, DoctorReport, DocumentKey,
        FactLifecycleStatus, FactRevisionProjection, FactSchema, ForgeDocumentUpdate, GetDocument,
        GetFactSchema, InspectFact, ListFactSchemas, MAX_FACT_OBJECT_BYTES, MutateDocument,
        Mutation, PrimitiveValue, Principal, PrincipalRole, ProjectionWrite, ProvenancePolicy,
        QueryFacts, RegisterFactSchema,
    },
    ports::{ContentHasher, DocumentEngine, LocalCommit, Repository, SecureLog},
};

#[derive(Clone)]
pub struct CommunityCore {
    repository: Arc<dyn Repository>,
    documents: Arc<dyn DocumentEngine>,
    secure_log: Arc<dyn SecureLog>,
    content_hasher: Arc<dyn ContentHasher>,
    writer_peer_id: u64,
    document_locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl CommunityCore {
    pub fn new(
        repository: Arc<dyn Repository>,
        documents: Arc<dyn DocumentEngine>,
        secure_log: Arc<dyn SecureLog>,
        content_hasher: Arc<dyn ContentHasher>,
        writer_peer_id: u64,
    ) -> Self {
        Self {
            repository,
            documents,
            secure_log,
            content_hasher,
            writer_peer_id,
            document_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn authenticate(&self, token: &str) -> Result<Option<Principal>> {
        let bytes = hex::decode(token).context("token must be hexadecimal")?;
        if bytes.len() != 32 {
            bail!("token must contain 32 bytes");
        }
        self.repository
            .authenticate(self.content_hasher.hash(&bytes))
            .await
    }

    pub async fn call_public(&self, method: &str) -> Result<Value> {
        match method {
            "health" => {
                let mut health = self.repository.health().await?;
                health["author_key"] = json!(hex::encode(self.secure_log.author_key()));
                Ok(health)
            }
            _ => bail!("unknown public method"),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn call_authenticated(
        &self,
        principal: &Principal,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        if principal.role == PrincipalRole::App && principal.id == crate::domain::PLANNING_NAMESPACE
        {
            bail!("APP principal uses a reserved code-owned data namespace");
        }
        match (&principal.role, method) {
            (PrincipalRole::App, "document.mutate") => {
                let request: MutateDocument = decode(params)?;
                let request_hash = self.hash_request(&request)?;
                self.mutate(&principal.id, request, request_hash, Vec::new())
                    .await
            }
            (PrincipalRole::App, "document.get") => self.get(&principal.id, decode(params)?).await,
            (PrincipalRole::App, "fact.assert") => {
                self.assert_fact(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "fact.query") => {
                self.query_facts(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "fact.inspect") => {
                self.inspect_fact(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "fact.history") => {
                self.fact_history(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App | PrincipalRole::Admin, "profile.list") => Self::profile_list(),
            (PrincipalRole::App | PrincipalRole::Admin, "profile.get") => {
                self.profile_get(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "planning.project.put") => {
                self.planning_project_put(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "planning.task.put") => {
                self.planning_task_put(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "planning.event.put") => {
                self.planning_event_put(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "planning.dependency.put") => {
                self.planning_dependency_put(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "planning.calendar.list") => {
                self.planning_calendar_list(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "planning.gantt.get") => {
                self.planning_gantt_get(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::Admin, "profile.grant") => self.profile_grant(decode(params)?).await,
            (PrincipalRole::Admin, "schema.register") => {
                self.register_schema(decode(params)?).await
            }
            (PrincipalRole::Admin, "schema.get") => self.get_schema(decode(params)?).await,
            (PrincipalRole::Admin, "schema.list") => self.list_schemas(decode(params)?).await,
            (PrincipalRole::Admin, "system.doctor") => self.doctor().await,
            (PrincipalRole::Admin, "projection.check") => Ok(serde_json::to_value(
                self.repository.projection_check().await?,
            )?),
            (PrincipalRole::Admin, "projection.rebuild") => {
                let check = self.repository.projection_check().await?;
                Ok(json!({
                    "enabled": false,
                    "applied": false,
                    "reason": "safe canonical replay and staging validation are not yet enabled",
                    "check": check
                }))
            }
            (PrincipalRole::App, "toolkit.schema.list") => {
                self.toolkit_schema_list(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "toolkit.schema.show") => {
                self.toolkit_schema_show(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "toolkit.schema.add") => {
                self.toolkit_schema_add(&principal.id, decode(params)?)
                    .await
            }
            (PrincipalRole::App, "toolkit.tool.add") => {
                self.toolkit_tool_add(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "toolkit.assert") => {
                self.toolkit_assert(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "toolkit.verify") => {
                self.toolkit_verify(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "toolkit.query") => {
                self.toolkit_query(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "toolkit.explain") => {
                self.toolkit_explain(&principal.id, decode(params)?).await
            }
            (PrincipalRole::App, "toolkit.export") => {
                self.toolkit_export(&principal.id, decode(params)?).await
            }
            (PrincipalRole::Transport, "outbox.claim") => {
                let request: ClaimOutbox = decode(params)?;
                if request.transport != principal.id {
                    bail!("transport token cannot claim for another adapter");
                }
                Ok(serde_json::to_value(
                    self.repository
                        .claim_outbox(&request.transport, request.limit, request.max_bytes)
                        .await?,
                )?)
            }
            (PrincipalRole::Transport, "outbox.ack") => {
                let request: AckOutbox = decode(params)?;
                if request.transport != principal.id {
                    bail!("transport token cannot ACK for another adapter");
                }
                self.repository.ack_outbox(&request).await?;
                Ok(json!({"durable": true}))
            }
            _ => bail!("method is unknown or not allowed for this principal"),
        }
    }

    async fn get(&self, app_id: &str, request: GetDocument) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_object_id(&request.document_id, "document_id")?;
        let key = DocumentKey {
            app_id: app_id.to_owned(),
            community_id: request.community_id,
            document_id: request.document_id,
        };
        let lock = self.document_lock(&key).await;
        let _guard = lock.lock().await;
        let updates = self.repository.load_document_updates(&key).await?;
        let state = self.documents.read_state(&updates, self.writer_peer_id)?;
        Ok(json!({"document": key, "state": state, "update_count": updates.len()}))
    }

    #[allow(clippy::too_many_lines)]
    async fn assert_fact(&self, app_id: &str, fact: AssertFact) -> Result<Value> {
        validate_object_id(&fact.community_id, "community_id")?;
        validate_object_id(&fact.claim_id, "claim_id")?;
        validate_fact_term(&fact.subject, "subject")?;
        validate_fact_term(&fact.predicate, "predicate")?;
        validate_object_id(&fact.schema_id, "schema_id")?;
        validate_idempotency_key(&fact.idempotency_key)?;
        let request_hash = self.hash_request(&fact)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &fact.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        if !fact.confidence.is_finite() || !(0.0..=1.0).contains(&fact.confidence) {
            bail!("confidence must be a finite value between 0 and 1");
        }
        validate_optional_term(fact.source.as_deref(), "source", 2_048)?;
        validate_optional_term(
            fact.source_document_id.as_deref(),
            "source_document_id",
            1_024,
        )?;
        let object_json = serde_json::to_vec(&fact.object)?;
        if object_json.len() > MAX_FACT_OBJECT_BYTES {
            bail!("fact object exceeds {MAX_FACT_OBJECT_BYTES} bytes");
        }
        let schema = self
            .repository
            .get_fact_schema(&GetFactSchema {
                app_id: app_id.to_owned(),
                schema_id: fact.schema_id.clone(),
                schema_version: Some(fact.schema_version),
            })
            .await?
            .context("fact schema is not registered")?;
        if !schema.enabled {
            bail!("fact schema is disabled");
        }
        validate_fact_against_schema(&fact, &schema)?;
        let lifecycle_status = FactLifecycleStatus::from(fact.lifecycle_status);

        let key = DocumentKey {
            app_id: app_id.to_owned(),
            community_id: fact.community_id.clone(),
            document_id: format!("facts/{}", fact.claim_id),
        };
        let lock = self.document_lock(&key).await;
        let _guard = lock.lock().await;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &fact.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let previous = self
            .repository
            .current_fact_revision(app_id, &fact.community_id, &fact.claim_id)
            .await?;
        if previous.as_ref().is_some_and(|revision| {
            revision.subject != fact.subject
                || revision.predicate != fact.predicate
                || revision.schema_id != fact.schema_id
                || revision.schema_version != fact.schema_version
        }) {
            bail!("claim subject, predicate, and schema are immutable across revisions");
        }
        validate_lifecycle_transition(previous.as_ref(), lifecycle_status, &schema)?;
        if lifecycle_status == FactLifecycleStatus::Asserted
            && !schema.multiple_active_claims
            && self
                .repository
                .active_fact_conflict(
                    app_id,
                    &fact.community_id,
                    &fact.claim_id,
                    &fact.subject,
                    &fact.predicate,
                )
                .await?
        {
            bail!("schema permits only one active claim for this subject and predicate");
        }

        let source_value = fact.source.clone().unwrap_or_default();
        let source_document_value = fact.source_document_id.clone().unwrap_or_default();
        let request = MutateDocument {
            community_id: fact.community_id.clone(),
            document_id: key.document_id.clone(),
            idempotency_key: fact.idempotency_key.clone(),
            schema_version: fact.schema_version,
            mutations: vec![
                map_string("claim_id", fact.claim_id.clone()),
                map_string("subject", fact.subject.clone()),
                map_string("predicate", fact.predicate.clone()),
                map_string("object_json", String::from_utf8(object_json.clone())?),
                map_string("source", source_value),
                map_string("source_document_id", source_document_value),
                map_string("schema_id", fact.schema_id.clone()),
                map_string("lifecycle_status", lifecycle_status.as_str().into()),
                Mutation::MapSet {
                    container: "claim".into(),
                    key: "schema_version".into(),
                    value: PrimitiveValue::Integer(i64::from(fact.schema_version)),
                },
                Mutation::MapSet {
                    container: "claim".into(),
                    key: "confidence".into(),
                    value: PrimitiveValue::Float(fact.confidence),
                },
            ],
        };
        let projection = ProjectionWrite::FactRevision(Box::new(FactRevisionProjection {
            claim_id: fact.claim_id,
            community_id: fact.community_id,
            subject: fact.subject,
            predicate: fact.predicate,
            object_json,
            source: fact.source,
            confidence: fact.confidence,
            schema_id: fact.schema_id,
            schema_version: fact.schema_version,
            lifecycle_status,
            source_document_id: fact.source_document_id,
            multiple_active_claims: schema.multiple_active_claims,
            expected_previous_operation_hash: previous.map(|revision| revision.operation_hash),
        }));
        self.mutate_locked(app_id, key, request, request_hash, vec![projection])
            .await
    }

    async fn query_facts(&self, app_id: &str, query: QueryFacts) -> Result<Value> {
        validate_object_id(&query.community_id, "community_id")?;
        if let Some(subject) = &query.subject {
            validate_fact_term(subject, "subject")?;
        }
        if let Some(predicate) = &query.predicate {
            validate_fact_term(predicate, "predicate")?;
        }
        Ok(serde_json::to_value(
            self.repository.query_facts(app_id, &query).await?,
        )?)
    }

    async fn inspect_fact(&self, app_id: &str, request: InspectFact) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_object_id(&request.claim_id, "claim_id")?;
        let inspection = self
            .repository
            .inspect_fact(app_id, &request)
            .await?
            .context("fact claim was not found")?;
        Ok(serde_json::to_value(inspection)?)
    }

    async fn fact_history(
        &self,
        app_id: &str,
        request: crate::domain::FactHistoryRequest,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_object_id(&request.claim_id, "claim_id")?;
        if request.limit == 0 || request.limit > crate::domain::MAX_FACT_HISTORY_PAGE {
            bail!(
                "history limit must be between 1 and {}",
                crate::domain::MAX_FACT_HISTORY_PAGE
            );
        }
        if request
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() > 96)
        {
            bail!("history cursor exceeds 96 bytes");
        }
        Ok(serde_json::to_value(
            self.repository.fact_history(app_id, &request).await?,
        )?)
    }

    async fn register_schema(&self, request: RegisterFactSchema) -> Result<Value> {
        validate_schema_registration(&request)?;
        Ok(serde_json::to_value(
            self.repository.register_fact_schema(&request).await?,
        )?)
    }

    async fn get_schema(&self, request: GetFactSchema) -> Result<Value> {
        validate_object_id(&request.app_id, "app_id")?;
        validate_object_id(&request.schema_id, "schema_id")?;
        Ok(serde_json::to_value(
            self.repository
                .get_fact_schema(&request)
                .await?
                .context("fact schema was not found")?,
        )?)
    }

    async fn list_schemas(&self, request: ListFactSchemas) -> Result<Value> {
        validate_object_id(&request.app_id, "app_id")?;
        if request.limit == 0 || request.limit > 500 {
            bail!("schema list limit must be between 1 and 500");
        }
        Ok(serde_json::to_value(
            self.repository.list_fact_schemas(&request).await?,
        )?)
    }

    async fn doctor(&self) -> Result<Value> {
        let mut checks = self.repository.doctor_storage().await?;
        let replay_inputs = self.repository.replay_inputs(101).await?;
        let mut failures = Vec::new();
        for input in replay_inputs.iter().take(100) {
            if self
                .documents
                .read_state(&input.updates, self.writer_peer_id)
                .is_err()
            {
                failures.push(input.identifier.clone());
                if failures.len() == 10 {
                    break;
                }
            }
        }
        let truncated = replay_inputs.len() > 100;
        checks.push(DoctorCheck {
            check_name: "applied_loro_replay".into(),
            status: if failures.is_empty() && !truncated {
                CheckStatus::Ok
            } else if failures.is_empty() {
                CheckStatus::Warning
            } else {
                CheckStatus::Error
            },
            count: u32::try_from(failures.len())?,
            examples: failures,
            repairable: false,
        });
        Ok(serde_json::to_value(DoctorReport {
            mode: "READ_ONLY".into(),
            checks,
        })?)
    }

    pub(super) async fn mutate(
        &self,
        app_id: &str,
        request: MutateDocument,
        request_hash: [u8; 32],
        projections: Vec<ProjectionWrite>,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_object_id(&request.document_id, "document_id")?;
        validate_idempotency_key(&request.idempotency_key)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        self.mutate_in_namespace(app_id, app_id, request, request_hash, projections)
            .await
    }

    pub(super) async fn mutate_in_namespace(
        &self,
        caller_app_id: &str,
        namespace_id: &str,
        request: MutateDocument,
        request_hash: [u8; 32],
        projections: Vec<ProjectionWrite>,
    ) -> Result<Value> {
        let key = DocumentKey {
            app_id: namespace_id.to_owned(),
            community_id: request.community_id.clone(),
            document_id: request.document_id.clone(),
        };
        let lock = self.document_lock(&key).await;
        let _guard = lock.lock().await;
        self.mutate_locked(caller_app_id, key, request, request_hash, projections)
            .await
    }

    async fn mutate_locked(
        &self,
        caller_app_id: &str,
        key: DocumentKey,
        request: MutateDocument,
        request_hash: [u8; 32],
        projections: Vec<ProjectionWrite>,
    ) -> Result<Value> {
        if let Some(response) = self
            .repository
            .idempotency(caller_app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let updates = self.repository.load_document_updates(&key).await?;
        let change =
            self.documents
                .stage_mutations(&updates, self.writer_peer_id, &request.mutations)?;
        let author = self.secure_log.author_key();
        let log_id = self.secure_log.document_log_id(&key);
        let head = self.repository.log_head(author, log_id).await?;
        let (sequence, backlink) = match head {
            Some(head) => (
                head.sequence
                    .checked_add(1)
                    .context("secure log sequence space exhausted")?,
                Some(head.operation_hash),
            ),
            None => (0, None),
        };
        let record = self.secure_log.forge_document_update(ForgeDocumentUpdate {
            key: &key,
            sequence,
            backlink,
            schema_version: request.schema_version,
            key_epoch: 1,
            auth_frontier: Vec::new(),
            loro_update: &change.update_bytes,
            semantic_transaction: &request.idempotency_key,
        })?;
        let response = json!({
            "operation_hash": hex::encode(record.operation_hash),
            "status": "APPLIED",
            "durable": true,
            "state": change.state,
        });
        self.repository
            .commit_local(LocalCommit {
                idempotency_app_id: caller_app_id.to_owned(),
                record,
                document: key,
                update_bytes: change.update_bytes,
                projections,
                request_hash,
                idempotency_key: request.idempotency_key,
                response: response.clone(),
            })
            .await?;
        Ok(response)
    }

    pub(super) async fn document_lock(&self, key: &DocumentKey) -> Arc<Mutex<()>> {
        let id = format!("{}\0{}\0{}", key.app_id, key.community_id, key.document_id);
        let mut locks = self.document_locks.lock().await;
        if let Some(lock) = locks.get(&id).and_then(Weak::upgrade) {
            return lock;
        }
        if locks.len() >= 4_096 {
            locks.retain(|_, lock| lock.strong_count() > 0);
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(id, Arc::downgrade(&lock));
        lock
    }
}

fn map_string(key: &str, value: String) -> Mutation {
    Mutation::MapSet {
        container: "claim".into(),
        key: key.into(),
        value: PrimitiveValue::String(value),
    }
}

impl CommunityCore {
    pub(super) fn hash_request<T: serde::Serialize>(&self, value: &T) -> Result<[u8; 32]> {
        Ok(self.content_hasher.hash(&serde_json::to_vec(value)?))
    }

    pub(super) fn hash_content(&self, bytes: &[u8]) -> [u8; 32] {
        self.content_hasher.hash(bytes)
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).context("invalid method parameters")
}

pub(super) fn validate_object_id(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 {
        bail!("{field} must contain 1..=256 bytes");
    }
    if value.chars().any(char::is_control) {
        bail!("{field} contains a control character");
    }
    Ok(())
}

fn validate_fact_term(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 1_024 {
        bail!("{field} must contain 1..=1024 bytes");
    }
    if value.chars().any(char::is_control) {
        bail!("{field} contains a control character");
    }
    Ok(())
}

pub(super) fn validate_idempotency_key(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        bail!("idempotency_key must contain 1..=128 bytes");
    }
    Ok(())
}

fn validate_optional_term(value: Option<&str>, field: &str, max: usize) -> Result<()> {
    if let Some(value) = value
        && (value.is_empty() || value.len() > max || value.chars().any(char::is_control))
    {
        bail!("{field} must contain 1..={max} non-control bytes when supplied");
    }
    Ok(())
}

fn validate_schema_registration(schema: &RegisterFactSchema) -> Result<()> {
    validate_object_id(&schema.app_id, "app_id")?;
    validate_object_id(&schema.schema_id, "schema_id")?;
    validate_fact_term(&schema.predicate, "predicate")?;
    if schema.schema_version == 0 {
        bail!("schema_version must be greater than zero");
    }
    if !schema.schema_id.starts_with(&format!("{}/", schema.app_id)) {
        bail!("schema_id must be namespaced by app_id");
    }
    if schema.predicate != format!("{}@{}", schema.schema_id, schema.schema_version) {
        bail!("predicate must equal <schema_id>@<schema_version>");
    }
    let encoded = serde_json::to_vec(&schema.object_schema)?;
    if encoded.len() > MAX_FACT_OBJECT_BYTES {
        bail!("object schema exceeds {MAX_FACT_OBJECT_BYTES} bytes");
    }
    let mut nodes = 0_u16;
    validate_schema_node(&schema.object_schema, 0, true, &mut nodes)
}

#[allow(clippy::too_many_lines)]
fn validate_schema_node(schema: &Value, depth: u8, root: bool, nodes: &mut u16) -> Result<()> {
    if depth > 4 {
        bail!("object schema nesting exceeds four levels");
    }
    *nodes = nodes.checked_add(1).context("object schema is too large")?;
    if *nodes > 256 {
        bail!("object schema exceeds 256 nodes");
    }
    let object = schema
        .as_object()
        .context("every schema node must be a JSON object")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .context("every schema node requires one string type")?;
    if root && kind != "object" {
        bail!("fact object schema root type must be object");
    }
    let common = ["type", "description"];
    let allowed: &[&str] = match kind {
        "object" => &[
            "type",
            "description",
            "properties",
            "required",
            "additionalProperties",
        ],
        "array" => &["type", "description", "items", "maxItems"],
        "string" => &["type", "description", "maxLength", "enum"],
        "integer" | "number" => &["type", "description", "minimum", "maximum"],
        "boolean" | "null" => &common,
        _ => bail!("unsupported JSON schema type {kind}"),
    };
    if let Some(keyword) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("unsupported JSON schema keyword {keyword}");
    }
    match kind {
        "object" => {
            if object.get("additionalProperties") != Some(&Value::Bool(false)) {
                bail!("object schemas must set additionalProperties to false");
            }
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .context("object schemas require a properties object")?;
            if properties.len() > 64 {
                bail!("object schema has more than 64 properties");
            }
            let required = object
                .get("required")
                .and_then(Value::as_array)
                .context("object schemas require a required array")?;
            if required.len() > properties.len()
                || required.iter().any(|name| {
                    name.as_str()
                        .is_none_or(|name| !properties.contains_key(name))
                })
            {
                bail!("required must contain only declared property names");
            }
            for child in properties.values() {
                validate_schema_node(child, depth + 1, false, nodes)?;
            }
        }
        "array" => {
            let max = object
                .get("maxItems")
                .and_then(Value::as_u64)
                .context("array schemas require maxItems")?;
            if max > 128 {
                bail!("array maxItems exceeds 128");
            }
            validate_schema_node(
                object.get("items").context("array schemas require items")?,
                depth + 1,
                false,
                nodes,
            )?;
        }
        "string" => {
            let max = object
                .get("maxLength")
                .and_then(Value::as_u64)
                .context("string schemas require maxLength")?;
            if max > 4_096 {
                bail!("string maxLength exceeds 4096");
            }
            if let Some(values) = object.get("enum") {
                let values = values.as_array().context("enum must be an array")?;
                if values.is_empty()
                    || values.len() > 128
                    || values.iter().any(|value| value.as_str().is_none())
                {
                    bail!("string enum must contain 1..=128 strings");
                }
            }
        }
        "integer" | "number" => {
            for bound in ["minimum", "maximum"] {
                if object
                    .get(bound)
                    .is_some_and(|value| value.as_f64().is_none_or(|number| !number.is_finite()))
                {
                    bail!("numeric schema bounds must be finite numbers");
                }
            }
        }
        "boolean" | "null" => {}
        _ => unreachable!(),
    }
    Ok(())
}

fn validate_fact_against_schema(fact: &AssertFact, schema: &FactSchema) -> Result<()> {
    if fact.predicate != schema.predicate
        || fact.schema_id != schema.schema_id
        || fact.schema_version != schema.schema_version
    {
        bail!("predicate and schema reference do not match the registered schema");
    }
    match schema.provenance_policy {
        ProvenancePolicy::Source if fact.source.is_none() => {
            bail!("schema requires source provenance")
        }
        ProvenancePolicy::SourceDocument if fact.source_document_id.is_none() => {
            bail!("schema requires source_document_id provenance");
        }
        ProvenancePolicy::SourceAndDocument
            if fact.source.is_none() || fact.source_document_id.is_none() =>
        {
            bail!("schema requires source and source_document_id provenance");
        }
        ProvenancePolicy::None
        | ProvenancePolicy::Source
        | ProvenancePolicy::SourceDocument
        | ProvenancePolicy::SourceAndDocument => {}
    }
    validate_json_value(&fact.object, &schema.object_schema, "$", 0)
}

#[allow(clippy::too_many_lines)]
fn validate_json_value(value: &Value, schema: &Value, path: &str, depth: u8) -> Result<()> {
    if depth > 4 {
        bail!("object at {path} exceeds schema depth");
    }
    let schema = schema
        .as_object()
        .context("stored object schema is invalid")?;
    let kind = schema["type"]
        .as_str()
        .context("stored object schema type is invalid")?;
    match kind {
        "object" => {
            let value = value
                .as_object()
                .with_context(|| format!("{path} must be an object"))?;
            let properties = schema["properties"]
                .as_object()
                .context("stored properties are invalid")?;
            if let Some(unknown) = value.keys().find(|key| !properties.contains_key(*key)) {
                bail!("{path}.{unknown} is not allowed by the schema");
            }
            for required in schema["required"]
                .as_array()
                .context("stored required list is invalid")?
            {
                let required = required
                    .as_str()
                    .context("stored required name is invalid")?;
                if !value.contains_key(required) {
                    bail!("{path}.{required} is required");
                }
            }
            for (name, child) in value {
                validate_json_value(
                    child,
                    &properties[name],
                    &format!("{path}.{name}"),
                    depth + 1,
                )?;
            }
        }
        "array" => {
            let value = value
                .as_array()
                .with_context(|| format!("{path} must be an array"))?;
            let max = schema["maxItems"]
                .as_u64()
                .context("stored maxItems is invalid")?;
            if u64::try_from(value.len())? > max {
                bail!("{path} exceeds maxItems");
            }
            for (index, child) in value.iter().enumerate() {
                validate_json_value(
                    child,
                    &schema["items"],
                    &format!("{path}[{index}]"),
                    depth + 1,
                )?;
            }
        }
        "string" => {
            let value = value
                .as_str()
                .with_context(|| format!("{path} must be a string"))?;
            if u64::try_from(value.chars().count())?
                > schema["maxLength"]
                    .as_u64()
                    .context("stored maxLength is invalid")?
            {
                bail!("{path} exceeds maxLength");
            }
            if let Some(values) = schema.get("enum").and_then(Value::as_array)
                && !values
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(value))
            {
                bail!("{path} is not an allowed enum value");
            }
        }
        "integer" if value.as_i64().is_none() && value.as_u64().is_none() => {
            bail!("{path} must be an integer");
        }
        "number" if value.as_f64().is_none_or(|number| !number.is_finite()) => {
            bail!("{path} must be a finite number");
        }
        "boolean" if !value.is_boolean() => bail!("{path} must be a boolean"),
        "null" if !value.is_null() => bail!("{path} must be null"),
        "integer" | "number" => {
            let number = value.as_f64().context("numeric value is out of range")?;
            if schema
                .get("minimum")
                .and_then(Value::as_f64)
                .is_some_and(|min| number < min)
                || schema
                    .get("maximum")
                    .and_then(Value::as_f64)
                    .is_some_and(|max| number > max)
            {
                bail!("{path} is outside the allowed numeric range");
            }
        }
        _ => bail!("stored object schema contains an unsupported type"),
    }
    Ok(())
}

fn validate_lifecycle_transition(
    previous: Option<&crate::domain::FactCurrentRevision>,
    next: FactLifecycleStatus,
    schema: &FactSchema,
) -> Result<()> {
    if previous.is_none() && next != FactLifecycleStatus::Asserted {
        bail!("the first claim revision must be ASSERTED");
    }
    if previous.is_some_and(|revision| {
        matches!(
            revision.lifecycle_status,
            FactLifecycleStatus::Retracted | FactLifecycleStatus::Expired
        )
    }) {
        bail!("retracted or expired claims are terminal; create a new claim_id");
    }
    if previous.is_some()
        && next == FactLifecycleStatus::Asserted
        && !schema.lifecycle_policy.allow_supersession
    {
        bail!("schema lifecycle policy forbids supersession");
    }
    if next == FactLifecycleStatus::Retracted && !schema.lifecycle_policy.allow_retraction {
        bail!("schema lifecycle policy forbids retraction");
    }
    if next == FactLifecycleStatus::Disputed && !schema.lifecycle_policy.allow_dispute {
        bail!("schema lifecycle policy forbids disputes");
    }
    if next == FactLifecycleStatus::Expired && !schema.lifecycle_policy.allow_expiry {
        bail!("schema lifecycle policy forbids expiry");
    }
    Ok(())
}
