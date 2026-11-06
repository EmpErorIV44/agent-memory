---
id: bench-ru-document-warehouse-cutoff
title: Завершение приёма на складе
memory:
  id: bench-ru-warehouse-cutoff
  project: bench-ru
  kind: decision
  status: accepted
  summary: Завершение приёма на складе
  relations:
    - kind: relates_to
      target: bench-ru-route-timezone
---

# Завершение приёма на складе

Заказы, поступившие до 16:00 по времени склада, передаются перевозчику сегодня. После 16:00 дата передачи переносится на следующий рабочий день. Праздничный календарь берётся из настроек склада, а не из часового пояса сервера.
