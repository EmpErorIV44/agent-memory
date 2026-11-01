# Stale knowledge review queue

The review queue collects active Memory records whose local Git Evidence has
changed or disappeared. It is derived on demand from readable Markdown and
current working-tree bytes. It does not declare a claim false, change knowledge
status, or persist a separate queue.

## CLI and MCP

Configure project-scoped evidence repositories as described in
[Git Evidence](git-evidence.md), then run:

```sh
agent-memory --database memory.sqlite review-queue --project demo \
  --evidence-repository demo:code=/absolute/path/to/code
```

Add `--include-unknown` to also include records whose Git references could not be
checked. For MCP, launch the server with the same owner-controlled mappings:

```sh
agent-memory --database memory.sqlite mcp --project demo \
  --evidence-repository demo:code=/absolute/path/to/code
```

Call `memory_review_queue` with `{"project":"demo"}` or
`{"project":"demo","include_unknown":true}`. Writes do not need to be enabled.
Both interfaces refresh the project index before deriving the report.

## Selection and response

Only readable Memory records belonging to the requested project are considered.
`deprecated` and `superseded` records are excluded; `planned`, `implemented`, and
`accepted` records are eligible. Ordinary Markdown without a structured `memory`
block does not enter the queue.

Evidence qualifies for queue selection only when its kind is `file` or `commit`
and its reference starts with `git:`. Legacy file paths, manual decisions, test
commands, and URLs do not cause unknown entries. Supported references and their
security checks follow the [Git Evidence contract](git-evidence.md).

The report contains:

- `project`, `local_only: true`, and `include_unknown`.
- `scanned_count`: all eligible readable Memory records examined, including
  records without Git evidence.
- `unknown_count`: eligible records with at least one unknown Git evidence
  result, whether or not `include_unknown` is enabled. A record with both changed
  and unknown evidence contributes to this count and appears in the default queue.
- `entries`: records with any `changed` or `missing` Git evidence; unknown-only
  records are added when requested. Entries follow Memory ID order.

Each entry contains `memory` (the complete structured MemoryRecord, including
source coordinates and full-document `content_hash`), `reasons` (any of `changed`,
`missing`, `unknown`, in that order), and `verification` (the complete evidence
report, including baseline/current blob IDs where available). The verification
report also contains its own full-document hash. No code content is returned.
An unchanged commit reference means only that the commit exists locally; it
does not by itself detect later edits to files.

There is no result limit or pagination: each request checks the entire eligible
project. Existing per-file and per-Git-command limits still apply. Large projects
or many Git references can therefore take time. The report is not an atomic
snapshot of all notes and working files; another editor can change them during
or after a request.

## Reviewing a result

Read the full current note using the returned source coordinates and hash.
Inspect the reported evidence and decide whether the note still holds, needs
updated evidence, or should be revised or retired. Apply the normal write policy
and full-document hash guard to any update; the report itself grants no write
permission. A fresh queue request recomputes results, so restoring original file
bytes can remove an entry without changing Markdown.

No background checks, fetch/pull, tests, or URL requests are triggered. Knowledge
statuses are never changed automatically. Missing mappings, unavailable commits,
unsafe paths, and check failures remain `unknown`, rather than proof of stale or
false knowledge. Without authorized repository mappings, Git evidence is counted
as unknown and appears only when explicitly requested.
