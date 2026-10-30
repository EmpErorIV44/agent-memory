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
Для разных окончаний используйте префикс, например `правил*`. Поиск учитывает
также путь, ID документа/Memory и summary в отдельном индексе метаданных;
подробности и ограничения — в [формате памяти](docs/structured-memory.md).

По умолчанию база находится в `./agent-memory.sqlite`. Другой путь:

```sh
cargo run -p agent-memory -- --database /tmp/demo.sqlite init
```

Сохраните регистрацию проектов, источников и прав в отдельную конфигурацию владельца:

```sh
agent-memory --database ./agent-memory.sqlite config export --output ./agent-memory.config.json
agent-memory --database ./agent-memory.sqlite rebuild --config ./agent-memory.config.json
```

`rebuild` восстанавливает регистрации и индекс после удаления SQLite. Для существующей
базы изменения применяются одной транзакцией: ошибка оставляет прежние регистрации
и индекс. Markdown не изменяется. После изменения проектов, источников или прав
обновите конфигурацию: `config export` без `--output` выводит JSON в stdout,
а с `--output` создаёт только новый файл. Формат и примеры описаны в
[инструкции](docs/structured-memory.md).

## MCP по stdio

После регистрации проекта и источника:

```sh
cargo run -p agent-memory -- mcp --project banking
```

