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
`{"record":...}`. Each record contains a `memory` object, `document_id`, `source_id`,
`path` and the full document's `content_hash`. To bootstrap constraints and
preferences, use the list and then read the complete source document:

```json
{"tool":"memory_get","arguments":{"project":"banking","source_id":1,"path":"decisions/auth.md","content_hash":"<full-document SHA-256>"}}
```

The `{"document":...}` response contains `content` with the complete Markdown,
including frontmatter, and the full file's `content_hash`. Use coordinates and a
hash from an actual record. `memory_get_record` returns a structured projection,
not complete Markdown. The `memory_get` mode using `chunk_id` and a hash remains
available; add `include_document=true` for the full file alongside the chunk.

Create and update structured records through ordinary `memory_write`: provide
complete Markdown and, for updates, the current full-document `expected_hash`.
Structural metadata is validated before file publication. Existing `indexed=false`
semantics remain: if the file was published but subsequent indexing failed, do not
repeat the write; fix the cause and run `index`.

SQLite v1 → v2 migration runs when the database is opened by the newer application.
An old v1 MCP process cannot open v2: restart the MCP client after updating the
binary. [Durable revision history](revisions.md) is stored in the source separately
from the index and is available through CLI and MCP.
