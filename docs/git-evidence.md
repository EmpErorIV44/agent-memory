# Git Evidence

Проверка evidence сравнивает сохранённую ссылку на Git blob с **сырыми байтами
локального рабочего файла**, включая незакоммиченные и staged изменения. Это
проверка изменений свидетельства, а не истинности утверждения. Markdown, статус
Memory и Git-репозиторий не изменяются; индекс памяти обновляется перед запросом.

## Ссылка в памяти

Существующий формат Evidence сохраняется, миграция SQLite не нужна:

```yaml
memory:
  id: parser-contract
  project: demo
  kind: fact
  status: accepted
  summary: Parser rejects invalid input
  evidence:
    - kind: file
      reference: git:code@0123456789abcdef0123456789abcdef01234567:src/parser.rs
    - kind: commit
      reference: git:code@0123456789abcdef0123456789abcdef01234567
```

Нужен полный commit OID (40 либо 64 hex символа). Ветки, сокращённые SHA и
revision expressions не поддерживаются. `code` — alias, а не путь. Путь файла
задаётся от корня репозитория, с `/`; пробелы и Unicode допустимы. Указание строк
и поиск переименованных файлов пока не поддерживаются. Старая произвольная
строка evidence продолжает читаться, но её проверка вернёт `unknown`.

## Доступ задаёт владелец

Только параметры запуска разрешают доступ к репозиторию. Markdown не может
назначить корень или получить права другого проекта. Alias ограничен проектом.

```sh
agent-memory --database memory.sqlite mcp --project demo \
  --evidence-repository demo:code=/absolute/path/to/code
```

MCP `memory_verify_evidence` принимает `project` и `memory_id`. Разрешение
`--allow-write` не требуется; сама Memory должна находиться в читаемом источнике.
Без mappings инструмент доступен, но Git-ссылки дадут `repository_not_configured`.
Повторяйте `--evidence-repository` для нескольких корней.

CLI использует тот же контракт:

```sh
agent-memory --database memory.sqlite verify-evidence --project demo \
  --memory-id parser-contract --evidence-repository demo:code=/absolute/path/to/code
```

## Результат

Ответ содержит `memory_id`, хеш Markdown `content_hash`, `local_only: true`,
`comparison: raw_worktree_bytes` и массив `evidence`. Каждая ссылка имеет
`kind`, `reference`, `status` и машинный `reason`:

- `unchanged`: байты файла совпадают с baseline blob. Для commit evidence это
  только наличие commit object локально, не достижимость из HEAD.
- `changed`: содержимое рабочего файла отличается от baseline.
- `missing`: baseline-файл существует, но текущий путь или родитель удалён.
- `unknown`: неподдерживаемая ссылка, нет разрешённого repository, недоступен
  commit/blob, небезопасный путь, symlink/submodule, превышен лимит или ошибка Git.

Для сравнения файла возвращаются `baseline_commit`, `baseline_blob`,
`current_blob` (если доступен), без содержимого кода. Посторонний новый коммит
не делает неизменённый файл устаревшим. Если baseline-файла нет, результат
`unknown`, поскольку сравнивать нечего. Исполняемый бит файла не сравнивается.

## Границы

Никаких fetch/pull, тестов, URL-запросов, shell-команд из evidence, Git filters,
textconv или external diff. Git routing переменные окружения исключаются;
replace objects и lazy fetch отключены. Используются literal pathspecs, bounded
вывод и таймаут команды 5 секунд. Читается максимум 16 MiB на файл. Symlink и
вложенные репозитории не проверяются. Репозитории, их Git configuration и
локальный Git executable считаются доверенными настройками владельца.

CRLF, LFS smudge и иные working-tree преобразования могут дать `changed` даже
при чистом `git status`: намеренно сравниваются сырые байты без запуска filters.
Проверка не является атомарным snapshot нескольких файлов. Изменения файлов или
репозитория во время проверки требуют повторного запроса. Для bare repositories,
проверки строк, автоматического исправления evidence и определения истинности
знания потребуется отдельная реализация.
