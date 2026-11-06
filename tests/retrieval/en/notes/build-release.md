---
id: bench-en-doc-build-release
title: Release artifact verification
memory:
  id: bench-en-build-release
  project: bench-en
  kind: implementation
  status: implemented
  summary: Release artifact verification
---
# Release artifact verification

Release archives include SHA256SUMS and a detached signature. Deployment verifies the archive checksum and signature before unpacking. A mismatched checksum stops installation and preserves the previous application binary.
