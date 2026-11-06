---
id: bench-ru-document-release-health
title: Проверка после выкладки
memory:
  id: bench-ru-release-health
  project: bench-ru
  kind: implementation
  status: implemented
  summary: Проверка после выкладки
---

# Проверка после выкладки

После выкладки оператор проверяет создание отправления, смену статуса через webhook и скачивание CSV. Проверка выполняется на отдельном тестовом заказчике. Готовность релиза не подтверждается одним ответом health endpoint.
