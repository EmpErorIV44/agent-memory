# Stable documents, recovery and structured memory

Markdown stores knowledge; owner configuration stores registrations and permissions;
SQLite stores rebuildable projections. Documents, search chunks and logical Memory
statements have different identifiers.

## Document IDs and frontmatter

```yaml
---
id: ADR-001
title: Agent Memory architecture
created: 2026-11-01
updated: 2026-11-02
status: accepted
tags: [architecture, local-first]
---
```

`id` is an optional nonempty string, unique within a physical source. Renaming a
file within the same source preserves its document row ID in the current SQLite
database. Files without IDs are supported; their identity is determined by path.
Numeric SQLite IDs may change after a full rebuild: use string IDs for permanent
references rather than `chunk_id`.

Optional `created`/`updated` calendar dates and a validated `tags` list describe
the authored note. They are returned in structured Memory and full-document
reads, remain distinct from index/filesystem timestamps, and survive rebuilds
from Markdown. See [note dates and tags](note-metadata.md) for validation,
schema-upgrade backfilling, and opt-in write stamping.

Unknown root-level ADR YAML fields are allowed. Numeric, empty or duplicate IDs,
unclosed frontmatter and invalid structured fields return an error. The ID is
limited to 256 bytes and frontmatter to 64 KiB. Search line numbers refer to the
original file, including frontmatter. The complete YAML is not included in FTS5
search; a separate metadata index contains the path, document ID, `memory.id` and
`memory.summary` within the accessible project. The whole query is evaluated
against the combined project view of text and metadata: `AND` can combine a word
from text with a word from metadata. Metadata matches return existing document
chunks; read documents without text chunks through `memory_list` and path-based
`memory_get`. Russian word forms are matched using Snowball stemming; kind/status
filters are available. See [search](search.md).

## Owner configuration and recovery

Export current registrations:

```sh
agent-memory --database ./agent-memory.sqlite config export --output ./agent-memory.config.json
```

Example JSON:

```json
{
  "version": 1,
  "projects": [
    {
      "name": "agent-memory",
      "sources": [
        {"path": "./knowledge", "can_read": true, "can_write": true}
      ]
    }
  ]
}
```

Relative paths are resolved against the configuration directory. Export writes
canonical absolute paths. Change these explicitly when moving to another device.
The configuration does not contain note text and is not a backup of the notes.
If the configuration file itself is a symbolic link, relative paths are resolved
against its target file's directory. The owner may explicitly specify a source
root through a symbolic link: loading converts it to a canonical path. The indexer
then opens the registered root without following substituted symbolic links;
symbolic links inside the source are skipped.

```sh
agent-memory --database ./agent-memory.sqlite rebuild --config ./agent-memory.config.json
```

This command restores projects, source bindings, permissions, documents, FTS5 and
structured knowledge. The configuration defines the complete set of registrations:
bindings absent from it are revoked. Read errors, duplicates or invalid metadata
roll back the transaction, preserving the existing working database. The open
database file is not replaced, so WAL and existing connections do not remain
attached to a deleted file. `rebuild` does not write Markdown.

Save a new configuration after changing registrations through the CLI. Without
`--output`, `config export` writes JSON to stdout; `--output` refuses to overwrite
an existing file. A new file is published in full after a temporary file in the
same directory has been written and synced; failures before publication do not
leave a partially written manifest. Updating an existing manifest is an explicit
owner action. Note content cannot register a source or grant access permissions.

## Memory, Evidence and Relation

A Markdown document can declare one logical Memory in its `memory` block. This is
a minimal format: ordinary documents may have only an `id` or no frontmatter at all.

```markdown
---
id: doc-auth-policy
memory:
  id: auth-policy
  project: banking
  kind: decision
  status: accepted
  summary: Sessions expire after 30 minutes
  evidence:
    - kind: document
      reference: decisions/session-policy.md
      description: Decision accepted by the project owner
  relations:
    - kind: supersedes
      target: previous-auth-policy
---
# Session lifetime

A session lasts 30 minutes. The rationale and constraints of the decision are described here.
```

Values:

| Field | Allowed values |
| --- | --- |
| `memory.kind` | `fact`, `decision`, `implementation`, `constraint` |
| `memory.status` | `planned`, `implemented`, `accepted`, `superseded`, `deprecated` |
| `evidence.kind` | `document`, `commit`, `file`, `test`, `url`, `manual` |
| `relations.kind` | `supersedes`, `relates_to`, `depends_on`, `implements`, `contradicts` |

`memory.id` is unique within a project and need not match the document ID.
`project`, `kind`, `status` and a nonempty `summary` are required. `evidence` and
`relations` default to empty lists. A relation targets another Memory ID in the
same project. Unknown fields inside structured Memory are rejected so that typos
do not silently lose knowledge.

The source must be explicitly bound to the specified project with read permission;
`memory.project` does not grant access. CLI/MCP writes cannot declare a Memory for
another project, even when the physical source is shared. Direct reads and lists
check current permissions. Only relations with existing, accessible targets in the
same project are returned; hidden and unresolved targets are omitted. Self-links
and identical duplicate relations are rejected.

Evidence references with `kind: document` are resolved by a path relative to the
source root or a string document ID among the project's accessible sources.
Unresolved references are omitted from structured responses but remain in the
original YAML. A `memory_get_record` response therefore does not replace the full
Markdown: read the original document and preserve its other fields when updating.

Evidence is a stored reference, not the result of automatic verification. Explicit
local [Git Evidence verification](git-evidence.md) is available through
`memory_verify_evidence` for owner-configured repositories; it does not change
knowledge status. The indexer does not open URLs, run tests or execute Git commands.
A `supersedes` relation records the relationship; it does not itself change the
target Memory's status. Edit the corresponding Markdown explicitly to change it.

