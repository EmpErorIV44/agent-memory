---
id: bench-en-doc-database-recovery
title: Database point in time recovery
memory:
  id: bench-en-database-recovery
  project: bench-en
  kind: decision
  status: accepted
  summary: Database point in time recovery
---
# Database point in time recovery

PostgreSQL base backups run nightly and WAL archives are retained for seven days. Recovery drills restore into an isolated database. The recovery point objective is five minutes and the recovery time objective is one hour.
