# Runtime diagnostics and memory health

## Inspect the connected MCP process

Call `memory_runtime_info` with `{"project":"demo"}` through the connection you
want to inspect. It returns the compiled package `version`, process `pid`,
`started_at_unix_ms`, sorted enabled `tools`, schema version, readable source IDs
and configured write permissions, the global write switch/policy, and effective
Git scheduler settings scoped to that project. Source `can_write` is the stored
permission; actual writes also require `allow_write: true` and tool authorization.

The PID and start time identify the serving process. Replacing a binary on disk
does not replace existing client processes. This tool cannot inspect or restart
other clients, prove that the installed binary is identical, or establish remote
reachability. It does not expose filesystem roots, remotes, credentials, other
allowed projects, or evidence repository paths. It bypasses Markdown refresh so
it remains useful during index failures. Store opening/authorization must still
succeed. Only a new connection can provide tools absent from an older server.

`agent-memory --database memory.sqlite runtime-info --project demo` describes
that new **CLI process**, with `mode: "cli"` and `mcp_runtime: null`. It explicitly
does not claim to inspect an already running MCP server.

## Aggregate memory checks

```sh
agent-memory --database memory.sqlite health --project demo \
  --evidence-repository demo:code=/absolute/path/to/code
```

MCP: `memory_health` with `{"project":"demo"}`. Evidence aliases come from the
server's owner-configured `--evidence-repository` options, never from note text.
Both interfaces produce the same JSON structure:

- `index`: refresh result. An error is reported without discarding other checks.
- `links`: missing, ambiguous and stale-path Markdown links with source coordinates.
- `relations`: explicit contradiction/supersession issues and dependency cycles.
- `evidence`: the current review queue, including unknown supported Git evidence.
- `operations`: per-readable-source unfinished rename and batch journals, their IDs
  and affected paths. Completed receipts are omitted. Discovery errors are retained
  per source and operation kind, without preventing checks of other sources.

Sections have `status` (`ok`, `partial`, `error`, `skipped`), `issue_count`, and
`unknown_count`; successful checks include `data`. A section can be `ok` and still
have findings: `ok` means the check completed. Index-dependent checks are skipped
when refresh fails, rather than returning stale indexed findings as current.
Journal discovery still runs, including when a partial ID swap prevents indexing.

Top-level `status` is `issues` when findings exist, otherwise `incomplete` when
any section is partial, skipped or failed, otherwise `clear`. `issue_count` sums
link issues, relation issues, records with changed/missing evidence, and pending
operations. `incomplete_checks` counts incomplete top-level sections, not individual
notes. Unknown evidence is reported separately and never counted as verified good
or known bad. A command that successfully constructs a report exits successfully;
inspect JSON status for automation. Invalid project/configuration errors still fail.

These are sequential local observations, not an atomic snapshot or a semantic
consistency proof. The evidence check covers supported `git:` references on active
records; bare paths/hashes, URLs, manual claims and test commands are not verified
or executed. No fetch, repairs, status changes or Git synchronization is triggered.
Local index refresh may update the rebuildable SQLite projection. Heading anchors
and unresolved semantic relation targets retain the existing diagnostics' limits.

For database/schema and local Git configuration diagnostics, use the existing
owner CLI [`doctor`](alpha-checklist.md). For recovery, inspect the journal and use
[rename recovery](link-repair.md) or [batch recovery](batch-writes.md) explicitly.
