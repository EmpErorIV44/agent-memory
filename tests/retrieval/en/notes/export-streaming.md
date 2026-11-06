---
id: bench-en-doc-export-streaming
title: Retired synchronous customer export
memory:
  id: bench-en-export-streaming
  project: bench-en
  kind: decision
  status: deprecated
  summary: Retired synchronous customer export
---
# Retired synchronous customer export

The original export endpoint streamed all customer records through the web process. Large exports exceeded the gateway timeout. The synchronous export endpoint is retired; the background worker is the supported path.
