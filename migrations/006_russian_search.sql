-- Project-scoped combined projections maintained by Rust during indexing.
BEGIN IMMEDIATE;
CREATE VIRTUAL TABLE search_exact_fts USING fts5(
 heading, body, title, path, stable_id, memory_id, summary,
 project_id UNINDEXED, chunk_id UNINDEXED, fingerprint UNINDEXED,
 tokenize='unicode61 remove_diacritics 2'
);
CREATE VIRTUAL TABLE search_russian_fts USING fts5(
 heading, body, title, path, stable_id, memory_id, summary,
 project_id UNINDEXED, chunk_id UNINDEXED, fingerprint UNINDEXED,
 tokenize='unicode61 remove_diacritics 2'
);
PRAGMA user_version=6;
COMMIT;
