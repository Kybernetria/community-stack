-- Inspectable, versioned fact governance. Canonical operation/update tables are
-- intentionally untouched: everything below is local policy or rebuildable data.
CREATE TABLE fact_schemas (
    app_id                  TEXT NOT NULL,
    schema_id               TEXT NOT NULL,
    schema_version          INTEGER NOT NULL CHECK(schema_version > 0),
    predicate               TEXT NOT NULL,
    object_schema_json      BLOB NOT NULL CHECK(length(object_schema_json) <= 65536),
    multiple_active_claims  INTEGER NOT NULL CHECK(multiple_active_claims IN (0,1)),
    provenance_policy       TEXT NOT NULL CHECK(provenance_policy IN ('NONE','SOURCE','SOURCE_DOCUMENT','SOURCE_AND_DOCUMENT')),
    lifecycle_policy_json   BLOB NOT NULL CHECK(length(lifecycle_policy_json) <= 4096),
    created_at_ms           INTEGER NOT NULL,
    enabled                 INTEGER NOT NULL CHECK(enabled IN (0,1)),
    PRIMARY KEY(app_id, schema_id, schema_version),
    UNIQUE(app_id, predicate, schema_version)
) WITHOUT ROWID, STRICT;

CREATE INDEX fact_schemas_list ON fact_schemas(app_id, schema_id, schema_version DESC);

-- ADMIN credentials are separate from application credentials so upgrading the
-- v1 applications CHECK constraint does not rebuild or risk its referenced data.
CREATE TABLE administrators (
    principal_id    TEXT PRIMARY KEY,
    token_hash      BLOB NOT NULL UNIQUE CHECK(length(token_hash) = 32),
    enabled         INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE fact_claim_revisions (
    operation_hash             BLOB PRIMARY KEY REFERENCES operations(operation_hash) CHECK(length(operation_hash) = 32),
    app_id                     TEXT NOT NULL,
    community_id               TEXT NOT NULL,
    claim_id                   TEXT NOT NULL,
    subject                    TEXT NOT NULL,
    predicate                  TEXT NOT NULL,
    object_json                BLOB NOT NULL,
    source                     TEXT,
    confidence                 REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
    author_key                 BLOB NOT NULL CHECK(length(author_key) = 32),
    schema_id                  TEXT NOT NULL,
    schema_version             INTEGER NOT NULL CHECK(schema_version >= 0),
    asserted_at_ms             INTEGER NOT NULL,
    supersedes_operation_hash  BLOB REFERENCES fact_claim_revisions(operation_hash) CHECK(supersedes_operation_hash IS NULL OR length(supersedes_operation_hash) = 32),
    lifecycle_status           TEXT NOT NULL CHECK(lifecycle_status IN ('ASSERTED','SUPERSEDED','RETRACTED','DISPUTED','EXPIRED')),
    source_document_id         TEXT,
    UNIQUE(app_id, community_id, claim_id, operation_hash)
) STRICT;

CREATE INDEX fact_revision_history
    ON fact_claim_revisions(app_id, community_id, claim_id, asserted_at_ms DESC, operation_hash DESC);
CREATE INDEX fact_revision_supersedes ON fact_claim_revisions(supersedes_operation_hash);

-- Upgrade the existing current-value projection additively. Existing rows are
-- explicitly legacy/untyped rather than being assigned a misleading schema.
ALTER TABLE fact_claims ADD COLUMN schema_id TEXT NOT NULL DEFAULT 'legacy/untyped';
ALTER TABLE fact_claims ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE fact_claims ADD COLUMN lifecycle_status TEXT NOT NULL DEFAULT 'ASSERTED';
ALTER TABLE fact_claims ADD COLUMN source_document_id TEXT;

INSERT INTO fact_claim_revisions(
    operation_hash,app_id,community_id,claim_id,subject,predicate,object_json,source,
    confidence,author_key,schema_id,schema_version,asserted_at_ms,
    supersedes_operation_hash,lifecycle_status,source_document_id
)
SELECT f.source_operation_hash,f.app_id,f.community_id,f.claim_id,f.subject,f.predicate,
       f.object_json,f.source,f.confidence,o.author_key,'legacy/untyped',0,f.updated_at_ms,
       NULL,CASE WHEN f.retracted=1 THEN 'RETRACTED' ELSE 'ASSERTED' END,
       NULL
FROM fact_claims f JOIN operations o ON o.operation_hash=f.source_operation_hash;

UPDATE fact_claims
SET lifecycle_status=CASE WHEN retracted=1 THEN 'RETRACTED' ELSE 'ASSERTED' END,
    source_document_id=NULL;

CREATE TRIGGER fact_claims_revision_insert_guard
BEFORE INSERT ON fact_claims
WHEN NOT EXISTS(SELECT 1 FROM fact_claim_revisions r WHERE r.operation_hash=NEW.source_operation_hash)
BEGIN
    SELECT RAISE(ABORT, 'current fact must reference a revision');
END;

CREATE TRIGGER fact_claims_revision_update_guard
BEFORE UPDATE OF source_operation_hash ON fact_claims
WHEN NOT EXISTS(SELECT 1 FROM fact_claim_revisions r WHERE r.operation_hash=NEW.source_operation_hash)
BEGIN
    SELECT RAISE(ABORT, 'current fact must reference a revision');
END;

-- Staging is deliberately present before repair is enabled. A run ID isolates
-- interrupted attempts; production current rows are never dropped first.
CREATE TABLE fact_projection_rebuild_runs (
    run_id              TEXT PRIMARY KEY,
    mode                TEXT NOT NULL CHECK(mode IN ('CHECK','REBUILD')),
    status              TEXT NOT NULL CHECK(status IN ('STARTED','VALIDATED','APPLIED','FAILED','DISABLED')),
    expected_count      INTEGER,
    staged_count        INTEGER,
    expected_hash       TEXT,
    staged_hash         TEXT,
    redacted_detail     TEXT,
    started_at_ms       INTEGER NOT NULL,
    finished_at_ms      INTEGER
) STRICT;

CREATE TABLE fact_claims_staging (
    run_id                  TEXT NOT NULL REFERENCES fact_projection_rebuild_runs(run_id) ON DELETE CASCADE,
    app_id                  TEXT NOT NULL,
    community_id            TEXT NOT NULL,
    claim_id                TEXT NOT NULL,
    subject                 TEXT NOT NULL,
    predicate               TEXT NOT NULL,
    object_json             BLOB NOT NULL,
    source                  TEXT,
    confidence              REAL NOT NULL,
    source_operation_hash   BLOB NOT NULL CHECK(length(source_operation_hash)=32),
    retracted               INTEGER NOT NULL CHECK(retracted IN (0,1)),
    updated_at_ms           INTEGER NOT NULL,
    schema_id               TEXT NOT NULL,
    schema_version          INTEGER NOT NULL,
    lifecycle_status        TEXT NOT NULL,
    source_document_id      TEXT,
    PRIMARY KEY(run_id,app_id,community_id,claim_id)
) WITHOUT ROWID, STRICT;
