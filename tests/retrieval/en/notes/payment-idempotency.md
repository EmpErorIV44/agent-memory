---
id: bench-en-doc-payment-idempotency
title: Idempotency for payment creation
memory:
  id: bench-en-payment-idempotency
  project: bench-en
  kind: decision
  status: accepted
  summary: Idempotency for payment creation
---
# Idempotency for payment creation

POST /v1/payments requires the Idempotency-Key header. The same key and request body return the original payment for 48 hours. Reusing a key with a different amount returns HTTP 409. Keys are scoped to a merchant account.
