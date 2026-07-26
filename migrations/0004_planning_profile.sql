-- Native cross-application planning profile. The built-in profile manifest is
-- code-owned and content-addressed; this migration stores only local grants and
-- rebuildable views of signed profile records.
CREATE TABLE profile_grants (
    app_id          TEXT NOT NULL REFERENCES applications(app_id),
    profile_id      TEXT NOT NULL,
    can_read        INTEGER NOT NULL CHECK(can_read IN (0,1)),
    can_write       INTEGER NOT NULL CHECK(can_write IN (0,1)),
    granted_at_ms   INTEGER NOT NULL,
    PRIMARY KEY(app_id,profile_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE planning_projects (
    namespace_id           TEXT NOT NULL,
    community_id           TEXT NOT NULL,
    project_id             TEXT NOT NULL,
    title                  TEXT NOT NULL,
    description            TEXT,
    status                 TEXT NOT NULL CHECK(status IN ('planned','active','completed','cancelled','archived')),
    profile_digest         TEXT NOT NULL CHECK(length(profile_digest)=64),
    submitted_by_app_id    TEXT NOT NULL,
    source_operation_hash  BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash)=32),
    updated_at_ms          INTEGER NOT NULL,
    PRIMARY KEY(namespace_id,community_id,project_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE planning_tasks (
    namespace_id           TEXT NOT NULL,
    community_id           TEXT NOT NULL,
    task_id                TEXT NOT NULL,
    project_id             TEXT,
    title                  TEXT NOT NULL,
    description            TEXT,
    status                 TEXT NOT NULL CHECK(status IN ('planned','active','completed','cancelled','archived')),
    start_at_ms            INTEGER,
    due_at_ms              INTEGER,
    progress_percent       INTEGER NOT NULL CHECK(progress_percent BETWEEN 0 AND 100),
    profile_digest         TEXT NOT NULL CHECK(length(profile_digest)=64),
    submitted_by_app_id    TEXT NOT NULL,
    source_operation_hash  BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash)=32),
    updated_at_ms          INTEGER NOT NULL,
    PRIMARY KEY(namespace_id,community_id,task_id),
    FOREIGN KEY(namespace_id,community_id,project_id)
      REFERENCES planning_projects(namespace_id,community_id,project_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE planning_events (
    namespace_id           TEXT NOT NULL,
    community_id           TEXT NOT NULL,
    event_id               TEXT NOT NULL,
    project_id             TEXT,
    title                  TEXT NOT NULL,
    description            TEXT,
    status                 TEXT NOT NULL CHECK(status IN ('planned','active','completed','cancelled','archived')),
    timing_json            BLOB NOT NULL CHECK(length(timing_json)<=8192),
    recurrence_json        BLOB CHECK(recurrence_json IS NULL OR length(recurrence_json)<=8192),
    location               TEXT,
    profile_digest         TEXT NOT NULL CHECK(length(profile_digest)=64),
    submitted_by_app_id    TEXT NOT NULL,
    source_operation_hash  BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash)=32),
    updated_at_ms          INTEGER NOT NULL,
    PRIMARY KEY(namespace_id,community_id,event_id),
    FOREIGN KEY(namespace_id,community_id,project_id)
      REFERENCES planning_projects(namespace_id,community_id,project_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE planning_dependencies (
    namespace_id             TEXT NOT NULL,
    community_id             TEXT NOT NULL,
    dependency_id            TEXT NOT NULL,
    predecessor_task_id      TEXT NOT NULL,
    successor_task_id        TEXT NOT NULL,
    dependency_kind          TEXT NOT NULL CHECK(dependency_kind IN ('finish_to_start','start_to_start','finish_to_finish','start_to_finish')),
    lag_ms                   INTEGER NOT NULL,
    profile_digest           TEXT NOT NULL CHECK(length(profile_digest)=64),
    submitted_by_app_id      TEXT NOT NULL,
    source_operation_hash    BLOB NOT NULL REFERENCES operations(operation_hash) CHECK(length(source_operation_hash)=32),
    updated_at_ms            INTEGER NOT NULL,
    PRIMARY KEY(namespace_id,community_id,dependency_id),
    FOREIGN KEY(namespace_id,community_id,predecessor_task_id)
      REFERENCES planning_tasks(namespace_id,community_id,task_id),
    FOREIGN KEY(namespace_id,community_id,successor_task_id)
      REFERENCES planning_tasks(namespace_id,community_id,task_id),
    CHECK(predecessor_task_id != successor_task_id)
) WITHOUT ROWID, STRICT;

CREATE INDEX planning_tasks_project
    ON planning_tasks(namespace_id,community_id,project_id,start_at_ms,due_at_ms);
CREATE INDEX planning_events_project
    ON planning_events(namespace_id,community_id,project_id);
CREATE INDEX planning_dependencies_tasks
    ON planning_dependencies(namespace_id,community_id,predecessor_task_id,successor_task_id);
