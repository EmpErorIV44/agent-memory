# Coordinated Markdown batches

A batch creates or updates 1..50 complete Markdown notes in **one project and one
source**. For example, create an accepted decision and mark its predecessor as
`superseded` in the same reviewed operation. Statuses and semantic relations are
explicit content supplied by the caller; the server never infers them.

Markdown is authoritative. A multi-file batch is **not an atomic filesystem
transaction**: external readers can observe intermediate publications. All known
path/hash/metadata/ID conflicts are checked before publication, a source lock
excludes cooperating writers for the whole batch, and a durable journal supports
explicit conflict-aware roll-forward recovery. Unexpected I/O failures or external
editors can still leave a partial batch. There is no automatic rollback.

## Preview and apply

Read every existing note completely with `memory_get` and retain its complete
`document.content_hash`. Write an `edits.json` array:

```json
[
  {
    "path": "decisions/old.md",
    "expected_hash": "<current-full-document-SHA-256>",
    "content": "<complete revised Markdown, including frontmatter>"
  },
  {
    "path": "decisions/new.md",
    "content": "<complete new Markdown, including frontmatter>"
  }
]
```

Omit `expected_hash` (or use null) only for creation without overwrite. Notes are
limited to 1 MiB each, with at most 4 MiB total content. Paths must be visible,
relative `.md` paths, with no symlinks, traversal, reserved names, duplicates,
case aliases or ancestor/descendant edit pairs. Batch writes do not stamp dates;
include intended metadata explicitly. No deletes, renames or cross-source edits
are supported.

```sh
agent-memory batch-write --project demo --source-id 1 --edits-file edits.json
agent-memory batch-write --project demo --source-id 1 --edits-file edits.json \
  --expected-plan-hash '<reviewed-plan-hash>'
```

The CLI limits the JSON edits file to 32 MiB before reading it. MCP uses
`memory_batch_write` with `project`, `source_id`, `edits` and optional
`expected_plan_hash`. Omit the plan hash for a read-only preview; supply the
reviewed hash to apply the same edits. Edits are sorted by path for deterministic
plan hashing and publication order. The plan hash binds project, source and full
edit payloads. It does not freeze unrelated source files.

Preview needs read permission and does not refresh the SQLite index or create
operation/history files. Apply additionally requires source write permission and
MCP `--allow-write`. Follow `confirm`/`verified` and existing user authorization:
a plan hash is a conflict guard, not proof of human approval.

Final-state document IDs are validated per readable physical source and Memory
IDs per readable project using current Markdown. Coordinated ID moves/swaps are
allowed; duplicate final IDs, foreign structured writes and attempts to overwrite
another project's structured record are refused. Shared-source ordinary Markdown
retains the source's normal read/write semantics. The SQLite write transaction
serializes permission changes in this database during publication. Non-cooperating
editors and independent owner configuration changes remain outside that guarantee.

## Results and recovery

Apply returns `operation_id`, `complete`, `partial`, `applied`, `error`, `indexed`
and `index_error`. `applied` contains paths observed at their intended hashes,
including publications recognized from a prior attempt or unchanged updates.
It is not a count of files newly modified by this particular call. An index
failure never rolls back saved Markdown, and `indexed: false` is not permission
to repeat the original write. Errors mentioning saved revision history require
inspecting the operation and resuming it, not blindly retrying creates.

Before any publication the immutable journal is saved under
`.agent-memory-operations/`. Each note keeps ordinary durable revision history.
The completion marker records that all intended publications and histories were
finalized. SQLite indexing happens once after the attempt, including partial
attempts; intermediate duplicate IDs can prevent that refresh.

```sh
agent-memory batch-write-operations --project demo --source-id 1
agent-memory batch-write-resume --project demo --source-id 1 \
  --operation-id '<journal-ID>'
```

MCP equivalents are `memory_batch_write_operations` and
`memory_batch_write_resume` with the same project/source and `operation_id`.
Discovery and resume bypass index refresh. A new MCP process can initialize in
recovery mode when index refresh fails and a readable unfinished batch exists.
Normal tools that need a successful refresh can still fail until recovery is
complete. Resume requires enabled writes/current write permission and preflights
all targets as either their recorded original state or intended state before
making further progress. It settles interrupted history for recognized bytes.
External edits and missing update targets are conflicts; reconcile them explicitly.

Completed journals are immutable receipts: resume never reapplies their contents
after subsequent edits. They report `complete: true` with no newly observed
`applied` paths; this does not assert that current notes still equal the old batch.
Journals survive a rebuilt database with different numeric source IDs: select the
currently authorized source containing the original journal. Project names remain
scope boundaries. Preserve operation and history directories in backups; portable
backup/restore already includes both. No schema migration is required.

## Git synchronization

Updated Git sync/pull takes the same source lock and refuses any unfinished batch
journal before committing/pushing or pulling incoming changes. After-write Git
sync is requested once for a complete, successfully indexed MCP batch, never once
per note. A Git failure is separate from the successful Markdown operation.
Periodic sync can retry a complete batch; an incomplete one requires explicit
recovery. The private untracked `.agent-memory.lock` alone does not make pull's
worktree check dirty; other untracked/staged/modified files retain their checks.

Update/restart **all** cooperating writer and synchronization processes before
using this workflow. Older sync binaries do not know about pending batch journals.
Direct Git commands and external editors do not participate in these safeguards.
