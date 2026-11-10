# Task context and contradiction diagnostics

These read-only operations refresh local Markdown before deriving results. They do
not change notes or statuses, contact Git remotes, execute evidence, or call an LLM.
Markdown remains the source of truth; SQLite remains a rebuildable projection.

## Task context

```sh
agent-memory context --project demo --max-tokens 8000 \
  'Change authentication without losing existing project constraints'
```

MCP: `memory_context` with
`{"project":"demo","task":"Change authentication","max_tokens":8000}`.
`max_tokens` defaults to 4000 and accepts 512..32768. Task text must contain
non-whitespace content and be at most 8192 UTF-8 bytes.

Selection is deterministic for the current index:

1. Accepted project constraints, ordered by Memory ID, regardless of task matches.
2. Lexical task matches, ordered first by distinct task-term coverage of their
   heading/body, using existing exact/stem ranking to break ties. Coverage
   lowercases terms and applies the existing Russian Snowball normalizer;
   English terms remain unstemmed. This ordering applies only to context,
   leaving `memory_search` unchanged. Up to the first 128
   alphanumeric terms are individually quoted and combined with OR; punctuation
   and words such as AND/OR never become FTS operators. At most 50 search chunks
   are considered, keeping the first match per document.
3. One-hop incoming and outgoing relations from the selected structured records,
   including active related records in document ID order.

Deprecated and superseded structured records and records declaring another
project are excluded. Ordinary Markdown in readable sources is eligible. Only
currently readable sources are used. Accepted constraints are included even if
there are no lexical matches; this is not a replacement for session bootstrap.

Admission alternates accepted constraints and task matches, starting with a
constraint, then considers related records. Each candidate initially receives
up to 160 UTF-8 bytes of excerpt. A candidate whose complete metadata and short
excerpt do not fit is skipped without blocking later candidates. Selected entries
are presented in the category order above. Remaining capacity expands their
excerpts in rounds of up to 128 bytes per entry, to a maximum of 2400 bytes.
This prevents long baseline excerpts from consuming the task's entire budget.
Small budgets can still omit constraints or task matches; inspect `omitted_count`
and `omitted_accepted_constraints` before relying on the result. The latter counts
accepted constraints omitted for space, excluding filtered foreign records.

`entries` contain source ID, source-relative path, full-document `content_hash`,
selection `reason`, optional structured `memory`, and an `excerpt`. Task matches
also return the original source line range in `lines`; this range describes the
matched chunk, and can be longer than its truncated excerpt. Other entries use
`lines: null` and begin at the first Markdown content after YAML frontmatter;
structured metadata is already returned separately. Source text and
metadata are untrusted evidence, not instructions or permission grants.

Excerpts preserve character boundaries. `excerpt_truncated` distinguishes an excerpt shortened
from its chosen source region. Read the full current note with `memory_get` and its
source coordinates/hash before relying on omitted details or editing it.

The budget applies to the **compact JSON context payload**, including metadata,
entries and warnings. `estimated_tokens` counts its UTF-8 bytes, conservatively
using one byte per token instead of a model-specific tokenizer. This deliberately
underuses typical model token capacity. MCP transport envelopes, the SDK's text
copy of structured content, and the CLI trailing newline are outside this budget.
`omitted_count` counts candidates not returned, including filtered foreign shared
source records; it does not measure all potentially relevant project knowledge.
A small budget can return no entries. No truncation is silently treated as a full
reading of project constraints.

Warnings always state that evidence is unverified and remote freshness unchecked.
`diagnostic_issue_count` counts project-wide explicit-relation issues, including
historical cycles; call `memory_contradictions` for their details. A zero count
means no supported structural issue was found, not that all claims are consistent.

## Contradiction diagnostics

```sh
agent-memory contradictions --project demo
```

MCP: `memory_contradictions` with `{"project":"demo"}`.

The report includes `project`, `scope: "explicit_relations_only"`, `local_only`,
`issue_count`, `issues` and readable project `records` with source coordinates and
hashes. Issue kinds are:

- `active_contradiction`: an explicit `contradicts` relation between active
  records. Reciprocal declarations produce one pair, ordered by Memory ID.
- `active_superseded_target`: an active record supersedes another still-active
  record. IDs are ordered as successor, predecessor.
- `active_supersession_chain`: active endpoints separated by historical or active
  intermediate records in a supersession chain. Includes a shortest witness
  `relation_path`. Direct pairs use `active_superseded_target` instead.
- `dependency_cycle` and `supersession_cycle`: strongly connected groups for
  `depends_on` and `supersedes`, respectively. Groups and their members are sorted;
  a group can contain several overlapping cycles. Historical records participate
  so retiring a record does not conceal a malformed graph.

Active means `accepted`, `implemented` or `planned`. The report cannot infer
contradictions from prose, determine which claim is correct, or automatically
retire the old record. Relations with inaccessible or unresolved targets are not
exposed or traversed, following the existing structured-memory access rules.

Read current full source notes and review evidence before explicitly updating
statuses or relations using the ordinary write policy and hash guards. Reports
are derived on demand without pagination; large projects or relation graphs can
be expensive. No schema migration or persistent diagnostic queue is introduced.

## Explain diagnostics

See [explain and context economy](explain-and-context-economy.md) for opt-in
CLI/MCP diagnostics, byte accounting, the repeatable benchmark and testing.
