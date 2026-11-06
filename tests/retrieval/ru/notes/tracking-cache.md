---
id: bench-ru-document-tracking-cache
title: Кэш отслеживания отправлений
memory:
  id: bench-ru-tracking-cache
  project: bench-ru
  kind: implementation
  status: implemented
  summary: Кэш отслеживания отправлений
  relations:
    - kind: depends_on
      target: bench-ru-tenant-isolation
    - kind: relates_to
      target: bench-ru-duplicate-webhooks
---

# Кэш отслеживания отправлений

Страница отслеживания кэширует ответ перевозчика на 60 секунд. Ключ кэша содержит tenant_id и номер накладной. После webhook о новом статусе запись немедленно инвалидируется. При недоступности перевозчика устаревший ответ показывается с отметкой времени.
