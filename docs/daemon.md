# Local indexing daemon

The daemon keeps the SQLite projection current for explicitly selected projects.
It runs in the foreground and polls source metadata, with periodic full
reconciliation. Markdown remains the source of truth. The daemon does not write
notes, change knowledge status, run Git synchronization, or contact the network.

## Run, inspect, and stop

Initialize the database and register its projects and sources first. On Linux,
macOS, or WSL, start a foreground process against that existing database:

```sh
agent-memory --database /absolute/path/memory.sqlite daemon run \
  --project demo --poll-interval-ms 500 --reconcile-interval-secs 30
```

Repeat `--project` to select multiple registered projects. The default polling
interval is 500 milliseconds; the default full-reconciliation interval is 30
seconds. Accepted values are 20–60000 milliseconds and 1–86400 seconds,
respectively. The database identity identifies the daemon instance. Use the same
database when inspecting or stopping it:

```sh
agent-memory --database /absolute/path/memory.sqlite daemon status
agent-memory --database /absolute/path/memory.sqlite daemon stop
```

The process uses a local Unix socket in a private per-user temporary directory:
`/tmp/.agent-memory-daemon-UID/<32hex>/control.sock` on Linux, or under
`/private/tmp` on macOS. The key is derived from the canonical database path and
its device/inode identity. Runtime directories require mode `0700` and the
socket mode `0600`; the live status response's `endpoint` is authoritative.
No TCP port or remote endpoint is opened. The database must already exist.
Symbolic-link aliases to it are canonicalized and supported; databases with
multiple hard links are rejected. Replacing or unlinking a running daemon's
database causes it to exit; restart it against the intended database.

`daemon stop`, Ctrl+C, and SIGTERM request graceful shutdown. Native Windows
daemon operation is rejected explicitly; run the Linux binary in WSL. Foreground
operation can be managed by a supervisor such as systemd or launchd. Starting
the command does not install or configure a service.

## Live status and MCP integration

`daemon status` queries the running process rather than reading a saved health
file. Its JSON includes `running`, `pid`, `database`, `endpoint`, `healthy`,
`poll_interval_ms`, `reconcile_interval_secs`, `watcher: "polling"`,
`local_only: true`, and `projects`. Each project reports `project`,
`last_success_at` (Unix milliseconds or `null`), `last_error`, `index_runs`, and
`last_trigger`. Triggers are `startup`, `poll`, `reconcile`, or `request`.
`index_runs` counts attempts, including failures; `healthy` means no selected
project currently records an indexing error. It does not prove remote freshness
or that the notes' claims are true.

MCP handlers that normally refresh the index can ask a running daemon to refresh
their project. If the daemon is unavailable, rejects the project, or reports an
error, the MCP process falls back to direct indexing. This delegates refresh
coordination only: document reads, hash checks, and project authorization remain
in the original MCP handlers. The daemon is not a proxy for the full MCP
protocol. CLI `search` still reads the current index; running the daemon makes
background refresh available without changing that command's read semantics.

## Polling and reconciliation

Each polling pass computes a deterministic fingerprint of eligible Markdown
paths, byte lengths, and modification times. It does not read or parse note
contents. Additions, removals, renames, and changes to those metadata values can
trigger an index refresh, coalescing changes within each polling interval. There
is no separate debounce queue. A regular full reconciliation also reads the
source, so edits that preserve both size and modification time are eventually
observed even if the polling hint is unchanged.

Polling follows the indexer's source scope: case-insensitive `.md` files,
including ordinary hidden Markdown paths, while excluding exactly
`.agent-memory-history` and `.agent-memory-operations`. Other non-Markdown files,
including daemon socket/control/lock files, do not affect the fingerprint.
Symbolic links are not traversed. Registered roots and descendant directories
are opened component by component without following symbolic links, and file
metadata is inspected without reading Markdown content.

A failed poll is an error, not evidence that a source became empty. Missing or
unreadable sources must not cause the daemon to interpret all notes as deleted.
Successful full indexing remains responsible for authoritative reconciliation
and its existing transaction guarantees. Polling is not an atomic filesystem
snapshot: a file may change during enumeration, and a rename can trigger a retry.

## Permissions and boundaries

Only explicitly selected projects and their readable registered sources are
eligible. Source registration and access remain owner-controlled; note text
cannot add projects or grant daemon permissions. Keeping an index current does
not authorize any Markdown edits or override the normal write policy.

Polling is portable metadata observation rather than operating-system event
notifications. There is no claim that every filesystem event is captured or that
queries always reflect an edit immediately. Poll timing, indexing time,
and the reconciliation interval bound normal freshness; source errors can delay
it further. The daemon does not fetch upstream knowledge, push commits, verify
claims automatically, execute tests, or replace Markdown/history backups.
