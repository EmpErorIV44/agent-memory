---
id: bench-en-doc-webhook-retry
title: Webhook delivery retry policy
memory:
  id: bench-en-webhook-retry
  project: bench-en
  kind: decision
  status: accepted
  summary: Webhook delivery retry policy
  relations:
    - kind: supersedes
      target: bench-en-webhook-fixed-delay
---
# Webhook delivery retry policy

Failed outbound webhook deliveries use exponential backoff with jitter. Retry HTTP 429 and 5xx responses for at most 24 hours. A 410 response permanently disables the subscription. A unique delivery identifier prevents duplicate side effects.
