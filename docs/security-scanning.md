# Security checks in CI

The [Security](../.github/workflows/security.yml) workflow runs on pushes to
`main`, pull requests, manual Actions runs, and weekly on Mondays at 05:15 UTC.
After committing and pushing, the workflow appears in GitHub Actions; manual
execution becomes available once it reaches the default branch. The presence of
a configuration does not mean remote CI has already run.

## Source code: Semgrep CE

Semgrep CE runs local Rust rules from [.semgrep.yml](../.semgrep.yml):

- Explicit shell-interpreter launches through `Command::new`.
- Direct `format!` use in selected SQL `prepare`/`execute` calls.
- `unsafe` blocks requiring separate review.
- Explicitly disabled TLS certificate or hostname verification.

Rules are restricted to `crates/**/src/**/*.rs`. The workflow also checks them
against dangerous and safe synthetic examples.

```sh
semgrep scan --oss-only --config .semgrep.yml --error --sarif \
  --output semgrep.sarif --metrics off --disable-version-check crates
```

This is a limited check of specific local patterns. It does not replace full
interprocedural data-flow analysis, manual review, or functional tests. The
rule file determines which constructs are checked; no findings does not prove
that the code is secure. `--error` fails the scan when findings exist. SARIF is
saved as the Actions artifact `semgrep-sarif` (file `semgrep.sarif`), without
uploading it to GitHub Code scanning.

## Dependencies: cargo audit

`cargo audit --file Cargo.lock` checks pinned dependency versions against the
current RustSec database downloaded on the runner. Detected vulnerabilities
fail the check; deprecation or maintenance notices need separate assessment,
rather than treating every warning as a vulnerability. Output is saved in the
`cargo-audit-report` artifact (file `cargo-audit.txt`). Both reports are uploaded
even after scanning errors if the files were created, and retained for 7 days.
Also check job logs: a tool failure is not a successful scan with no findings.

Scheduled runs matter even without code changes: new advisories can affect
versions already in the same lockfile.

The initial local check on 2026-10-30 found no known vulnerabilities, but reported
`RUSTSEC-2024-0449`: `unsafe-libyaml 0.2.11` is unmaintained (transitively through
`serde_yaml_ng`). The warning is not suppressed; migration to a maintained YAML
backend requires a separate frontmatter compatibility check. This result applies
to the advisory database revision checked; later scans may give different results.

On 2026-11-04, the audit reproduced the failure of Actions run `41453152212`
(commit `4d85785`): `pulldown-cmark 0.13.5` is affected by
[RUSTSEC-2026-0433](https://rustsec.org/advisories/RUSTSEC-2026-0433.html).
The dependency minimum and lockfile were updated to `0.13.6`, which fixes
quadratic parsing of nested image wikilinks. Current call sites use
`Parser::new` without `ENABLE_WIKILINKS`; the advisory identifies that option
as necessary for exploitation. No advisory suppression was added.

On 2026-11-10, the failures in Actions run `42855613501` were reproduced locally:

- Semgrep rejected a `prepare(&format!(...))` call in natural search. Its table
  names came from two constants; the query now uses two static SQL statements,
  keeping search expressions and project IDs as bound parameters.
- RustSec reported [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html)
  for bundled SQLite through `libsqlite3-sys 0.36.0` and
  [RUSTSEC-2026-0462](https://rustsec.org/advisories/RUSTSEC-2026-0462.html)
  for `rmcp 3.8.0`.
  `rusqlite` was upgraded to `0.41.1` (`libsqlite3-sys 0.39.1`) and `rmcp` to
  `3.9.0`, with corresponding minimum versions in `Cargo.toml` and a refreshed
  lockfile. No advisory or source-rule suppressions were added.

The SBOM job in that run succeeded: these two advisories have MEDIUM severity,
below its HIGH/CRITICAL threshold. RustSec remains an independent required check.

Local verification after the fixes: Semgrep CE 1.183.0 found no findings in 37
production Rust files (checked on an identical source copy in `/tmp` to avoid
slow WSL filesystem traversal); both rule tests passed. Cargo-audit 0.22.2 found
no vulnerabilities in 174 dependencies against the RustSec snapshot retrieved
on 2026-11-10; the existing unmaintained warning remains visible. Workspace
tests, strict Clippy, formatting, and release-packaging tests passed. These local
results do not establish a successful remote Actions run for the changes.

## Access and limitations

Scanning does not require a Semgrep account, service API tokens, or a paid GitHub
Code Security license. Reports are available as Actions artifacts; SARIF display
in private GitHub Code scanning is not enabled here. PRs use `pull_request`,
without `pull_request_target`, service secrets, or privileged execution of PR
code. Installing tools and updating advisory databases require runner network
access.

The SBOM check also runs in a separate temporary GitHub Actions job: no persistent
server, service account, or API secret is needed. This is a serverless operating
model on a runner, rather than a central component dashboard. Actions minutes
and artifact storage follow GitHub's terms, including private-repository quotas;
having no server does not eliminate CI resource consumption.

CodeQL supports Rust, but GitHub requires a qualifying organization plan with
GitHub Code Security for private or internal repositories. CodeQL is not selected
because the account settings are unknown; see the
[CodeQL documentation](https://docs.github.com/en/code-security/concepts/code-scanning/codeql/codeql-code-scanning).

## SBOM: Trivy without a server

The third independent job first creates a CycloneDX SBOM from the source tree and
`Cargo.lock`, then scans that SBOM with Trivy for known vulnerabilities. It covers
the source project's dependencies, rather than a complete inventory of runtime
files and components in the deployed binary. SBOM scanning does not replace
Semgrep or cargo audit: all three checks run independently.

Trivy version `0.76.1` is installed after verifying its official SHA-256 checksum.
The advisory database is downloaded to the job's local cache; updates use public
dependency metadata requests without service secrets. HIGH or CRITICAL
vulnerabilities fail the check, including those with no fix: `--ignore-unfixed`
is not used.

Main job commands:

```sh
trivy fs --format cyclonedx --output sbom.cdx.json \
  --skip-dirs target --skip-dirs .git --skip-dirs dist .
trivy sbom --scanners vuln --format json --output sbom-vulnerabilities.json \
  --severity HIGH,CRITICAL --exit-code 1 sbom.cdx.json
```

The `security-sbom` artifact contains `sbom.cdx.json` and
`sbom-vulnerabilities.json`, is retained for 7 days, and is uploaded after failures
if reports were created. Check the job result alongside the report: database
download or SBOM parsing failures do not mean there are no vulnerabilities.
Formats and scanning capabilities are described in the
[Trivy SBOM documentation](https://trivy.dev/latest/docs/target/sbom/).

## If Dependency-Track is needed

If "deeptrack" meant Dependency-Track, that is SBOM-based software composition
analysis (SCA), rather than Rust source-code analysis (SAST). Dependency-Track
accepts CycloneDX SBOMs and tracks component risks; see the
[platform overview](https://docs.dependencytrack.org/).

Connecting it remains a separate step: it requires a running server endpoint,
a configured project, and an API key in a GitHub secret to upload CycloneDX SBOMs.
Uploading to Dependency-Track is not enabled, and no dummy endpoints or secrets
have been added. The integration approach is described in the
[official CI/CD guide](https://docs.dependencytrack.org/usage/cicd/).
