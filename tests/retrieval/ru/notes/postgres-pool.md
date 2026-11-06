---
id: bench-ru-document-postgres-pool
title: Пул соединений PostgreSQL
memory:
  id: bench-ru-postgres-pool
  project: bench-ru
  kind: implementation
  status: implemented
  summary: Пул соединений PostgreSQL
---

# Пул соединений PostgreSQL

API использует пул из двадцати соединений PostgreSQL. Фоновые выгрузки используют отдельный пул из четырёх соединений, чтобы большой CSV не блокировал выдачу списка отправлений. Таймаут ожидания соединения равен двум секундам.
