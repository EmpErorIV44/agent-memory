# Link diagnostics and renaming

`memory_link_diagnostics` inspects links in the project's readable sources.
Optional source_id and path restrict originating documents; path requires source_id.
The response includes link coordinates, target and status:

- missing: the target is absent or inaccessible.
- ambiguous: a wiki target has multiple candidates.
- stale_path: the index retained a link through a stable ID, but the physical path
  in source Markdown no longer resolves to the same note.

These diagnostics cover local Markdown/wiki links. External URLs and images are
not checked; fragments after # are preserved, but heading anchor existence is not
checked. Semantic memory.relations remain a separate model.

```json
{"project":"agent-memory","source_id":1}
```

To rename, first call `memory_rename` without expected_plan_hash:

```json
{"project":"agent-memory","source_id":1,"from":"old.md","to":"notes/new.md"}
```

The plan contains the source hash, source Markdown manifest, modified content of
the moved note, incoming link edits, warnings and plan_hash. Review it and pass
plan_hash as expected_plan_hash with the same paths. The application rebuilds the
plan and checks files and permissions; it does not accept client-supplied content
for application. New or changed notes invalidate the plan. Applying through MCP
requires --allow-write and can_write; preview requires only read access. Unsafe
paths, symlinks and existing destinations are rejected.

Incoming relative Markdown/wiki links are rewritten, along with outgoing relative
links in the moved note when its directory changes. Stable id: references remain
unchanged. Labels, wiki aliases, fragments, code, frontmatter and external links
are preserved. Ambiguous links are not guessed.

Reference definitions change only when used by a text link. Unused and image-only
definitions remain unchanged. If a definition is shared by a text link and an
image, it changes for the text link.

The manifest is checked before publication, but file locking covers individual
writes. Concurrent edits from an external editor or new client can appear after
that check: rerun diagnostics after renaming.

Multi-file publication is **not atomic**. First the destination is created without
overwriting, then the source is deleted with a hash check, then repairs are saved
individually. Each write uses existing hash protection and history. History remains
bound to paths: the old path's archive is preserved and the new path starts its own.

The response reports operation_id, complete, partial, moved, applied, destination_present,
source_present, error and indexing state. An error may leave both copies or only
some repaired links. Duplicate stable IDs may also cause indexing to fail. Inspect
the resulting paths and applied first, fix the cause and build a fresh plan; do not
blindly repeat the old operation. After publication, the application attempts to
refresh the index and memory_changes, reporting any error separately.

## Resuming interrupted operations

Before the first Markdown mutation, applying a reviewed plan durably saves its
complete content and expected hashes in `.agent-memory-operations/` inside the
source. The journal uses immutable checksummed JSON records and separate durable
completion markers. It is independent of SQLite and excluded from indexing and
ordinary Git sync. On POSIX systems, journal directories and files are private
(`0700` and `0600`); Windows source ACLs still apply. Preserve this directory in
backups if unfinished operations must remain resumable.

`operation_id` identifies the saved operation. If a process exits before returning
its result, list the source's operations to discover it:

```json
{"tool":"memory_rename_operations","arguments":{"project":"agent-memory","source_id":1}}
{"tool":"memory_rename_resume","arguments":{"project":"agent-memory","source_id":1,"operation_id":"<operation SHA-256>"}}
```

The list returns an array of operations, each with `operation_id`, `from`, `to` and `complete`.
A completion marker records that the operation finished earlier; it does not
certify that its files remain unchanged. Listing requires current read access.
Resuming requires current read/write access and MCP `--allow-write`. A journal
cannot grant access or authorize another project's operation. Affected structured
Memory records must belong to the authorized project, including in shared sources.

Resume recognizes destination and repair content that already matches the planned
hashes, including a destination published before source deletion. It validates
both copies before removing the original. Missing or divergent affected content
stops progress without overwriting the conflict. All repairs are checked before
further publication; individual writes still use hash guards. Resolve conflicts
explicitly and resume again with the same ID. A completed operation resumes as a
no-op for Markdown and does not overwrite later edits. Multi-file publication
remains non-atomic; external editors can still race individual steps.

After recreating SQLite, register the source with the same project and current
permissions, list its journals and resume using its newly assigned source ID.
Numeric source IDs stored in an old plan are not used as authorization. If both
source and destination remain after a crash, their duplicate document ID can
prevent indexing: recover the operation after registration, before indexing or a
full `rebuild`. No file mutation is needed to list journals. Corrupt or symlinked
journals are rejected rather than silently discarded.
```sh
agent-memory link-diagnostics --project agent-memory --source-id 1
agent-memory rename --project agent-memory --source-id 1 --from old.md --to notes/new.md
agent-memory rename --project agent-memory --source-id 1 --from old.md --to notes/new.md --expected-plan-hash '<reviewed hash>'
agent-memory rename-operations --project agent-memory --source-id 1
agent-memory rename-resume --project agent-memory --source-id 1 --operation-id '<operation SHA-256>'
```
