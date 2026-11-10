# Context economy fixtures

Small fictional English/Russian notes and nine labeled queries for a repeatable
text-volume and fact-presence experiment. These are not real project memories.
Cross-section, long-section and mismatched-query cases deliberately exercise
missing facts; do not treat their quality misses as operational failures.

Run `python3 scripts/benchmark-context-economy.py --output /tmp/economy.json`
from the repository root after building the CLI. See
[method, diagnostics and testing](../../docs/explain-and-context-economy.md).
These authored fixtures are not an independent or production-quality evaluation.
Keep notes and labels frozen for comparisons; input hashes accompany every report.

## Initial implementation check, 2026-11-11

The [initial report](baseline.md) and [JSON evidence](baseline.json) record nine
cases with zero operational errors and unchanged explain/plain retrieval outputs.
Three excerpts contain all labeled facts, four lose facts available in selected
full documents, one deliberately mismatched query is a retrieval miss, and the
negative query returns nothing. These counts describe the authored cases, not a
production success rate. The larger context budget grows the long-section excerpt
from 544 to 2400 bytes, but cannot include the final fact beyond the existing
per-entry cap.

Focused validation passed: 21 SQLite tests across explain, natural search, FTS
options, Russian search and context/budget regressions; three CLI/MCP integration
tests; 15 Python tests across packaging and both benchmark runners. Formatting,
strict workspace Clippy and whitespace checks passed. After final diagnostic
accounting edits, both new SQLite tests and the new CLI/MCP test were rerun and
passed; the retained report was generated from that build. The full workspace
acceptance run and existing retrieval baseline comparisons are the next stage.
The installed client MCP binary has not been upgraded by this implementation.

The subsequent [full validation](../../docs/explain-validation.md) passed on
2026-11-11; see that report for full-suite counts and baseline comparisons.
