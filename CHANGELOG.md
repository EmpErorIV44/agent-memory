# Changelog

## Unreleased

## 0.4.1 — 2026-11-10

- Use Obsidian-compatible nested tags (`recall/always`, `recall/task`, and
  `topic/...`) in recall examples and client instructions. Keep legacy colon
  tags readable, including mixed-format scope conflict checks and topic matching.

## 0.4.0 — 2026-11-10

- Split bootstrap recall into an explicit always-read core and bounded task
  preferences with topic triggers, full-read statuses and a 0..20 preference limit;
  preserve legacy unmarked rules and keep deferred notes out of general excerpts.

- Add CLI/MCP project bootstrap: full accepted constraints and tagged preferences,
  optional task excerpts, per-source Git freshness and a changes cursor, bounded
  by one compact JSON byte budget with explicit read and omission statuses.

- Add CLI search `--paths-only` for unique source-relative paths in search order.

- Add explicit CLI/MCP natural search mode ranked by query-word coverage and
  exactness, preserving default FTS semantics and literal phrase searches.

- Add frozen synthetic English/Russian retrieval corpora and an isolated CLI
  evaluation runner with reproducible quality reports and input fingerprints.
- Balance context budgets between short constraint and task excerpts before
  expanding them; report omitted accepted constraints explicitly and avoid
  duplicating YAML frontmatter in baseline excerpts.
- Prefer broader task-term coverage when ordering context matches, preserving
  ordinary search semantics and existing ranking as the tie-breaker.

## 0.3.2 — 2026-11-06

- Add kind/status/exact-tag/source filters and opt-in limit/offset pagination to
  CLI/MCP memory lists, retaining legacy unpaged output and full-record access.
- Add CLI `memory get-many` and MCP `memory_get_many` for ordered hash-checked
  full-note reads within a total JSON byte budget, with explicit per-item
  omissions and unavailable results; never silently truncate documents.

## 0.3.1 — 2026-11-06

- Default successful MCP responses to a single structured payload. Add
  `mcp --response-format text|both` for text-only and legacy clients; preserve
  text errors and report the active format in runtime diagnostics.
- Default CLI/MCP memory lists to compact summaries and full-note read
  coordinates, without loading evidence/relations. Keep full records through
  `memory list --full`, MCP `include_details: true`, and individual lookups.

## 0.3.0 — 2026-11-05

- Add MCP `memory_runtime_info` for the serving process version, PID, start time,
  enabled tool names, scoped source permissions and effective Git scheduler
  settings; CLI `runtime-info` explicitly describes only its own process.
- Add CLI `health` and MCP `memory_health` aggregating links, explicit relations,
  Git evidence review and pending rename/batch operations. Report incomplete and
  unavailable checks explicitly; preserve journal discovery after index failures.
- Bump workspace package versions to 0.3.0; no new SQLite migration is required.

- Add single-source CLI/MCP batch create/update previews, reviewed plan hashes,
  final-state ID validation and source-wide writer locking with per-note history.
- Recover interrupted batches from durable journals across index rebuilds;
  expose journal discovery and allow MCP initialization for pending-batch recovery.
- Prevent updated Git sync/pull from publishing or importing across unfinished
  batches; request after-write sync once after a complete indexed MCP batch.

- Clarify memory recall alongside other skills in the portable skill, MCP policy
  and always-loaded instruction template; document global Codex AGENTS.md setup
  and add an explicit memory-project mapping for this repository.

- Add read-only CLI `context` and MCP `memory_context`: accepted constraints,
  lexical task matches and one-hop active relations, source hashes, bounded
  excerpts and a conservative UTF-8 byte budget for the JSON context payload.
- Add CLI `contradictions` and MCP `memory_contradictions` for explicit active
  contradictions, active endpoints of supersession chains and dependency or
  supersession cycles, without changing knowledge statuses.

### Upgrade notes

- Update and restart every writer and Git synchronization process before using
  batch writes. Replacing a binary does not update existing MCP connections.
- Use `memory_runtime_info` on each updated connection to inspect its serving
  version and tools. Existing schema v8 remains supported without a new migration.

## 0.2.2 — 2026-11-04

- Add CLI search JSON output with the MCP-compatible hits envelope and an
  optional result limit of 1..50 in CLI/MCP (default 50).
- Add explicit literal phrase search in CLI/MCP, escaping punctuation and
  embedded quotes while preserving default FTS5 expression behavior.
- Explain FTS5 syntax errors with quoting and literal-mode hints.
- Update pulldown-cmark to 0.13.6 for RUSTSEC-2026-0433.
- Add owner CLI backup creation, checksum verification, and restoration into a
  new directory, including Markdown, durable revisions, rename journals, and
  portable project/source permissions. Rebuild SQLite from the restored config.
- Export owner registrations from a consistent read transaction and support
  opening an existing index without creating or migrating it during backup.
- Add Make targets for development builds, tests, formatting, and lint checks.

## 0.2.1 — 2026-11-02

- Add a foreground local indexing daemon with metadata polling, full reconciliation,
  private Unix IPC, singleton ownership, live status and graceful shutdown.
- Let MCP coordinate refreshes through a running daemon, with standalone fallback.
- Replace raw unsafe Unix IPC calls with safe Rust APIs while preserving connection
  deadlines, private sockets, and stale endpoint recovery.
- Explicitly release daemon ownership locks during shutdown, including when
  a concurrent child process temporarily inherits the descriptor.
- Set private Unix socket permissions on macOS without opening the socket,
  retaining symlink rejection and mode 0600.
- Update the macOS ARM64 release runner from macOS 14 to macOS 15 and run
  release tests sequentially to avoid cross-test descriptor inheritance races.

- Add a project-scoped disposable embedding cache and semantic/hybrid ranking
  primitives in the SQLite library (schema v8); default MCP search remains lexical.

### Upgrade notes

- Update all binaries sharing an index before opening schema v8. Older binaries
  cannot read it. Markdown and owner configuration remain the recovery sources;
  embeddings are disposable cache.

## 0.2.0 — 2026-11-02

- Add an on-demand review queue for active knowledge with changed or missing
  local Git evidence. Uncheckable evidence can be included explicitly; knowledge
  status is never changed automatically.
- Persist rename operation journals before publishing files and expose CLI/MCP
  discovery and guarded resume, including recovery after SQLite recreation.
- Support validated `created`, `updated`, and `tags` frontmatter, authored titles
  in search, and explicit UTC date stamping through `write --stamp-metadata` or
  `memory_write` with `stamp_metadata: true`.
- Expose authored dates and tags in structured records and full documents.
  Upgrade the rebuildable SQLite index from schema v6 to v7; unchanged notes
  backfill their metadata on the next index refresh.
- Translate documentation into English and document review, recovery, and note
  metadata workflows.

### Upgrade notes

- Update every binary sharing an index before opening schema v7. Older binaries
  cannot read the new schema. Back up Markdown and owner configuration; preserve
  `.agent-memory-history/` and `.agent-memory-operations/` when history and rename
  recovery are needed. SQLite is rebuildable and is not the knowledge archive.
- Date stamping is opt-in. Ordinary writes, restores, and rename repairs retain
  their exact-content behavior. Legacy notes do not receive invented creation dates.
- Multi-file renames remain non-atomic and stop on conflicts. Resume the saved
  operation rather than blindly reapplying the original plan.
- Release platforms remain Linux x86_64 and macOS ARM64. Use WSL on Windows;
  native Windows write/history support remains deferred.

## 0.1.0

Initial binary release. Subsequent development before 0.2.0 added change tracking,
Russian search, guarded link repair, and local Git evidence verification.
