# Memory changes during a session

The journal observes local files. Knowledge received through Git from another
machine first requires a [remote check and, if necessary, pull](git-pull.md).
MCP explicitly returns `remote_checked=false`, `scope=local_markdown`.

`memory_changes` detects Markdown changes when polled. At session start, call
`memory_changes({"project":"agent-memory"})`: the response contains an empty
`changes` list, the current numeric `revision` and an opaque `cursor`. After reading
relevant knowledge, repeat the call at task boundaries and before memory-dependent
decisions, passing the last cursor as `since`. Save the new cursor after processing
each page. While `has_more=true`, keep polling with it: responses are limited to
100 events, including when a single revision spans multiple pages.

```json
{"project":"agent-memory","since":"<cursor from the previous response>"}
```

Each successful indexing operation with changes increments the affected project's
revision once. Unchanged files, repeated reads and mtime-only changes do not
increment it. The journal is shared by MCP clients using the same SQLite database.
External edits are detected on the next scan; intermediate edits between scans
are not retained. This is an observation journal, not a watcher.

Events are `added`, `updated`, `renamed` and `deleted`. They contain `source_id`,
`document_id`, path, hash and an optional stable document ID. Renames are recognized
by frontmatter `id`; without it they produce a deletion and an addition. If text
also changes during a rename, the `renamed` event contains the new hash and
`previous_path`. For deletions, the hash refers to the last observed content. The
response contains no note text: use search and reads to retrieve relevant knowledge.
A note may have multiple events, including subsequent deletion. If the state is
still current, `chunk_id` allows `memory_get` with the event's hash. For stale states
and deletions, `chunk_id=null`; this is not historical reading.

A cursor is bound to a project and index instance and survives MCP restarts.
Invalid, foreign or future cursors are rejected. After recreating SQLite, establish
a new baseline and reread relevant memory. Rebuilding an existing database preserves
cursors and records observed changes. Migration v4 → v5 creates the journal; earlier
edits are not recovered retroactively. Scan errors roll back the index and journal
together.

Events are filtered by current source read permissions. Revoked access is not a
deletion notification: clients must clear local caches when access configuration
changes. Absolute source paths are not returned. The journal is not pruned yet;
regular polling is recommended for long sessions.

CLI:

```sh
agent-memory memory changes --project agent-memory
agent-memory memory changes --project agent-memory --since '<cursor>'
```
