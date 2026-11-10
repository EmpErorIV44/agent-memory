# Optional search coverage validation, 2026-11-11

Natural search now accepts optional `min_coverage` in CLI/MCP. FTS remains the
default, and natural without a threshold preserves the frozen ordered results,
fixture hashes and quality summaries. Filtering uses distinct Russian-normalized
query word coverage across a document, before limit. Explain reports rejected
paths and counts even when no result survives. It is not semantic scoring.

## Frozen RU/EN experiment

| Natural threshold | Search cases passed | Recall@5 | Judged precision |
| --- | --- | --- | --- |
| Omitted or 0 | 22/23 | 95.45% | 56.05% |
| 0.5 | 20/23 | 86.36% | 83.33% |
| 0.6 | 18/23 | 77.27% | 90.20% |
| 1 | 18/23 | 77.27% | 93.14% |

All 13 context cases remain unchanged. Judged precision uses incomplete synthetic
labels and is a noise diagnostic, not a semantic quality guarantee. Higher
thresholds discard relevant paraphrases; no automatic threshold was selected.

The separate four-case noise corpus improves from 1/4 to 3/4 at thresholds
0.5, 0.6 and 1. It removes common-word and partial-negative matches. The complete
lexical collision deliberately remains a negative violation at every threshold.

## Checks and reproduction

Targeted SQLite coverage/explain tests and CLI/MCP parity tests passed in WSL.
Full `make check` passed: 233 Rust tests, 15 Python tests, formatting and strict
Clippy. Native Windows `cargo check -p agent-memory --tests --locked --offline`
also passed, with existing platform-specific warnings.
Checks include validation, threshold boundaries, normalization, cross-section
coverage, pre-limit filtering, empty results and filtered/project-safe rejection
diagnostics. Independent read-only review found no blocking issue. Its benchmark
binary-consistency finding was fixed by checking one SHA before/after every run.

```sh
make check
python3 scripts/benchmark-search-coverage.py --output /tmp/agent-memory-coverage-20261111
```

The runner retains per-run JSON, `parity.json`, `summary.json` and `summary.md`.
Final report directory: `/tmp/agent-memory-coverage-final-20261111` in Ubuntu WSL.
Tested CLI SHA256:
`ec62ef1fd74bb1c34caa183d9dbe0b04cd426bc9fc958571425f6e2d6299230c`.
It verifies frozen FTS/natural fixture hashes, complete case results and summaries,
and refuses a binary change during or between runs. Source changes are local;
the installed MCP process is separate from the tested build. Native Windows
test execution encountered source-directory access denial; Linux/WSL is the
validation platform. No release or installed-client update is implied.