CLI:

```sh
agent-memory --database ./agent-memory.sqlite memory list --project banking
agent-memory --database ./agent-memory.sqlite memory get --project banking auth-policy
```

MCP:

```json
{"tool":"memory_list","arguments":{"project":"banking"}}
{"tool":"memory_get_record","arguments":{"project":"banking","memory_id":"auth-policy"}}
```

`memory_list` returns `{"memories":[...]}` and `memory_get_record` returns
`{"record":...}`. Compact list entries contain `memory.id/kind/status/summary`, document `title`,
`source_id`, `path` and the full document's `content_hash`. Evidence, relations,
tags and other full-record metadata are omitted, not reported as empty. Use
`include_details: true` in MCP or CLI `memory list --full` for the previous full
list shape, or `memory_get_record`/CLI `memory get` for one complete structured
record. Full records retain document IDs, metadata, evidence and authorized
relations. Compact listing uses one scoped query without loading those details. To bootstrap constraints and
preferences, use the list and then read the complete source document:

```json
{"tool":"memory_get","arguments":{"project":"banking","source_id":1,"path":"decisions/auth.md","content_hash":"<full-document SHA-256>"}}
```

The `{"document":...}` response contains `content` with the complete Markdown,
including frontmatter, and the full file's `content_hash`. Use coordinates and a
hash from an actual record. `memory_get_record` returns a structured projection,
not complete Markdown. The `memory_get` mode using `chunk_id` and a hash remains
available and returns only the chunk by default. Use this for narrow task
questions when the chunk provides enough context. Prefer `source_id + path` for
full-note reads: adding `include_document=true` in chunk mode returns the full
file alongside the chunk, repeating part of the text. Chunks do not replace
required baseline/selected preference reads or the complete current document
before an update; read more when omitted context affects the answer.

Create and update structured records through ordinary `memory_write`: provide
complete Markdown and, for updates, the current full-document `expected_hash`.
Structural metadata is validated before file publication. Existing `indexed=false`
semantics remain: if the file was published but subsequent indexing failed, do not
repeat the write; fix the cause and run `index`.

SQLite v1 → v2 migration runs when the database is opened by the newer application.
An old v1 MCP process cannot open v2: restart the MCP client after updating the
binary. [Durable revision history](revisions.md) is stored in the source separately
from the index and is available through CLI and MCP.

## Filter and page memory lists

MCP `memory_list` accepts optional `kind`, `status`, `tag` and `source_id` filters.
Filters combine with AND; `tag` is one exact, case-sensitive tag, not a substring
or FTS query. Only readable project sources and that project's structured memories
are eligible. Invalid kind/status values and empty tags are rejected.

```json
{"project":"banking","kind":"constraint","status":"accepted","tag":"auth","limit":20,"offset":0}
```

`limit` (1..200) or `offset` opts into pagination, ordered by logical Memory ID.
Offset-only requests default to 50 entries. The response retains `memories` and
adds `limit`, `offset`, `has_more`, and `next_offset` (null at the end). Filters
apply before pagination. Follow `next_offset` until `has_more` is false when a
complete filtered list is required. Pages are live observations, not a snapshot;
edits between requests can shift offsets. Restart listing if consistency matters.

Without either paging parameter the previous unbounded envelope is retained.
`include_details: true` applies the same filters/page to full records, loading
evidence/relations only for selected entries. No total-count query is performed.

CLI accepts the same flags, with `--full` for details:

```sh
agent-memory memory list --project banking --kind constraint --status accepted \
  --tag auth --limit 20 --offset 0
```

CLI preserves its legacy array output without paging flags. With `--limit` or
`--offset`, it emits the same paginated envelope as MCP. All filters are optional;
`--source-id` scopes a list to one readable source.

## Read several complete documents with one budget

MCP `memory_get_many` accepts 1..50 unique `documents` selectors from list/search
coordinates and an optional `max_bytes` (4096..1048576, default 65536):

```json
{"project":"banking","documents":[{"source_id":1,"path":"decisions/auth.md","content_hash":"<full SHA-256>"}],"max_bytes":65536}
```

This operation refreshes the project index once, then checks each selector through
the same source/path/current-hash read used by `memory_get`. It accepts path
selectors only, not chunk IDs. CLI reads the selector array from a file:

```sh
agent-memory memory get-many --project banking --documents-file selectors.json \
  --max-bytes 65536
```

`results` follows input order; each row contains its zero-based input `index` and:

- `status: "ok"` plus `document`, containing the complete Markdown and metadata.
- `status: "omitted_budget"` with no document text. Retry selected omissions with
  a larger budget or fewer documents, or use ordinary `memory_get`.
- `status: "unavailable"` for missing, inaccessible, stale or unreadable documents.
  These cases intentionally share one response. Refresh discovery coordinates
  before retrying; the batch does not claim to have read these notes.

No document is truncated or replaced by an excerpt. A large omitted document does
not prevent later smaller documents from fitting. `omitted_count` and
`unavailable_count` count each category; `complete` is true only when every item
returned a full document. This is a sequence of hash-checked reads, not an atomic
snapshot, and it does not establish truth or semantic consistency.

`used_bytes` is the exact UTF-8 size of the compact JSON report, including result
metadata, omissions and the counter itself, and is at most `max_bytes`. This is
not a token estimate, an input/file-I/O limit, or a limit on MCP envelopes, CLI
pretty-printing, or duplicated compatibility output in `--response-format both`.
An invalid/duplicate selector or out-of-range budget fails the whole request;
per-document failures are retained as `unavailable`. Required complete-note reads
are satisfied only by `ok` entries, never by an omitted/unavailable result.
