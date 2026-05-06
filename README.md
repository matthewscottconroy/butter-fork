# Butterfork

> Fork, build, install, and improve open source software — as smooth as butter.

Butterfork collapses the gap between *using* open source software and *contributing* to it
into a single integrated workflow backed by an LLM coding agent. The target user is a
developer (or power user who codes occasionally) who uses a lot of OSS CLI tools and wishes
a papercut or missing feature were easier to fix and share back.

---

## Table of Contents

1. [What it does](#1-what-it-does)
2. [Design principles](#2-design-principles)
3. [Architecture](#3-architecture)
4. [Component reference](#4-component-reference)
5. [On-disk layout](#5-on-disk-layout)
6. [Data model](#6-data-model)
7. [Prerequisites](#7-prerequisites)
8. [Building from source](#8-building-from-source)
9. [Bootstrap install](#9-bootstrap-install)
10. [Usage walkthrough](#10-usage-walkthrough)
11. [Environment variables](#11-environment-variables)
12. [Configuration](#12-configuration)
13. [Security model](#13-security-model)
14. [Self-hosting](#14-self-hosting)
15. [Contributing](#15-contributing)
16. [License](#16-license)

---

## 1. What it does

### The contribution flow

```
bf install ripgrep
```

That one command:

1. Looks up `ripgrep` in the catalog to find `BurntSushi/ripgrep` on GitHub.
2. Forks the repo to your GitHub account.
3. Clones your fork into `~/.butterfork/repos/ripgrep`.
4. Detects the build system (Cargo), builds a release binary.
5. Registers the build as generation `1` under `~/.butterfork/generations/ripgrep/1/`.
6. Atomically symlinks `~/.butterfork/bin/rg` → generation `1`.

You now run *your own build* of `rg`. Later:

```
bf request ripgrep "add a --no-hidden flag that excludes hidden files by default"
```

This invokes the agent, which reads the source, drafts a patch on a feature branch,
runs `cargo test`, and installs the new build. Your `rg` now carries the flag.

When you are happy with the result:

```
bf submit ripgrep
```

This pushes the branch, drafts a PR body from the commit message, runs pre-flight checks
(DCO, tests, format, diff size, anti-spam heuristics), and opens the PR against upstream.

### The greenfield flow

The same pipeline covers starting a new project from an idea:

```
bf new my-tool --description "a Rust CLI that prints response headers for a URL" \
               --mode hello-world --language rust
```

`bf-scaffold` produces a directory with working code, tests, a README, CI config, and a
license. From there: build it, use it, iterate with the agent, and — when it is ready —
publish it to the catalog so other Butterfork users can fork and improve it.

There is one Butterfork workflow. Contribution to existing OSS and creation of new OSS
are the same pipeline. The scaffold is where the pipeline begins for greenfield projects;
every later step is shared code.

---

## 2. Design principles

### 2.1 Unix composition

Every capability is a standalone executable with a small, stable CLI contract. `bf` is a
thin orchestrator — it contains no capability that is not already in a component binary.
The invariant is enforced concretely: every high-level flow in `bf` has a documented
shell-script equivalent in `scripts/`, and CI verifies that both produce identical output.
This is what prevents `bf` from accreting into a monolith.

A user who wants only the sandboxed-build capability can use `bf-sandbox` and `bf-build`
from their own shell scripts without ever touching `bf`, the daemon, or the agent. A
contributor who wants to replace a component writes a new binary with the same CLI contract
and puts it on `PATH` — no plugin ABI, no SDK, no review process owned by the core team.

Where a component would duplicate an existing Unix tool, it wraps that tool thinly. `bf-forge`
wraps `gh`. `bf-sandbox` wraps `bubblewrap`. `bf-build-cargo` shells out to `cargo`.
Implementation budget is spent on the seams, not on redoing work the ecosystem does well.

### 2.2 Inspectable state

All durable state lives in plain files under `~/.butterfork/`. The database is SQLite,
readable with `sqlite3`. Repos are ordinary git checkouts. A user with a Unix shell can
inspect, back up, and reason about everything Butterfork has done without running Butterfork
at all.

### 2.3 OSS posture

Apache-2.0 OR MIT — the most compatible permissive pair, so any downstream OSS project
can absorb any piece of Butterfork without licensing friction. No CLA; the DCO is
sufficient and enforced on every PR. Public roadmap, public RFC process, per-component
commit access for sustained contributors, and an explicit commitment to never gate
existing capabilities behind payment or inject telemetry without opt-in.

---

## 3. Architecture

```
┌─────────────────────────────────────────────────┐
│  bf  (thin orchestrator)                        │
│  install · request · submit · new · doctor      │
└────────────┬──────────────────────────┬─────────┘
             │                          │
     ┌───────▼───────┐          ┌───────▼───────┐
     │  bf-forge     │          │  bf-build     │
     │  fork / clone │          │  detect / run │
     │  issue / PR   │          └───────┬───────┘
     └───────┬───────┘                  │
             │                   ┌──────▼──────────┐
     ┌───────▼───────┐           │ bf-build-cargo  │
     │bf-forge-github│           │ bf-build-cmake  │
     │ (wraps gh)    │           │ bf-build-npm    │
     └───────────────┘           └─────────────────┘
                                          │
                              ┌───────────▼──────────┐
                              │  bf-install           │
                              │  add · activate       │
                              │  rollback · gc        │
                              └──────────────────────┘

     bf-agent ──► Claude / Ollama API
     bf-index ──► tree-sitter + embeddings
     bf-sandbox ──► bubblewrap / container runtime
     bf-catalog ──► curated index + GitHub search
     bf-scaffold ──► templates + bf-agent
     bf-daemon ──► optional supervisor (long-run)
     bf-bootstrap ──► one-shot installer
```

Components communicate through **NDJSON on stdout** and **human-readable progress on
stderr**. Exit codes follow `sysexits.h`. Long-running commands stream one JSON object
per line so shell consumers can `| jq` them live.

The fat binary (`bf-fat`) packages all components as a single executable that dispatches
by `argv[0]`, busybox-style. It is the default artifact for end users; contributors work
with separate binaries.

---

## 4. Component reference

### `bf` — orchestrator

The top-level command. Implements user flows by scripting the component binaries.

```
bf install <slug-or-url>        # fork → clone → build → install
  --dest <path>                 # override clone destination
  --no-fork                     # skip forking (clone upstream directly)
  --debug                       # build in debug mode

bf request <slug> "<description>"  # invoke agent on a feature branch

bf submit <slug>                # push branch + open upstream PR

bf new <path>                   # scaffold a new OSS project
  --description "<text>"
  --mode hello-world|poc|design-doc
  --language <lang>
  --spec <design_doc.md>

bf doctor                       # check system health (tools, auth, components)

bf rescue list <slug>           # list install generations
bf rescue activate <slug> <id> # activate a specific generation (rollback)

bf help-all                     # discover all bf-* binaries on PATH

bf telemetry status|enable|disable|show|clear

bf self-test [--repo <path>] [--no-sandbox]  # integration self-test
```

### `bf-catalog` — project discovery

Searches the curated index, GitHub Search API, and user-added URLs.

```
bf-catalog search <query>    # NDJSON catalog entries
bf-catalog show <slug>       # detailed entry (includes upstream_url)
bf-catalog add <url>         # add a project to the local catalog
bf-catalog update            # refresh the cached index
```

Catalog entries carry contribution-friendliness signals: `CONTRIBUTING.md` presence,
code of conduct, license category, median PR response latency, and a composite score.
All of these are surfaced to the UI and equally available via `jq`.

### `bf-forge` — forge operations

Fork, clone, issue, and PR management. Dispatch to a backend by sniffing the URL
(github.com → `bf-forge-github`). Backends are PATH-discovered binaries.

```
bf-forge fork <upstream_url>
bf-forge clone <fork_url> <dest>
bf-forge issue open --repo <owner/repo> --title "…" --body "…"
bf-forge pr open --repo <owner/repo> --head <branch> --base main \
                 --title "…" --body "…"
bf-forge pr status <url>
bf-forge pr watch <url>      # streams events
```

### `bf-forge-github` — GitHub backend

A thin wrapper over the `gh` CLI. Requires `gh` to be installed and authenticated
(`gh auth login`). Emits `fork-created` and `pr-created` NDJSON events.

### `bf-build` — build dispatcher

Detect a project's build system and run the appropriate adapter.

```
bf-build detect <repo>       # { adapter, confidence, hints }
bf-build plan <repo>         # concrete BuildPlan JSON
bf-build run <repo>          # build; emits build-complete with manifest_path
  --release                  # release build (default when invoked from bf)
  --plan plan.json           # use a pre-generated plan
```

Adapters are `bf-build-<name>` on PATH. A new adapter is just an executable that responds
to `detect`, `plan`, and `run` — no API to learn.

### `bf-build-cargo` / `bf-build-cmake` / `bf-build-meson` / `bf-build-npm`

Build adapters for their respective ecosystems. Each writes an artifact manifest at
`<repo>/target/bf-artifact-manifest.json` (or equivalent) on success.

### `bf-sandbox` — sandboxed execution

Runs a command under a named sandbox profile. On Linux wraps `bubblewrap`; elsewhere
wraps a container runtime.

```
bf-sandbox run --profile build|agent|run \
               [--allow-net host[,host…]] \
               [--bind path[:mode]] \
               -- <cmd> [args…]
```

Named profiles ship with sensible defaults. User-overridable via
`~/.butterfork/sandbox-profiles/<name>.toml`. The binary is standalone useful for anyone
who builds untrusted code locally.

### `bf-install` — generational install manager

Generations are directories with a `/usr/local`-mirroring layout (`bin/`, `lib/`, `share/`).
The active generation is a symlink, swapped atomically.

```
bf-install add <project> <artifact_manifest.json>
bf-install activate <project> <generation_id>      # "latest" accepted
bf-install list [<project>]
bf-install rollback <project>
bf-install gc                  # remove inactive generations older than 7 days
```

### `bf-index` — codebase index

Maintains an incremental index (tree-sitter structural queries + optional embedding store
for semantic search). Index data lives under `.bf/index/` inside the repo (gitignored).

```
bf-index update <repo>
bf-index query <repo> --symbol <name>
bf-index query <repo> --grep <pattern>
bf-index query <repo> --semantic <text>   # requires embeddings
```

Pass `--no-embeddings` to skip the ML dependency; structural and grep queries still work.

### `bf-agent` — LLM tool-use loop

Reads a prompt and a tool manifest; drives a Claude (or Ollama) tool-use loop; streams
NDJSON events.

```
bf-agent --repo <path> --prompt "<text>" --tools <manifest.json>
         [--max-iterations 50]
         [--model claude-opus-4-7-20251101]
```

The tool manifest declares external commands (or built-ins) the agent may invoke. Built-in
tools available during a `bf request`: `read_file`, `write_file`, `list_files`,
`run_shell`, `git_diff`, `git_add`, `git_commit`.

Swapping the agent backend means replacing the `bf-agent` binary. The NDJSON event
protocol is stable and documented in `bf-common`. `bf-agent-ollama` is provided as an
alternative backend.

### `bf-scaffold` — project scaffolding

Turns an idea into a directory the rest of Butterfork can immediately consume.

```
bf-scaffold new <path>
  --description "<text>"
  --mode hello-world|poc|design-doc
  --language <lang>
  [--spec <design_doc.md>]
  [--template <name>]

bf-scaffold template list
bf-scaffold template add <git_url>
bf-scaffold template show <name>
```

Three output modes:

| Mode | Output |
|------|--------|
| `hello-world` | Working program, tests, build system, README, CI, license |
| `poc` | Core skeleton with explicit `TODO:` markers for the agent to fill in |
| `design-doc` | Repo with `docs/DESIGN.md` only; no executable code yet |

Templates are ordinary git repos with a `scaffold.toml` manifest. Add any git repo as a
template with `bf-scaffold template add <url>`.

### `bf-daemon` — optional supervisor

Runs the same component binaries as long-lived supervised tasks with persistence: watches
PRs, resumes interrupted builds, pipes review comments back into `bf-agent`. Reads and
writes the same SQLite DB as the CLI tools — it has no privileged capability. Entirely
optional; Butterfork without a daemon is still Butterfork.

**IPC:** Unix-domain socket at `$XDG_RUNTIME_DIR/butterfork.sock`  
**Log:** `~/.butterfork/logs/daemon.log`  
**PID file:** `~/.butterfork/daemon.pid`

```sh
# Start the daemon in the background
bf-daemon start

# Start attached (useful for debugging)
bf-daemon start --foreground

# Show status and active task list
bf-daemon status

# Watch a pull request for CI/merge state changes
bf-daemon watch-pr https://github.com/owner/repo/pull/42 --slug ripgrep

# Tail the daemon log
bf-daemon log --follow

# Stop gracefully
bf-daemon stop
```

`bf-forge-github pr watch` and `bf-forge-gitlab mr watch` automatically delegate to
`bf-daemon watch-pr` when the daemon is running. If no daemon is found, they fall back
to an in-process polling loop with a 4-hour timeout.

### `bf-bootstrap` — one-shot installer

Runs the fork → clone → build → install flow against the canonical `butterfork` repo. Only
used once, to get `bf` onto PATH before Butterfork can install itself. Statically links
only `bf-forge-github` and `bf-build-cargo`; no daemon, no agent, no UI. Keeping it tiny
makes it auditable.

---

## 5. On-disk layout

```
~/.butterfork/
├── state.db                   # SQLite, documented schema (docs/schema.md)
├── repos/
│   └── <slug>/                # ordinary git checkout (forkable by hand)
├── generations/
│   └── <slug>/
│       └── <id>/
│           ├── bin/           # compiled executables
│           ├── lib/
│           └── share/
├── bin/                       # symlinks → active generation per project
├── sandbox-profiles/          # user-overridable sandbox configs (.toml)
├── pr-policy/                 # per-project PR policy overrides (.toml)
├── logs/                      # plain text, one file per operation
├── telemetry.jsonl            # opt-in local event log (never transmitted)
└── telemetry-enabled          # sentinel file; delete to disable telemetry
```

Everything is readable, queryable, and backupable with standard Unix tools. There is no
proprietary state format and no server that needs to be running to inspect your data.

---

## 6. Data model

State lives in `~/.butterfork/state.db` (SQLite). Query it at any time:

```sh
sqlite3 ~/.butterfork/state.db .schema
sqlite3 ~/.butterfork/state.db 'SELECT slug, install_generation FROM projects'
```

### Key tables

**`projects`** — one row per managed OSS project.

| Column | Description |
|--------|-------------|
| `slug` | Short identifier, e.g. `ripgrep` |
| `upstream_url` | Original upstream repo URL |
| `fork_url` | Your fork URL |
| `repo_path` | Local checkout path |
| `install_generation` | Currently active generation ID |

**`generations`** — one row per build.

| Column | Description |
|--------|-------------|
| `id` | Generation ID |
| `project_id` | Parent project |
| `git_ref` | Commit SHA at build time |
| `active` | 1 if currently active |

**`intents`** — a change request from `bf request`.

| Column | Description |
|--------|-------------|
| `branch` | Feature branch, e.g. `bf/add-no-hidden-1714000000` |
| `status` | `open` / `in-progress` / `done` / `submitted` / `merged` |
| `title` | Short description |

**`agent_runs`** — one row per `bf-agent` invocation.

| Column | Description |
|--------|-------------|
| `tokens_in` / `tokens_out` | Tokens consumed |
| `cost_cents` | Estimated cost (integer, US cents × 100) |
| `result` | `success` / `failed` / `interrupted` |

**`prs`** — pull requests opened on behalf of an intent.

| Column | Description |
|--------|-------------|
| `upstream_pr_url` | URL on the upstream forge |
| `state` | `open` / `closed` / `merged` |

Full schema and migration history: [docs/schema.md](docs/schema.md).

---

## 7. Prerequisites

| Tool | Required for | Install |
|------|-------------|---------|
| `git` | Everything | System package manager |
| `cargo` | Building Rust projects | [rustup.rs](https://rustup.rs) |
| `gh` | GitHub fork/PR/clone | [cli.github.com](https://cli.github.com) |
| `bwrap` | Sandboxed builds (Linux) | `apt install bubblewrap` |
| `rg` | Faster index grep queries | Optional |
| `ANTHROPIC_API_KEY` env var | `bf-agent` (Claude backend) | [console.anthropic.com](https://console.anthropic.com) |
| Ollama running at `localhost:11434` | `bf-agent-ollama` backend | Optional |

After installing `gh`, authenticate once:

```sh
gh auth login
```

Run `bf doctor` at any time to verify all tools are present and authenticated.

---

## 8. Building from source

```sh
git clone https://github.com/matthewscottconroy/butter-fork
cd butter-fork

cargo build --all           # build every component binary
cargo test --all            # run all tests (unit + integration)
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

To install all components onto your PATH:

```sh
bash scripts/fat-install.sh   # installs the fat binary (recommended)
# — or —
cargo install --path bf
cargo install --path bf-catalog
cargo install --path bf-forge
cargo install --path bf-forge-github
cargo install --path bf-build
cargo install --path bf-build-cargo
cargo install --path bf-sandbox
cargo install --path bf-install
cargo install --path bf-index
cargo install --path bf-agent
cargo install --path bf-scaffold
cargo install --path bf-bootstrap
```

Then add `~/.butterfork/bin` to your `PATH` if you haven't already:

```sh
bash scripts/setup-path.sh   # prints the export line; source it or add to your shell rc
```

---

## 9. Bootstrap install

If you want to install Butterfork using Butterfork (the intended path after the first
bootstrap):

```sh
# Download the static bootstrap binary from GitHub Releases (Linux x86_64 example)
curl -Lo bf-bootstrap "https://github.com/matthewscottconroy/butter-fork/releases/latest/download/bf-bootstrap-x86_64-unknown-linux-musl"
chmod +x bf-bootstrap
./bf-bootstrap
```

The bootstrap binary forks the `butter-fork` repo, clones it, builds it with Cargo, and
installs it under `~/.butterfork/bin`. After that, every future Butterfork update flows
through `bf install` or `bf request`.

---

## 10. Usage walkthrough

### Install an OSS project

```sh
# From the catalog by slug
bf install ripgrep

# Or by full URL (if not in the catalog yet)
bf install https://github.com/sharkdp/fd

# Without forking (useful when testing or if you have no forge account)
bf install ripgrep --no-fork

# Debug build
bf install ripgrep --debug

# Clone to a custom path
bf install ripgrep --dest ~/src/ripgrep
```

Progress goes to stderr in human-readable form. The final NDJSON event on stdout:

```json
{"type":"install-complete","project":"ripgrep","generation_id":"1","bin_dir":"/home/you/.butterfork/bin"}
```

### Make a change request

```sh
bf request ripgrep "add a --no-hidden flag that excludes .hidden dirs by default"
```

The agent will:
1. Create a branch `bf/add-a---no-hidden-<timestamp>`.
2. Read relevant source files.
3. Draft a patch.
4. Run `cargo test`.
5. Commit with a DCO `Signed-off-by` trailer.
6. Rebuild and reinstall.

Follow the agent's progress on stderr. The modified `rg` is available immediately.
Iterate by running `bf request` again — the agent picks up the existing branch.

### Submit upstream

```sh
bf submit ripgrep
```

Pre-flight checks run before any PR is opened:
- `cargo test` must pass.
- DCO `Signed-off-by` required on every commit.
- Diff is checked for whitespace churn (>80 % whitespace → warning).
- Giant diff (>1000 changed lines by default) → warning.
- The branch must not be `main` or `master`.

After checks pass, the branch is pushed to your fork and a PR is opened against upstream.
The PR body includes the commit message and an AI-assistance disclosure footer
(configurable per project — see [Configuration](#12-configuration)).

### Scaffold a new project

```sh
# Produce a working hello-world
bf new ~/src/my-tool \
    --description "a Rust CLI that prints response headers for a URL" \
    --mode hello-world \
    --language rust

# Design-doc only (think before coding)
bf new ~/src/my-idea --description "…" --mode design-doc
```

The scaffold is immediately buildable: `bf install ~/src/my-tool` will build and install it.

### Rollback

```sh
bf rescue list ripgrep           # show all generations
bf rescue activate ripgrep 1     # atomically activate generation 1
```

### Check system health

```sh
bf doctor
```

Reports status of `git`, `gh`, `cargo`, `bwrap`, `ANTHROPIC_API_KEY`, Ollama,
and all installed `bf-*` components. Exits non-zero if any required dependency is absent.

### Integration self-test

```sh
bf self-test                     # test against current directory
bf self-test --repo ~/src/foo    # test against a specific repo
bf self-test --no-sandbox        # skip sandbox test (useful in CI without bwrap)
```

Runs: `bf-build detect`, `bf-build plan`, `bf-index update`, `bf-sandbox` echo test,
and `bf-catalog search`. Emits a JSON summary on stdout.

### Local telemetry

Telemetry is **off by default** and **never transmitted automatically**. If you want to
record local event logs for your own analysis:

```sh
bf telemetry enable    # opt in
bf telemetry show      # print all records as NDJSON
bf telemetry disable   # opt out
bf telemetry clear     # delete all records
```

Records are written to `~/.butterfork/telemetry.jsonl`.

### Discover installed components

```sh
bf help-all            # walks PATH, finds every bf-* binary, prints its --help
```

---

## 11. Environment variables

Every component reads only the variables relevant to it. All are optional unless noted.

### Core / path overrides

| Variable | Default | Read by | Effect |
|----------|---------|---------|--------|
| `BF_HOME` | `~/.butterfork` | all | Override the Butterfork state directory |
| `BF_FORGE` | _(auto)_ | `bf-forge` | Force a specific forge backend binary (e.g. `bf-forge-github`) |
| `BF_BUILD` | _(auto)_ | `bf-build` | Force a specific build adapter binary (e.g. `bf-build-cargo`) |
| `BF_SANDBOX` | _(auto)_ | `bf-sandbox` | Set to `none` to disable sandboxing entirely |
| `BF_AGENT` | `bf-agent` | `bf` | Override the agent component binary |

### Fork / forge

| Variable | Default | Read by | Effect |
|----------|---------|---------|--------|
| `BF_NO_FORK` | unset | `bf-forge-github`, `bf-forge-gitlab`, `bf-bootstrap` | Set to `1` to skip forking (clone upstream directly) |
| `BF_GITHUB_USER` | _(from `gh api user`)_ | `bf-forge-github` | Override authenticated GitHub username; skips API round-trip |
| `BF_GITLAB_USER` | _(from `glab api user`)_ | `bf-forge-gitlab` | Override authenticated GitLab username; skips API round-trip |
| `BF_NO_AI_FOOTER` | unset | `bf-forge-github` | Set to `1` to suppress the AI-assistance footer on all PRs |

### Sandbox

| Variable | Default | Read by | Effect |
|----------|---------|---------|--------|
| `BF_SANDBOX_IMAGE` | `debian:bookworm-slim` | `bf-sandbox` | Container image used by Podman/Docker backends |
| `BF_SANDBOX` | _(auto-detect)_ | `bf-sandbox` | Set to `none` to force unsandboxed execution |

### Agent / AI

| Variable | Default | Read by | Effect |
|----------|---------|---------|--------|
| `ANTHROPIC_API_KEY` | — | `bf-agent` | **Required** for the Claude backend |
| `BF_AGENT_MODEL` | `claude-opus-4-7-20251101` | `bf-agent` | Claude model ID to use |
| `OLLAMA_HOST` | `http://localhost:11434` | `bf-agent-ollama` | Ollama server address |

### Catalog

| Variable | Default | Read by | Effect |
|----------|---------|---------|--------|
| `BF_CATALOG_INDEX_URL` | _(upstream release URL)_ | `bf-catalog` | Override the remote signed catalog index URL |
| `BF_NO_GITHUB_SEARCH` | unset | `bf-catalog` | Set to `1` to skip live GitHub Search results |

Every component binary also respects `--help` and `--version`.

---

## 12. Configuration

### PR policy

Per-project PR policy lives in `~/.butterfork/pr-policy/<slug>.toml`. The file is **flat
TOML** — no section headers. See [docs/pr-policy.md](docs/pr-policy.md) for the full
reference.

Quick example:

```toml
# ~/.butterfork/pr-policy/ripgrep.toml
require_dco = true
require_tests = true
require_format_check = false
ai_footer = "include"     # "include" | "exclude" | "ask"
max_diff_lines = 1000
block_new_dependencies = false
warn_whitespace_churn = true
```

`ai_footer = "exclude"` is appropriate for projects that prohibit AI-assistance
disclosure in their `CONTRIBUTING.md`. Butterfork inspects `CONTRIBUTING.md` at fork
time and surfaces a suggestion if it detects a relevant clause.

### Sandbox profiles

Custom sandbox profiles live in `~/.butterfork/sandbox-profiles/<name>.toml`. The
built-in profiles (`build`, `agent`, `run`) cover common cases; override them only if
you have specific network or filesystem needs for a particular project.

---

## 13. Security model

Butterfork downloads and executes arbitrary third-party code and lets an LLM modify it.
The mitigations:

- **Sandboxed builds.** `bf-build` runs inside a `bf-sandbox` profile with a
  per-project network allow list and a restricted filesystem view. The build cannot read
  `~/.ssh`, your shell history, or arbitrary home-directory paths.
- **No root.** Binaries install to `~/.butterfork/bin` under your user. There is no
  `sudo install` path.
- **Secrets in the OS keychain.** GitHub PAT and LLM API keys are stored via the
  `keyring` crate, never in `state.db` or config files.
- **Scoped agent tools.** The agent's `write_file` and `run_shell` tools operate only
  within the project repo. They cannot reach credentials, other projects' repos, or
  arbitrary paths.
- **Human-confirmed PRs.** Agent-written code is never pushed anywhere automatically.
  `bf submit` is a separate, explicit command.
- **Prompt injection defense.** Repo contents are treated as data, not instructions.
  Tool calls that touch credentials or remotes require explicit plan approval from the
  user.

---

## 14. Self-hosting

Butterfork is itself managed by Butterfork. The same fork → build → install → request →
submit workflow the project provides to its users applies to Butterfork's own development.

If Butterfork cannot fluidly improve itself, it is not ready to be trusted improving
anything else.

### Self-modification safety

- **Write, don't overwrite.** A new Butterfork build lands in a new generation directory.
  The symlink is swapped only after smoke tests pass and the daemon is confirmed idle.
- **Daemon handoff.** The new daemon starts alongside the old one; tasks drain from the
  old before it shuts down. If the new daemon fails its healthcheck within 30 seconds,
  the old one continues and the attempt is logged as a failed generation.
- **Agent quarantine.** When the agent modifies Butterfork's own source, it runs in a
  sandbox with no access to the keyring and network only to the forge needed for the PR.
- **Rescue mode.** `bf rescue activate ripgrep <id>` activates any previous generation.
  `bf-bootstrap` can rebuild from upstream as a floor case.
- **Denylist.** `bf-sandbox/*`, `bf-agent/safety/*`, keyring integration, and release
  tooling are in an agent-edit denylist. Overriding it requires an explicit user flag
  and a written justification captured in the PR.

---

## 15. Contributing

DCO sign-off is required on every commit (`git commit -s`). No CLA.

```sh
# Run the full local check suite before opening a PR
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

A few things to know before you start:

- **Adding a new component.** Run `cargo new --name bf-<name> bf-<name>`, add it to
  `[workspace] members` in the root `Cargo.toml`, implement the CLI contract from
  section 6 of [butterfork-design.md](butterfork-design.md), make stdout NDJSON and
  stderr human-readable, and add `bf-common` as a dependency.

- **Adding a build adapter.** Write a binary `bf-build-<name>` that responds to
  `detect`, `plan`, and `run`. No Butterfork SDK to learn. See `bf-build-cargo/src/main.rs`
  as a reference.

- **Adding a forge backend.** Write a binary `bf-forge-<name>` that responds to
  `fork`, `clone`, `issue open`, `pr open`, `pr status`, and `pr watch`. See
  `bf-forge-github/src/main.rs`.

- **Shell-script parity.** If you change a high-level flow in `bf/src/main.rs`, update
  the corresponding script in `scripts/` to match. CI enforces this.

- **RFC process.** Non-trivial design changes go through an RFC (markdown file in
  `rfcs/`, open for community comment before merging).

For bugs, feature requests, and questions, open an issue on GitHub.

---

## 16. License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
