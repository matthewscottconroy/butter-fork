# Butterfork — Claude Code context

## What this project is

Butterfork collapses the gap between *using* open source software and *contributing*
to it into a single integrated workflow backed by an LLM coding agent. See
[butterfork-design.md](butterfork-design.md) for the full design.

**Language:** Rust  
**License:** Apache-2.0 OR MIT

## Workspace layout

Each `bf-*` directory is a standalone binary crate. `bf-common` is the only library
crate — it holds shared types, the NDJSON event protocol, and sysexits.h exit codes.

| Crate | Binary | Purpose |
|-------|--------|---------|
| `bf` | `bf` | Top-level orchestrator |
| `bf-catalog` | `bf-catalog` | Project discovery |
| `bf-forge` | `bf-forge` | Forge dispatcher (forks, clones, issues, PRs) |
| `bf-forge-github` | `bf-forge-github` | GitHub backend (wraps `gh`) |
| `bf-build` | `bf-build` | Build adapter dispatcher |
| `bf-build-cargo` | `bf-build-cargo` | Cargo build adapter |
| `bf-sandbox` | `bf-sandbox` | Sandboxed execution (wraps `bwrap`) |
| `bf-install` | `bf-install` | Generational install manager |
| `bf-index` | `bf-index` | Codebase index (tree-sitter + embeddings) |
| `bf-agent` | `bf-agent` | LLM tool-use loop |
| `bf-scaffold` | `bf-scaffold` | Project scaffolding |
| `bf-daemon` | `bf-daemon` | Optional long-run supervisor |
| `bf-bootstrap` | `bf-bootstrap` | One-shot bootstrap installer |

## Key conventions

- **stdout = NDJSON, stderr = human readable.** Every component emits one JSON
  object per line on stdout for machine consumers; progress messages go to stderr.
- **Exit codes follow sysexits.h.** Constants live in `bf_common::exit`.
- **Adapters and backends are PATH-discovered.** `bf-build` finds `bf-build-cargo`
  on PATH; `bf-forge` finds `bf-forge-github`. Override with `BF_BUILD` / `BF_FORGE`
  env vars.
- **State lives in `~/.butterfork/`.** The SQLite DB is at `~/.butterfork/state.db`.
  Schema documented in [docs/schema.md](docs/schema.md).
- **Every high-level `bf` flow has a shell-script equivalent in `scripts/`.**
  CI verifies parity. If you change a pipeline in `bf/src/main.rs`, update the
  corresponding script too.
- **DCO, not CLA.** Sign off commits with `git commit -s`.

## Build & test

```sh
cargo build --all       # build everything
cargo test --all        # run all tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Phase 0 scope (current)

- `bf-build-cargo`, `bf-install`, minimal `bf`, `bf-forge-github`
- Prove: fork → build → install → rollback loop on ripgrep/fd/bat
- No daemon, no agent, no UI yet

## Adding a new component

1. `cargo new --name bf-<name> bf-<name>` inside the workspace root
2. Add `"bf-<name>"` to `[workspace] members` in the root `Cargo.toml`
3. Implement the CLI contract described in the design doc (section 6)
4. Make stdout NDJSON and stderr human-readable
5. Add `bf_common` as a dependency for event types and exit codes
