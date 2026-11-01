-- Embeddings are disposable project-scoped cache, not authored knowledge.
BEGIN IMMEDIATE;
CREATE TABLE semantic_vectors (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    chunk_id INTEGER NOT NULL REFERENCES document_chunks(id) ON DELETE CASCADE,
    model_key TEXT NOT NULL CHECK(length(model_key) BETWEEN 1 AND 1024),
    input_hash TEXT NOT NULL CHECK(length(input_hash)=64),
    dimensions INTEGER NOT NULL CHECK(dimensions BETWEEN 1 AND 8192),
    vector_json TEXT NOT NULL,
    PRIMARY KEY(project_id, chunk_id, model_key)
) STRICT;
PRAGMA user_version=8;
COMMIT;
