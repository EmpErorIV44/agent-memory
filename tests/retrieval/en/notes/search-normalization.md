---
id: bench-en-doc-search-normalization
title: Catalog text normalization
memory:
  id: bench-en-search-normalization
  project: bench-en
  kind: implementation
  status: implemented
  summary: Catalog text normalization
---
# Catalog text normalization

Catalog search folds accents and lowercases product names before indexing. It preserves SKU punctuation in the displayed value. The query adapter uses the same text normalization as the indexer. This note describes product discovery, not database backup searches.
