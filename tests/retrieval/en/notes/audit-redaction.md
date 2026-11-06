---
id: bench-en-doc-audit-redaction
title: Audit events exclude customer secrets
memory:
  id: bench-en-audit-redaction
  project: bench-en
  kind: constraint
  status: accepted
  summary: Audit events exclude customer secrets
---
# Audit events exclude customer secrets

Audit events may contain actor identifiers, operation names and resource identifiers. Access tokens, customer email addresses and raw webhook payloads must never enter audit logs. Use a payload digest when correlation is needed.
