# Required rules and task preferences

Bootstrap supports two explicit recall tiers, using document tags already stored
in the index. No new SQLite migration or Memory kind is required.

Keep the always-read core short. For example:

```yaml
tags: ['recall:always']
memory:
  id: project-core
  project: demo
  kind: constraint
  status: accepted
  summary: Short mandatory project rules
```

The Markdown should contain the actual required rules, not a historical transcript.
It is returned in full in `baseline`, subject to the total byte budget. A long core
is not silently summarized; if it does not fit, `baseline_complete` is false.

Move lengthy background and conditional preferences into separate accepted notes:

```yaml
tags: ['recall:task', 'topic:release', 'topic:релиз']
memory:
  id: release-preferences
  project: demo
  kind: constraint
  status: accepted
  summary: Release preparation and delivery preferences
```

When no task is supplied, bootstrap returns the required core and no contextual
preferences. It returns only a count of deferred preferences, not a manifest or
text for every deferred note. Candidate selection reads indexed metadata rather
than opening every preference document or hydrating its Evidence/Relation graph.

With a task, bootstrap selects at most three matching contextual notes by default,
reads them in full after required rules and before general task excerpts, and
returns them in `preferences.entries`. Set `preference_limit` in MCP or
`--preference-limit` in CLI to 0..20; zero disables full preference reads.

```sh
agent-memory bootstrap --project demo 'Prepare a release'
agent-memory bootstrap --project demo --preference-limit 5 'Prepare a release'
```

`preferences` reports `limit`, `matched_count` (before the count limit),
`deferred_count` (all contextual records not selected), `status` (`not_requested`
or `ok`), and `complete`. Each selected entry has source coordinates/hash and
`status: ok|omitted_budget|unavailable`, with a whole `document` only for `ok`.
`complete` means all selected preference reads succeeded, not that every
preference was loaded. `baseline_complete` separately covers mandatory rules.
Preferences obey the same whole-response byte budget and full-read guarantees.

## Matching and compatibility

- Only readable, project-owned, accepted structured records participate.
- `recall:always` explicitly makes an accepted record required. `recall:task`
  explicitly defers it. The two tags cannot coexist.
- For existing notes without either tag, accepted constraints and accepted notes
  tagged `preference`/`preferences` retain their previous required behavior.
  No existing requirement is silently downgraded. A project must classify its
  existing notes to benefit from the smaller core.
- Nonempty `topic:` tags are explicit triggers. A topic can contain several words;
  all its normalized words must occur in the task, in any order. Any matching
  topic is sufficient. Matching is case-insensitive, uses Russian Snowball stems
  and whole words, and does not infer synonyms or translate languages.
- When topic tags exist, body matches cannot activate the preference. Explicit
  topic matches rank before lexical fallback; within that group, broader distinct
  topic coverage ranks first. Authors should choose specific trigger words.
- Without topic tags, contextual preferences may match the existing lexical
  search's first 50 chunks for a safely quoted OR of the task's first 128 words.
  Coverage and existing lexical order break ties, then Memory ID. This fallback
  is bounded, can miss a note and may match common words; topic tags are preferred.
- Deferred preferences stay out of the mandatory section of `memory_context`.
  They remain searchable. Bootstrap excludes all managed core/contextual notes
  from general excerpts, so a deferred preference cannot leak back as an excerpt
  or consume an extra full-read obligation through that channel.

## Migrating a project

Read existing accepted rules before classifying them. Preserve unconditional
requirements in a concise `recall:always` core; defer their detailed history and
conditional instructions with `recall:task` and suitable topics. Do not change
statuses or interpret a shorter core as new authorization. Use hash-guarded MCP
writes and the configured confirmation policy for knowledge changes. Read-only
selection never edits or reclassifies notes. Update the installed server and
client instructions before expecting new chats to use the tiers.
