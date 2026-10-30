-- Derived outgoing links; incoming links are queried, never duplicated in Markdown.
BEGIN IMMEDIATE;
CREATE TABLE document_links (
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    target TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('markdown', 'wiki')),
    start_line INTEGER NOT NULL CHECK (start_line >= 1),
    source_hash TEXT NOT NULL,
    target_document_id INTEGER REFERENCES documents(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('resolved', 'missing', 'ambiguous')),
    PRIMARY KEY (document_id, ordinal)
) STRICT;
CREATE INDEX document_links_incoming ON document_links(target_document_id);
PRAGMA user_version = 4;
COMMIT;
