BEGIN IMMEDIATE;
CREATE TABLE project_clocks (
 project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
 epoch TEXT NOT NULL DEFAULT (lower(hex(randomblob(16)))),
 revision INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE project_snapshot (
 project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 document_id INTEGER NOT NULL,
 source_id INTEGER NOT NULL,
 path TEXT NOT NULL,
 content_hash TEXT NOT NULL,
 stable_id TEXT,
 PRIMARY KEY(project_id, document_id)
);
CREATE TABLE project_changes (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL,
 document_id INTEGER NOT NULL,
 source_id INTEGER NOT NULL,
 kind TEXT NOT NULL,
 path TEXT NOT NULL,
 previous_path TEXT,
 content_hash TEXT NOT NULL,
 stable_id TEXT
);
CREATE INDEX project_changes_revision ON project_changes(project_id, revision);
PRAGMA user_version = 5;
COMMIT;
