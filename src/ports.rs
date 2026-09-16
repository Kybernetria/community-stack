//! Interfaces owned by the application layer.
//!
//! Ports are segregated by cohesive capability. The runtime injects one SQLite
//! adapter implementing the composite `Repository`, preserving one transaction
//! authority without creating a god interface in individual use cases.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::domain::{
    AckOutbox, CanonicalRecord, DoctorCheck, DocumentChange, DocumentKey, DocumentReplayInput,
    FactClaim, FactCurrentRevision, FactHistoryPage, FactHistoryRequest, FactInspection,
    FactSchema, ForgeDocumentUpdate, GetFactSchema, InspectFact, ListFactSchemas, LogHead,
    Mutation, OutboxItem, PlanningCatalog, Principal, ProfileAccess, ProjectionCheck,
    ProjectionWrite, QueryFacts, RegisterFactSchema, ToolkitCatalog, ToolkitConcept,
};

pub struct LocalCommit {
    pub expected_update_count: u64,
    pub idempotency_app_id: String,
    pub record: CanonicalRecord,
    pub document: DocumentKey,
    pub update_bytes: Vec<u8>,
    pub projections: Vec<ProjectionWrite>,
    pub request_hash: [u8; 32],
    pub idempotency_key: String,
    pub response: Value,
}

#[async_trait]
pub trait PrincipalRepository: Send + Sync {
    async fn authenticate(&self, token_hash: [u8; 32]) -> Result<Option<Principal>>;
    async fn principal_by_id(&self, principal_id: &str) -> Result<Option<Principal>>;
}

#[async_trait]
pub trait DocumentRepository: Send + Sync {
    async fn list_documents(
        &self,
        app_id: &str,
        request: &crate::domain::ListDocuments,
    ) -> Result<crate::domain::DocumentPage>;
    async fn document_changes(
        &self,
        app_id: &str,
        request: &crate::domain::DocumentChanges,
    ) -> Result<crate::domain::DocumentChangesPage>;
    async fn idempotency(
        &self,
        app_id: &str,
        key: &str,
        request_hash: [u8; 32],
    ) -> Result<Option<Value>>;
    async fn load_document_updates(&self, key: &DocumentKey) -> Result<Vec<Vec<u8>>>;
    async fn log_head(&self, author: [u8; 32], log_id: [u8; 32]) -> Result<Option<LogHead>>;
    async fn commit_local(&self, commit: LocalCommit) -> Result<()>;
}

#[async_trait]
pub trait FactRepository: Send + Sync {
    async fn query_facts(&self, app_id: &str, query: &QueryFacts) -> Result<Vec<FactClaim>>;
    async fn current_fact_revision(
        &self,
        app_id: &str,
        community_id: &str,
        claim_id: &str,
    ) -> Result<Option<FactCurrentRevision>>;
    async fn active_fact_conflict(
        &self,
        app_id: &str,
        community_id: &str,
        claim_id: &str,
        subject: &str,
        predicate: &str,
    ) -> Result<bool>;
}

#[async_trait]
pub trait FactSchemaRepository: Send + Sync {
    async fn register_fact_schema(&self, schema: &RegisterFactSchema) -> Result<FactSchema>;
    async fn get_fact_schema(&self, request: &GetFactSchema) -> Result<Option<FactSchema>>;
    async fn list_fact_schemas(&self, request: &ListFactSchemas) -> Result<Vec<FactSchema>>;
}

#[async_trait]
pub trait FactInspectionRepository: Send + Sync {
    async fn inspect_fact(
        &self,
        app_id: &str,
        request: &InspectFact,
    ) -> Result<Option<FactInspection>>;
    async fn fact_history(
        &self,
        app_id: &str,
        request: &FactHistoryRequest,
    ) -> Result<FactHistoryPage>;
}

#[async_trait]
pub trait ProfileRepository: Send + Sync {
    async fn profile_access(
        &self,
        app_id: &str,
        profile_id: &str,
        community_id: &str,
    ) -> Result<ProfileAccess>;
    async fn grant_profile(
        &self,
        app_id: &str,
        profile_id: &str,
        community_id: &str,
        access: ProfileAccess,
    ) -> Result<()>;
}

#[async_trait]
pub trait PlanningRepository: Send + Sync {
    async fn planning_catalog(
        &self,
        namespace_id: &str,
        community_id: &str,
        limit: u16,
    ) -> Result<PlanningCatalog>;
}

#[async_trait]
pub trait ToolkitRepository: Send + Sync {
    async fn toolkit_catalog(&self, app_id: &str, community_id: &str) -> Result<ToolkitCatalog>;
    async fn toolkit_concepts(
        &self,
        app_id: &str,
        community_id: &str,
        active_only: bool,
        limit: u16,
    ) -> Result<Vec<ToolkitConcept>>;
    async fn toolkit_export(
        &self,
        app_id: &str,
        community_id: &str,
        offset: u32,
        limit: u16,
    ) -> Result<ToolkitCatalog>;
}

#[async_trait]
pub trait OutboxRepository: Send + Sync {
    async fn claim_outbox(
        &self,
        transport: &str,
        limit: u16,
        max_bytes: u32,
    ) -> Result<Vec<OutboxItem>>;
    async fn ack_outbox(&self, ack: &AckOutbox) -> Result<()>;
}

#[async_trait]
pub trait SystemRepository: Send + Sync {
    async fn health(&self) -> Result<Value>;
    async fn doctor_storage(&self) -> Result<Vec<DoctorCheck>>;
    async fn replay_inputs(&self, limit: u16) -> Result<Vec<DocumentReplayInput>>;
    async fn projection_check(&self) -> Result<ProjectionCheck>;
}

pub trait Repository:
    PrincipalRepository
    + DocumentRepository
    + FactRepository
    + FactSchemaRepository
    + FactInspectionRepository
    + ProfileRepository
    + PlanningRepository
    + ToolkitRepository
    + OutboxRepository
    + SystemRepository
{
}

impl<T> Repository for T where
    T: PrincipalRepository
        + DocumentRepository
        + FactRepository
        + FactSchemaRepository
        + FactInspectionRepository
        + ProfileRepository
        + PlanningRepository
        + ToolkitRepository
        + OutboxRepository
        + SystemRepository
{
}

pub trait DocumentEngine: Send + Sync {
    fn read_state(&self, updates: &[Vec<u8>], writer_peer_id: u64) -> Result<Value>;
    fn stage_mutations(
        &self,
        updates: &[Vec<u8>],
        writer_peer_id: u64,
        mutations: &[Mutation],
    ) -> Result<DocumentChange>;
}

pub trait ContentHasher: Send + Sync {
    fn hash(&self, bytes: &[u8]) -> [u8; 32];
}

pub trait SecureLog: Send + Sync {
    fn author_key(&self) -> [u8; 32];
    fn document_log_id(&self, key: &DocumentKey) -> [u8; 32];
    fn forge_document_update(&self, input: ForgeDocumentUpdate<'_>) -> Result<CanonicalRecord>;
}
