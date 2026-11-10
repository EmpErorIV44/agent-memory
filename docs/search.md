# Russian search and filters

`memory_search` uses two project-scoped FTS5 views: the original text and text
normalized with the Russian Snowball stemmer (`rust-stemmers`). This matches word
stems, rather than using a lemma dictionary or semantic search: forms such as
«заметка», «заметки» and «заметками» match, but irregular forms and synonyms are not
guaranteed. Russian words are lowercased and ё → е; English words are not stemmed.

For example, the alternating vowel in «заметки»/«заметок» may produce different
stems; use another word form or an explicit OR in such cases.

```json
{"project":"agent-memory","query":"связи заметок","kind":"implementation","status":"implemented"}
```

Filters are optional and independent:

- kind: fact, decision, implementation, constraint.
- status: planned, implemented, accepted, superseded, deprecated.

With filters, only documents containing structured Memory owned by the selected
project are returned. Ordinary Markdown without Memory remains available without
filters. Another project's kind or status in a shared source does not affect the
results. Invalid values are rejected. Status is author-supplied metadata, not
proof that a record is true; deprecated and superseded records are not hidden by
default.

The FTS5 expression determines eligible results as a whole, including AND, OR, NOT,
parentheses, phrases, NEAR and column restrictions. Exact matches rank first,
followed by weighted BM25: title/heading and summary/IDs carry more weight than
body. Stable sorting breaks ties; duplicate chunks are removed, with a maximum of
50 results. Original chunks, line numbers and full-document hashes are returned,
rather than stemmed text.

The title, heading, body, path, stable_id, memory_id and summary fields can restrict
queries, for example `heading:заметки`. Conditions can combine chunk content and
metadata in a single project-scoped index row.

Prefixes are preserved. Queries with a Cyrillic prefix, such as `заметко*`, also
require a literal lexical match of the original FTS expression to prevent the
stemmer from broadening the prefix. Morphological matching of other positive words
in such a query is therefore limited to exact matching. NOT remains a constraint
of the normalized expression. An ordinary `заметка AND config*` uses Russian
stemming and an English prefix. Invalid syntax is not silently corrected.

SQLite v6 adds exact/stem views. Migration populates them from the existing index;
subsequent indexing updates them by fingerprint in the same transaction as links
and the change journal. Deletions, revoked permissions and changes to owned
metadata are reflected during updates.

```sh
agent-memory index --project agent-memory
agent-memory search --project agent-memory --kind decision --status accepted 'заметка'
```

CLI search reads the current index; running index first refreshes the files.
MCP memory_search refreshes the index automatically before searching.

Quote literal terms containing punctuation for FTS5, keeping operators outside:
`коммит OR commit OR "Co-Authored-By"`. Double embedded quotes (`a"b` becomes
`"a""b"`). JSON encoding is a separate layer:

```json
{"project":"agent-memory","query":"коммит OR commit OR \"Co-Authored-By\""}
```

Unquoted hyphens can produce `no such column: Authored`; quote the literal term
and retry instead of interpreting a syntax error as no matches.

## Literal input, result limits and CLI JSON

FTS5 remains the default query language. For user text containing punctuation or
operator words, set `literal: true` in MCP or pass `--literal` in the CLI. The
server quotes the entire input as one phrase and doubles embedded quotes:
`OR`, `NOT`, column selectors and `*` have no operator meaning in this mode.
This is a token phrase search, not an exact byte or substring comparison;
punctuation follows the FTS tokenizer and Russian stemming still applies.
To combine alternatives, use default FTS5 mode and quote each literal term.
Malformed FTS5 queries return a quoting/literal-mode hint without silently
changing the query's meaning.

Both interfaces accept a result limit from 1 through 50, defaulting to 50.
The limit applies after filtering and ranking, so smaller requests retain the
same leading results. Invalid limits are rejected.

```sh
agent-memory search --project agent-memory --literal --limit 5 --json 'Co-Authored-By'
```

