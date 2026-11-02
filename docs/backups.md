# Portable memory backups

Git protects tracked, committed files and their commit history. Agent Memory's
Git sync deliberately excludes hidden revision history, rename recovery journals,
and non-Markdown owner configuration. It also does not capture uncommitted edits
until they are synchronized. A backup collects these memory files and project
registrations into one verified, portable snapshot.

If you already back up all of those files and have tested your recovery process,
you do not need a second backup mechanism just to recover a committed note.

## Create and verify

The owner CLI backs up every registered source, deduplicating roots shared by
multiple projects and retaining each project's read/write permissions. This is
an owner operation, not an MCP tool restricted to the client's allowed projects.

```sh
agent-memory --database /absolute/path/memory.sqlite backup create \
  --output /absolute/path/backups/memory.ambackup

agent-memory backup verify --archive /absolute/path/backups/memory.ambackup
```

The backup contains:

- Markdown, including eligible hidden Markdown files and uncommitted edits.
- `.agent-memory-history/`, including saved revisions and pending records.
- `.agent-memory-operations/`, including unfinished rename journals.
- Portable project/source registrations and explicit access permissions.

Git metadata and commit history, SQLite indexes, locks, runtime sockets, and
non-Markdown attachments are excluded. Markdown inside Git repositories is
included. Symbolic links are never followed. Keep Git and attachment backups when
those are needed. SQLite is rebuilt after restoration; the archive is independent
of the original source directories and database.

Create the archive outside all registered sources, in an existing directory.
Existing output files are never overwritten. Stop edits while creating a backup:
the command checks source stability and refuses detected concurrent changes,
but external editors do not participate in Agent Memory's advisory locks.
Distinct nested source roots are currently refused; identical roots shared by
projects are supported.

The archive is an uncompressed, versioned binary format with a checksummed JSON
manifest and SHA-256 checksums for every payload file. Verification reads all
payload bytes, checks the manifest, and rejects truncation or trailing data.
Checksums detect corruption; they are not signatures or encryption. Version 1
limits are 1,024 distinct sources, 100,000 files, 64 MiB per file, 8 GiB of
payload, and a 16 MiB manifest. Unsupported paths or exceeded limits cause
refusal before publication.

## Restore into a new directory

Restore works without the original index or original source paths:

```sh
agent-memory restore --archive /absolute/path/backups/memory.ambackup \
  --destination /absolute/path/recovered-memory

agent-memory --database /absolute/path/recovered-memory/index.sqlite rebuild \
  --config /absolute/path/recovered-memory/agent-memory.config.json

agent-memory --database /absolute/path/recovered-memory/index.sqlite doctor \
  --project demo
```

Replace `demo` with a project from the restored configuration. Inspect the
restored notes and registrations, then point your MCP client or daemon at the
new database if you want to use the recovered copy. Restore does not start
services, reconnect remotes, or change a running client's configuration.

The destination must not exist, even as an empty directory or symlink. Restore
validates the archive under a private staging directory and publishes the
complete tree without replacing existing data. The original sources and index
are left alone. Restored files have private permissions (directories `0700`,
files `0600`). Numeric source IDs may change after rebuilding: discover them
again with `source list --project PROJECT`.

The restored layout is:

```text
recovered-memory/
  agent-memory.config.json
  sources/
    0/
      note.md
      .agent-memory-history/
      .agent-memory-operations/
```

The owner configuration uses relative paths resolved against its own directory.
Shared sources remain shared, and revision and operation journal bytes are kept
unchanged. Git remotes and repository configuration are not restored; configure
Git separately if the recovered copy should be synchronized.

Backup/restore supports Linux, macOS, and WSL. Native Windows support remains
deferred, as with the existing write/history release support.
