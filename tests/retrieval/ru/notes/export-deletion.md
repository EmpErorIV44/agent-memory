---
id: bench-ru-document-export-deletion
title: Удаление готовых выгрузок
memory:
  id: bench-ru-export-deletion
  project: bench-ru
  kind: decision
  status: accepted
  summary: Удаление готовых выгрузок
  relations:
    - kind: depends_on
      target: bench-ru-tenant-isolation
---

# Удаление готовых выгрузок

Готовые CSV-выгрузки отправлений удаляются из объектного хранилища через 24 часа. Ссылка подписана и действует 15 минут. Перед выдачей ссылки проверяется заказчик текущего пользователя. Удаление запускается фоновым заданием каждый час.
