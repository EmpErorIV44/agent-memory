# Context economy benchmark

Natural search top 1 or context at labeled budget; identical selected documents for excerpt/full comparison. Full text read from hash-verified isolated fixtures, not timed memory_get calls.

| Case | Outcome | Excerpt bytes | Full document bytes | Explain extra bytes |
| --- | --- | ---: | ---: | ---: |
| en-direct | sufficient_excerpt | 60 | 157 | 796 |
| en-cross-section | excerpt_loss | 61 | 161 | 803 |
| ru-direct | sufficient_excerpt | 158 | 258 | 874 |
| ru-cross-section | excerpt_loss | 158 | 258 | 874 |
| en-flat | sufficient_excerpt | 68 | 69 | 784 |
| wrong-topic | retrieval_miss | 105 | 106 | 849 |
| negative | negative_pass | 0 | 0 | 0 |
| long-context-small | excerpt_loss | 544 | 4511 | 1042 |
| long-context-large | excerpt_loss | 2400 | 4511 | 1047 |

## Summary

```json
{
  "cases": 9,
  "operational_errors": 0,
  "sufficient_excerpt": 3,
  "excerpt_loss": 4,
  "retrieval_miss": 1,
  "negative_pass": 1,
  "negative_violation": 0,
  "excerpt_text_bytes": 3554,
  "selected_full_document_text_bytes": 10031
}
```

## Limits

- UTF-8 text bytes, not model tokens or latency.
- Exact normalized fact presence, not generated-answer correctness.
- Small authored synthetic corpus; misses and excerpt losses are expected diagnostic outcomes.
- Full documents deduplicated within each case; no cache across cases.
- Search already returns body: refetch bytes model avoidable repeated text, excluding MCP envelopes.
- Explain payload is measured separately and is outside the context budget.
