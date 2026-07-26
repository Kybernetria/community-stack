PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version         INTEGER PRIMARY KEY,
    applied_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS applications (
    app_id          TEXT PRIMARY KEY,
    token_hash      BLOB NOT NULL UNIQUE CHECK(length(token_hash) = 32),
    role            TEXT NOT NULL CHECK(role IN ('APP', 'TRANSPORT')),
    enabled         INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS operations (
    operation_hash  BLOB PRIMARY KEY CHECK(length(operation_hash) = 32),
    canonical_header BLOB NOT NULL,
    body_ciphertext BLOB NOT NULL,
    author_key      BLOB NOT NULL CHECK(length(author_key) = 32),
    log_id          BLOB NOT NULL CHECK(length(log_id) = 32),
    generation      INTEGER NOT NULL DEFAULT 0,
    sequence        INTEGER NOT NULL CHECK(sequence >= 0),
    backlink        BLOB CHECK(backlink IS NULL OR length(backlink) = 32),
    app_id          TEXT NOT NULL,
    community_id    TEXT NOT NULL,
    document_id     TEXT NOT NULL,
    record_kind     INTEGER NOT NULL,
    verification_status TEXT NOT NULL,
    apply_status    TEXT NOT NULL,
    received_at_ms  INTEGER NOT NULL,
    UNIQUE(author_key, log_id, generation, sequence)
) STRICT;

CREATE INDEX IF NOT EXISTS operations_document_order
    ON operations(app_id, community_id, document_id, received_at_ms, sequence);

CREATE TABLE IF NOT EXISTS log_heads (
    author_key      BLOB NOT NULL CHECK(length(author_key) = 32),
    log_id          BLOB NOT NULL CHECK(length(log_id) = 32),
    generation      INTEGER NOT NULL DEFAULT 0,
    sequence        INTEGER NOT NULL,
    operation_hash  BLOB NOT NULL REFERENCES operations(operation_hash),
    PRIMARY KEY(author_key, log_id, generation)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS document_updates (
    operation_hash  BLOB PRIMARY KEY REFERENCES operations(operation_hash) ON DELETE CASCADE,
    app_id          TEXT NOT NULL,
    community_id    TEXT NOT NULL,
    document_id     TEXT NOT NULL,
    codec            INTEGER NOT NULL,
    key_epoch       INTEGER NOT NULL,
    update_bytes    BLOB NOT NULL,
    applied         INTEGER NOT NULL CHECK(applied IN (0, 1)),
    pending_deps    BLOB,
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS document_updates_replay
    ON document_updates(app_id, community_id, document_id, created_at_ms, operation_hash)
    WHERE applied = 1;

CREATE TABLE IF NOT EXISTS document_snapshots (
    snapshot_hash   BLOB PRIMARY KEY CHECK(length(snapshot_hash) = 32),
    app_id          TEXT NOT NULL,
    community_id    TEXT NOT NULL,
    document_id     TEXT NOT NULL,
    base_frontiers  BLOB NOT NULL,
    base_version_vector BLOB NOT NULL,
    auth_frontier   BLOB NOT NULL,
    key_epoch       INTEGER NOT NULL,
    encrypted_bytes BLOB NOT NULL,
    verified        INTEGER NOT NULL CHECK(verified IN (0, 1)),
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS durable_inbox (
    inbox_id        INTEGER PRIMARY KEY,
    transport       TEXT NOT NULL,
    peer_hint       TEXT,
    canonical_header BLOB NOT NULL,
    body_ciphertext BLOB NOT NULL,
    status          TEXT NOT NULL,
    reason_code     TEXT,
    received_at_ms  INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS durable_outbox (
    outbox_id       INTEGER PRIMARY KEY,
    operation_hash  BLOB NOT NULL UNIQUE REFERENCES operations(operation_hash) ON DELETE CASCADE,
    priority        INTEGER NOT NULL,
    available_at_ms INTEGER NOT NULL,
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS transport_deliveries (
    operation_hash  BLOB NOT NULL REFERENCES operations(operation_hash) ON DELETE CASCADE,
    transport       TEXT NOT NULL,
    state           TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    lease_until_ms  INTEGER,
    last_error      TEXT,
    durable_ack     TEXT,
    updated_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(operation_hash, transport)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS peer_sync_cursors (
    transport       TEXT NOT NULL,
    peer_id         TEXT NOT NULL,
    topic           BLOB NOT NULL,
    cursor          BLOB NOT NULL,
    updated_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(transport, peer_id, topic)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS sync_sessions (
    session_id      TEXT PRIMARY KEY,
    transport       TEXT NOT NULL,
    peer_id         TEXT,
    state           TEXT NOT NULL,
    byte_budget     INTEGER NOT NULL,
    bytes_sent      INTEGER NOT NULL DEFAULT 0,
    bytes_received  INTEGER NOT NULL DEFAULT 0,
    started_at_ms   INTEGER NOT NULL,
    updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS application_acks (
    operation_hash  BLOB NOT NULL REFERENCES operations(operation_hash) ON DELETE CASCADE,
    peer_id         TEXT NOT NULL,
    status          TEXT NOT NULL CHECK(status IN ('RECEIVED','STORED','APPLIED','PENDING_DEPS','REJECTED')),
    detail          TEXT,
    created_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(operation_hash, peer_id, status)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS group_operations (
    operation_hash  BLOB PRIMARY KEY REFERENCES operations(operation_hash) ON DELETE CASCADE,
    community_id    TEXT NOT NULL,
    control_kind    TEXT NOT NULL,
    applied         INTEGER NOT NULL CHECK(applied IN (0, 1)),
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS group_state (
    community_id    TEXT PRIMARY KEY,
    auth_frontier   BLOB NOT NULL,
    canonical_state BLOB NOT NULL,
    updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS device_credentials (
    credential_hash BLOB PRIMARY KEY CHECK(length(credential_hash) = 32),
    user_root_key   BLOB NOT NULL,
    device_author_key BLOB NOT NULL CHECK(length(device_author_key) = 32),
    canonical_credential BLOB NOT NULL,
    expires_at_ms   INTEGER,
    accepted_at_ms  INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS device_revocations (
    credential_hash BLOB PRIMARY KEY REFERENCES device_credentials(credential_hash),
    canonical_revocation BLOB NOT NULL,
    effective_frontier BLOB NOT NULL,
    revoked_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS key_epochs (
    community_id    TEXT NOT NULL,
    space_id        BLOB NOT NULL,
    epoch           INTEGER NOT NULL,
    state           TEXT NOT NULL CHECK(state IN ('CURRENT','HISTORICAL','REVOKED')),
    created_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(community_id, space_id, epoch)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS wrapped_key_references (
    community_id    TEXT NOT NULL,
    space_id        BLOB NOT NULL,
    epoch           INTEGER NOT NULL,
    recipient_key   BLOB NOT NULL,
    wrapped_key_hash BLOB NOT NULL CHECK(length(wrapped_key_hash) = 32),
    wrapped_key_bytes BLOB NOT NULL,
    PRIMARY KEY(community_id, space_id, epoch, recipient_key),
    FOREIGN KEY(community_id, space_id, epoch) REFERENCES key_epochs(community_id, space_id, epoch)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS trusted_authors (
    community_id    TEXT NOT NULL,
    author_key      BLOB NOT NULL CHECK(length(author_key) = 32),
    capability      TEXT NOT NULL CHECK(capability IN ('PULL','READ','WRITE','MANAGE')),
    auth_frontier   BLOB NOT NULL,
    revoked_at_ms   INTEGER,
    PRIMARY KEY(community_id, author_key)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS projection_rows (
    app_id          TEXT NOT NULL,
    projection      TEXT NOT NULL,
    row_key         TEXT NOT NULL,
    value_json      BLOB NOT NULL,
    source_operation BLOB NOT NULL REFERENCES operations(operation_hash),
    updated_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(app_id, projection, row_key)
) WITHOUT ROWID, STRICT;

-- Rebuildable EAV-style projection of authoritative Loro fact documents.
CREATE TABLE IF NOT EXISTS fact_claims (
    app_id          TEXT NOT NULL,
    community_id    TEXT NOT NULL,
    claim_id        TEXT NOT NULL,
    subject         TEXT NOT NULL,
    predicate       TEXT NOT NULL,
    object_json     BLOB NOT NULL,
    source          TEXT,
    confidence      REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
    source_operation_hash BLOB NOT NULL REFERENCES operations(operation_hash),
    retracted       INTEGER NOT NULL DEFAULT 0 CHECK(retracted IN (0, 1)),
    updated_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(app_id, community_id, claim_id)
) WITHOUT ROWID, STRICT;

CREATE INDEX IF NOT EXISTS fact_claims_sp
    ON fact_claims(app_id, community_id, subject, predicate)
    WHERE retracted = 0;

CREATE TABLE IF NOT EXISTS idempotency_keys (
    app_id          TEXT NOT NULL REFERENCES applications(app_id),
    key             TEXT NOT NULL,
    request_hash    BLOB NOT NULL CHECK(length(request_hash) = 32),
    response_json   BLOB NOT NULL,
    created_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(app_id, key)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS quarantine (
    quarantine_id   INTEGER PRIMARY KEY,
    operation_hash  BLOB,
    transport       TEXT,
    reason_code     TEXT NOT NULL,
    detail          TEXT,
    canonical_header BLOB,
    body_ciphertext BLOB,
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS audit_events (
    event_id        INTEGER PRIMARY KEY,
    event_kind      TEXT NOT NULL,
    app_id          TEXT,
    community_id    TEXT,
    object_id       TEXT,
    redacted_detail TEXT,
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS blob_metadata (
    blob_hash       BLOB PRIMARY KEY CHECK(length(blob_hash) = 32),
    byte_length     INTEGER NOT NULL,
    media_type      TEXT,
    relative_path   TEXT NOT NULL,
    verified        INTEGER NOT NULL CHECK(verified IN (0, 1)),
    created_at_ms   INTEGER NOT NULL
) STRICT;
