# Binary releases

The [Release binaries](../.github/workflows/release.yml) workflow builds binaries
on native GitHub Actions runners. Pushing a `v*` tag runs checks and builds for
all platforms, then publishes a GitHub Release with archives and `SHA256SUMS`.
Each platform must pass `cargo test --workspace --locked`; publication starts
only after the entire check and build matrix succeeds. A tag containing a
hyphen, such as `v0.1.0-alpha.1`, creates a prerelease.
A manual Actions run builds artifacts for review without publishing a Release,
even when a tag is selected. These instructions do not imply that remote CI has
already run.

## Platforms and installation

| Platform | Variant |
|---|---|
| Linux x86_64 | Built on Ubuntu 22.04 with glibc; dynamic system dependencies |
| macOS ARM64 | Native Apple Silicon build; Intel macOS is not included |
| Windows | Linux x86_64 build through WSL; no native `.exe` in Releases |

The Linux archive is not a universal static binary: verify compatibility with
other systems separately. Native Windows support is deferred: history storage
needs directory-synchronization fixes and passing functional write/history
tests on Windows before an `.exe` can be published.
SQLite is bundled into the binary. Git needs to be installed at runtime only
for Git features; ordinary memory reads and writes do not require Git.

Archives are named `agent-memory-VERSION-TARGET.tar.gz`, where `TARGET` is
`x86_64-unknown-linux-gnu` or `aarch64-apple-darwin`. Each contains the binary,
README, and LICENSE; a separate `.sha256` is published alongside it, and the
Release also includes a combined `SHA256SUMS`.

Download your platform's archive and `SHA256SUMS` from the same Release. Compare
the archive's SHA-256 with its entry: use `sha256sum ARCHIVE` on Linux,
`shasum -a 256 ARCHIVE` on macOS, or `Get-FileHash ARCHIVE -Algorithm SHA256`
in PowerShell. Extract the binary, README, and LICENSE. On Linux/macOS, run
`chmod +x agent-memory` if needed; add the directory to `PATH` or use an absolute
path in the MCP client. On Windows, extract the Linux archive inside WSL and
launch it through `wsl.exe`, using Linux paths for the binary, SQLite, and sources.
See the [MCP configuration example](../examples/mcp-windows.json).

## Preparing a tag as the owner

The version in `[workspace.package]` in `Cargo.toml` must match the tag without
its `v` prefix. The current version `0.1.0` corresponds to `v0.1.0`. For the first
alpha, first change the version to `0.1.0-alpha.1`, update the lockfile, and check
the build:

```sh
cargo check --workspace
cargo test --workspace --locked
git diff -- Cargo.toml Cargo.lock
```

Commit the verified changes and push the branch normally. When the owner decides
to publish that specific commit, they create and push the tag:

```sh
git tag v0.1.0-alpha.1
git push origin v0.1.0-alpha.1
```

The tag must point to a commit containing the workflow and matching version.
Monitor Actions jobs after the push; a failed run does not produce a ready
Release. These commands are for the operator: the documentation itself does not
create a tag, push, or publish a release.

## GitHub Actions usage

Standard GitHub-hosted runners are free for public repositories. Private
repositories are subject to their plan's included quotas and overage charges;
artifact storage is accounted for separately. Larger runners have separate
terms. See the current [GitHub Actions billing documentation](https://docs.github.com/en/billing/concepts/product-billing/github-actions).
