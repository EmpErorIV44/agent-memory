---
id: bench-ru-document-export-old-retention
title: Прежний срок хранения выгрузок
memory:
  id: bench-ru-export-old-retention
  project: bench-ru
  kind: decision
  status: superseded
  summary: Прежний срок хранения выгрузок
---

# Прежний срок хранения выгрузок

Раньше CSV-выгрузки отправлений хранились семь дней. Длительные ссылки позволяли повторно скачать файл без нового запроса. Это решение отменено после проверки модели доступа; текущий срок хранения описан в export-deletion.md.
