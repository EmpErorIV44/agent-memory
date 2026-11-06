---
id: bench-en-doc-tenant-isolation
title: Tenant data isolation
memory:
  id: bench-en-tenant-isolation
  project: bench-en
  kind: constraint
  status: accepted
  summary: Tenant data isolation
---
# Tenant data isolation

Every API request must derive its tenant from the authenticated session. Never accept a tenant identifier from a client header as authorization. Database queries must include tenant_id. Background jobs carry a validated tenant context.
