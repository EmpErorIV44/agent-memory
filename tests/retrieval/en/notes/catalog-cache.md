---
id: bench-en-doc-catalog-cache
title: Product catalog cache invalidation
memory:
  id: bench-en-catalog-cache
  project: bench-en
  kind: implementation
  status: implemented
  summary: Product catalog cache invalidation
---
# Product catalog cache invalidation

Catalog pages are cached in Redis for five minutes. A product price update publishes catalog.price.changed and deletes the affected product cache entry. Search result pages expire naturally; checkout always loads the authoritative price.
