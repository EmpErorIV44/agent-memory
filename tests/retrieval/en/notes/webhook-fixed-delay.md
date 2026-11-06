---
id: bench-en-doc-webhook-fixed-delay
title: Historical fixed delay webhook retries
memory:
  id: bench-en-webhook-fixed-delay
  project: bench-en
  kind: decision
  status: superseded
  summary: Historical fixed delay webhook retries
---
# Historical fixed delay webhook retries

The previous webhook dispatcher retried failures every 60 seconds for seven days. This fixed delay policy caused synchronized traffic bursts and has been replaced by exponential backoff.
