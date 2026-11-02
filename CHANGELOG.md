# Changelog

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
