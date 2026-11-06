---
id: bench-ru-document-float-prices-old
title: Старый расчёт стоимости доставки
memory:
  id: bench-ru-float-prices-old
  project: bench-ru
  kind: implementation
  status: deprecated
  summary: Старый расчёт стоимости доставки
---

# Старый расчёт стоимости доставки

В прототипе стоимость доставки вычислялась через float и округлялась перед сохранением. Этот код удалён: накопленные ошибки округления расходились с отчётом перевозчика. Актуальное ограничение описано в money-precision.md.
