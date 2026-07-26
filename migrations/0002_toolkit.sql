-- Rebuildable, app/community-scoped toolkit projection. Signed operations and
-- their Loro documents remain authoritative; these rows are never replicated.
CREATE TABLE IF NOT EXISTS toolkit_concepts (
    app_id                  TEXT NOT NULL,
    community_id            TEXT NOT NULL,
    concept_key             TEXT NOT NULL,
    revision_hash           TEXT NOT NULL CHECK(length(revision_hash) = 64),
    name                    TEXT NOT NULL,
    kind                    TEXT NOT NULL CHECK(kind IN ('domain','role','capability','standard','attribute','dimension','descriptor')),
    definition              TEXT NOT NULL,
    aliases_json            BLOB NOT NULL,
    value_type              TEXT NOT NULL CHECK(value_type IN ('boolean','integer','number','text','enum','date','reference')),
    cardinality             TEXT NOT NULL CHECK(cardinality IN ('one','many')),
    applicable_json         BLOB NOT NULL,
    allowed_values_json     BLOB,
    scale_json              BLOB,
    parent_key              TEXT,
    status                  TEXT NOT NULL CHECK(status IN ('active','deprecated')),
    active_revision         INTEGER NOT NULL CHECK(active_revision IN (0,1)),
    source_operation_hash   BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash) = 32),
    PRIMARY KEY(app_id, community_id, concept_key, revision_hash)
) WITHOUT ROWID, STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS toolkit_concepts_active_revision
    ON toolkit_concepts(app_id, community_id, concept_key)
    WHERE active_revision = 1;

CREATE TABLE IF NOT EXISTS toolkit_tools (
    app_id                  TEXT NOT NULL,
    community_id            TEXT NOT NULL,
    tool_key                TEXT NOT NULL,
    name                    TEXT NOT NULL,
    description             TEXT,
    homepage                TEXT,
    status                  TEXT NOT NULL CHECK(status IN ('active','deprecated')),
    source_operation_hash   BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash) = 32),
    PRIMARY KEY(app_id, community_id, tool_key)
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS toolkit_assertions (
    app_id                  TEXT NOT NULL,
    community_id            TEXT NOT NULL,
    assertion_id            TEXT NOT NULL,
    tool_key                TEXT NOT NULL,
    concept_key             TEXT NOT NULL,
    concept_revision        TEXT NOT NULL CHECK(length(concept_revision) = 64),
    value_json              BLOB,
    value_kind              TEXT NOT NULL CHECK(value_kind IN ('none','boolean','number','text')),
    value_number            REAL,
    value_text              TEXT,
    value_boolean           INTEGER CHECK(value_boolean IN (0,1)),
    origin                  TEXT NOT NULL CHECK(origin IN ('human','ai','fixture')),
    initial_state           TEXT NOT NULL CHECK(initial_state IN ('proposed','verified','rejected','unknown')),
    effective_state         TEXT NOT NULL CHECK(effective_state IN ('proposed','verified','rejected','unknown')),
    source                  TEXT NOT NULL,
    evidence                TEXT NOT NULL,
    as_of                   TEXT NOT NULL,
    rubric                  TEXT,
    rubric_version          TEXT,
    rationale               TEXT,
    evaluator_type          TEXT,
    evaluation_date         TEXT,
    source_operation_hash   BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash) = 32),
    review_operation_hash   BLOB REFERENCES operations(operation_hash) CHECK(review_operation_hash IS NULL OR length(review_operation_hash) = 32),
    PRIMARY KEY(app_id, community_id, assertion_id),
    FOREIGN KEY(app_id, community_id, tool_key)
        REFERENCES toolkit_tools(app_id, community_id, tool_key),
    FOREIGN KEY(app_id, community_id, concept_key, concept_revision)
        REFERENCES toolkit_concepts(app_id, community_id, concept_key, revision_hash),
    CHECK(
        (value_kind='none' AND value_json IS NULL AND value_number IS NULL AND value_text IS NULL AND value_boolean IS NULL)
        OR (value_kind='boolean' AND value_json IS NOT NULL AND value_number IS NULL AND value_text IS NULL AND value_boolean IS NOT NULL)
        OR (value_kind='number' AND value_json IS NOT NULL AND value_number IS NOT NULL AND value_text IS NULL AND value_boolean IS NULL)
        OR (value_kind='text' AND value_json IS NOT NULL AND value_number IS NULL AND value_text IS NOT NULL AND value_boolean IS NULL)
    )
) WITHOUT ROWID, STRICT;

CREATE TABLE IF NOT EXISTS toolkit_reviews (
    app_id                  TEXT NOT NULL,
    community_id            TEXT NOT NULL,
    review_id               TEXT NOT NULL,
    assertion_id            TEXT NOT NULL,
    state                   TEXT NOT NULL CHECK(state IN ('verified','rejected')),
    reviewer                TEXT NOT NULL,
    rationale               TEXT NOT NULL,
    review_date             TEXT NOT NULL,
    source_operation_hash   BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash) = 32),
    PRIMARY KEY(app_id, community_id, review_id),
    FOREIGN KEY(app_id, community_id, assertion_id)
        REFERENCES toolkit_assertions(app_id, community_id, assertion_id)
) WITHOUT ROWID, STRICT;

CREATE INDEX IF NOT EXISTS toolkit_assertions_tool_concept
    ON toolkit_assertions(app_id, community_id, tool_key, concept_key, concept_revision);
CREATE INDEX IF NOT EXISTS toolkit_assertions_concept_state
    ON toolkit_assertions(app_id, community_id, concept_key, concept_revision, effective_state);
CREATE INDEX IF NOT EXISTS toolkit_assertions_exact_text
    ON toolkit_assertions(app_id, community_id, concept_key, value_text, effective_state);
CREATE INDEX IF NOT EXISTS toolkit_assertions_exact_boolean
    ON toolkit_assertions(app_id, community_id, concept_key, value_boolean, effective_state);
CREATE INDEX IF NOT EXISTS toolkit_assertions_numeric_range
    ON toolkit_assertions(app_id, community_id, concept_key, value_number, effective_state);
CREATE INDEX IF NOT EXISTS toolkit_reviews_assertion
    ON toolkit_reviews(app_id, community_id, assertion_id, state);
