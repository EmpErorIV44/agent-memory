---
id: bench-ru-document-money-precision
title: Точность денежных расчётов
memory:
  id: bench-ru-money-precision
  project: bench-ru
  kind: constraint
  status: accepted
  summary: Точность денежных расчётов
---

# Точность денежных расчётов

Стоимость доставки и наложенный платёж храним целым числом копеек. Числа с плавающей точкой для денежных расчётов запрещены. В интерфейсе показываем рубли с двумя знаками после запятой.
