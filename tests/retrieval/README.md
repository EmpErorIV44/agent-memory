# Frozen synthetic retrieval benchmark

Two independent agents authored the English commerce and Russian delivery
corpora without inspecting retrieval code or running the retriever. Each contains
18 fictional notes and 18 cases. Labels were frozen before the first measurement;
they identify required documents rather than every potentially useful document.
These sources are test data, not evidence about real projects.

See [the runner guide](../../docs/retrieval-evaluation.md) for isolation, metrics,
exit codes and the reproduction command. The retained
[v0.3.2 baseline](baseline-v0.3.2.md) has a companion
[JSON report](baseline-v0.3.2.json) with input hashes, rankings and diagnostics.

## Initial baseline, 2026-11-06

- Search: 18/23 cases meet their labels, including the negative case. Macro
  Recall@5 and Recall@10 are 77.3% across the 22 positive cases. All five misses
  are natural paraphrases returning no hits; the default search API evaluates
  a multiword FTS expression with implicit AND, not a natural language question.
- Context: 11/13 cases include every required note. All ten cases with budgets
  of 16000 or 32768 include every required note. At budget 4000, the English
  webhook task and Russian duplicate-event task omit their central task note;
  their larger-budget counterparts recover it. The third 4000 case passes.
- No CLI errors, forbidden historical returns, or negative-case violations were
  observed. Required-document recall does not establish that an excerpt contains
  all necessary information. Unlabeled returns are not necessarily irrelevant.

This small synthetic corpus establishes a reproducible baseline, not production
quality or a statistical estimate. No ranking or context implementation was
changed. The next candidate experiment is a separate natural-language query mode
and better allocation between baseline constraints and task-specific context.
Keep this baseline and its labels intact when comparing future implementations;
do not silently change the existing FTS contract to make paraphrases pass.

## Balanced context allocation, 2026-11-06

The [follow-up report](context-balanced.md) and its [JSON](context-balanced.json)
use the exact same fixture hashes. The development build retains version 0.3.2;
its distinct binary hash identifies the implementation under evaluation.

Context now passes 13/13 cases, including all three at budget 4000. The two
previously omitted central task notes are included, and no accepted constraint is
omitted in these three cases. All responses remain within their measured compact
JSON byte budgets. Search results and metrics are unchanged at 18/23 cases.

The implementation admits short excerpts before growing them and alternates
constraints with task matches during admission. Context-only match ordering
prefers broader normalized task-term coverage. The tradeoff is shorter excerpts
when more documents fit: document recall does not prove that every needed fact
fits in an excerpt. Full-note reads remain necessary before relying on details.

Validation: all 210 workspace Rust tests and seven evaluator tests passed;
formatting and strict Clippy passed. The workspace regression required permission
for temporary local Unix sockets. Five independent budget regressions cover long
constraints, oversized metadata, Unicode/escaping, source isolation and task-term
coverage. These are source changes; the installed MCP process was not upgraded.

## Explicit natural search, 2026-11-06

The same-binary [FTS report](search-fts.md) and [natural report](search-natural.md)
have companion JSON files with full rankings. All fixture fingerprints match the
original baseline. Explicit literal cases retain phrase mode in both runs.

| Metric | FTS | Natural |
| --- | --- | --- |
| Search cases passing | 18/23 | 22/23 |
| Macro recall@5 and @10 (22 positive cases) | 77.3% | 95.5% |
| Mean reciprocal rank | 0.773 | 0.924 |
| Mean judged precision | 93.1% | 56.1% |
| Returned search documents | 20 | 95 |
| Returned documents without a relevant label | 3 | 74 |
| Forbidden / negative violations / operational errors | 0 / 0 / 0 | 0 / 0 / 0 |
| Context cases passing | 13/13 | 13/13 |

All FTS and context ordered document results match the balanced-context report.
Natural recovers four of the five previous paraphrase misses; `ru-money-synonym`
still misses the expected note and returns four unlabeled notes. More recall
comes with more noise: judged labels are incomplete, but the increased returned
set and lower judged precision are material limitations. This remains an explicit
lexical alternative, not a semantic search claim or a change to default FTS.
