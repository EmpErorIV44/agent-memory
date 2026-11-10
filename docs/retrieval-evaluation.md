# Synthetic retrieval evaluation

The frozen fictional notes and judgments in `tests/retrieval/en` and
`tests/retrieval/ru` measure retrieval quality independently of the production
knowledge base. Dataset authors choose expected documents before running the
retriever. Do not adjust labels to make the current implementation pass; review
an incorrect label with its rationale and record the reason for a revision.

Build the current CLI, then run:

```sh
cargo build -p agent-memory --locked -j 1
python3 scripts/evaluate-retrieval.py --output /tmp/retrieval-baseline.json
python3 -m unittest discover -s tests -p 'test_evaluate_retrieval.py'
```

`--binary PATH` selects another build. `--fixtures PATH` selects a suite parent
containing `NAME/cases.json` and `NAME/notes/`. Projects are named `bench-NAME`.
Only Markdown under each `notes/` directory enters its source. The runner copies
sources into a temporary directory and registers them read-only in a new temporary
SQLite database using a temporary owner configuration. It does not read or modify
the configured project memory database, call MCP, or synchronize Git.

`--output PATH.json` writes a detailed JSON report and companion Markdown report.
Without it the complete report is printed. SHA256 fingerprints of the binary and
all suite inputs (`cases.json`, suite README, and note files), plus the CLI version,
are captured before evaluation. Root report artifacts are excluded from input hashes. Reports
contain expected labels, ordered document paths, missing expected paths, forbidden
returns, errors, and context omission/truncation/budget diagnostics. Keep local
reports under `/tmp`, or explicitly retain a reviewed baseline as a project artifact.

## Cases and metrics

Each suite's `cases.json` is an array of objects with `id`, `mode` (`search` or
`context`), `query`, `relevant`, `forbidden`, `category`, and `rationale`. Label
paths are relative to that suite's `notes/`. Search cases optionally specify
`kind`, `status`, and `literal`. Context cases optionally specify `max_tokens`
(default 4000). Context queries are task text; search queries retain FTS5 semantics
unless `literal` is true.

Search requests 50 chunks, then deduplicates document paths in first-hit order
before computing recall@5, recall@10, and reciprocal rank. This is document recall
within the returned chunk window, not an exhaustive corpus ranking. Context recall
uses all returned document entries. Both report full recall and judged precision
(number of labeled relevant returned documents divided by all returned documents).
Labels may be incomplete: an unlabeled return can still be useful, so judged
precision is a diagnostic rather than proof of irrelevance. Context excerpts are
not equivalent to full document recall; truncation is reported separately.

A positive search case passes when all labeled relevant documents appear in its
first ten unique documents; a positive context case passes when all appear anywhere
in its returned entries. Any forbidden document fails the case. Empty `relevant`
is a negative case and requires no returned documents; its recall and reciprocal
rank are null, excluded from aggregate means. A missing relevant note is a measured
quality gap, not automatically an implementation bug, especially for paraphrases
outside the documented lexical search contract or tight context budgets.

Exit 0 means measurement completed, including measured quality gaps. Exit 2 means
an invalid fixture, failed CLI invocation, malformed result, or budget diagnostic
failure; case-level errors remain in the report. Every returned path and full-document
hash must match a note in the selected suite, so same-named documents cannot hide
project leakage. Context budget validation checks both the reported estimate and
the independently serialized compact JSON UTF-8 byte length. Optional `--fail-on-miss` returns
1 for quality gaps if there are no operational errors. No retrieval implementation
is changed by this tool.

## Comparing natural queries

Run the same frozen suites with `--search-mode natural` and a separate output
path. Only non-literal search cases use natural mode; explicitly labeled literal
phrase cases retain their phrase contract. Context cases remain unchanged.
The report records `search_mode`; compare input hashes, complete ordered rankings,
judged precision, forbidden returns, recall and operational errors with an FTS run
of the same binary. Do not edit fixtures or labels to improve a score.

## Coverage threshold experiment

```sh
python3 scripts/benchmark-search-coverage.py --output /tmp/agent-memory-coverage
```

This runs unchanged RU/EN labels and the separate `tests/retrieval-noise` corpus
with natural mode at no threshold, 0, 0.5, 0.6 and 1. Each report retains fixture
and binary hashes. Compare recall, ranked paths, judged precision, forbidden
returns and negative violations. The noise corpus includes a common-word false
positive, a partial-match negative and a complete lexical collision that coverage
cannot reject. A successful script exit establishes operational success only.
No automatic production threshold follows from these small synthetic datasets.

Individual runs accept `--search-mode natural --min-coverage 0.6` on
`scripts/evaluate-retrieval.py`. Literal cases and context remain unchanged.
