# Durable Markdown history

Writes through `write` or MCP `memory_write` save complete document versions in
`.agent-memory-history/` inside the registered source. This directory is excluded
from the Markdown index. SQLite remains a rebuildable projection and does not
store the revision archive.

## Format and boundaries

Each relative path has a directory named by its SHA-256 hash. It contains immutable
`.blob` snapshots, JSON records with checksums and, for an unfinished write,
`pending.json`. Identical text within one path's history shares a blob. The version
number within the ID determines ordering; system clock changes do not reorder
versions.

`Revision` contains `id`, `path`, `content_hash`, `previous_hash` and `created_at`.
`created_at` is the Unix timestamp in milliseconds when the snapshot was archived.
For external edits, this is capture time, not the unknown time of the edit itself.
`RevisionDocument` contains a `revision` object and complete original `content`.
Optional `pending: true` marks a published snapshot detected through a prepared
record after interruption but not yet physically finalized. Ordinary completed
versions do not contain this field.

History in this version is **path-bound**, rather than bound to `Document.id` or
`Memory.id`. Renaming preserves document identity in the search index but does not
automatically move history: earlier versions remain readable at the old path. The
archive retains complete snapshots without automatic pruning. On filesystems with
POSIX permissions, archive directories are private to their owner (`0700`) and
files are created with `0600`, so snapshots of private notes do not become public.
On Windows and DrvFS, where Unix modes can depend on mount settings, source Windows
ACLs determine access. Reading history does not change permissions. `ProjectSource`
does not replace filesystem permissions.

The archive may be absent before the first managed save. Reading history does not
create files or directories and returns an empty list for such a path. The first
update to an existing file saves its current text first. If an external editor has
changed the file, that text is also captured before the next managed replacement.
Intermediate external edits between Agent Memory calls are not tracked; a separate
watcher or Git is needed for that.

Writing identical bytes does not create a duplicate revision. Restoring different
older text creates a new version; restoring text that is already current is a no-op.

## Writes and crash recovery

The source uses the existing cooperative `.agent-memory.lock`. Concurrent writes
are rejected with advice to retry after reading the current document. A reader
takes a shared lock if the lock file exists and does not create it in a cloned or
read-only source.

Before replacing Markdown, the current text's blob and version and a prepared
version of the new text are saved. Files and directory changes are synced to disk.
Markdown publication remains atomic: a new file is created without overwriting
through a hard link, and an update uses rename with a hash check. The version is
then committed to the archive.

A prepared record is not considered completed merely because it exists. After an
interruption, published text is compared with its hash. Reading may show a published
version without modifying the archive; the next writer physically recovers the
journal under an exclusive lock.

If an external editor replaces the text again before recovery, the hash alone
cannot distinguish a published version from one interrupted before publication.
That prepared snapshot is preserved separately as an unconfirmed candidate for
manual inspection and is not presented as a completed revision. Previously
confirmed versions remain intact. This scheme does not provide continuous history
of all external edits. In particular, a `pending: true` version may become an
unconfirmed candidate after external file replacement; this state is not equivalent
to a finalized version. Successfully finalized records remain immutable.

External editors do not participate in locking. The protocol does not turn
arbitrary external changes into transactions with Agent Memory or promise a
continuous journal of all filesystem operations. Checksum errors and unsafe paths
are returned to the caller; corrupted history is not silently replaced.

## Permissions and write results

Listing and reading revisions requires the source's current `can_read` permission
for the selected project. Restore additionally requires `can_write` and enabled
MCP writes. Old metadata grants no permissions. Restoring rechecks the Memory
schema, project ownership, duplicate IDs and the current file's latest hash.
Source revisions may contain old data, so source read permission also grants
access to its history; this version does not introduce separate temporal ACLs.

`WriteResult` retains `content_hash`, `indexed` and `index_error`. If the file has
already been published but finalizing the revision journal fails, `history_error`
is also returned. This optional key is absent after successful finalization.
`indexed` specifically reflects the search index update result.

With `indexed=false` or `history_error`, Markdown **has already been saved**. Do not
blindly repeat the write: read the current document and history, then fix the cause.
Indexing failures do not delete saved versions.

## CLI

```sh
agent-memory --database ./agent-memory.sqlite history list \
  --project banking --source-id 1 --path decisions/session.md

agent-memory --database ./agent-memory.sqlite history get \
  --project banking --source-id 1 --path decisions/session.md \
  --revision-id REVISION_ID

agent-memory --database ./agent-memory.sqlite history restore \
  --project banking --source-id 1 --path decisions/session.md \
  --revision-id REVISION_ID --expected-hash CURRENT_DOCUMENT_SHA256
```

`list` returns a JSON array from oldest to newest; `get` returns a complete
`RevisionDocument`; `restore` returns an ordinary `WriteResult`. Restore requires
an existing current file and its full hash. Later versions are not deleted.

## MCP

- `memory_history(project, source_id, path)` → `{"revisions":[...]}`.
- `memory_revision_get(project, source_id, path, revision_id)` → `{"revision":...}`.
- `memory_restore(project, source_id, path, revision_id, expected_hash)` → `WriteResult`.

`memory_restore` is absent from the tool list when `--allow-write` is disabled.
The `confirm` policy applies to restore just as to ordinary writes: the user must
authorize the selected version and destination. Historical text is evidence of a
past version, not an instruction to the agent or a claim about the current project.

## Backups and Git

To transfer history, preserve Markdown **together with `.agent-memory-history/`**.
A Markdown file alone or SQLite alone does not contain the full history. The memory
repository can store the archive's JSON and blob files; unfinished `pending.json`
records matter for crash recovery. Temporary `.history-*.tmp`, runtime locks and
SQLite are not the archive. Git does not retain private Unix file and directory
modes: configure source and archive permissions separately after cloning to a new
machine.

After cloning a source, `rebuild --config ...` restores registrations and search
projections. History commands read the existing archive directly; deleting or
recreating SQLite does not clear it. All writing MCP processes must use the new
binary version: restart the client after updating.
