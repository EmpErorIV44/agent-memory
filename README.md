# Agent Memory

Первый сценарий: Markdown → SQLite FTS5 → поиск через CLI с изоляцией проектов.
Markdown остаётся источником истины; база — пересоздаваемый индекс.

## Запуск

Нужны Rust/Cargo и C-компилятор для встроенного SQLite. В текущей WSL-среде
Rust уже установлен. Команды выполняются из корня репозитория:

```sh
cargo build --workspace
cargo run -p agent-memory -- init
cargo run -p agent-memory -- project create banking
cargo run -p agent-memory -- source add --project banking ./tests/fixtures/banking
cargo run -p agent-memory -- index --project banking
cargo run -p agent-memory -- search --project banking OAuth2

cargo run -p agent-memory -- project create storefront
cargo run -p agent-memory -- source add --project storefront ./tests/fixtures/storefront
cargo run -p agent-memory -- index --project storefront
cargo run -p agent-memory -- search --project storefront OAuth2
```

Результат содержит относительный путь от корня источника, включительный диапазон
строк (с 1), заголовок и текст фрагмента. Общий термин `OAuth2` находится в обоих
проектах, но запрос возвращает только документы указанного проекта.
FTS5 поддерживает слова, фразы и операторы запросов; русской морфологии нет.

По умолчанию база находится в `./agent-memory.sqlite`. Другой путь:

```sh
cargo run -p agent-memory -- --database /tmp/demo.sqlite init
```

После удаления базы повторите `init`, регистрацию проектов/источников и `index`.
Регистрация пока хранится только в SQLite, поэтому её требуется повторить.

## Проверка

```sh
cargo test --workspace
cargo fmt --all -- --check
```

Тесты проверяют номера строк, поиск по реальным fixtures, изоляцию проектов,
повторную индексацию, восстановление новой базы и вызовы CLI отдельными процессами.

## Границы первого этапа

- `memory-core`: чтение UTF-8 Markdown, SHA-256 и разбиение по заголовкам.
- `memory-sqlite`: миграция, транзакционная индексация и FTS5-поиск.
- `agent-memory`: команды `init`, `project create`, `source add`, `index`, `search`.

Сейчас индексирование полностью заменяет документы зарегистрированных источников
в одной транзакции. Символические ссылки внутри источника пропускаются. YAML
frontmatter исключается из поиска; извлечение стабильного ID пока не реализовано.
Инкрементальное обновление, MCP, Memory, история, Git и daemon — следующие этапы.
