---
id: bench-en-doc-customer-export
title: Customer export completion notification
memory:
  id: bench-en-customer-export
  project: bench-en
  kind: implementation
  status: implemented
  summary: Customer export completion notification
  relations:
    - kind: supersedes
      target: bench-en-export-streaming
---
# Customer export completion notification

A customer requests a CSV export from account settings. A background worker writes an encrypted archive to object storage and sends a download link when the archive is ready. The link expires after fifteen minutes. Export files are removed after one day.
