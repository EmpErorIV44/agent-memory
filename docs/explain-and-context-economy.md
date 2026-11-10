# Explain and context economy

`explain` is opt-in diagnostic output for the current lexical retrieval pipeline.
It exposes observed ranking and selection decisions, rather than asking a model
to invent a relevance justification. It does not change ranking, filters, selected
chunks, context admission or excerpt growth. It does not verify that a note is true
or current on a remote. These operations use the existing project/source access
checks; explanations are untrusted data just like the notes they describe.

## Search

```sh
agent-memory --database /absolute/path/index.sqlite search \
  --project demo --mode natural --limit 5 --explain -- 'webhook retry'
```

MCP: `memory_search({"project":"demo","query":"webhook retry","mode":"natural","explain":true})`.
CLI `--explain` implies JSON output and conflicts with `--paths-only`.
Normal output remains unchanged; explain output adds `explanation` to each
returned hit. Without `min_coverage`, empty results remain `{"hits":[]}`.
With natural `min_coverage` and explain, a top-level `coverage_filter` reports
observed threshold rejections even when no hit survives. It does not infer why
other documents were absent; see the [coverage filter contract](search.md#optional-natural-coverage-filter).

Every explanation includes the evaluated FTS expression, its Russian-normalized
form, whether the original expression matched, the actual BM25 used, its source
(`exact` or `russian`), and the lexical rank before the public limit. Lower BM25
sorts first within the exact/stem group; this is a relative ordering value, not a
probability or a score comparable between arbitrary queries.

In FTS mode the order is exact-expression match, BM25, path and start line.
The expression keeps boolean, prefix, literal and punctuation semantics; it is
not reduced to an inaccurate list of independent word matches. A negative term
is a constraint, not evidence of relevance.

Natural mode additionally reports:

- `document_coverage` and `chunk_coverage`: `[normalized, exact]` counts used by
  the actual sort, in that order. Document coverage unions terms across sections.
- `document_matched_terms` and `chunk_matched_terms`: corresponding sorted
  `[normalized_expressions, exact_words]` arrays. Normalized expressions are quoted
  FTS terms after Russian stemming; multiple input forms may share one stem.
- `ranking_order`, `returned_rank`, and `deduplicated_by_document`.

Matches can come from indexed headings, body, title, path, IDs or project-owned
summary. A matching word need not occur in the visible chunk body. This first
version identifies that scope but does not attribute every match to a particular
indexed column. Tie breaking uses the same lexical rank as normal retrieval.
Ranking diagnostics cover returned hits only. The optional coverage filter also
reports rejected candidate paths and matched/query word counts, after the same
source/project/kind/status access filters; rejected bodies are not returned.

## Context

```sh
agent-memory --database /absolute/path/index.sqlite context \
  --project demo --max-tokens 4000 --explain -- 'webhook retry'
```

MCP: `memory_context({"project":"demo","task":"webhook retry","max_tokens":4000,"explain":true})`.

The opt-in response is `{"context":{...},"explanation":{...}}`.
`context` is identical to the ordinary response and remains inside the requested
UTF-8 byte budget. The wrapper and explanation are **outside that budget**;
requesting diagnostics consumes extra context. This separation lets comparisons
verify identical selection at the same budget, including small budgets.
`max_tokens` retains its historical byte-based meaning, not model token counts.

`explanation.candidates` lists prepared candidates with:

- Their existing reason: `accepted_constraint`, `task_match` or `related_memory`.
- Actual normalized task-word intersections in the selected chunk heading/body,
  its chunk ID and original lines for a `task_match` candidate. Indexed
  metadata may explain eligibility even when this intersection is empty.
- Explicit one-hop relation witnesses (`from`, `to`, `kind`) when applicable.
- Admission order, `included` or `omitted_budget`, final entry/excerpt bytes and
  truncation state. Omitted candidates have null final size fields.

Admission alternates required constraints with task matches, then tries related
records. Initial excerpts use up to 160 UTF-8 bytes, then grow in 128-byte rounds
up to 2400 bytes per entry. UTF-8 boundaries and JSON escaping are accounted for.
Presentation order remains constraints, task matches, related records. Relation
witnesses connect to the original constraint/match candidate pool before budget
admission; a witness endpoint may itself be omitted by the budget.

Byte accounting is exact for the compact serialized `context`:

- `entry_bytes_by_reason`: complete serialized entries grouped by reason,
  including their own metadata.
- `metadata_and_container_bytes`: outer fields and JSON container punctuation.
- `excerpt_json_bytes`: serialized excerpt string values, including quotes and
  escaping. `entry_metadata_bytes` is the rest of the entry bytes.
- `used_bytes`, `remaining_bytes`, `limit_bytes`.

Thus `sum(entry_bytes_by_reason) + metadata_and_container_bytes = used_bytes`;
also `excerpt_json_bytes + entry_metadata_bytes` equals the grouped entry total.
Raw `excerpt_bytes` in each candidate measures text only, so it need not equal
serialized string bytes.

Scope is bounded: lexical search admits at most 50 hits before the context
coverage re-sort; required constraints and relations add candidates separately.
The report does not enumerate unreadable documents, retired records, documents
outside that lexical pool, or all possible missing knowledge. It is not an atomic
snapshot of concurrent file changes. Bootstrap's separate budgeting is unchanged
and does not gain an `explain` flag in this version.

## Repeatable economy benchmark

```sh
cargo build -p agent-memory --locked
python3 scripts/benchmark-context-economy.py \
  --binary target/debug/agent-memory \
  --output /tmp/agent-memory-economy.json
```

The script copies `tests/context-economy/notes` to a temporary directory, registers
a read-only source in a disposable SQLite database, and never accesses the real
memory database or Git remotes. It runs each query with and without explain and
rejects changed retrieval results or incorrect context byte accounting. JSON and
companion Markdown reports include binary/fixture SHA256 hashes and selected
source/path/document hashes. Keep the fixture labels unchanged when comparing
implementations; use a separate fixture directory for new experiments.

Each case declares required fact strings and the document they must come from.
Coverage uses case-folded, whitespace-normalized exact text, not a model judge.
A fact in another document does not satisfy the label. The report distinguishes:

| Outcome | Meaning |
| --- | --- |
| `sufficient_excerpt` | All labeled facts occur in the returned text |
| `excerpt_loss` | Selected full documents have all facts, excerpts do not |
| `retrieval_miss` | Even selected full documents lack a required fact |
| `negative_pass` | A case with no relevant facts returns nothing |
| `negative_violation` | A negative case returns something |

Search uses natural mode with top 1; context uses each case's declared budget.
Full document text comes from hash-verified fixture files for the **same selected
documents**, deduplicated within the case. No full-read MCP calls are timed or
simulated as measured transport. Metrics include excerpt/full UTF-8 text bytes,
fact coverage, actual compact CLI payload size and extra explain payload bytes.
The corpus includes English/Russian, cross-section facts, a document with a long
section, a deliberately mismatched query and a negative query. Some quality
misses are intentional diagnostic cases; they do not make the command fail.
Exit 0 means the experiment ran successfully, not perfect quality; exit 2 means
schema, CLI, identity, explain parity, byte-accounting or other operational error.

Search already returns chunk `body`. Reading it directly adds no new read call;
fetching the identical chunk again repeats its text. The benchmark reports those
avoidable text bytes separately. It does not claim token, money or latency savings,
measure answer generation, establish production quality, or model caching across
questions. Fact presence is necessary evidence, not proof that an answer will be
correct or that omitted context cannot qualify a fact.

## Focused verification and later large run

For implementation validation:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p memory-sqlite --locked --test explain --test natural_search \
  --test search_options --test search_russian --test context --test context_budget
cargo test -p agent-memory --locked --test explain_mcp --test natural_search_mcp
python3 -m unittest discover -s tests -p 'test_context_economy.py'
python3 scripts/benchmark-context-economy.py --output /tmp/agent-memory-economy.json
```

Regression assertions cover unchanged FTS/natural rankings and limits, Russian
stemming and exact coverage, title-only matches, cross-section coverage, empty and
invalid queries, default omission of diagnostics, CLI/MCP equality and schemas,
project isolation, relation witnesses, omissions, Unicode/escaping, small budgets,
and sums matching exact serialized bytes. Python tests cover fact attribution,
missing facts versus missing documents, negative cases, identity tampering,
fixture validation, deduplication and isolated read-only fixture copies.

The later large acceptance run should add `make check` (with local Unix sockets
available for daemon IPC tests), existing retrieval evaluation in both FTS and
natural modes, repeated economy reports from the same binary, and manual review
of misses. Compare fixture hashes, ordered results, fact coverage, omissions and
payload sizes. Quality changes require reviewing the cases; do not silently tune
labels to make the results pass. Run this larger stage separately after reviewing
the implementation and initial report.
