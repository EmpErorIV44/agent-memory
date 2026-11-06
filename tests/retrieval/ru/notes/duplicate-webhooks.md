---
id: bench-ru-document-duplicate-webhooks
title: Защита от повторных уведомлений
memory:
  id: bench-ru-duplicate-webhooks
  project: bench-ru
  kind: implementation
  status: implemented
  summary: Защита от повторных уведомлений
  relations:
    - kind: depends_on
      target: bench-ru-tenant-isolation
---

# Защита от повторных уведомлений

Перевозчик повторяет webhook при потере ответа. Сохраняем event_id в таблице delivery_events с уникальным индексом по tenant_id и event_id. Повторное событие возвращает 200 и не меняет состояние отправления второй раз. Запись события и смена статуса выполняются одной транзакцией.
