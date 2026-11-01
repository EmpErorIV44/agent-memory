# SQLite schema

`001_initial.sql` creates the initial v0.1 schema; `002_memory_projection.sql`
adds a rebuildable Memory/Evidence/Relation projection and internal ID sequences.
SQLite 3.38 or newer with FTS5 is required (`STRICT`, `unixepoch()` and
`RETURNING`). `003_search_metadata.sql` adds a full-text index of paths,
document IDs, Memory IDs and summaries. `004_document_links.sql` adds the
outgoing-link projection and an index for backlinks. After migration, the next
`index` populates links from Markdown, including unchanged documents. Targets
resolve within one physical source; backlinks are computed when read.
The current `PRAGMA user_version` is 7; see the v5, v6, and v7 sections below.

At startup, the application reads the version, obtains `BEGIN IMMEDIATE` for an
upgrade and checks the version again under the lock: concurrent clients cannot
run the same migration twice. The initial schema and required upgrades are
applied in one transaction; an error rolls back the changes. Existing documents
and FTS data are preserved. SQL files can also be applied separately in version
order: they contain their own transaction wrappers, which the application strips
when executing them inside its transaction. Directly applying a migration twice
intentionally fails.

## Connection setup

Before migration and before starting any transactions:

```sql
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA journal_mode = WAL;
```

Set `foreign_keys` and `busy_timeout` for every connection. WAL persists for the
database file; the application checks the resulting mode when opening it
(`:memory:` does not support WAL). WAL allows readers during writes, but does not
coordinate indexing jobs. Check the hash, update the document and replace all
its chunks in one transaction with coordinated writers. PRAGMA settings are
intentionally kept outside migrations.

## Model

- `projects` defines named access namespaces.
- `sources` stores the canonical absolute path of a registered directory.
- `project_sources` binds a source to any number of projects and separates read
  and write permissions. Sources are read-only by default.
- `documents` describes a physical file within a source. `content_hash` is the
  lowercase SHA-256 of the original file bytes; `file_mtime_ns` is the Unix
  modification time in nanoseconds; `byte_size` is the size. The hash determines
  content changes; time and size are hints, not proof that content is unchanged.
- `stable_id` is the optional frontmatter `id`, unique within a source. Copies in
  different sources are allowed. A duplicate within a source is a reported
  indexing error. The ID is extracted from YAML when reading the source; a rename
  that preserves the ID preserves document identity. Replacing an explicitly
  assigned ID removes the old identity and adds a new one. Documents without IDs
  are supported by path. Migration backfills metadata even if the hash is unchanged.
- `document_chunks` stores search chunks, headings and inclusive source Markdown
  line numbers, starting at 1. `ordinal` starts at 0.
- `document_chunks_fts` is an external-content FTS5 index; triggers synchronize
  inserts, updates and deletes, including cascading document/source deletion.
  `unicode61` supports Unicode but does not perform Russian morphology.
- `document_metadata_fts` separately stores shared document paths/IDs and
  project-scoped Memory IDs/summaries. Triggers update it on changes, renames and
  deletion; migration populates the index from existing projections. Search checks
  the Memory project before returning matches, even if two projects share a source.
  Before v6, the whole query was evaluated independently against the text and
  metadata indexes, so `AND` between a text-only word and a metadata-only word
  was unsupported. The current v6 projections combine content and metadata in
  one project-scoped row and support such queries. Search returns actual document
  chunks; documents without text chunks are not returned. Russian endings can
  be searched with a prefix, for example `правил*`, or through v6 Russian stemming.

`created_at` and `indexed_at` are Unix UTC seconds. Rebuilding the index requires
Markdown and a separately saved configuration of project/source registrations
and permissions: migrations do not recover these from the files themselves.
During `rebuild`, owner configuration atomically restores registrations,
permissions, documents, FTS and structured knowledge; Markdown does not create
projects or grant permissions. Durable author revisions are stored separately
from the rebuildable index; see [revisions](../docs/revisions.md).

## Structured knowledge

YAML may contain a separate `memory` block with its own `id`, explicit `project`,
`kind`, `status`, `summary`, `evidence` and `relations`. A logical Memory ID differs
from a document ID; one document may describe one Memory. Source data remains in
Markdown, while `memories`, `memory_evidence` and `memory_relations` are projections
rebuilt by indexing. The pair `(project_id, memory_id)` is unique.

A regular `index --project` updates structured records only for the selected
project. This prevents a stale record in another project's private source from
blocking indexing of a shared source. When a physical document changes, its old
structured records are invalidated for every binding; a rename without byte
changes preserves them. An owner `rebuild` updates all explicitly authorized projects.

