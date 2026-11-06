---
id: bench-ru-document-tenant-isolation
title: Изоляция данных заказчиков
memory:
  id: bench-ru-tenant-isolation
  project: bench-ru
  kind: constraint
  status: accepted
  summary: Изоляция данных заказчиков
---

# Изоляция данных заказчиков

Любой запрос к отправлениям обязан включать tenant_id. Курьер видит отправления только своего заказчика. Номер накладной не является разрешением на доступ. Ограничение распространяется на поиск, выгрузку и фоновые задания.
