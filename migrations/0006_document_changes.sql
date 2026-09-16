-- Device-local cursors are durable across restarts and VACUUM. This is an
-- invalidation index, not a replication log or a remotely meaningful sequence.
CREATE TABLE document_changes (
    change_id INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_hash BLOB NOT NULL UNIQUE REFERENCES operations(operation_hash),
    app_id TEXT NOT NULL,
    community_id TEXT NOT NULL,
    document_id TEXT NOT NULL
) STRICT;
CREATE INDEX document_changes_scope ON document_changes(app_id, community_id, change_id);

INSERT INTO document_changes(operation_hash,app_id,community_id,document_id)
SELECT operation_hash,app_id,community_id,document_id FROM document_updates
WHERE applied=1 ORDER BY created_at_ms,operation_hash;

CREATE TRIGGER document_changes_after_insert AFTER INSERT ON document_updates
WHEN NEW.applied=1 BEGIN
    INSERT INTO document_changes(operation_hash,app_id,community_id,document_id)
    VALUES(NEW.operation_hash,NEW.app_id,NEW.community_id,NEW.document_id);
END;
