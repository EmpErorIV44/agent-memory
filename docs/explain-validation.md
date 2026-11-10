# Explain validation, 2026-11-11

Full validation completed in Linux/WSL against the local uncommitted implementation.
No observed regression relative to the retained lexical retrieval baselines.

| Check | Result |
| --- | --- |
| `make check` | Exit 0: 232 Rust tests, 15 Python tests, formatting and strict Clippy passed |
| FTS retrieval | 18/23 search quality cases; 13/13 context cases; zero operational errors |
| Natural retrieval | 22/23 search quality cases; 13/13 context cases; zero operational errors |
| Explain parity | 72/72 plain/explained comparisons equal across both modes |
| Additional stress matrix | 48/48 checks passed |
| Economy benchmark | Nine cases repeated twice; full reports identical and case results match retained baseline |

For both retrieval modes, input hashes, complete ordered returned document paths,
quality metrics, missing/forbidden returns and context diagnostics match the
retained `tests/retrieval/search-{fts,natural}.json` reports. No fixture labels
were changed. All benchmark and stress reports match the same CLI SHA256:

```text
c516d6042eb665f3bdb4b56ec85a7ab7a0378a0013d93fc991b5a3bc4be84efe
```

## Additional checks

The stress matrix uses a temporary source with 32 structured notes (including
Unicode, quotes, multiple sections, constraints, relations and retired records),
a readable note whose structured metadata belongs to another project, and a
separate unreadable source. It checks both FTS/natural search at limits 1, 5 and
50 with four queries, plus context for three queries at budgets 512, 800, 1200,
2000, 4000, 8000, 16000 and 32768. Explain/plain output equality, context byte
bounds and hidden-source/foreign-summary isolation pass. A unique alphanumeric
summary marker is used so natural tokenization cannot accidentally match a
public path component.

The retrieval parity run independently checks compact context byte estimates,
remaining budget, grouped entry sizes, excerpt/metadata accounting and candidate
outcomes in addition to exact returned payload equality. The economy runner also
checks hash-verified note identity, labeled fact coverage and added explain bytes.

## Measured limitations

- Default FTS still has five known quality misses; natural mode still has one.
  Identical baseline results establish absence of an observed regression, not
  perfect retrieval or proof of semantic relevance.
- Economy results remain three sufficient excerpts, four cases losing facts that
  exist in selected full documents, one wrong-document retrieval and one correct
  empty negative result. Some failures are deliberate diagnostic fixtures.
- The long-section case still loses the final fact at both 1200 and 8000 budgets:
  the excerpt grows from 544 to the existing 2400-byte per-entry cap. Increasing
  total context capacity alone does not remove that cap.
- Explain consumes additional output bytes. Context diagnostics remain outside
  the normal context budget, and the budgeted `context` is unchanged.
- Tests run in Linux/WSL with local Unix sockets allowed. Native Windows/macOS,
  production answer generation, model token counts, latency and remote account
  behavior were not measured. The installed client MCP process remains separate
  from the build tested here.

## Evidence and reproduction

Run the commands in [explain and economy testing](explain-and-context-economy.md),
then `make check` and the existing retrieval runner with `--search-mode fts` and
`--search-mode natural`. The full Rust suite requires local Unix sockets for IPC.
Retain the same fixture hashes when comparing future builds.

Session artifacts are under `/tmp/agent-memory-large-20261111/`:
`summary.json`, `make-check.log`, `comparison.json`, `retrieval-fts.{json,md}`,
`retrieval-natural.{json,md}`, `explain-parity.json`, `economy-1.{json,md}`,
`economy-2.{json,md}`, `stress.json`, and their runner scripts/logs.
The temporary parity and stress scripts are retained there for exact session
reproduction; the permanent benchmark runners are in `scripts/`.
