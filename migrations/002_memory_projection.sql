-- Derived structured knowledge and monotonically allocated client-visible IDs.
BEGIN IMMEDIATE;

CREATE TABLE id_sequences (
    entity TEXT PRIMARY KEY,
    next_id INTEGER NOT NULL CHECK (next_id > 0)
) STRICT;
INSERT INTO id_sequences VALUES
    ('projects', (SELECT coalesce(max(id), 0) + 1 FROM projects)),
    ('sources', (SELECT coalesce(max(id), 0) + 1 FROM sources)),
    ('documents', (SELECT coalesce(max(id), 0) + 1 FROM documents)),
    ('document_chunks', (SELECT coalesce(max(id), 0) + 1 FROM document_chunks));

CREATE TABLE memories (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    memory_id TEXT NOT NULL CHECK (length(trim(memory_id)) > 0),
    document_id INTEGER NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('fact', 'decision', 'implementation', 'constraint')),
    status TEXT NOT NULL CHECK (status IN ('planned', 'implemented', 'accepted', 'superseded', 'deprecated')),
    summary TEXT NOT NULL CHECK (length(trim(summary)) > 0),
    PRIMARY KEY (project_id, memory_id)
) STRICT;

CREATE TABLE memory_evidence (
    project_id INTEGER NOT NULL,
    memory_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('document', 'commit', 'file', 'test', 'url', 'manual')),
    reference TEXT NOT NULL CHECK (length(trim(reference)) > 0),
    description TEXT,
    PRIMARY KEY (project_id, memory_id, ordinal),
    FOREIGN KEY (project_id, memory_id) REFERENCES memories(project_id, memory_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE memory_relations (
    project_id INTEGER NOT NULL,
    memory_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('supersedes', 'relates_to', 'depends_on', 'implements', 'contradicts')),
    target_memory_id TEXT NOT NULL CHECK (length(trim(target_memory_id)) > 0),
    PRIMARY KEY (project_id, memory_id, kind, target_memory_id),
    UNIQUE (project_id, memory_id, ordinal),
    FOREIGN KEY (project_id, memory_id) REFERENCES memories(project_id, memory_id) ON DELETE CASCADE
) STRICT;

PRAGMA user_version = 2;
COMMIT;
