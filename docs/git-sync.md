# Markdown synchronization through Git

Git sync commits Markdown changes from one source and performs a normal
`git push` to its configured upstream. It is separate from Git Evidence
verification: it does not verify source-code evidence or determine whether notes
are true. It is also not a daemon or two-way synchronization: there are no
automatic `pull`, merge, or force-push operations.

## Explicit owner opt-in

MCP synchronization is disabled by default. The owner enables it for specific
sources through startup arguments, rather than Markdown content. It requires
`--allow-write` and write access to the source for an allowed project. Find the
actual ID with `source list --project PROJECT` or `memory_sources` and replace `1`:

```sh
agent-memory --database /absolute/path/memory.sqlite mcp \
  --project agent-memory --allow-write --write-policy confirm \
  --git-sync-source 1 --git-sync-interval-secs 900
```

Repeat `--git-sync-source ID` for multiple sources. Set the interval with
`--git-sync-interval-secs N` or the environment variable
`AGENT_MEMORY_GIT_SYNC_INTERVAL_SECS`; the argument takes precedence. The default
is 900 seconds. The timer runs only while the MCP process is alive. Closing the
client or stopping its MCP connection stops periodic synchronization attempts.

To also synchronize a selected source after a successful `memory_write` or
`memory_restore`, add `--git-sync-after-write`. Saved Markdown is synchronized
even when an index-update warning is returned. These options do not change the
`confirm`/`verified` policy or authorize agents to save unrelated knowledge
without existing permission.

## One-time execution

The CLI makes one attempt for an explicitly selected project and source:

```sh
agent-memory --database /absolute/path/memory.sqlite git-sync \
  --project agent-memory --source-id 1
```

Before the first run, the owner must configure the Git repository, commit
identity, remote, current branch's upstream, and remote access. This command can
publish data: use it only for a source you intend to synchronize.

## Affected files

The source root must match the Git repository root. A repository in a parent
directory is not used. Changes to visible `.md` files are included: tracked,
untracked, and deleted files, subject to ignore rules. Hidden paths,
`.agent-memory-history/`, `.agent-memory-operations/`, SQLite, and other files are not staged.

Pre-existing staged changes cause refusal: synchronization does not mix them
into its commit. Conflicts, detached HEAD, and missing upstream also cause
refusal. If remote changes require a pull, the normal push fails; resolving the
divergence remains the owner's responsibility.

## Results and errors

On success, the one-time CLI prints `{"status":"ok","source_id":1}` (using the
actual ID). Periodic errors go to stderr, leaving stdout for MCP. When
synchronization after writes is enabled for a selected source, the response
includes `git_sync`:

```json
{"git_sync":{"status":"ok"}}
```

A Git error produces
`{"git_sync":{"status":"error","error":"error description"}}`.
The `git_sync` field is absent when the mode is disabled or the source is not
selected. A Git error does not undo saved Markdown: do not repeat `memory_write`
or `memory_restore` merely to retry a push. Fix the cause and run a one-time
`git-sync`, or wait for the next interval.

Check `indexed` and `index_error` separately: publishing to Git and updating
SQLite are separate operations. Git sync does not replace a backup of durable
revision history or rename recovery journals because `.agent-memory-history/`
and `.agent-memory-operations/` are excluded from commits.
