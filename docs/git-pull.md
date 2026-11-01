# Memory freshness across machines

`memory_changes` and search refresh the index of **local** Markdown. Knowledge
added on another machine and pushed to Git appears here only after a pull. MCP
`memory_changes` responses explicitly contain `remote_checked=false` and
`scope=local_markdown`: an empty journal does not imply remote memory is current.

At session start, the agent checks the relevant source:

```json
{"project":"agent-memory","source_id":1,"fetch":true}
```

These are arguments for `memory_freshness`. Its `freshness` response contains a
status of `up_to_date`, `ahead`, `behind`, `diverged`, `unknown` or `not_repository`,
`ahead`/`behind` counts when upstream is available, and `checked_remote`.
`fetch=false` does not access the network: comparison with the local tracking ref
may be stale. Even `up_to_date` with `checked_remote=false` does not establish that
remote memory was checked. A fetch check reflects the remote state at request
time; it does not promise the absence of subsequent changes.

When behind and authorized by the user to update memory, the agent calls
`memory_pull({"project":"agent-memory","source_id":1})`. MCP requires `--allow-write`
and source write permission. The application uses the configured upstream, fetches
changes and performs only a fast-forward. Local changes, conflicts or diverged
branches stop the operation. No automatic stash, merge commit, rebase or push is
performed. Ignored local files are also protected against overwriting if upstream
starts tracking such a path, for example the local SQLite index path. The Markdown
push timer does not automatically start pulling changes.

After a successful pull, the project index and journal are refreshed. To observe
new knowledge, call `memory_changes` with the old cursor, process the pages and
reread relevant notes. `pulled=true` with `indexed=false` means Git has already
updated but indexing failed. Fix the cause and rerun indexing; do not repeat note
writes.

If the network is unavailable, the application returns a check error; local files
remain accessible, but freshness relative to the remote is unknown. The last push
history in `memory_sync_status` does not replace this check.

CLI:

```sh
agent-memory git-freshness --project agent-memory --source-id 1 --fetch
agent-memory git-pull --project agent-memory --source-id 1
agent-memory memory changes --project agent-memory --since '<previous cursor>'
```
