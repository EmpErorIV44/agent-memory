-- Authored note metadata is a rebuildable projection, never filesystem dates.
BEGIN IMMEDIATE;

ALTER TABLE documents ADD COLUMN authored_created TEXT;
ALTER TABLE documents ADD COLUMN authored_updated TEXT;
ALTER TABLE documents ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]';

PRAGMA user_version=7;
COMMIT;
