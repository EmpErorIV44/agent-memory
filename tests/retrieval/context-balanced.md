# Synthetic retrieval evaluation

agent-memory 0.3.2

Synthetic, author-judged labels; judged_precision treats unlabeled returns as irrelevant and is not proof of irrelevance. Quality misses are not automatically product defects.

| Suite | Case | Mode | Recall@5 | Recall@10 | Recall | Forbidden | Error |
| --- | --- | --- | --- | --- | --- | --- | --- |
| en | en-search-webhook | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-webhook-filter | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-payment-header | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-duplicate-charge | search | 0.0 | 0.0 | 0.0 |  |  |
| en | en-search-signin | search | 0.0 | 0.0 | 0.0 |  |  |
| en | en-search-service-key | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-invoice | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-cache | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-export-ready | search | 0.0 | 0.0 | 0.0 |  |  |
| en | en-search-recovery | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-refund | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-checksum | search | 1.0 | 1.0 | 1.0 |  |  |
| en | en-context-webhook | context | 1.0 | 1.0 | 1.0 |  |  |
| en | en-context-migration | context | 1.0 | 1.0 | 1.0 |  |  |
| en | en-context-export | context | 1.0 | 1.0 | 1.0 |  |  |
| en | en-context-tracing | context | 1.0 | 1.0 | 1.0 |  |  |
| en | en-search-no-match | search | None | None | None |  |  |
| en | en-context-webhook-small-budget | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-inflection | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-idempotency-paraphrase | search | 0.0 | 0.0 | 0.0 |  |  |
| ru | ru-money-synonym | search | 0.0 | 0.0 | 0.0 |  |  |
| ru | ru-punctuation | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-filter-current | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-filter-implemented | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-signature | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-notification-versus-courier | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-phone-privacy | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-db-resource | search | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-duplicate | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-export | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-cutoff | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-refund | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-cache | context | 0.75 | 1.0 | 1.0 |  |  |
| ru | ru-context-no-lexical | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-duplicate-budget4000 | context | 1.0 | 1.0 | 1.0 |  |  |
| ru | ru-context-export-budget4000 | context | 1.0 | 1.0 | 1.0 |  |  |

## Aggregate metrics

```json
{
  "search": {
    "cases": 23,
    "errors": 0,
    "quality_passes": 18,
    "mean_recall_at_5": 0.7727272727272727,
    "mean_recall_at_10": 0.7727272727272727,
    "mean_recall": 0.7727272727272727,
    "mean_reciprocal_rank": 0.7727272727272727,
    "mean_judged_precision": 0.9313725490196079,
    "forbidden_violations": 0,
    "negative_violations": 0
  },
  "context": {
    "cases": 13,
    "errors": 0,
    "quality_passes": 13,
    "mean_recall_at_5": 0.9807692307692307,
    "mean_recall_at_10": 1.0,
    "mean_recall": 1.0,
    "mean_reciprocal_rank": 1.0,
    "mean_judged_precision": 0.42632666478820325,
    "forbidden_violations": 0,
    "negative_violations": 0
  }
}
```

Full rankings, labels, context diagnostics, and SHA256 fingerprints are in the companion JSON report.
