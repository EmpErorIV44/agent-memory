---
id: bench-en-doc-database-migration
title: Compatible database migrations
memory:
  id: bench-en-database-migration
  project: bench-en
  kind: constraint
  status: accepted
  summary: Compatible database migrations
---
# Compatible database migrations

Database schema changes must support the previous application version during a rolling deployment. Add nullable columns first, deploy writers, backfill in bounded batches, and enforce constraints in a later release. Production migrations must not drop columns in the initial deployment.
