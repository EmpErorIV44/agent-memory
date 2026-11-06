---
id: bench-ru-document-returns-refund
title: Возврат наложенного платежа
memory:
  id: bench-ru-returns-refund
  project: bench-ru
  kind: decision
  status: planned
  summary: Возврат наложенного платежа
  relations:
    - kind: depends_on
      target: bench-ru-money-precision
---

# Возврат наложенного платежа

Планируется возврат наложенного платежа после подтверждения приёма посылки складом. Сумма возврата берётся из оплаченной суммы заказа; доставка не возвращается автоматически. До реализации оператор оформляет возврат вручную.
