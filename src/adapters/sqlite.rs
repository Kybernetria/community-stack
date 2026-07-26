mod facts;
mod planning;
mod toolkit;

use std::{
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

use crate::{
    domain::{
        AckOutbox, DeliveryAck, DoctorCheck, DocumentKey, DocumentReplayInput, FactClaim,
        FactCurrentRevision, FactHistoryPage, FactHistoryRequest, FactInspection, FactSchema,
        GetFactSchema, InspectFact, ListFactSchemas, LogHead, OutboxItem, PlanningCatalog,
        Principal, PrincipalRole, ProfileAccess, ProjectionCheck, ProjectionWrite, QueryFacts,
        RegisterFactSchema, ToolkitCatalog, ToolkitConcept,
    },
    ports::{
        DocumentRepository, FactInspectionRepository, FactRepository, FactSchemaRepository,
        LocalCommit, OutboxRepository, PlanningRepository, PrincipalRepository, ProfileRepository,
        SystemRepository, ToolkitRepository,
    },
};

#[derive(Clone)]
pub struct StoreHandle {
    sender: mpsc::Sender<Request>,
}

enum Request {
    Authenticate {
        token_hash: [u8; 32],
        reply: Reply<Option<Principal>>,
    },
    PrincipalById {
        principal_id: String,
        reply: Reply<Option<Principal>>,
    },
    Idempotency {
        app_id: String,
        key: String,
        request_hash: [u8; 32],
        reply: Reply<Option<Value>>,
    },
    LoadUpdates {
        key: DocumentKey,
        reply: Reply<Vec<Vec<u8>>>,
    },
    LogHead {
        author: [u8; 32],
        log_id: [u8; 32],
        reply: Reply<Option<LogHead>>,
    },
    CommitLocal {
        commit: Box<LocalCommit>,
        reply: Reply<()>,
    },
    QueryFacts {
        app_id: String,
        query: QueryFacts,
        reply: Reply<Vec<FactClaim>>,
    },
    CurrentFactRevision {
        app_id: String,
        community_id: String,
        claim_id: String,
        reply: Reply<Option<FactCurrentRevision>>,
    },
    ActiveFactConflict {
        app_id: String,
        community_id: String,
        claim_id: String,
        subject: String,
        predicate: String,
        reply: Reply<bool>,
    },
    RegisterFactSchema {
        schema: RegisterFactSchema,
        reply: Reply<FactSchema>,
    },
    GetFactSchema {
        request: GetFactSchema,
        reply: Reply<Option<FactSchema>>,
    },
    ListFactSchemas {
        request: ListFactSchemas,
        reply: Reply<Vec<FactSchema>>,
    },
    InspectFact {
        app_id: String,
        request: InspectFact,
        reply: Reply<Option<FactInspection>>,
    },
    FactHistory {
        app_id: String,
        request: FactHistoryRequest,
        reply: Reply<FactHistoryPage>,
    },
    ProfileAccess {
        app_id: String,
        profile_id: String,
        community_id: String,
        reply: Reply<ProfileAccess>,
    },
    GrantProfile {
        app_id: String,
        profile_id: String,
        community_id: String,
        access: ProfileAccess,
        reply: Reply<()>,
    },
    PlanningCatalog {
        namespace_id: String,
        community_id: String,
        limit: u16,
        reply: Reply<PlanningCatalog>,
    },
    ToolkitCatalog {
        app_id: String,
        community_id: String,
        reply: Reply<ToolkitCatalog>,
    },
    ToolkitConcepts {
        app_id: String,
        community_id: String,
        active_only: bool,
        limit: u16,
        reply: Reply<Vec<ToolkitConcept>>,
    },
    ToolkitExport {
        app_id: String,
        community_id: String,
        offset: u32,
        limit: u16,
        reply: Reply<ToolkitCatalog>,
    },
    ClaimOutbox {
        transport: String,
        limit: u16,
        max_bytes: u32,
        reply: Reply<Vec<OutboxItem>>,
    },
    AckOutbox {
        ack: AckOutbox,
        reply: Reply<()>,
    },
    Health {
        reply: Reply<Value>,
    },
    DoctorStorage {
        reply: Reply<Vec<DoctorCheck>>,
    },
    ReplayInputs {
        limit: u16,
        reply: Reply<Vec<DocumentReplayInput>>,
    },
    ProjectionCheck {
        reply: Reply<ProjectionCheck>,
    },
}

type Reply<T> = mpsc::SyncSender<Result<T, String>>;

impl StoreHandle {
    pub fn start(path: &Path) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let path = path.to_owned();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("sqlite-writer".into())
            .spawn(move || match open(&path) {
                Ok(mut connection) => {
                    let _ = ready_tx.send(Ok(()));
                    while let Ok(request) = receiver.recv() {
                        dispatch(&mut connection, request);
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                }
            })
            .context("could not start SQLite writer thread")?;
        ready_rx
            .recv()
            .context("SQLite writer stopped during startup")?
            .map_err(anyhow::Error::msg)?;
        Ok(Self { sender })
    }
}

#[async_trait]
impl PrincipalRepository for StoreHandle {
    async fn authenticate(&self, token_hash: [u8; 32]) -> Result<Option<Principal>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::Authenticate {
            token_hash,
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn principal_by_id(&self, principal_id: &str) -> Result<Option<Principal>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::PrincipalById {
            principal_id: principal_id.to_owned(),
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl DocumentRepository for StoreHandle {
    async fn idempotency(
        &self,
        app_id: &str,
        key: &str,
        request_hash: [u8; 32],
    ) -> Result<Option<Value>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::Idempotency {
            app_id: app_id.to_owned(),
            key: key.to_owned(),
            request_hash,
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn load_document_updates(&self, key: &DocumentKey) -> Result<Vec<Vec<u8>>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::LoadUpdates {
            key: key.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn log_head(&self, author: [u8; 32], log_id: [u8; 32]) -> Result<Option<LogHead>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::LogHead {
            author,
            log_id,
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn commit_local(&self, commit: LocalCommit) -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::CommitLocal {
            commit: Box::new(commit),
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl FactRepository for StoreHandle {
    async fn query_facts(&self, app_id: &str, query: &QueryFacts) -> Result<Vec<FactClaim>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::QueryFacts {
            app_id: app_id.to_owned(),
            query: query.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn current_fact_revision(
        &self,
        app_id: &str,
        community_id: &str,
        claim_id: &str,
    ) -> Result<Option<FactCurrentRevision>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::CurrentFactRevision {
            app_id: app_id.to_owned(),
            community_id: community_id.to_owned(),
            claim_id: claim_id.to_owned(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn active_fact_conflict(
        &self,
        app_id: &str,
        community_id: &str,
        claim_id: &str,
        subject: &str,
        predicate: &str,
    ) -> Result<bool> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ActiveFactConflict {
            app_id: app_id.to_owned(),
            community_id: community_id.to_owned(),
            claim_id: claim_id.to_owned(),
            subject: subject.to_owned(),
            predicate: predicate.to_owned(),
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl FactSchemaRepository for StoreHandle {
    async fn register_fact_schema(&self, schema: &RegisterFactSchema) -> Result<FactSchema> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::RegisterFactSchema {
            schema: schema.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn get_fact_schema(&self, request: &GetFactSchema) -> Result<Option<FactSchema>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::GetFactSchema {
            request: request.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn list_fact_schemas(&self, request: &ListFactSchemas) -> Result<Vec<FactSchema>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ListFactSchemas {
            request: request.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl FactInspectionRepository for StoreHandle {
    async fn inspect_fact(
        &self,
        app_id: &str,
        request: &InspectFact,
    ) -> Result<Option<FactInspection>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::InspectFact {
            app_id: app_id.to_owned(),
            request: request.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn fact_history(
        &self,
        app_id: &str,
        request: &FactHistoryRequest,
    ) -> Result<FactHistoryPage> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::FactHistory {
            app_id: app_id.to_owned(),
            request: request.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl ProfileRepository for StoreHandle {
    async fn profile_access(
        &self,
        app_id: &str,
        profile_id: &str,
        community_id: &str,
    ) -> Result<ProfileAccess> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ProfileAccess {
            app_id: app_id.to_owned(),
            profile_id: profile_id.to_owned(),
            community_id: community_id.to_owned(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn grant_profile(
        &self,
        app_id: &str,
        profile_id: &str,
        community_id: &str,
        access: ProfileAccess,
    ) -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::GrantProfile {
            app_id: app_id.to_owned(),
            profile_id: profile_id.to_owned(),
            community_id: community_id.to_owned(),
            access,
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl PlanningRepository for StoreHandle {
    async fn planning_catalog(
        &self,
        namespace_id: &str,
        community_id: &str,
        limit: u16,
    ) -> Result<PlanningCatalog> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::PlanningCatalog {
            namespace_id: namespace_id.to_owned(),
            community_id: community_id.to_owned(),
            limit,
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl ToolkitRepository for StoreHandle {
    async fn toolkit_catalog(&self, app_id: &str, community_id: &str) -> Result<ToolkitCatalog> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ToolkitCatalog {
            app_id: app_id.to_owned(),
            community_id: community_id.to_owned(),
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn toolkit_concepts(
        &self,
        app_id: &str,
        community_id: &str,
        active_only: bool,
        limit: u16,
    ) -> Result<Vec<ToolkitConcept>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ToolkitConcepts {
            app_id: app_id.to_owned(),
            community_id: community_id.to_owned(),
            active_only,
            limit,
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn toolkit_export(
        &self,
        app_id: &str,
        community_id: &str,
        offset: u32,
        limit: u16,
    ) -> Result<ToolkitCatalog> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ToolkitExport {
            app_id: app_id.to_owned(),
            community_id: community_id.to_owned(),
            offset,
            limit,
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl OutboxRepository for StoreHandle {
    async fn claim_outbox(
        &self,
        transport: &str,
        limit: u16,
        max_bytes: u32,
    ) -> Result<Vec<OutboxItem>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ClaimOutbox {
            transport: transport.to_owned(),
            limit,
            max_bytes,
            reply: tx,
        })?;
        receive(rx).await
    }

    async fn ack_outbox(&self, ack: &AckOutbox) -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::AckOutbox {
            ack: ack.clone(),
            reply: tx,
        })?;
        receive(rx).await
    }
}

#[async_trait]
impl SystemRepository for StoreHandle {
    async fn health(&self) -> Result<Value> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::Health { reply: tx })?;
        receive(rx).await
    }

    async fn doctor_storage(&self) -> Result<Vec<DoctorCheck>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::DoctorStorage { reply: tx })?;
        receive(rx).await
    }

    async fn replay_inputs(&self, limit: u16) -> Result<Vec<DocumentReplayInput>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .send(Request::ReplayInputs { limit, reply: tx })?;
        receive(rx).await
    }

    async fn projection_check(&self) -> Result<ProjectionCheck> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender.send(Request::ProjectionCheck { reply: tx })?;
        receive(rx).await
    }
}

async fn receive<T: Send + 'static>(receiver: mpsc::Receiver<Result<T, String>>) -> Result<T> {
    tokio::task::spawn_blocking(move || receiver.recv())
        .await
        .context("SQLite response task panicked")?
        .context("SQLite writer stopped")?
        .map_err(anyhow::Error::msg)
}

#[allow(clippy::too_many_lines)]
fn dispatch(connection: &mut Connection, request: Request) {
    match request {
        Request::Authenticate { token_hash, reply } => {
            respond(reply, authenticate(connection, &token_hash));
        }
        Request::PrincipalById {
            principal_id,
            reply,
        } => respond(reply, principal_by_id(connection, &principal_id)),
        Request::Idempotency {
            app_id,
            key,
            request_hash,
            reply,
        } => respond(reply, idempotency(connection, &app_id, &key, &request_hash)),
        Request::LoadUpdates { key, reply } => respond(reply, load_updates(connection, &key)),
        Request::LogHead {
            author,
            log_id,
            reply,
        } => respond(reply, log_head(connection, &author, &log_id)),
        Request::CommitLocal { commit, reply } => respond(reply, commit_local(connection, *commit)),
        Request::QueryFacts {
            app_id,
            query,
            reply,
        } => respond(reply, query_facts(connection, &app_id, &query)),
        Request::CurrentFactRevision {
            app_id,
            community_id,
            claim_id,
            reply,
        } => respond(
            reply,
            facts::current_revision(connection, &app_id, &community_id, &claim_id),
        ),
        Request::ActiveFactConflict {
            app_id,
            community_id,
            claim_id,
            subject,
            predicate,
            reply,
        } => respond(
            reply,
            facts::active_conflict(
                connection,
                &app_id,
                &community_id,
                &claim_id,
                &subject,
                &predicate,
            ),
        ),
        Request::RegisterFactSchema { schema, reply } => {
            respond(reply, facts::register_schema(connection, &schema));
        }
        Request::GetFactSchema { request, reply } => {
            respond(reply, facts::get_schema(connection, &request));
        }
        Request::ListFactSchemas { request, reply } => {
            respond(reply, facts::list_schemas(connection, &request));
        }
        Request::InspectFact {
            app_id,
            request,
            reply,
        } => respond(reply, facts::inspect(connection, &app_id, &request)),
        Request::FactHistory {
            app_id,
            request,
            reply,
        } => respond(reply, facts::history(connection, &app_id, &request)),
        Request::ProfileAccess {
            app_id,
            profile_id,
            community_id,
            reply,
        } => respond(
            reply,
            planning::profile_access(connection, &app_id, &profile_id, &community_id),
        ),
        Request::GrantProfile {
            app_id,
            profile_id,
            community_id,
            access,
            reply,
        } => respond(
            reply,
            planning::grant_profile(connection, &app_id, &profile_id, &community_id, access),
        ),
        Request::PlanningCatalog {
            namespace_id,
            community_id,
            limit,
            reply,
        } => respond(
            reply,
            planning::catalog(connection, &namespace_id, &community_id, limit),
        ),
        Request::ToolkitCatalog {
            app_id,
            community_id,
            reply,
        } => respond(reply, toolkit::catalog(connection, &app_id, &community_id)),
        Request::ToolkitConcepts {
            app_id,
            community_id,
            active_only,
            limit,
            reply,
        } => respond(
            reply,
            toolkit::concepts(connection, &app_id, &community_id, active_only, limit),
        ),
        Request::ToolkitExport {
            app_id,
            community_id,
            offset,
            limit,
            reply,
        } => respond(
            reply,
            toolkit::export(connection, &app_id, &community_id, offset, limit),
        ),
        Request::ClaimOutbox {
            transport,
            limit,
            max_bytes,
            reply,
        } => respond(
            reply,
            claim_outbox(connection, &transport, limit, max_bytes),
        ),
        Request::AckOutbox { ack, reply } => respond(reply, ack_outbox(connection, &ack)),
        Request::Health { reply } => respond(reply, health(connection)),
        Request::DoctorStorage { reply } => respond(reply, facts::doctor(connection)),
        Request::ReplayInputs { limit, reply } => {
            respond(reply, facts::replay_inputs(connection, limit));
        }
        Request::ProjectionCheck { reply } => {
            respond(reply, facts::projection_check(connection));
        }
    }
}

fn respond<T>(reply: Reply<T>, result: Result<T>) {
    let _ = reply.send(result.map_err(|error| error.to_string()));
}

pub fn initialize(path: &Path) -> Result<()> {
    open(path).map(drop)
}

pub fn register_application(
    path: &Path,
    app_id: &str,
    token_hash: &[u8; 32],
    role: &str,
) -> Result<()> {
    validate_id(app_id, "principal id")?;
    if app_id == crate::domain::PLANNING_NAMESPACE {
        bail!("principal ID is reserved for a code-owned data namespace");
    }
    if !matches!(role, "APP" | "ADMIN" | "TRANSPORT") {
        bail!("role must be APP, ADMIN, or TRANSPORT");
    }
    let mut connection = open(path)?;
    let now = now_ms()?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if role != "ADMIN" {
        tx.execute(
            "INSERT INTO applications(app_id,token_hash,role,enabled,created_at_ms) VALUES(?1,?2,?3,1,?4) \
             ON CONFLICT(app_id) DO UPDATE SET token_hash=excluded.token_hash,role=excluded.role,enabled=1",
            params![app_id, token_hash.as_slice(), role, now],
        )?;
    }
    tx.execute(
        "INSERT INTO local_principals(principal_id,token_hash,role,enabled,created_at_ms) VALUES(?1,?2,?3,1,?4) \
         ON CONFLICT(principal_id) DO UPDATE SET token_hash=excluded.token_hash,role=excluded.role,enabled=1",
        params![app_id, token_hash.as_slice(), role, now],
    )?;
    tx.commit()?;
    Ok(())
}

fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection =
        Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    set_wal_with_retry(&connection)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 1000_i64)?;
    apply_migrations(&mut connection)?;
    Ok(connection)
}

fn set_wal_with_retry(connection: &Connection) -> Result<()> {
    for attempt in 0..=20 {
        match connection.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < 20
                    && matches!(
                        error.sqlite_error_code(),
                        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                    ) =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!()
}

fn apply_migrations(connection: &mut Connection) -> Result<()> {
    const MIGRATIONS: &[(i64, &str)] = &[
        (1, include_str!("../../migrations/0001_core.sql")),
        (2, include_str!("../../migrations/0002_toolkit.sql")),
        (3, include_str!("../../migrations/0003_fact_governance.sql")),
        (
            4,
            include_str!("../../migrations/0004_planning_profile.sql"),
        ),
        (
            5,
            include_str!("../../migrations/0005_authorization_and_invariants.sql"),
        ),
    ];
    const DIGESTS: [&str; 5] = [
        "9bbdbdb32bfa205624db23724fb13b10c1ba0c8b19b549c66153b8b683c2b2c0",
        "a07e36958351e4633e8fe978cd213a11d97417512c4b1e139d1f415ad0fb0bb5",
        "c6a1ff0c8368f95a301a6e2ca1048d0cdedac55c462463b0331521df3b189cd6",
        "6d7d7893d1836c94fbfdd3720d88bfb0fc7cb987131963e304248feabfa5a3f1",
        "241a9e34078d5a60998c15dc99cce9f35048c059a5a6e44714ca898415126963",
    ];
    for ((version, sql), expected) in MIGRATIONS.iter().zip(DIGESTS) {
        if blake3::hash(sql.as_bytes()).to_hex().as_str() != expected {
            bail!("embedded migration {version} does not match its reviewed digest");
        }
    }

    let migration_table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if migration_table_exists {
        let newest: Option<i64> =
            connection.query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })?;
        if newest.is_some_and(|version| version > 5) {
            bail!("database was created by a newer unsupported migration version");
        }
    }

    for (version, sql) in MIGRATIONS {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let table_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )?;
        let applied = table_exists
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=?1)",
                [version],
                |row| row.get(0),
            )?;
        if !applied {
            if *version == 4 {
                preflight_v4_upgrade(&tx)?;
            } else if *version == 5 {
                preflight_v5_upgrade(&tx)?;
            }
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version,applied_at_ms) VALUES(?1,?2)",
                params![version, now_ms()?],
            )?;
            if *version == 5 {
                for ((known_version, _), digest) in MIGRATIONS.iter().zip(DIGESTS) {
                    tx.execute(
                        "UPDATE schema_migrations SET checksum=?1 WHERE version=?2",
                        params![digest, known_version],
                    )?;
                }
            }
        }
        tx.commit()?;
    }

    let count: i64 = connection.query_row("SELECT count(*) FROM schema_migrations", [], |row| {
        row.get(0)
    })?;
    if count != i64::try_from(MIGRATIONS.len())? {
        bail!("database migration set is incomplete or unsupported");
    }
    for ((version, _), expected) in MIGRATIONS.iter().zip(DIGESTS) {
        let stored: Option<String> = connection.query_row(
            "SELECT checksum FROM schema_migrations WHERE version=?1",
            [version],
            |row| row.get(0),
        )?;
        if stored.as_deref() != Some(expected) {
            bail!("database migration checksum mismatch at version {version}");
        }
    }
    Ok(())
}

fn preflight_v4_upgrade(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let collision: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE app_id=?1 AND enabled=1) OR EXISTS(SELECT 1 FROM administrators WHERE principal_id=?1 AND enabled=1)",
        [crate::domain::PLANNING_NAMESPACE],
        |row| row.get(0),
    )?;
    if collision {
        bail!(
            "legacy principal uses reserved namespace community.planning; disable or rename it with the previous release before upgrading"
        );
    }
    Ok(())
}

fn preflight_v5_upgrade(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    for table in [
        "applications",
        "administrators",
        "operations",
        "document_updates",
        "toolkit_concepts",
        "fact_claim_revisions",
        "planning_projects",
        "profile_grants",
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("legacy migration state is missing required table {table}");
        }
    }
    let ambiguous: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM applications a JOIN administrators d ON a.app_id=d.principal_id OR a.token_hash=d.token_hash)",
        [],
        |row| row.get(0),
    )?;
    if ambiguous {
        bail!(
            "legacy APP/ADMIN credentials are ambiguous; remove or rotate the duplicate while running the previous release, then retry"
        );
    }
    Ok(())
}

fn authenticate(connection: &Connection, token_hash: &[u8; 32]) -> Result<Option<Principal>> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT principal_id,role FROM local_principals WHERE token_hash=?1 AND enabled=1",
            [token_hash.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(id, role)| {
        let role = match role.as_str() {
            "APP" => PrincipalRole::App,
            "ADMIN" => PrincipalRole::Admin,
            "TRANSPORT" => PrincipalRole::Transport,
            _ => bail!("stored principal has an invalid role"),
        };
        Ok(Principal { id, role })
    })
    .transpose()
}

fn principal_by_id(connection: &Connection, principal_id: &str) -> Result<Option<Principal>> {
    let row: Option<String> = connection
        .query_row(
            "SELECT role FROM local_principals WHERE principal_id=?1 AND enabled=1",
            [principal_id],
            |row| row.get(0),
        )
        .optional()?;
    row.map(|role| {
        let role = match role.as_str() {
            "APP" => PrincipalRole::App,
            "ADMIN" => PrincipalRole::Admin,
            "TRANSPORT" => PrincipalRole::Transport,
            _ => bail!("stored principal has an invalid role"),
        };
        Ok(Principal {
            id: principal_id.to_owned(),
            role,
        })
    })
    .transpose()
}

fn idempotency(
    connection: &Connection,
    app_id: &str,
    key: &str,
    request_hash: &[u8; 32],
) -> Result<Option<Value>> {
    let stored: Option<(Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT request_hash, response_json FROM idempotency_keys WHERE app_id=?1 AND key=?2",
            params![app_id, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((stored_hash, response)) = stored else {
        return Ok(None);
    };
    if stored_hash.as_slice() != request_hash {
        bail!("idempotency key was already used for a different request");
    }
    Ok(Some(
        serde_json::from_slice(&response).context("stored idempotency response is corrupt")?,
    ))
}

fn load_updates(connection: &Connection, key: &DocumentKey) -> Result<Vec<Vec<u8>>> {
    let mut statement = connection.prepare(
        "SELECT update_bytes FROM document_updates \
         WHERE app_id=?1 AND community_id=?2 AND document_id=?3 AND applied=1 \
         ORDER BY created_at_ms, operation_hash",
    )?;
    let rows = statement.query_map(
        params![key.app_id, key.community_id, key.document_id],
        |row| row.get(0),
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn log_head(
    connection: &Connection,
    author: &[u8; 32],
    log_id: &[u8; 32],
) -> Result<Option<LogHead>> {
    let row: Option<(i64, Vec<u8>)> = connection.query_row(
        "SELECT sequence, operation_hash FROM log_heads WHERE author_key=?1 AND log_id=?2 AND generation=0",
        params![author.as_slice(), log_id.as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    row.map(|(sequence, hash)| {
        Ok(LogHead {
            sequence: sequence.try_into()?,
            operation_hash: hash
                .try_into()
                .map_err(|_| anyhow!("corrupt log head hash"))?,
        })
    })
    .transpose()
}

fn commit_local(connection: &mut Connection, commit: LocalCommit) -> Result<()> {
    let now = now_ms()?;
    let record = &commit.record;
    if record.metadata.app_id != commit.document.app_id
        || record.metadata.community_id != commit.document.community_id
        || record.metadata.document_id != commit.document.document_id
    {
        bail!("record metadata does not match its document commit");
    }
    let sequence = i64::from(record.sequence);
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = log_head(&tx, &record.author_key, &record.log_id)?;
    match (record.sequence, current) {
        (0, None) => {}
        (0, Some(_)) => bail!("log head changed before commit"),
        (sequence, Some(head))
            if head.sequence.checked_add(1) == Some(sequence)
                && Some(head.operation_hash) == record.backlink => {}
        _ => bail!("log head changed before commit"),
    }

    tx.execute(
        "INSERT INTO operations(operation_hash, canonical_header, body_ciphertext, author_key, log_id, generation, sequence, backlink, app_id, community_id, document_id, record_kind, verification_status, apply_status, received_at_ms) \
         VALUES(?1,?2,?3,?4,?5,0,?6,?7,?8,?9,?10,?11,'VERIFIED','APPLIED',?12)",
        params![record.operation_hash.as_slice(), record.canonical_header, record.body_ciphertext, record.author_key.as_slice(), record.log_id.as_slice(), sequence,
            record.backlink.as_ref().map(<[u8; 32]>::as_slice), commit.document.app_id, commit.document.community_id,
            commit.document.document_id, i64::from(record.metadata.record_kind), now],
    )?;
    tx.execute(
        "INSERT INTO document_updates(operation_hash, app_id, community_id, document_id, codec, key_epoch, update_bytes, applied, pending_deps, created_at_ms) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,1,NULL,?8)",
        params![record.operation_hash.as_slice(), commit.document.app_id, commit.document.community_id, commit.document.document_id,
            i64::from(record.metadata.codec_version), i64::from(record.metadata.key_epoch), commit.update_bytes, now],
    )?;
    tx.execute(
        "INSERT INTO log_heads(author_key,log_id,generation,sequence,operation_hash) VALUES(?1,?2,0,?3,?4) \
         ON CONFLICT(author_key,log_id,generation) DO UPDATE SET sequence=excluded.sequence, operation_hash=excluded.operation_hash",
        params![record.author_key.as_slice(), record.log_id.as_slice(), sequence, record.operation_hash.as_slice()],
    )?;
    tx.execute(
        "INSERT INTO durable_outbox(operation_hash,priority,available_at_ms,created_at_ms) VALUES(?1,?2,?3,?3)",
        params![record.operation_hash.as_slice(), priority_for(record.metadata.record_kind), now],
    )?;
    for projection in commit.projections {
        apply_projection(
            &tx,
            &commit.document.app_id,
            &commit.document.community_id,
            &record.operation_hash,
            projection,
            now,
        )?;
    }
    tx.execute(
        "INSERT INTO idempotency_keys(app_id,key,request_hash,response_json,created_at_ms) VALUES(?1,?2,?3,?4,?5)",
        params![commit.idempotency_app_id, commit.idempotency_key, commit.request_hash.as_slice(), serde_json::to_vec(&commit.response)?, now],
    )?;
    tx.commit()?;
    Ok(())
}

fn apply_projection(
    tx: &rusqlite::Transaction<'_>,
    app_id: &str,
    community_id: &str,
    operation_hash: &[u8; 32],
    projection: ProjectionWrite,
    now: i64,
) -> Result<()> {
    match projection {
        ProjectionWrite::FactRevision(projection) => {
            let crate::domain::FactRevisionProjection {
                claim_id,
                community_id,
                subject,
                predicate,
                object_json,
                source,
                confidence,
                schema_id,
                schema_version,
                lifecycle_status,
                source_document_id,
                multiple_active_claims,
                expected_previous_operation_hash,
            } = *projection;
            let stored_previous: Option<(Vec<u8>, i64)> = tx
                .query_row(
                    "SELECT source_operation_hash,updated_at_ms FROM fact_claims WHERE app_id=?1 AND community_id=?2 AND claim_id=?3",
                    params![app_id, community_id, claim_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let stored_previous_hash = stored_previous.as_ref().map(|(hash, _)| hash.as_slice());
            let revision_now = stored_previous.as_ref().map_or(now, |(_, previous_time)| {
                now.max(previous_time.saturating_add(1))
            });
            let expected_previous = expected_previous_operation_hash
                .as_deref()
                .map(hex::decode)
                .transpose()
                .context("invalid expected previous revision hash")?;
            if stored_previous_hash != expected_previous.as_deref() {
                bail!("fact current revision changed before commit");
            }
            if lifecycle_status == crate::domain::FactLifecycleStatus::Asserted
                && !multiple_active_claims
            {
                let conflict: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM fact_claims WHERE app_id=?1 AND community_id=?2 AND claim_id!=?3 AND subject=?4 AND predicate=?5 AND retracted=0)",
                    params![app_id, community_id, claim_id, subject, predicate],
                    |row| row.get(0),
                )?;
                if conflict {
                    bail!("schema permits only one active claim for this subject and predicate");
                }
            }
            if let Some(previous) = stored_previous_hash {
                tx.execute(
                    "UPDATE fact_claim_revisions SET lifecycle_status='SUPERSEDED' WHERE operation_hash=?1",
                    [previous],
                )?;
            }
            tx.execute(
                "INSERT INTO fact_claim_revisions(operation_hash,app_id,community_id,claim_id,subject,predicate,object_json,source,confidence,author_key,schema_id,schema_version,asserted_at_ms,supersedes_operation_hash,lifecycle_status,source_document_id) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![operation_hash.as_slice(), app_id, community_id, claim_id, subject, predicate, object_json, source, confidence,
                    commit_author(tx, operation_hash)?, schema_id, i64::from(schema_version), revision_now, stored_previous_hash, lifecycle_status.as_str(), source_document_id],
            )?;
            let retracted =
                i64::from(lifecycle_status != crate::domain::FactLifecycleStatus::Asserted);
            tx.execute(
                "INSERT INTO fact_claims(app_id,community_id,claim_id,subject,predicate,object_json,source,confidence,source_operation_hash,retracted,updated_at_ms,schema_id,schema_version,lifecycle_status,source_document_id) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) \
                 ON CONFLICT(app_id,community_id,claim_id) DO UPDATE SET subject=excluded.subject,predicate=excluded.predicate,object_json=excluded.object_json,source=excluded.source,confidence=excluded.confidence,source_operation_hash=excluded.source_operation_hash,retracted=excluded.retracted,updated_at_ms=excluded.updated_at_ms,schema_id=excluded.schema_id,schema_version=excluded.schema_version,lifecycle_status=excluded.lifecycle_status,source_document_id=excluded.source_document_id",
                params![app_id, community_id, claim_id, subject, predicate, object_json, source, confidence, operation_hash.as_slice(), retracted, revision_now,
                    schema_id, i64::from(schema_version), lifecycle_status.as_str(), source_document_id],
            )?;
        }
        ProjectionWrite::Planning(projection) => {
            planning::apply_projection(tx, app_id, community_id, operation_hash, *projection, now)?;
        }
        ProjectionWrite::Toolkit(projection) => {
            toolkit::apply_projection(tx, app_id, community_id, operation_hash, *projection)?;
        }
    }
    Ok(())
}

fn commit_author(tx: &rusqlite::Transaction<'_>, operation_hash: &[u8; 32]) -> Result<Vec<u8>> {
    tx.query_row(
        "SELECT author_key FROM operations WHERE operation_hash=?1",
        [operation_hash.as_slice()],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn query_facts(
    connection: &Connection,
    app_id: &str,
    query: &QueryFacts,
) -> Result<Vec<FactClaim>> {
    let mut statement = connection.prepare(
        "SELECT claim_id,community_id,subject,predicate,object_json,source,confidence,hex(source_operation_hash),updated_at_ms,schema_id,schema_version,lifecycle_status,source_document_id \
         FROM fact_claims WHERE app_id=?1 AND community_id=?2 AND retracted=0 \
         AND (?3 IS NULL OR subject=?3) AND (?4 IS NULL OR predicate=?4) \
         ORDER BY updated_at_ms DESC, claim_id LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![
            app_id,
            query.community_id,
            query.subject,
            query.predicate,
            i64::from(query.limit.min(500))
        ],
        |row| {
            let object_json: Vec<u8> = row.get(4)?;
            let object = serde_json::from_slice(&object_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    object_json.len(),
                    rusqlite::types::Type::Blob,
                    Box::new(error),
                )
            })?;
            Ok(FactClaim {
                claim_id: row.get(0)?,
                community_id: row.get(1)?,
                subject: row.get(2)?,
                predicate: row.get(3)?,
                object,
                source: row.get(5)?,
                confidence: row.get(6)?,
                source_operation_hash: row.get::<_, String>(7)?.to_ascii_lowercase(),
                updated_at_ms: row.get(8)?,
                schema_id: row.get(9)?,
                schema_version: row.get::<_, u32>(10)?,
                lifecycle_status: facts::lifecycle(row.get::<_, String>(11)?.as_str())?,
                source_document_id: row.get(12)?,
            })
        },
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn claim_outbox(
    connection: &mut Connection,
    transport: &str,
    limit: u16,
    max_bytes: u32,
) -> Result<Vec<OutboxItem>> {
    validate_id(transport, "transport")?;
    let now = now_ms()?;
    let lease_until = now + 60_000;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut candidates = Vec::new();
    {
        let mut statement = tx.prepare(
            "SELECT hex(o.operation_hash), o.canonical_header, o.body_ciphertext, q.priority \
             FROM durable_outbox q JOIN operations o ON o.operation_hash=q.operation_hash \
             LEFT JOIN transport_deliveries d ON d.operation_hash=o.operation_hash AND d.transport=?1 \
             WHERE q.available_at_ms<=?2 AND (d.state IS NULL OR (d.state='CLAIMED' AND d.lease_until_ms<?2) OR d.state='RETRY') \
             ORDER BY q.priority ASC, q.outbox_id ASC LIMIT ?3"
        )?;
        let rows =
            statement.query_map(params![transport, now, i64::from(limit.min(256))], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
        for row in rows {
            candidates.push(row?);
        }
    }

    let mut result = Vec::new();
    let mut used = 0_u64;
    for (hash_hex, header, body, priority) in candidates {
        let size = (header.len() + body.len()) as u64;
        if !result.is_empty() && used + size > u64::from(max_bytes) {
            break;
        }
        if size > u64::from(max_bytes) {
            continue;
        }
        let hash = hex::decode(&hash_hex)?;
        tx.execute(
            "INSERT INTO transport_deliveries(operation_hash,transport,state,attempts,lease_until_ms,updated_at_ms) VALUES(?1,?2,'CLAIMED',1,?3,?4) \
             ON CONFLICT(operation_hash,transport) DO UPDATE SET state='CLAIMED',attempts=attempts+1,lease_until_ms=excluded.lease_until_ms,updated_at_ms=excluded.updated_at_ms",
            params![hash, transport, lease_until, now],
        )?;
        let lease_attempt: u32 = tx.query_row(
            "SELECT attempts FROM transport_deliveries WHERE operation_hash=?1 AND transport=?2",
            params![hash, transport],
            |row| row.get(0),
        )?;
        used += size;
        result.push(OutboxItem {
            operation_hash: hash_hex.to_ascii_lowercase(),
            lease_attempt,
            header_base64: base64_encode(&header),
            body_base64: base64_encode(&body),
            priority,
        });
    }
    tx.commit()?;
    Ok(result)
}

fn ack_outbox(connection: &Connection, ack: &AckOutbox) -> Result<()> {
    validate_id(&ack.transport, "transport")?;
    let hash = hex::decode(&ack.operation_hash).context("invalid operation hash")?;
    if hash.len() != 32 {
        bail!("operation hash must contain 32 bytes");
    }
    let (state, durable_ack) = match ack.status {
        DeliveryAck::Stored => ("DELIVERED", "STORED"),
        DeliveryAck::Applied => ("DELIVERED", "APPLIED"),
        DeliveryAck::PendingDeps => ("DELIVERED", "PENDING_DEPS"),
        DeliveryAck::Rejected => ("REJECTED", "REJECTED"),
    };
    let now = now_ms()?;
    let changed = connection.execute(
        "UPDATE transport_deliveries SET state=?1,durable_ack=?2,last_error=?3,lease_until_ms=NULL,updated_at_ms=?4 \
         WHERE operation_hash=?5 AND transport=?6 AND state='CLAIMED' AND attempts=?7 AND lease_until_ms>=?4",
        params![state, durable_ack, ack.detail, now, hash, ack.transport, ack.lease_attempt],
    )?;
    if changed == 0 {
        bail!("outbox item lease is stale or is not claimed by this transport");
    }
    Ok(())
}

fn health(connection: &Connection) -> Result<Value> {
    let operations: i64 =
        connection.query_row("SELECT count(*) FROM operations", [], |row| row.get(0))?;
    let pending: i64 =
        connection.query_row("SELECT count(*) FROM durable_outbox", [], |row| row.get(0))?;
    let integrity: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    Ok(
        json!({"status": if integrity == "ok" { "ok" } else { "degraded" }, "operations": operations, "outbox_records": pending, "sqlite_quick_check": integrity}),
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn priority_for(record_kind: u8) -> i64 {
    if record_kind == 1 { 40 } else { 50 }
}

fn now_ms() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

fn validate_id(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        bail!("{field} must contain 1..=128 bytes");
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
    }) {
        bail!("{field} contains unsupported characters");
    }
    Ok(())
}
