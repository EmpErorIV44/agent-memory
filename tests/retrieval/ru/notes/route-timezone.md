---
id: bench-ru-document-route-timezone
title: Часовой пояс окна доставки
memory:
  id: bench-ru-route-timezone
  project: bench-ru
  kind: implementation
  status: implemented
  summary: Часовой пояс окна доставки
---

# Часовой пояс окна доставки

Обещанное окно доставки хранится с IANA timezone склада. В расписании курьера время показывается в часовом поясе адреса доставки. При переходе на летнее время несуществующее локальное время отклоняется при создании окна.
