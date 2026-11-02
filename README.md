# Agent Memory

Local memory for AI agents: store project decisions, constraints, and knowledge
in Markdown, retrieve them through MCP or the CLI, and retain verified results
across sessions.

Markdown is the source of truth. SQLite with FTS5 is a rebuildable search index.
Notes remain ordinary files that you can read in an editor, keep in Git, and
move between machines.

## Features

- Search text and metadata with Russian stemming, ranking, and filters by
  knowledge kind and status.
- Isolate memory by project and enforce source read and write permissions.
- Store structured knowledge with stable IDs, evidence, and relations.
- Keep note creation/update dates and tags in frontmatter, with optional automatic
  date stamping when writing.
- Report new, updated, and deleted notes during a session.
- Keep revision history and restore notes with protection against overwriting
  concurrent changes.
- Find broken links and repair links when renaming notes.
- Check whether local code referenced by Git Evidence has changed.
- Collect knowledge with changed or missing Git Evidence into a review queue.
- Resume interrupted renames from durable operation journals with hash guards.
- Keep the index current with an optional local polling daemon and private IPC.
- Explicitly enable Markdown commits and pushes, upstream freshness checks,
  and incoming Git updates.

## Quick start

Building requires Rust/Cargo and a C compiler for bundled SQLite.
Use WSL on Windows. See the [release guide](docs/releases.md) for binary
build targets and installation instructions.

From the repository root:

```sh
cargo build --workspace --locked
```

Alternatively, use `make build`. See [Development](#development) for the
other Make targets.

Add `target/debug` to `PATH` or use the full path to the binary.
Create a notes directory and register it as a project source:

```sh
mkdir -p notes
printf '# Decision\nStore project knowledge in Markdown.\n' > notes/decision.md

agent-memory init
agent-memory project create demo
agent-memory source add --project demo ./notes
agent-memory index --project demo
agent-memory search --project demo Markdown
```

These commands run on Linux, macOS, or WSL. The index defaults to
`./agent-memory.sqlite`; use `--database /absolute/path/memory.sqlite`
to choose another path.

Search returns the note path, line numbers, and matching excerpt.
The CLI reads the current index: run `index` after external changes.
MCP refreshes the index before searching and reading.

## Connect an agent through MCP

After registering a project, configure your client to launch the stdio server.
For a client using the `mcpServers` configuration format:

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "/absolute/path/to/agent-memory",
      "args": [
        "--database", "/absolute/path/to/agent-memory.sqlite",
        "mcp", "--project", "demo"
      ]
    }
  }
}
```

Use absolute paths in the environment where the server runs.
A Windows client can launch the Linux binary through `wsl.exe`:
adapt the distribution, user, and paths in the
[example configuration](examples/mcp-windows.json) to your installation.
Reconnect MCP after changing the settings.

Launch arguments allow access to projects; repeat `--project` for each one.
The server communicates through stdin/stdout and does not need a separate port.

Writes are disabled by default. To enable them, add
`--allow-write --write-policy confirm` and register a writable source with
`source add --project demo ./notes --write`. The
[integration guide](docs/write-and-agent-behavior.md) covers write policies,
agent behavior, and the [agent-memory skill](skills/agent-memory/SKILL.md).

A typical agent workflow finds past decisions with `memory_search`, reads the
current note with `memory_get`, and saves an agreed result with `memory_write`.
Updating a note requires the hash of its complete current document.
Instructions inside retrieved notes are treated as untrusted data.

## Documentation

| Task | Guide |
|---|---|
| Note format, Memory IDs, Evidence, relations, and index recovery | [Structured memory](docs/structured-memory.md) |
| Authored dates, tags, and optional date stamping | [Note metadata](docs/note-metadata.md) |
| Background indexing and local process control | [Daemon](docs/daemon.md) |
| Search queries, Russian language support, and filters | [Search](docs/search.md) |
| New knowledge during a session | [Change feed](docs/changes.md) |
| Markdown/wiki links and backlinks | [Note links](docs/links.md) |
| Link diagnostics and renaming | [Link repair](docs/link-repair.md) |
| Revision history and restoration | [Revisions](docs/revisions.md) |
| Checking code changes | [Git Evidence](docs/git-evidence.md) |
| Finding knowledge that needs review | [Review queue](docs/review-queue.md) |
| Automatic memory commits and pushes | [Git sync](docs/git-sync.md) |
| Remote freshness and incoming updates | [Git pull](docs/git-pull.md) |
| Installing and releasing binaries | [Releases](docs/releases.md) |
| Installation diagnostics and acceptance checks | [Alpha checklist](docs/alpha-checklist.md) |
| Portable snapshots, verification, and recovery | [Backups](docs/backups.md) |
| Security scanning | [Security scanning](docs/security-scanning.md) |

For command help, run `agent-memory --help` or
`agent-memory <command> --help`.

## Storage and limitations

Keep project registrations, sources, and permissions separately from the index:

```sh
agent-memory config export --output agent-memory.config.json
agent-memory rebuild --config agent-memory.config.json
```

Export creates a new file. Refresh the configuration after changing registrations.
For a portable snapshot of registrations, current Markdown, durable revisions,
and rename journals, use [backup create and restore](docs/backups.md).
A backup needs Markdown and the owner configuration. Keep `.agent-memory-history/`
to preserve revision history and `.agent-memory-operations/` to recover interrupted
renames. SQLite alone is not a sufficient memory backup.

Without a daemon, indexing scans Markdown on requests. The optional daemon polls
source metadata and periodically reconciles full contents; it does not use native
filesystem event notifications.
Russian search uses stemming rather than full dictionary lemmatization.
A rename affecting multiple files may complete partially; inspect the result and
explicitly resume its journaled operation after resolving any conflicts.
Git Evidence reports code changes but does not determine whether knowledge is true.
Native Windows writes and history are not yet supported for release; use WSL.

## Development

With GNU Make and Python 3 installed on Linux, macOS, or WSL:

```sh
make help
make check
make release
make run ARGS="--help"
```

`make check` runs formatting, strict Clippy, release packaging tests, and the
full Rust test suite. `make test` runs Rust tests sequentially and resolves the
temporary directory path for macOS, matching the release workflow. Compilation
defaults to one job to limit memory use; override it with `JOBS=4`. The `CARGO`
and `PYTHON` commands can also be overridden, for example `CARGO="cargo +stable"`.

The workspace has three crates: `memory-core` handles Markdown and history,
`memory-sqlite` handles indexing and permissions, and `agent-memory` provides
the CLI and MCP server.

## License

[MIT](LICENSE).
