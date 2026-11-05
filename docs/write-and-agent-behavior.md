# Writes and agent behavior

`memory_write` creates and updates Markdown, the source of truth. Structured
Memory/Evidence/Relation and durable revision history are already available;
see [structured memory](structured-memory.md) and [history](revisions.md).
On-demand local Git Evidence verification is available through MCP and the CLI;
see [Git Evidence](git-evidence.md). Verification is not run automatically on
writes or by a background watcher.

## Enabling writes

Writes require both permissions: the source must be registered with `--write`
for the specific project, and MCP must be launched with `--allow-write`.
Existing connections retain read-only access.

For already registered test data, run from the repository root:

```sh
cargo build --workspace
cargo run -p agent-memory -- source add --project banking --write ./tests/fixtures/banking
cargo run -p agent-memory -- source list --project banking
```

Registration reports `source_id`. Re-registering the same source without
`--write` removes write permission for that project. Other source bindings are
unchanged. Writes to a shared source are visible to all projects with access.

Keep absolute binary and SQLite paths in the MCP settings, and change startup
arguments to:

```text
--database /path/to/agent-memory.sqlite mcp --project banking --allow-write
```

Restart the client's MCP connection. Without `--allow-write`, `memory_write`
is absent from the tool list and cannot be called. Source permissions are read
from the database on every request, including direct writes by ID.

## When agents should write

A concise policy is embedded in the server:
[agent-memory-policy.md](agent-memory-policy.md). It is supplied through MCP
`initialize.instructions` together with the allowed projects and selected mode.
Compliance depends on the client and agent.

- `--write-policy confirm` is the default: show a draft and destination before
  writing. Do not request permission already given; a request to "remember"
  authorizes the corresponding note. An explicit standing preference such as
  "always do..." or "next time..." also authorizes saving that specific preference
  without asking again. It does not authorize unrelated writes or actions.
- `--write-policy verified` means the user authorizes automatic retention of
  verified task outcomes, accepted decisions, and enduring project constraints.

Both modes require duplicate searches, supporting evidence, and separation of
facts from plans. Do not retain secrets, guesses, transient logs, or every
intermediate step. The modes are instructions to the agent: the server does not
validate human consent from call content. It checks the project, permissions,
path, and hash.

## A skill for all of the user's projects

A portable skill is available at
[skills/agent-memory/SKILL.md](../skills/agent-memory/SKILL.md). It covers reading,
selecting knowledge to retain, confirmation, full-document updates, and conflicts.
Automatic selection is allowed by default; the skill does not expand access
permissions or change MCP settings.

For global installation in Codex from WSL, copy the skill into the user directory.
Run from the repository root if a skill with this name is not already installed:

```sh
mkdir -p "$HOME/.agents/skills"
cp -R skills/agent-memory "$HOME/.agents/skills/agent-memory"
```

