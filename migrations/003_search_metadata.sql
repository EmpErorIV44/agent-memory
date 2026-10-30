-- Search document identifiers and project-scoped structured metadata.
BEGIN IMMEDIATE;

CREATE VIRTUAL TABLE document_metadata_fts USING fts5(
    -- Preserve existing heading:/body: query syntax. These columns stay empty;
    -- the original chunk index remains responsible for text matches.
    heading,
    body,
    path,
    stable_id,
    memory_id,
    summary,
    document_id UNINDEXED,
    project_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO document_metadata_fts(path, stable_id, document_id)
SELECT relative_path, coalesce(stable_id, ''), id FROM documents;
INSERT INTO document_metadata_fts(memory_id, summary, document_id, project_id)
SELECT memory_id, summary, document_id, project_id FROM memories;

CREATE TRIGGER document_metadata_ai AFTER INSERT ON documents BEGIN
    INSERT INTO document_metadata_fts(path, stable_id, document_id)
    VALUES (new.relative_path, coalesce(new.stable_id, ''), new.id);
END;
CREATE TRIGGER document_metadata_au AFTER UPDATE OF relative_path, stable_id ON documents BEGIN
    UPDATE document_metadata_fts
    SET path = new.relative_path, stable_id = coalesce(new.stable_id, '')
    WHERE document_id = new.id AND project_id IS NULL;
END;
CREATE TRIGGER document_metadata_ad AFTER DELETE ON documents BEGIN
    DELETE FROM document_metadata_fts WHERE document_id = old.id;
END;
CREATE TRIGGER memory_metadata_ai AFTER INSERT ON memories BEGIN
    INSERT INTO document_metadata_fts(memory_id, summary, document_id, project_id)
    VALUES (new.memory_id, new.summary, new.document_id, new.project_id);
END;
CREATE TRIGGER memory_metadata_au AFTER UPDATE ON memories BEGIN
    DELETE FROM document_metadata_fts
    WHERE document_id = old.document_id AND project_id = old.project_id;
    INSERT INTO document_metadata_fts(memory_id, summary, document_id, project_id)
    VALUES (new.memory_id, new.summary, new.document_id, new.project_id);
END;
CREATE TRIGGER memory_metadata_ad AFTER DELETE ON memories BEGIN
    DELETE FROM document_metadata_fts
    WHERE document_id = old.document_id AND project_id = old.project_id;
END;

PRAGMA user_version = 3;
COMMIT;