```json
{"project":"agent-memory","query":"Co-Authored-By","literal":true,"limit":5}
```

CLI `--json` emits the same `{"hits":[...]}` envelope as MCP, including source
and document IDs, paths, excerpts, line numbers and full-document `content_hash`.
No matches produce `{"hits":[]}`; errors still fail the command. Without `--json`,
the existing human-readable CLI format is retained. CLI search still reads the
current index; these flags do not trigger indexing.

## Ordinary word queries

FTS remains the default (`--mode fts`, MCP `mode: "fts"`). Select natural mode
explicitly when the input is an ordinary question rather than an FTS expression:

```sh
agent-memory search --project demo --mode natural --json 'avoid charging a merchant twice after a timeout'
```

```json
{"project":"demo","query":"avoid charging a merchant twice after a timeout","mode":"natural","limit":5}
```

Natural mode splits on non-alphanumeric characters, lowercases and deduplicates
words, and safely quotes them into an OR query. Operators, quotes, punctuation
and `*` have no special meaning. It accepts at most 8192 UTF-8 bytes and 128
distinct words; empty or punctuation-only input returns no hits. `literal: true`
cannot be combined with natural mode.

Candidates use the existing readable-project index and kind/status filters.
Documents rank by distinct normalized query words covered across all their chunks,
then distinct original words covered. Within equal document coverage, chunk
coverage and existing exact/BM25/path/line ordering choose a representative.
Natural mode returns one best chunk per document. Coverage includes
indexed metadata as well as headings and body; the result remains a real source
chunk with a full-document hash. The limit applies after ranking all candidates.
Russian Snowball stemming is retained; English words are not stemmed. There are
no synonyms, stop-word removal, embeddings or model calls. Common words can
produce many weak matches, and lexical retrieval cannot guarantee recovery of
paraphrases with no shared words. Existing FTS and context behavior are unchanged.

## Optional natural coverage filter

FTS remains the default. Natural search retains its existing results unless
`--min-coverage` (MCP `min_coverage`) is explicitly supplied:

```sh
agent-memory search --project demo --mode natural --min-coverage 0.6 --explain 'webhook retry delivery'
```

```json
{"project":"demo","query":"webhook retry delivery","mode":"natural","min_coverage":0.6,"explain":true}
```

The value must be finite and between 0 and 1 inclusive, and requires natural
mode. Coverage is matched distinct Russian-normalized query words divided by
all distinct normalized query words. Matches may span document sections and
indexed metadata. Morphological variants count once. Filtering precedes the
result limit and preserves the remaining order. Zero preserves all candidates;
one requires every normalized word. Punctuation-only queries still return none.

With this filter and explain enabled, a top-level `coverage_filter` includes
rejected document source/path, matched/query word counts, ratio, threshold and
`below_min_coverage` reason, even when `hits` is empty. Returned hits include
their own coverage counts. Diagnostics never add rejected bodies. Without
explain, rejected documents are absent from output. A high lexical coverage is
not a semantic relevance score; common words can pass, and paraphrases can fail.
No automatic threshold or fallback is selected.

## Paths for scripts

Use `--paths-only` to print unique source-relative paths, one per line, in the
order of their first search hit:

```sh
agent-memory search --project demo --paths-only 'backup OR recovery'
agent-memory search --project demo --mode natural --paths-only 'database recovery'
```

No hits produce empty stdout with exit status 0. The flag works with either search
mode, literal phrases, filters and `--limit`, and cannot combine with `--json`.
Deduplication happens after the search limit: FTS limits chunks, so several hits
from one document may produce fewer paths than the requested limit. Paths are
relative to each hit's source; identical paths across sources collapse to one
line. Use JSON when source IDs or full identity matter. Line-delimited output
assumes filenames without newline characters; JSON preserves arbitrary filenames.
MCP already returns paths and source IDs in structured hits.

## Explain diagnostics

See [explain and context economy](explain-and-context-economy.md) for opt-in
CLI/MCP diagnostics, byte accounting, the repeatable benchmark and testing.
