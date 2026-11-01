# Note links and backlinks

Explicit links help agents find related decisions, constraints and implementation
results. Before saving a note, find and read suitable existing knowledge. If the
connection is supported, add a link with a clear explanation:

```markdown
## Related knowledge

- [Authentication decision](../decisions/auth.md): constrains this implementation.
- [[decisions/auth|Authentication decision]]
```

Do not invent missing targets or infer relationship types automatically from
matching words. Every note does not need a link: relevance depends on its content
and evidence.

## Reading links through MCP

```json
{"project":"agent-memory","source_id":1,"path":"implementations/auth.md","direction":"both"}
```

Pass this object to `memory_links`. Use an actual source and path; this operation
does not require a hash. `direction` selects `both`, `outgoing` or `incoming`:
outgoing links, incoming backlinks or both. The response contains `outgoing` and
`incoming` lists; an unselected direction returns an empty array. The default is
`both`. Backlinks are computed from other documents' links; there is no need to edit
target notes to maintain a separate backlinks list.

Each list item contains `kind` (`markdown` or `wiki`), the original `target`, a
one-based `start_line`, `status` (`resolved`, `missing`, `ambiguous`), the originating
`source` document and a resolved `target_document` or `null`. Document coordinates
include `document_id`, `source_id`, `path`, `stable_id` and full-document
`content_hash`; use them to read complete Markdown through `memory_get`.

Links resolve only within the same accessible source. Ordinary relative Markdown
links and `[[wiki-links]]` are supported. An `id:stable-id` target in
`[Label](id:stable-id)` or `[[id:stable-id]]` refers to the frontmatter `id` of a
document in the same source, not its `memory.id`. Wiki paths first match a path from
the source root or the originating note, then a unique filename; the `.md` extension
may be omitted. A `#heading` fragment is stripped when locating the target document:
heading existence is not checked. Missing or ambiguous targets remain unresolved;
agents must not substitute a guess.

## Documents and semantic Memory

A Markdown link connects documents. Frontmatter `memory.relations` is a separate
model of semantic relationships, such as `implements`, `depends_on` and `supersedes`
between stable Memory IDs. Add such a relation only when its meaning is supported
and the target Memory ID exists. An ordinary link does not itself prove that one
record implements, replaces or contradicts another.

## Renaming

Frontmatter `id` preserves document identity in the index when renamed, but does
not rewrite relative links in source Markdown. Check related paths after renaming.
Rebuilding SQLite from Markdown cannot recover an old path that no longer exists
in source files. For a link that must survive renaming and index recreation, use
`id:stable-id` and preserve that frontmatter `id` in the target document.
