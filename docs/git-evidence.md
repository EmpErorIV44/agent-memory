# Git Evidence

Evidence verification compares a stored Git blob reference with the **raw bytes
of the local working file**, including uncommitted and staged changes. This checks
whether evidence changed, rather than whether a statement is true. Markdown, Memory
status and the Git repository remain unchanged; the memory index is refreshed
before the request.

## References in memory

The existing Evidence format is preserved; no SQLite migration is needed:

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

A full commit OID is required (40 or 64 hexadecimal characters). Branches,
abbreviated SHAs and revision expressions are unsupported. `code` is an alias,
not a path. File paths are relative to the repository root and use `/`; spaces
and Unicode are allowed. Line references and renamed-file lookup are not yet
supported. Old arbitrary evidence strings remain readable, but verification
returns `unknown` for them.

## Owner-configured access

Only startup options authorize repository access. Markdown cannot assign a root
or acquire another project's permissions. Aliases are project-scoped.

```sh
agent-memory --database memory.sqlite mcp --project demo \
  --evidence-repository demo:code=/absolute/path/to/code
```

MCP `memory_verify_evidence` accepts `project` and `memory_id`. `--allow-write` is
not required; the Memory itself must be in a readable source. Without mappings,
the tool is available but Git references return `repository_not_configured`.
Repeat `--evidence-repository` for multiple roots.

CLI uses the same contract:

```sh
agent-memory --database memory.sqlite verify-evidence --project demo \
  --memory-id parser-contract --evidence-repository demo:code=/absolute/path/to/code
```

## Results

The response contains `memory_id`, the Markdown `content_hash`, `local_only: true`,
`comparison: raw_worktree_bytes` and an `evidence` array. Each reference has `kind`,
`reference`, `status` and a machine-readable `reason`:

- `unchanged`: file bytes match the baseline blob. For commit evidence, this only
  means the commit object exists locally, not that it is reachable from HEAD.
- `changed`: working file content differs from the baseline.
- `missing`: the baseline file exists, but the current path or a parent was deleted.
- `unknown`: unsupported reference, no authorized repository, unavailable
  commit/blob, unsafe path, symlink/submodule, exceeded limit or Git error.

File comparisons return `baseline_commit`, `baseline_blob` and `current_blob`
(if available), without code content. An unrelated new commit does not make an
unchanged file stale. If the baseline file is absent, the result is `unknown`
because there is nothing to compare. The executable bit is not compared.

## Boundaries

No fetch/pull, tests, URL requests, shell commands from evidence, Git filters,
textconv or external diff are run. Git routing environment variables are removed;
replace objects and lazy fetch are disabled. Operations use literal pathspecs,
bounded output and a 5-second command timeout. At most 16 MiB is read per file.
Symlinks and nested repositories are not checked. Repositories, their Git
configuration and the local Git executable are trusted owner settings.

CRLF, LFS smudge and other working-tree transformations may produce `changed` even
with a clean `git status`: raw bytes are deliberately compared without running
filters. Verification is not an atomic snapshot of multiple files. Changes to
files or the repository during verification require another request. Bare
repositories, line verification, automatic evidence repair and determining whether
knowledge is true require separate implementation.
