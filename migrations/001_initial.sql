-- Apply once to a new database, outside any caller-owned transaction.
-- Configure foreign_keys, journal_mode=WAL and busy_timeout before BEGIN.
-- Requires SQLite >= 3.38 with FTS5. Markdown remains the source of truth.
BEGIN IMMEDIATE;

CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(trim(name)) > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    root_path TEXT NOT NULL UNIQUE CHECK (length(trim(root_path)) > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE TABLE project_sources (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    can_read INTEGER NOT NULL DEFAULT 1 CHECK (can_read IN (0, 1)),
    can_write INTEGER NOT NULL DEFAULT 0 CHECK (can_write IN (0, 1)),
    PRIMARY KEY (project_id, source_id)
) STRICT;

CREATE INDEX project_sources_by_source ON project_sources(source_id, project_id);

CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL CHECK (length(trim(relative_path)) > 0),
    stable_id TEXT CHECK (stable_id IS NULL OR length(trim(stable_id)) > 0),
    title TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL CHECK (
        length(content_hash) = 64 AND content_hash NOT GLOB '*[^0-9a-f]*'
    ),
    file_mtime_ns INTEGER NOT NULL CHECK (file_mtime_ns >= 0),
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (source_id, relative_path)
) STRICT;

CREATE UNIQUE INDEX documents_by_stable_id
    ON documents(source_id, stable_id) WHERE stable_id IS NOT NULL;

CREATE TABLE document_chunks (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    heading TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL CHECK (length(trim(body)) > 0),
    start_line INTEGER NOT NULL CHECK (start_line >= 1),
    end_line INTEGER NOT NULL CHECK (end_line >= start_line),
    UNIQUE (document_id, ordinal)
) STRICT;

CREATE VIRTUAL TABLE document_chunks_fts USING fts5(
    heading,
    body,
    content = 'document_chunks',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER document_chunks_ai AFTER INSERT ON document_chunks BEGIN
    INSERT INTO document_chunks_fts(rowid, heading, body)
    VALUES (new.id, new.heading, new.body);
END;

CREATE TRIGGER document_chunks_ad AFTER DELETE ON document_chunks BEGIN
    INSERT INTO document_chunks_fts(document_chunks_fts, rowid, heading, body)
    VALUES ('delete', old.id, old.heading, old.body);
END;

CREATE TRIGGER document_chunks_au AFTER UPDATE ON document_chunks BEGIN
    INSERT INTO document_chunks_fts(document_chunks_fts, rowid, heading, body)
    VALUES ('delete', old.id, old.heading, old.body);
    INSERT INTO document_chunks_fts(rowid, heading, body)
    VALUES (new.id, new.heading, new.body);
END;

PRAGMA user_version = 1;
COMMIT;
