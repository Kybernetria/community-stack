-- Tighten local principal uniqueness, scope planning grants by community, add
-- transactional planning generations, and attach migration checksums. Global
-- v4 planning grants are intentionally revoked: an administrator must re-grant
-- each APP for an explicit community.
ALTER TABLE schema_migrations ADD COLUMN checksum TEXT;

CREATE TABLE local_principals (
    principal_id    TEXT PRIMARY KEY,
    token_hash      BLOB NOT NULL UNIQUE CHECK(length(token_hash)=32),
    role            TEXT NOT NULL CHECK(role IN ('APP','ADMIN','TRANSPORT')),
    enabled         INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    created_at_ms   INTEGER NOT NULL
) STRICT;

INSERT INTO local_principals(principal_id,token_hash,role,enabled,created_at_ms)
SELECT app_id,token_hash,role,enabled,created_at_ms FROM applications;
INSERT INTO local_principals(principal_id,token_hash,role,enabled,created_at_ms)
SELECT principal_id,token_hash,'ADMIN',enabled,created_at_ms FROM administrators;

ALTER TABLE profile_grants RENAME TO profile_grants_v4;
CREATE TABLE profile_grants (
    app_id          TEXT NOT NULL,
    profile_id      TEXT NOT NULL,
    community_id    TEXT NOT NULL,
    can_read        INTEGER NOT NULL CHECK(can_read IN (0,1)),
    can_write       INTEGER NOT NULL CHECK(can_write IN (0,1)),
    granted_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(app_id,profile_id,community_id),
    FOREIGN KEY(app_id) REFERENCES local_principals(principal_id)
) WITHOUT ROWID, STRICT;
DROP TABLE profile_grants_v4;

CREATE TABLE planning_community_versions (
    namespace_id   TEXT NOT NULL,
    community_id   TEXT NOT NULL,
    version        INTEGER NOT NULL CHECK(version > 0),
    PRIMARY KEY(namespace_id,community_id)
) WITHOUT ROWID, STRICT;

-- v3 incorrectly inferred semantic provenance from the owning Loro document.
-- Preserve unknown legacy provenance as unknown; operation.document_id remains
-- available independently for inspection.
UPDATE fact_claim_revisions
SET source_document_id=NULL
WHERE schema_id='legacy/untyped' AND schema_version=0;
UPDATE fact_claims
SET source_document_id=NULL
WHERE schema_id='legacy/untyped' AND schema_version=0;
