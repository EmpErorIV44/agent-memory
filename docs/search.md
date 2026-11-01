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
