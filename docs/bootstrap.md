# Project session bootstrap

`memory_bootstrap` assembles the initial project memory in one call. Markdown
remains authoritative and untrusted; the result grants no permissions and does
not verify implementation evidence or decide which assertions are true.

```json
{"project":"demo","task":"Change delivery scheduling","max_bytes":65536,"fetch":false}
```

The CLI prints the same compact JSON payload:

```sh
agent-memory bootstrap --project demo 'Change delivery scheduling'
agent-memory bootstrap --project demo --max-bytes 131072 --fetch
```

The task is optional. When supplied it must be non-whitespace and at most 8192
UTF-8 bytes. `max_bytes` defaults to 65536 and accepts 4096..1048576; it bounds the
entire compact JSON payload, including manifests, freshness, excerpts, warnings,
cursor and the `used_bytes` counter. It is a byte budget, not a model tokenizer.
MCP envelopes, optional text duplication and the CLI trailing newline are outside
this budget. A large mandatory manifest returns an explicit budget error; increase
the budget or use paginated `memory_list` and bounded `memory_get_many`.

## Full baseline before excerpts

The baseline contains required accepted records from readable project-owned
sources. `recall:always` marks the short required core; `recall:task` marks
conditional preferences that are selected only for a task (default limit 3).
Unmarked accepted constraints and accepted notes tagged `preference`/`preferences`
keep their previous required behavior. No existing rule is silently downgraded.
See [recall tiers](recall-tiers.md) for matching, migration and response fields.

`baseline` lists every selected record with `memory_id`, source coordinates,
full-document hash and reason (`accepted_constraint` or `accepted_preference`).
Constraints come first; each group is ordered by Memory ID. Each entry has one of:

- `ok`: `document` contains the complete Markdown, including frontmatter.
- `omitted_budget`: the whole document did not fit; no partial document is returned.
- `unavailable`: the expected document could not be read with its hash and access
  checks. Missing, stale and inaccessible files share this status.

Full baseline documents have priority. An oversized note does not block later
smaller notes. Inspect `baseline_complete`; false means some required full reads
remain. Retry listed omissions with `memory_get` or `memory_get_many`. True means
only that this selected baseline was returned completely, not that all relevant
project knowledge was discovered or that remote knowledge is current.

## Optional task context

After the required baseline, selected `preferences.entries` receive full reads.
Remaining capacity admits short task excerpts before expanding them in rounds.
`context.entries` uses existing `memory_context` entries, including source hashes,
line ranges and `excerpt_truncated`. Required and all deferred preference documents are excluded from these excerpts
even if their full reads were omitted or they were not selected; excerpts cannot
disguise an incomplete full read or bypass preference selection. `context.status` is `not_requested`, `ok`, `omitted_budget` (no candidate
fit) or `unavailable` (context assembly failed while baseline remained usable).

`context.omitted_entries` lists source coordinates and reasons for known task
candidates excluded by the outer budget; `omitted_count` counts those entries.
`candidate_omitted_count` separately reports omissions in the existing bounded
context selection (32768-byte candidate payload, at most 50 lexical hits plus
constraints/relations). It is not a count of all missing relevant notes; required/contextual notes are
excluded from this general candidate pool before full-text excerpt reads. The candidate pool and its excerpts are bounded and do
not establish exhaustive retrieval. `ok` can still have omissions or truncated
excerpts. Read full task notes before relying on details or editing them.

## Git freshness and changes

`freshness` contains one result per currently readable source, without owner root
paths. Each has `source_id`, `status: ok|error` and `freshness`. An error has
`freshness: null` and the fixed `error: freshness_unavailable`; raw Git stderr is
not copied into the response. Local baseline/context results remain available.

By default only cached upstream refs are compared. `checked_remote: false`
never proves remote freshness, even for `up_to_date`. `fetch: true` explicitly
contacts configured upstreams and updates Git metadata without pulling Markdown.
Non-repositories and missing upstreams remain explicit unknown/not-repository
states. Inspect every source's `checked_remote` and status, including behind or
diverged. No automatic pull, commit, push, note write or status change occurs.
Fetching is optional network access; callers must have authorization for it.

After freshness checks the operation refreshes the local index and captures a
new changes baseline **before** reading notes. `cursor` and `revision` can be
passed to `memory_changes` after work begins. Changes observed during or after
these reads will not be silently skipped by a cursor taken at the end. Index
refresh failures fail the operation rather than returning a successful stale
bootstrap. This is not an atomic filesystem snapshot (`atomic_snapshot: false`);
concurrent edits may make reads unavailable and require retrying. Poll the cursor,
process each page, and reread relevant changes. Cursor scope and rebuild behavior
are the same as [the change journal](changes.md).

## Agent workflow

Use bootstrap once per explicitly mapped project/session when supported. Inspect
baseline statuses, task omissions, freshness and warnings before proceeding.
Complete missing full baseline and selected preference reads, and use the returned cursor for later change
polls. If bootstrap is unavailable, retain the existing `memory_list` plus full
`memory_get`/`memory_get_many`, freshness and changes calls. Bootstrap does not
replace task-specific skills, explicit project mapping, authorization or evidence
verification. It does not enforce that a client calls any tool.
