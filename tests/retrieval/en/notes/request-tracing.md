---
id: bench-en-doc-request-tracing
title: Request tracing across workers
memory:
  id: bench-en-request-tracing
  project: bench-en
  kind: implementation
  status: implemented
  summary: Request tracing across workers
  relations:
    - kind: relates_to
      target: bench-en-audit-redaction
---
# Request tracing across workers

The gateway accepts a valid W3C traceparent header or creates a new trace. Queue messages propagate the trace context into background workers. Operators use the trace identifier to connect API latency with worker execution. Customer secrets remain excluded from trace attributes.
