# Alpha checklist

This is an acceptance procedure for a local installation, rather than a report
of checks already performed. Run writes, restores, and Git sync against a
separate test source. Creating a release or Git tag is outside this procedure.

## Installation and diagnostics

Build the binary, register the project and source, and export the owner's
configuration. Configure the MCP client with absolute paths and restart the
connection; for Windows, see the
[configuration example](../examples/mcp-windows.json).

```sh
agent-memory --database /absolute/path/memory.sqlite doctor --project agent-memory
```

`doctor` opens the existing SQLite database read-only and returns JSON containing
`healthy` and `checks`. A failed check produces a nonzero exit code. Git
diagnostics are optional and local: the command does not fetch or contact the
network. A successful `doctor` does not prove remote availability or that an
untested MCP client works.

## MCP acceptance

1. Connect, read `memory_list` and the relevant full documents. Find a known note
   with `memory_search` and check a full `memory_get`.
2. Create a test note with `memory_write`, then update it using the current
   `document.content_hash` as `expected_hash`. A repeated write with a stale hash
   must be rejected while preserving the current text.
3. Add a justified link to another test note. Check the outgoing and computed
   backlink through `memory_links`.
4. Read history through `memory_history` and a snapshot through
   `memory_revision_get`. Restore a selected snapshot with `memory_restore` and
   the current hash: the result must become a new revision while retaining later
   history.

If a write reports `indexed=false` or a saved-history warning, inspect the
already-published file before repeating the action. A later operation's failure
does not mean Markdown was not saved.

## Git sync and its status

Test Git sync in a separate local repository with a local bare upstream, rather
than working remotes. Setup and limitations are described in
[Git sync](git-sync.md). Check a successful push and refusal when changes have
already been staged in the Git index.

The persisted result of the latest attempt is available separately:

```sh
agent-memory --database /absolute/path/memory.sqlite git-sync-status \
  --project agent-memory --source-id 1
```

In MCP, use `memory_sync_status({"project":"agent-memory","source_id":1})`.
The response is `{"source_id":1,"sync":null}` if there have been no attempts or
the source is not a Git repository. Otherwise, `sync` contains:

```json
{
  "status": "ok",
  "last_attempt_at": 1793300000000,
  "last_success_at": 1793300000000,
  "last_error": null
}
```

`status` is `running`, `ok`, or `error`. Times are Unix milliseconds;
`last_success_at` remains `null` until an attempt succeeds. `last_error` contains
a safe description of the latest error or `null`. A corrupt status file returns
a read error rather than a "no attempts" state.

Reading status does not start synchronization, a fetch, or network diagnostics.
The result is saved in `.git/agent-memory-sync-status.json`, so it survives MCP
process termination. It does not confirm that synchronization is currently
enabled, the timer is running, or the remote is available. A `running` status
left after a crash also does not prove that a process is alive.

## Backup and recovery

Save Markdown, `.agent-memory-history/`, `.agent-memory-operations/`, and the
exported owner configuration.
Ordinary Git sync excludes hidden history and does not replace this backup.
Test `rebuild --config PATH` against a new test SQLite database, then repeat
searches and reads of structured knowledge, links, and saved history. Do not
use a working database to test recovery: `rebuild` replaces the selected
database's registrations and index with the configuration's data.