`id_sequences` allocates increasing INTEGER IDs for projects, sources, documents
and chunks. Deletion or access revocation does not allow ID reuse during the
SQLite database's lifetime; saved chunk references cannot start pointing to a new
document with identical bytes. Unchanged registrations and documents retain their
IDs during `rebuild`. After complete database deletion, only author-assigned
Markdown IDs remain permanent identifiers.

## Access boundary

The application first obtains an authorized set of projects. A client-supplied
`project_id` or `scope=global` does not itself grant permission. For one already
authorized project, search looks like this (all values are bound parameters):

```sql
SELECT c.id, d.id AS document_id, d.relative_path,
       c.heading, c.body, c.start_line, c.end_line
FROM document_chunks_fts
JOIN document_chunks AS c ON c.id = document_chunks_fts.rowid
JOIN documents AS d ON d.id = c.document_id
WHERE document_chunks_fts MATCH :query
  AND EXISTS (
      SELECT 1 FROM project_sources AS ps
      WHERE ps.source_id = d.source_id
        AND ps.project_id = :authorized_project_id
        AND ps.can_read = 1
  )
ORDER BY c.id
LIMIT :limit;
```

For global search, use the same `EXISTS` with the authorized set of IDs; return
an empty result immediately for an empty set. `EXISTS` avoids duplicating documents
from sources accessible through multiple projects. Direct `memory_get` by document
or chunk ID must include the same source check **in SQL**, before reading text or
files from disk. SQLite does not provide row-level security here: application
services and correct queries enforce isolation.

Structured `list/get` checks both the Memory project and its document's `can_read`
before reading fields. Relations are returned only for existing, readable Memory
records in the same project. Evidence references of kind `document` are returned
only if the path or stable ID resolves to a readable project document. Other
Evidence consists of unverified author references: the application does not open
URLs/files or run tests automatically. Writing through project A cannot declare
`memory.project: B`, even for a shared physical source.

Deleting a project removes its bindings but preserves physical sources and
documents that other projects may access. Deleting a source cascades through
documents, chunks and FTS. The indexer reflects Markdown file deletion by deleting
the corresponding `documents` record in a transaction.

## Paths and writes

Before registration, the application canonicalizes paths, accounting for case
and Windows specifics. `relative_path` uses `/`; absolute paths, `..`, empty
components and root escapes are prohibited. Checking the actual resolved path,
symlinks/junctions and guarding against path substitution when opening files are
the filesystem adapter's responsibilities; a nonempty-path SQL check cannot
replace them. Sources with overlapping roots require an explicit registration
policy, otherwise one physical file may have multiple representations.

Every Markdown write must separately check the authorized project, `can_write`
and the global setting that enables agent writes. Source registration and
permission changes are management operations unavailable through search.
Markdown content is untrusted data.

## Initial migration validation

Historically verified with SQLite 3.53.1 in memory: application and rejection of
duplicate application, FTS insert/update/delete, cascading FTS cleanup, isolation
of two projects and read denial with `can_read=0`, FK/CHECK constraints, document
rollback together with FTS, `foreign_key_check`, `integrity_check` and FTS5
`integrity-check` with external-content checking. This is a historical check of
the initial schema. Current CLI, filesystem adapter, MCP, migrations, recovery
and structured projections are covered by Cargo tests.

## Project change journal (v5)

`005_project_changes.sql` adds project clocks, observed document snapshots and
an append-only change feed. Project revisions advance in the indexing transaction;
event sequences use an explicit AUTOINCREMENT primary key to survive VACUUM.
Opaque cursors include a per-project epoch, so rebuilding a deleted SQLite index
requires a new baseline. Markdown remains authoritative; the feed captures only
states observed by successful scans. See [changes](../docs/changes.md).

## Russian search (v6)

`006_russian_search.sql` adds project-scoped exact and Russian Snowball FTS
projections. `Store::open` backfills them from the existing index inside the
migration transaction; indexing reconciles them by content fingerprint. The
exported `SCHEMA_VERSION` is shared by migration and doctor validation.
See [search](../docs/search.md) for query semantics and typed filters.

# Authored note metadata (v7)

`007_note_metadata.sql` adds optional authored creation/update date columns and
`tags_json` to documents. These are rebuildable Markdown projections, distinct
from filesystem mtime and indexing timestamps. Existing rows default to null
dates and empty tags until the next successful index refresh, which backfills
even documents with unchanged content hashes. Memory list/get stay index-only;
full-document reads parse the same fields from hash-verified source content.
See [note dates and tags](../docs/note-metadata.md).