User skills are available across repositories. Alternatively, install the skill
only in a selected repository's `.agents/skills`. See the
[official Codex documentation](https://learn.chatgpt.com/docs/build-skills).
Restart Codex if the new skill does not appear.

## Keep recall active alongside other skills

Skills are selected for a task; their complete instructions are loaded when
selected. A discoverable memory skill alone therefore does not establish an
always-loaded recall rule. This can leave project memory unused when another
skill owns the workflow. MCP `initialize.instructions` provides another guidance
channel, but compliance still depends on the client and agent.

For Codex, add the short instructions from
[agent-memory-agents.example.md](agent-memory-agents.example.md) to the global
`~/.codex/AGENTS.md` (or `$CODEX_HOME/AGENTS.md` for a custom profile), preserving
existing instructions. This applies across repositories. Add an explicit
workspace-to-memory-project mapping in the repository's own `AGENTS.md`; the
allowed-project list and directory name alone are not a mapping. This repository
uses `agent-memory`.

For other clients, place the same rule in their supported always-loaded rules or
user-instructions mechanism. This applies with or without skill support. Keep
specialist skills focused on their own work rather than editing every skill to
repeat the memory policy.

The rule combines recall with the task skill: bootstrap once per session/project,
retrieve context when scope changes, and consider verified retention before the
final response under the existing write policy. It does not require memory calls
for unrelated conversation or grant extra source access. Restart the Codex session
after changing global/project instructions. The MCP server's embedded policy also
requires a rebuilt binary and a restarted connection to update.

Official documentation: [AGENTS.md discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
and [skill loading](https://learn.chatgpt.com/docs/build-skills).

## Tools

At the start of a new session, the agent calls `memory_list`, selects current
accepted constraints and preferences, then reads the relevant full documents
with `memory_get(project, source_id, path, content_hash)`. `memory_get_record`
returns a structured record rather than source Markdown. Search provides
additional context and notes without structured metadata; `memory_sources`
provides source permissions and writable destinations.
Search includes paths, document/Memory IDs, and summaries. Russian search uses
Snowball stemming and kind/status filters; exact matches rank first.
See [search](search.md).

For task-focused retrieval, `memory_context` accepts plain task text and a
conservative JSON payload budget. `memory_contradictions` diagnoses explicit
relations and supersession chains without changing notes. See
[task context and contradiction diagnostics](task-context.md) for selection,
budget limits and review behavior.

`memory_sources({"project":"banking"})` returns the ID, name, and effective write
permission of accessible sources. Without `--allow-write`, every `can_write` is
`false`. This tool also works for empty sources.

To create a note, pass the following to `memory_write`:

```json
{
  "project": "banking",
  "source_id": 1,
  "path": "decisions/session-policy.md",
  "content": "# Session policy\n\nStatus: accepted decision.\n\nA session lasts 30 minutes.\n\nEvidence: user decision.\n"
}
```

Omitting `expected_hash` means creating a new file: an existing file is never
overwritten. Substitute the actual ID from `memory_sources`.

To update an existing file:

1. Find the note with `memory_list` or `memory_search`.
2. Call `memory_get` with `project`, `source_id`, `path`, and `content_hash`.
   Alternatively, use chunk mode: `chunk_id`, `content_hash`, and
   `"include_document":true`. `document.content` contains the entire source
   Markdown, including frontmatter; `document.content_hash` is the full file's
   hash. Ordinary `chunk.body` is only an excerpt.
3. Edit the full text, preserving other sections.
4. Pass the full text to `memory_write` with `expected_hash` equal to
   `document.content_hash`. A hash mismatch rejects the update.

The response contains `source_id`, `path`, `content_hash`, `indexed`, and
`index_error`. If `indexed=false`, Markdown **has already been saved**, but the
SQLite update did not complete. Do not repeat the write: fix the cause and run
`index`.

## CLI

Prepare a file containing the full text outside the indexed source, then run:

```sh
agent-memory write --project banking --source-id 1 \
  --path decisions/session-policy.md --content-file /tmp/session-policy.md
```

For updates, add `--expected-hash` with the hash of the full document you read.
The CLI also checks `can_write`; `--allow-write` applies to the MCP process.

## Guarantees and boundaries

- Nonempty UTF-8 Markdown up to 1 MiB is allowed, using relative `.md` paths
  without hidden components, `..`, absolute paths, or special Windows names.
- Writes use [cap-std](https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html)
  for operations relative to an open source directory. Symbolic links in the
  path are rejected; parent directories are created inside the source.
- The complete file is first written to a temporary file beside its destination.
  Updates are published through rename; new files use a hard link without
  replacing existing files. The filesystem must support these operations.
- `.agent-memory.lock` coordinates Agent Memory writers, even with different
  SQLite databases. This service file remains in the source and is not indexed.
  Temporary files are removed on normal completion, including errors.
- The hash is checked against current Markdown and checked again before
  publication. Locking is cooperative: external editors do not use it. Absolute
  protection against an external edit immediately before rename is not guaranteed.
- Markdown and SQLite do not share a transaction. If the process stops between
  saving a file and updating the index, the next `index` rebuilds the projection.
  Durable history and recovery are available through `memory_history`,
  `memory_revision_get`, `memory_restore`, and CLI `history`; restoration requires
  the current full-document hash and creates a new revision.
