---
id: bench-en-doc-delivery-receipts
title: Webhook delivery receipts
memory:
  id: bench-en-delivery-receipts
  project: bench-en
  kind: implementation
  status: implemented
  summary: Webhook delivery receipts
  relations:
    - kind: relates_to
      target: bench-en-webhook-retry
---
# Webhook delivery receipts

The delivery_receipts table records subscription_id, delivery_id, attempt number, next_attempt_at and response status. A unique index on subscription_id and delivery_id supports idempotent dispatch. The dispatcher schedules attempts according to the current retry policy.
