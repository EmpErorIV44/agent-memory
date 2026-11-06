---
id: bench-en-doc-session-expiry
title: Session expiration and rotation
memory:
  id: bench-en-session-expiry
  project: bench-en
  kind: decision
  status: accepted
  summary: Session expiration and rotation
---
# Session expiration and rotation

Browser sessions expire after 30 minutes of inactivity and at most 12 hours after sign-in. Refresh tokens rotate on every successful refresh. Reuse of an old refresh token revokes the entire session family.
