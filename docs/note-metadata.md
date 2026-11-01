# Note dates and tags

Optional root-level frontmatter records authored note metadata:

```yaml
---
id: doc-search-design
title: Search design
created: 2026-11-01
updated: 2026-11-02
tags: [architecture, search]
---
# Search design

Project knowledge belongs here.
```

`created` and `updated` are calendar dates in exact `YYYY-MM-DD` form, with a
valid Gregorian day and a year from 0001 through 9999. They may be quoted YAML
strings or plain YAML dates. When both exist, `created` must not be later than
`updated`. These dates describe the note, not when SQLite indexed it, when the
filesystem last changed, or whether its claims were verified.

`tags` is an optional list of up to 64 unique strings. Each tag must be nonempty,
without surrounding whitespace or control characters, and at most 128 UTF-8
bytes. Matching is case-sensitive. A missing list defaults to empty. Unknown
root-level frontmatter fields remain supported; dates and tags do not change
source access permissions or logical Memory status.

## Reading and indexing

Structured Memory list/get responses include `created`, `updated`, and `tags`
from the source document alongside the logical Memory. Full-document reads
include the same fields plus the optional authored `title`, parsed from the
exact hash-verified Markdown returned in `content`. Dates absent from the note
are returned as `null`; absent tags are returned as `[]`. A full-document read
never returns an unfiltered duplicate `memory` block as a metadata object.

Memory list/get remain index-only operations. Schema v7 adds rebuildable
`authored_created`, `authored_updated`, and `tags_json` document columns. After
upgrading an existing database, run `index` to populate them; normal MCP refreshes
also do this. Unchanged notes are backfilled without rewriting Markdown or
changing document identity. Rebuilding from source recreates the projection.
Before that first refresh, old indexed rows have null dates and empty tags.

Search results retain their existing shape. An authored `title` participates in
title search, falling back to the first chunk heading when absent. Dates and
tags are exposed as metadata; date-range filters and tag search are not currently
implemented. Ordinary frontmatter-free notes remain valid.

## Optional date stamping

Ordinary writes preserve the supplied content without adding or changing dates.
Opt-in stamped writes can add creation/update dates and maintain the update date
while preserving the original creation date. See the CLI and MCP write options
for other write arguments. Enable stamping with CLI `write --stamp-metadata` or
MCP `memory_write` field `"stamp_metadata": true` (default `false`). New notes
receive missing creation/update dates; updating a legacy note without `created`
does not invent its historical creation date. Stamp dates use the server's current UTC calendar date;
explicit authored dates are independent of filesystem timestamps. Stamping does
not verify a note's evidence or grant write permissions. The regular source
permissions, full-document hash guard, revision history, and write policy apply.

Prefer descriptive filenames such as `search-design.md`; dates belong in
frontmatter. This is a naming recommendation, not a restriction on existing
date-prefixed notes. Renaming a note should preserve its `id` and `memory.id`.