Это сервер для запуска MCP-клиентом: он читает протокол из stdin, а stdout
содержит только сообщения MCP. Диагностика идёт в stderr. Используется
официальный Rust SDK [rmcp](https://github.com/modelcontextprotocol/rust-sdk).

Список доступных проектов задаёт владелец процесса через `--project`.
Чтобы разрешить два проекта, укажите `--project banking --project storefront`.
Все проекты должны быть зарегистрированы заранее. Глобального поиска нет:
каждый вызов относится к одному разрешённому проекту и проверяет `can_read`
источника на уровне SQL. Сервер сообщает разрешённые проекты при инициализации.

Пример конфигурации для клиента с полем `mcpServers`, работающего внутри WSL:

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "/mnt/c/Users/maksim.smirnov/ai-mem/target/debug/agent-memory",
      "args": [
        "--database", "/mnt/c/Users/maksim.smirnov/ai-mem/agent-memory.sqlite",
        "mcp", "--project", "banking"
      ]
    }
  }
}
```

Перед подключением выполните `cargo build --workspace`. Укажите абсолютные пути
к бинарнику и базе в окружении, где клиент запускает процесс; Linux-бинарник из
WSL требует запуска в WSL. Конкретный формат настроек зависит от MCP-клиента.

### Подключение с Windows-хоста

Windows-клиент может запускать stdio MCP через `wsl.exe`.
Готовый пример для текущих путей: [examples/mcp-windows.json](examples/mcp-windows.json).
Скопируйте запись `agent-memory` в настройки `mcpServers` своего клиента.
Дистрибутив (`wsl --list --quiet`), Linux-пользователь и абсолютные пути должны
соответствовать вашей установке. HTTP-транспорт в утилите пока отсутствует.

Для подготовки базы знаний выполните **в WSL** из корня репозитория:

```sh
cargo build --workspace --locked
target/debug/agent-memory --database /mnt/c/Users/maksim.smirnov/agent-memory-knowlage/agent-memory.sqlite rebuild --config /mnt/c/Users/maksim.smirnov/agent-memory-knowlage/agent-memory.config.example.json
```

`rebuild` заменяет регистрации в выбранном индексе регистрациями из конфигурации.
Отдельная база знаний из примера сохраняет демонстрационный проект `banking`
в прежней базе приложения. Режим записи — `confirm`.

Проверка из PowerShell в каталоге `C:\Users\maksim.smirnov\ai-mem` (Python 3.8+):

```powershell
python scripts/check-mcp.py examples/mcp-windows.json
```

Проверка того же Windows-моста из WSL:

```sh
python3 scripts/check-mcp.py examples/mcp-windows.json --from-wsl
```

Она выполняет MCP handshake, проверяет девять инструментов, включая историю
и восстановление, и поиск по реальной базе знаний. После настройки перезапустите
подключение MCP в клиенте. Клиент запускает собственный процесс через stdin/stdout;
постоянно запущенный сервер или открытый порт не нужны.

Инструменты:

- `memory_search({"project":"banking","query":"OAuth2"})` возвращает
  `{"hits":[...]}` — до 50 фрагментов с `chunk_id`, `document_id`, `source_id`,
  `content_hash`, `path`, `heading`, `body`, `start_line`, `end_line`.
- `memory_get({"project":"banking","chunk_id":123,"content_hash":"…"})`
  возвращает `{"chunk":{...}}`. ID и полный хеш берутся из результата поиска.
  Хеш защищает от чтения устаревшей ревизии: если документ изменился, повторите
  поиск. ID фрагментов не являются постоянными ссылками.
  С `"include_document":true` дополнительно возвращает полный исходный Markdown
  в `document.content` для последующего обновления.
- `memory_get({"project":"banking","source_id":1,"path":"note.md","content_hash":"…"})`
  возвращает полный Markdown в `document.content` напрямую. Координаты и хеш
  полного документа можно взять из `memory_list`; передавайте
  `document.content_hash` как `expected_hash` при обновлении.
- `memory_sources({"project":"banking"})` — доступные источники и права записи.
- `memory_list({"project":"banking"})` — структурированные знания из YAML
  frontmatter: `Memory`, подтверждения `Evidence` и доступные связи `Relation`.
- `memory_get_record({"project":"banking","memory_id":"auth-policy"})` —
  запись по устойчивому логическому ID, отличному от ID документа и фрагмента.
  Это структурированная проекция, а не полный исходный Markdown.
- `memory_write` — создание/обновление полного Markdown; доступен только при
  запуске с `--allow-write` и требует источника с `can_write=1`.
- `memory_history`, `memory_revision_get`, `memory_restore` — долговечные версии,
  чтение полного снимка и восстановление с проверкой текущего хеша.
  Формат и команды описаны в [инструкции по истории](docs/revisions.md).

При запуске и перед поиском/чтением сервер инкрементально обновляет индекс
соответствующего проекта. Ошибка сканирования возвращается клиенту; устаревший
индекс молча не используется. Регистрация источников и изменение прав доступны
владельцу через CLI. Найденный Markdown считается
недоверенными данными, а не инструкциями для агента.

## Запись и правила поведения

Включение записи, режимы `confirm`/`verified`, установка skill для всех проектов
пользователя и параметры инструментов описаны в
[инструкции](docs/write-and-agent-behavior.md).

Сервер передаёт краткую политику агенту при подключении. Переносимый
[skill agent-memory](skills/agent-memory/SKILL.md) помогает искать прошлые решения
и сохранять проверенные результаты. Для записи нужны явные права; существующие
подключения сохраняют режим чтения.

Явно включаемые коммиты и push Markdown по таймеру или после записи описаны в
[инструкции Git sync](docs/git-sync.md).

## Проверка

```sh
cargo test --workspace
cargo fmt --all -- --check
```

Тесты проверяют номера строк, поиск по реальным fixtures, изоляцию проектов,
повторную индексацию без изменения записей, добавление/обновление/удаление,
откат при ошибке записи, отказ сканирования, целостность FTS, восстановление новой
базы и вызовы CLI отдельными процессами.
MCP-тест запускает отдельный сервер, выполняет handshake через SDK-клиент,
обнаруживает инструменты, проверяет поиск/get, запрет чужих проектов и ID,
обновление/удаление без перезапуска и отклонение устаревшей ссылки.
Тесты записи проверяют права, конфликты хешей, конкурирующие записи, запрет
выхода за корень и символических ссылок, полное чтение Markdown, создание и
обновление через CLI/MCP и сохранение файла при сбое обновления индекса.

## Границы первого этапа

- `memory-core`: чтение UTF-8 Markdown, SHA-256 и разбиение по заголовкам.
- `memory-sqlite`: миграция, транзакционная индексация и FTS5-поиск.
- `agent-memory`: команды `init`, `project create`, `source add`, `index`, `search`,
  `source list`, `write`, `config export`, `rebuild`, `memory list`, `memory get`,
  `mcp`; инструменты `memory_search`, `memory_get`, `memory_sources`,
  `memory_write`, `memory_list`, `memory_get_record`.

Индексация сравнивает SHA-256: неизменённые документы и фрагменты не переписываются,
изменённые документы сохраняют ID, их фрагменты заменяются. Исчезнувшие файлы
удаляются только после успешного сканирования источника. Все источники проекта
обрабатываются в одной транзакции: ошибка откатывает весь запуск. CLI выводит
`added=… updated=… unchanged=… deleted=…` только после успешного завершения.

Каждый запуск пока читает, хеширует и разбирает все Markdown-файлы; экономия
касается записей в SQLite и FTS. Поле `id` в YAML сохраняет идентичность документа
при переименовании внутри источника; без `id` переименование считается удалением
и добавлением. Дубликаты ID отклоняются с откатом всей индексации.
Метаданные времени изменения не обновляются, если хеш остался прежним.
Символические ссылки внутри источника пропускаются. Полный YAML frontmatter
исключается из текстового поиска; путь, ID и summary доступны через отдельный
индекс метаданных. `id` и блок `memory` разбираются и валидируются отдельно.
Структурированные знания и связи восстанавливаются из Markdown при `rebuild`.
История ревизий хранится отдельно от SQLite в `.agent-memory-history/`;
команды `history list/get/restore` доступны через CLI и MCP. В этой версии
история привязана к пути документа. Проверка Git Evidence и daemon остаются
следующими этапами.
