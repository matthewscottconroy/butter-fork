# Butterfork: Design Document

**Status:** Draft v0.1
**Language:** Rust
**One-line pitch:** Collapse the gap between *using* open source software and *contributing* to it into a single integrated workflow backed by an LLM coding agent.

---

## 1. Overview

Today, modifying an OSS tool to fit your needs requires a mountain of orthogonal steps: finding the repo, forking it, cloning, installing a build toolchain, building, installing (probably uninstalling the distro version first), and later remembering to sync, branch, and PR. Each step is well-understood in isolation; the friction is in the seams.

Butterfork treats that whole lifecycle — discovery, installation, local modification, and upstream contribution — as a single, resumable workflow. The target user is a developer (or power user who programs occasionally) who already uses a lot of OSS CLI tools and desktop apps and wishes a papercut or missing feature in one of them were easier to fix and share back.

## 2. Goals and Non-Goals

### Goals

- Make *fork, build, install* a one-click operation for common build systems.
- Keep the user's local installation synced to their fork, not a stale binary.
- Turn a natural-language bug report or feature request into: an issue in the user's fork, a feature branch, a code change, a fresh build, a reinstall.
- Let the user iterate with the agent while actually using the modified app.
- Produce upstream-quality PRs: matching code style, tests, sensible commit messages, adherence to `CONTRIBUTING.md`.
- Clear visibility and overrides at every step. This is an assistant, not a black box.

### Non-Goals (v1)

- Building a full IDE. Butterfork embeds an editor view for reviewing diffs; deep editing happens in the user's preferred editor.
- Cross-platform packaging of GUI apps with exotic runtime requirements (kernel modules, proprietary drivers). Start with CLI tools and standard GUI apps.
- Supporting every build system. Start with a defined set and extend via plugins.
- Replacing package managers for users who don't want local modifications. Butterfork coexists with apt/brew/etc.; it does not try to own `/usr/bin`.

## 3. Design Principles

Butterfork's architecture is constrained by three commitments, roughly in priority order. These are not aesthetic — they're what make the project contributable-to, replaceable-in-parts, and worth trusting as a long-running piece of a developer's toolchain.

### 3.1 Unix Composition

Each capability — forge interaction, build detection, sandboxing, install management, codebase indexing, agent orchestration, catalog lookup — is an independent executable with a small CLI contract, line-oriented stdout, and conventional exit codes. The top-level `bf` command composes these pieces; it does not contain them.

A user who wants only Butterfork's sandboxed-build capability can use `bf-sandbox` and `bf-build` from their own shell scripts without ever launching the daemon, the UI, or the agent. A contributor who wants to *replace* a component writes a new binary conforming to the same CLI contract and puts it on `PATH` — no bespoke plugin ABI, no SDK to learn. This is the `git` model: `git-lfs`, `git-extras`, `git-flow` extend `git` without `git` knowing they exist.

Where a component would duplicate an existing Unix tool, it wraps that tool thinly rather than reimplementing it. `bf-forge` wraps `gh`/`glab`. `bf-sandbox` wraps `bwrap`/`podman`. `bf-build-cargo` shells out to `cargo`. We spend our implementation budget on the seams, not on re-doing work the ecosystem already does well.

### 3.2 Inspectable State

Durable state lives in plain files under `~/.butterfork/` with a documented layout. The generation database is SQLite, readable with the `sqlite3` CLI. Index data is on-disk and queryable by any tool. Repos are ordinary git checkouts. Secrets live in the OS keychain, never in the state directory. A user with a Unix shell should be able to inspect, back up, and reason about everything Butterfork has done on their machine without running Butterfork at all. A daemon that hides state behind an RPC is a daemon that grows into a kingdom — we choose files and `sqlite3 state.db` instead.

### 3.3 OSS Posture

Butterfork exists to make open source easier to improve, so the project holds itself to the standards it expects of its catalog. License: Apache-2.0 OR MIT, the most compatible permissive pair, so downstream OSS projects can absorb any piece of Butterfork without licensing friction. Governance: public roadmap, public RFC process for non-trivial changes, public decision records. Contribution: each component is small enough to read in an afternoon, with an independent test suite and an independent release cadence. When a component matures — `bf-sandbox` as a generic sandbox for anyone doing local builds, `bf-install` as a standalone Nix-lite — it should be able to spin out and live on its own terms under its own maintainers. Nothing in Butterfork's architecture should create lock-in, either for users or for the people maintaining it.

## 4. Primary User Flow

### 4.1 Contribution Flow

The canonical flow mirrors the original concept:

1. User opens Butterfork and browses a curated + searchable catalog.
2. User selects, say, `ripgrep`. Butterfork asks for confirmation (fork target, install location, consent to run build scripts).
3. Butterfork forks `BurntSushi/ripgrep` on the user's GitHub, clones the fork into `~/.butterfork/repos/ripgrep`, detects the build system (Cargo), builds it sandboxed, and places the binary under `~/.butterfork/bin` (on `PATH`).
4. User uses `rg` for a week. Runs into a papercut: wants a flag that excludes hidden files by default per-directory.
5. From Butterfork's tray, the app UI, or the CLI (`bf request ripgrep "…"`), the user describes the change.
6. The agent opens a plan, creates an issue in the fork, creates a `bf/exclude-hidden-default` branch, drafts a patch, runs tests, rebuilds, reinstalls. The user's `rg` now carries the flag.
7. User iterates by re-prompting. The agent amends or stacks commits; rebuilds.
8. When happy, the user clicks *Submit upstream*. Butterfork runs pre-PR checks (lint, test, format, `CONTRIBUTING` compliance, CLA detection), drafts a PR body from the issue and commit messages, and opens the PR against upstream.
9. Butterfork tracks the PR. Reviewer feedback comes back into the same agent conversation.

### 4.2 Greenfield Flow

The same workflow covers starting a new project from an idea:

1. User types `bf new` (or uses the UI) and describes what they want to build: *"A Rust CLI that takes a URL and prints its response headers."*
2. `bf-scaffold` picks a template, optionally consults `bf-agent`, and produces a directory containing working hello-world code, tests, a README, a license, and a build manifest — or, if the user wants to think before writing any code, a repo containing only a design doc.
3. `bf-forge repo create --push` creates a new repo on the user's GitHub and pushes the scaffold.
4. `bf-build` builds it and `bf-install` installs it. The new command is on the user's `PATH` within minutes of the first prompt.
5. User uses it, hits the inevitable rough edges, and reprompts the agent exactly as in the contribution flow. The only difference: "upstream" is the user's own repo, so there is no one else to PR to. Yet.
6. If the project matures and the user chooses to publish it to the catalog, it becomes forkable by other Butterfork users. At that point it enters the contribution flow *from the other side* — the user is now the maintainer whose PRs other people send.

This closes the loop. Butterfork is not just a contributor's tool; it is a pipeline from *idea* to *published OSS project*, with every step after scaffolding reusing code that already exists for the contribution flow.

## 5. High-Level Architecture

Butterfork is a family of single-purpose binaries plus a thin orchestrator. There is no monolithic core. The daemon that existed in earlier drafts is gone in its old form — what remains is an *optional* supervisor that owns no capability, only continuity.

### 5.1 The `bf-*` component binaries

Each of the following is a standalone executable, shipped as its own crate with its own README and release cadence. Each has a stable documented CLI, emits NDJSON on stdout for machine consumers, and puts human-readable progress on stderr. Each is useful by itself, independent of the rest.

- `bf-catalog` — project discovery. Searches the curated index plus GitHub search plus user-added URLs.
- `bf-forge` — forge operations: fork, clone, issue, PR, PR-status. Forge backends (`bf-forge-github`, `bf-forge-gitlab`, `bf-forge-gitea`) are themselves PATH-discovered binaries, git-subcommand style.
- `bf-build` — detect a project's build system, plan a build, run it. Build adapters (`bf-build-cargo`, `bf-build-cmake`, `bf-build-meson`, …) are PATH-discovered binaries.
- `bf-sandbox` — run a command under a named sandbox profile. Wraps `bubblewrap` on Linux, container runtimes elsewhere.
- `bf-install` — manage install generations: add, activate, rollback, garbage-collect. The Nix-profile-inspired piece, cleanly separated.
- `bf-index` — maintain an incremental codebase index (tree-sitter + optional embeddings). Query by symbol, pattern, or semantic lookup.
- `bf-agent` — the LLM tool loop. Reads a prompt and an external-tool manifest; streams NDJSON events. Model backend (Claude, OpenAI, Ollama, …) is itself a choice of binary.
- `bf-scaffold` — turn an idea into a buildable project seed: hello-world, POC, or design-doc-only. The entry point for greenfield projects; composes `bf-agent` for idea→code transformation.

Any component can be replaced by shadowing its binary on PATH or setting the corresponding `BF_FORGE`, `BF_BUILD`, `BF_SANDBOX`, `BF_AGENT`, … env var. The project does not privilege its own implementations.

### 5.2 The `bf` orchestrator

`bf` is the top-level command the user types: `bf install ripgrep`, `bf request ripgrep "…"`, `bf submit`. It is small. It contains no capability that is not already in a component binary — by construction, because every high-level flow in `bf` must have a documented shell-script equivalent in the repo, and the repo's tests verify that the script and `bf` produce identical output. This invariant is what keeps `bf` from accreting into a new monolith by stealth.

### 5.3 The `bf-daemon` (optional)

A small supervisor for work that needs to outlive a shell invocation: multi-hour builds, long agent runs, PR watchers that poll over days. It shells out to the same component binaries a user would invoke by hand; it owns no private capability. Users who don't want a daemon can script their workflows directly. The daemon is a convenience, not a chokepoint, and Butterfork without a daemon is still Butterfork.

### 5.4 The UI

`butterfork-ui` is a Tauri desktop app — just another client of the component binaries and, when present, the daemon. Every action the UI offers has a CLI equivalent. Every screen in the UI can be reproduced by piping a few component binaries into `jq`.

### 5.5 IPC and interchange

Between components: stdin/stdout/stderr with NDJSON payloads. Exit codes follow `sysexits.h` conventions. Events on long-running commands stream as one JSON object per line, so shell consumers can `| jq` them live.

Between clients and the daemon: a Unix-domain socket at `$XDG_RUNTIME_DIR/butterfork.sock`, speaking a protocol that is a thin extension of the component CLI schemas rather than a privileged superset.

On-disk layout:

```
~/.butterfork/
├── state.db           # SQLite, documented schema, read with sqlite3
├── repos/<slug>/      # ordinary git checkouts, forkable by hand
├── generations/<slug>/<id>/{bin,lib,share}
├── bin/               # symlinks to the active generation per project
└── logs/              # plain text, one file per operation
```

A `bf doctor` subcommand exists, but it's a convenience wrapper over commands you could run yourself.

### 5.6 Packaging

Two distribution modes, user's choice:

- **Separate binaries.** `cargo install bf-forge bf-build bf-install …`. Each on PATH, each updatable independently. This is the default for contributors and for people who want to mix and match.
- **Fat binary.** A single `bf` executable that dispatches by `argv[0]` (busybox style) and exposes the component binaries as symlinks. Same code, same CLI contract, one file to install. This is what most end users will consume. Precedent: `busybox`, and — at the subcommand level — `cargo`.

Either way, the CLI contract is identical, and a user can freely mix a fat `bf` with a PATH-shadowed custom component.

## 6. Component Breakdown

Each subsection describes the CLI contract for one component. Implementation detail is intentionally thin — the interface is the API.

### 6.1 `bf-catalog`

```
bf-catalog search <query>         # NDJSON entries
bf-catalog show <slug>            # one detailed entry
bf-catalog add <url>              # user-added project
bf-catalog update                 # refresh local cache
```

Sources: a curated index (signed JSON file distributed via the project's own releases), GitHub Search API, user-added URLs. Community-contribution signals — PR response latency, `CONTRIBUTING.md` presence, code of conduct, license category — are part of the entry schema. Surfaced by the UI, but equally available to anyone piping `bf-catalog search` into `jq`.

### 6.2 `bf-forge`

```
bf-forge fork <upstream_url>
bf-forge clone <fork_url> <dest>
bf-forge issue open --repo <slug> --title … --body …
bf-forge pr open --repo <slug> --head … --base … --title … --body …
bf-forge pr status <url>
bf-forge pr watch <url>           # streams events
```

Backend selection by URL sniffing (`github.com` → `bf-forge-github`). Backends are PATH-discovered binaries implementing the same CLI surface. v1 backends are thin wrappers over `gh` and `glab`; rewriting a backend to direct API calls is a local decision inside that backend, invisible to `bf-forge`.

### 6.3 `bf-build`

```
bf-build detect <repo>            # {adapter, confidence, hints}
bf-build plan <repo>              # concrete BuildPlan JSON
bf-build run <repo> [--plan plan.json]
```

Build adapters are `bf-build-<name>` on PATH. A new adapter is just an executable that responds to `detect`, `plan`, and `run`. Shipped: Cargo, CMake, Meson, Autotools, Make, Go modules, npm/pnpm, Python, Gradle. A contributor adding Zig build support writes `bf-build-zig` and a README — no Butterfork API to learn, no SDK, no review process owned by the core team.

### 6.4 `bf-sandbox`

```
bf-sandbox run --profile <name>
              [--allow-net host[,host…]]
              [--bind path[:mode]]
              -- <cmd> [args…]
```

Named profiles (`build`, `agent`, `run`) ship with sensible defaults and are user-overridable via files in `~/.butterfork/sandbox-profiles/`. On Linux, wraps `bubblewrap`; elsewhere, wraps a container runtime. The binary is standalone useful: anyone who builds untrusted code locally can use it without touching the rest of Butterfork.

### 6.5 `bf-install`

```
bf-install add <project> <artifact_manifest.json>
bf-install activate <project> <generation_id>
bf-install list [<project>]
bf-install rollback <project>
bf-install gc
```

Generations are directories under `~/.butterfork/generations/<project>/<id>/` with a `/usr/local`-mirroring layout. The active one is pointed at by a symlink, swapped atomically. Standalone usefulness: developers who build local tools constantly can use `bf-install` as a generational install manager without any other Butterfork component.

### 6.6 `bf-index`

```
bf-index update <repo>
bf-index query <repo> --symbol <name>
bf-index query <repo> --grep <pattern>
bf-index query <repo> --semantic <text>
```

Index data lives under `.bf/index/` inside the repo (gitignored). Tree-sitter drives structural queries; a local embedding store drives semantic queries. A `--no-embeddings` mode exists for users who don't want any ML dependency — the index still works, just without semantic search.

### 6.7 `bf-agent`

```
bf-agent run --repo <path>
             --prompt <string>
             --tools <manifest.json>   # NDJSON event stream
```

The tool manifest declares external commands the agent may invoke, each with a schema. In normal Butterfork use, `bf` generates the manifest and lists `bf-forge`, `bf-build`, `bf-index`, and scoped file-edit tools. The event protocol (plan, tool-call, tool-result, message, done) is documented and test-covered. Swapping the agent means replacing the `bf-agent` binary with anything else speaking that protocol — `bf-agent-claude`, `bf-agent-ollama`, `bf-agent-codex`. The model vendor is not a Butterfork concern.

### 6.8 `bf-scaffold`

```
bf-scaffold new <path>
              --description <text>        # natural-language idea
              [--spec <design_doc.md>]    # existing design as seed
              [--template <name>]         # named template
              [--mode hello-world|poc|design-doc]
              [--language <lang>]
bf-scaffold template list
bf-scaffold template add <git_url>
bf-scaffold template show <name>
```

Turns an idea into a directory that the rest of Butterfork can immediately consume. Three output modes:

- **hello-world** — smallest thing that runs. Working program, tests, build system, README, CI workflow, license. Something the user can install and see print *hello* within minutes of describing it.
- **poc** — one level up: enough structure to demonstrate the core idea, with explicit `TODO:` markers the agent can later be asked to fill in. Suitable when the idea is clear enough to sketch but not trivial enough for hello-world.
- **design-doc** — a repo containing `docs/DESIGN.md` and a minimal build manifest. No executable code yet. Useful for thinking through an idea before writing any of it; the agent can later be asked to produce the initial code *from the design doc* — the same reprompting workflow as any other iteration. (This very conversation is an example of that mode in use.)

Composition, not reimplementation: `bf-scaffold` is not a second agent. When idea-to-code transformation is needed, it invokes `bf-agent` with a scaffolding-specific prompt and a tool manifest scoped to the scaffold directory. The model choice is `bf-agent`'s problem, not `bf-scaffold`'s. For mechanical templating (license headers, CI workflow files, `.gitignore`), `bf-scaffold` uses plain template rendering without the agent — deterministic, fast, free.

Templates are ordinary git repos containing a `scaffold.toml` manifest and a `template/` tree. Precedent: `cargo-generate`, `cookiecutter`. New templates are added with `bf-scaffold template add <url>`; there is no central registry and no gatekeeping. A curated subset is surfaced through `bf-catalog` under a "templates" section.

After the scaffold exists, the greenfield flow hands off to the existing pipeline — `bf-forge repo create --push`, `bf-install register`, `bf-build run` — with no further special casing. **This is the property that matters: there is one Butterfork workflow, and it covers both contribution to existing OSS and creation of new OSS.** The scaffold is where the pipeline begins for greenfield projects; every later step is shared code.

### 6.9 `bf` (orchestrator)

Implements the user flows from §4 by scripting the component binaries. Contains no capability not already in a component; every high-level flow has a shell-script equivalent in `scripts/` in the repo, and CI verifies that `bf <flow>` and `scripts/<flow>.sh` produce identical output. This test is the load-bearing invariant that prevents `bf` from accreting into a new monolith.

### 6.10 PR etiquette gate

PR quality checks — tests pass, lint, format, `CONTRIBUTING.md` surfaced, CLA/DCO handled, anti-spam heuristics (giant diff, unrelated changes, new dependencies, whitespace churn) — live in `bf-forge pr open`'s pre-flight phase. The checks are declared in a YAML file per project, overridable per user. Because the gate is a distinct code path in one component, contributors who want to strengthen it can do so without touching anything else.

### 6.11 `bf-daemon`

Runs the same component binaries, but as long-lived supervised tasks with persistence. Watches PRs, resumes interrupted builds, pipes review comments back into `bf-agent`. It has no schema privileges the component binaries lack — it reads and writes the same SQLite DB the CLI tools do. Entirely optional.

## 7. Data Model

State lives in SQLite at `~/.butterfork/state.db`. The schema is stable, versioned, and documented in `docs/schema.md` in the repo — readable and queryable with the `sqlite3` CLI. Butterfork does not hide its state from its users. Core tables:

- `projects(id, upstream_url, fork_url, repo_path, default_build_adapter, install_generation, …)`
- `generations(id, project_id, ref, built_at, artifact_paths, active)`
- `intents(id, project_id, branch, issue_url, status, title, body)`
- `agent_runs(id, intent_id, started_at, finished_at, tokens_in, tokens_out, cost_cents, result)`
- `prs(id, intent_id, upstream_pr_url, state, last_polled_at)`

All long-running operations are resumable: the daemon writes checkpoints (currently-building, currently-running-agent) so a restart continues rather than restarts.

## 8. Security Considerations

Butterfork's central hazard is that it downloads and executes arbitrary third-party code and lets an LLM modify it. Mitigations:

- Builds run sandboxed with per-project network allow lists. Post-install binaries live under `$HOME` and run with the user's privileges. Butterfork does *not* escalate to root; there is no sudo install path in v1.
- Secrets (GitHub PAT, LLM API keys) live in the OS keychain via the `keyring` crate, never in the DB or on-disk config.
- The agent's `edit_file` and `run_*` tools operate only within the project repo and its sandbox. They cannot read `~/.ssh`, shell history, or arbitrary paths.
- Agent-written code is never pushed anywhere automatic beyond the user's own fork. PRs are explicit, human-confirmed actions.
- Prompt-injection risk: repo contents may contain adversarial instructions ("ignore prior and do X"). The agent treats repo contents as data, not instructions, and tool calls that touch credentials or remotes require explicit plan approval.

## 9. Self-Hosting

Butterfork is itself open source, hosted on GitHub, built with Cargo. The same workflow described in §4 must apply to Butterfork operating on Butterfork: fork, build, install, modify via the agent, PR upstream. This is not vanity — it is the test that matters. If Butterfork cannot fluidly improve itself, it is not ready to be trusted improving anything else; and if the team can fluidly improve it, every papercut gets fixed faster than an external user would have filed it.

### 9.1 Bootstrap

Chicken and egg: installing Butterfork with Butterfork requires Butterfork. Resolved by shipping a minimal static bootstrap binary, `bf-bootstrap`, from the project's GitHub releases. It does exactly one thing: runs the fork → clone → build → install flow against the canonical `butterfork` repo, then exits. After that, `bf` is on `PATH` and every future Butterfork update flows through Butterfork.

The bootstrap binary has no plugins, no agent, no UI, no daemon — just `bf-forge/github` and `bf-build/cargo` statically linked. Keeping it tiny makes it auditable and suitable for a one-command install.

### 9.2 Self-Modification Safety

The invariant: **the currently running Butterfork must never depend on an unproven next generation of itself.** Concretely:

- **Write, don't overwrite.** A new Butterfork build lands in a new generation directory. The `bf` symlink is swapped atomically only after post-build smoke tests pass *and* the daemon is confirmed idle. If smoke tests fail, the new generation is marked failed and the symlink is never touched.
- **Daemon supervision.** The running daemon is not killed mid-task. A supervisor starts the new daemon alongside, waits for healthcheck, drains tasks from the old one, then shuts the old one down. If the new daemon fails healthcheck within 30 seconds, the old one continues serving and the attempt logs as a failed generation.
- **Agent quarantine.** When the agent modifies Butterfork's own code, its tool loop runs inside the *old* generation, in a sandbox with no access to the user's keyring, no access to other projects' repos, and network only to the forge it needs to open the issue. The new build consumes the resulting patch, but the agent that wrote it never holds credentials for upstream Butterfork.
- **Rescue mode.** A `bf rescue` subcommand ships in the bootstrap and in every generation. It ignores the current symlink, lists generations, and activates any of them. Floor case: if every generation is broken, re-running `bf-bootstrap` rebuilds from upstream.

### 9.3 Dogfooding Properties

Running Butterfork-on-Butterfork gives the project three things for free:

- **First-class test target.** Every change the team ships to Butterfork is exercised through Butterfork's own workflow. A regression in the Cargo adapter surfaces the next time anyone pulls an update.
- **Real PRs from the team to itself.** The PR-etiquette gate from §6.10 gets exercised against the team's own reviewers — an early signal on whether the rules are too loose or annoyingly tight.
- **Agent-written agent code, bounded.** The agent can be asked to improve its own prompts, tool descriptions, or error handling. Humans review every such diff; self-modification never auto-merges.

### 9.4 Special Handling

Butterfork's own repo carries a project-level flag `self_hosting: true` that triggers:

- Stricter pre-PR gates — full test matrix across adapters, not just the changed one.
- Two-person review on merges; the agent's authorship counts as one, and a human is required as the second.
- An agent-edit denylist for paths where a mistake would undermine the safety story for every other project: `bf-sandbox/*`, `bf-agent/safety/*`, anything touching `keyring`, and release tooling. Overriding the denylist requires an explicit user flag and a written justification captured in the PR.

Prompt-injection concern in this setting: an attacker could plant instructions in Butterfork's own source trying to subvert the agent. Mitigation is the same data-not-instructions discipline from §8, plus the denylist.

## 10. Licensing, OSS Posture, and Etiquette

### 10.1 Project license

Butterfork is distributed under **Apache-2.0 OR MIT** — the dual-permissive pattern used by the bulk of the Rust ecosystem. Rationale: any downstream OSS project in Butterfork's catalog should be able to absorb any piece of Butterfork's code without licensing friction, regardless of whether they've standardized on Apache, MIT, BSD, or something else compatible. The dual grant is applied consistently across every crate and every binary. No CLA for contributors; the DCO is sufficient and is enforced on every PR.

### 10.2 Governance

Public roadmap in the repo. Non-trivial design changes go through a lightweight RFC process (markdown files in `rfcs/`), open for community comment. Decision records are kept — when the answer is "no", the reasoning is captured so contributors are not surprised later. Maintainership is additive: anyone who has landed sustained quality contributions to a component is offered commit access to that component. Ownership is per-component, not project-wide, which matches the architectural separation.

### 10.3 Component maturity and spinoff

The architecture is deliberately designed so components can leave. If `bf-sandbox` becomes the best user-space sandbox wrapper in the Rust ecosystem, it should graduate to its own repo, its own release cadence, and its own maintainers — with Butterfork consuming it as a regular dependency. Same for `bf-install`, which is one careful refactor away from being a standalone Nix-profile-style install manager usable outside Butterfork entirely. The project's success is not measured by what stays inside it.

### 10.4 Pipeline for new OSS projects

Because `bf-scaffold` produces projects that flow through the same pipeline as forked OSS, Butterfork becomes a natural on-ramp for *new* open source — not just a conduit for contributing to existing OSS. A developer describes an idea, scaffolds and iterates on it locally, and, when it has earned it, flips a switch to publish it in the catalog. Other Butterfork users can then discover, fork, install, and contribute using the identical workflow. The mechanics that bring a PR back to `ripgrep` will, without special cases, bring PRs to a project that didn't exist a month ago. This is how Butterfork earns its place in the open source world rather than merely consuming from it.

### 10.5 Etiquette toward catalog projects

Butterfork performs SPDX license detection at fork time and surfaces the result to the user. Users are warned when modifying copyleft-licensed code about redistribution implications for install-for-others use cases. For PRs opened by Butterfork on the user's behalf, the default disclosure policy includes a footer noting the change was drafted with AI assistance; this is user-configurable (some projects require disclosure, others prohibit it — `CONTRIBUTING.md` is inspected for hints) and never silently omitted. The PR-etiquette gate in §6.10 is the enforcement point.

### 10.6 Non-exploitation commitment

Butterfork will never gate existing capabilities behind payment, never inject telemetry without an explicit opt-in, and never take positions in which maintainer goodwill toward Butterfork becomes a commercial asset of Butterfork's. If Butterfork ever needs to sustain itself financially, the path is a hosted companion service (catalog hosting, PR-watching for teams, managed LLM credits) — never a rug pull on the tool the community is using.

## 11. Open Questions

- **Quality floor for PRs.** What automated checks suffice to prevent Butterfork from degrading the signal-to-noise ratio across OSS? A Butterfork-internal reviewer panel for first-time contributions to a given project is a candidate.
- **Dependency hell.** Some projects need system libraries Butterfork cannot guarantee (e.g. `libgtk-4-dev`). Option A: curated per-distro install commands and ask the user. Option B: container with the deps pre-staged. Leaning B, but the perf cost for everyday use is real.
- **macOS / Windows parity.** A Linux-first MVP is simpler; macOS and Windows add signing, notarization, and install-location headaches. Phase in.
- **Offline / air-gapped.** With a local LLM backend, can Butterfork run fully offline once a project is cloned? Goal: yes. Caveat: agent quality may degrade sharply — flag honestly.
- **Process-per-call overhead.** The Unix-composition model costs a process fork per component invocation. Tolerable for user-initiated flows, potentially painful for the agent loop (which may invoke tools hundreds of times per task). Mitigation candidates: a long-running `bf-agent` worker, NDJSON-over-socket fast paths between hot-pair components, the fat-binary packaging as the default. Must be measured, not guessed.
- **Drawing the `bf` orchestrator line.** What belongs in `bf` versus a user's shell script? The shell-script-equivalence test constrains this, but the team still has to choose which flows are first-class. Risk: `bf` accretes into the monolith we're trying to avoid.
- **Discovery of PATH components.** How does a user know what `bf-*` binaries are installed and what they do? `bf help-all` walking PATH and invoking each `--help` is the sketch; needs design.
- **Self-hosting gate to public release.** How long must Butterfork be the team's daily driver, with zero rescue-mode invocations, before v1 is shippable? Candidate answer: N weeks across M OS/arch combinations.
- **Component versioning.** If `bf-build-cargo` and `bf-forge` ship on independent cadences, how does `bf` express compatibility ranges? Candidate: each component declares a CLI-schema version, `bf doctor` checks consistency.
- **Scaffold vs. agent boundary.** How much of scaffolding should be mechanical templating and how much should hand off to `bf-agent`? Mechanical is fast, deterministic, and free; the agent is flexible but slow and uses tokens. Default split TBD; user-tunable.
- **Template curation.** The template ecosystem is open — anyone can publish a git repo as a template. Who curates the "official" subset surfaced in `bf-catalog`'s templates section? Same answer as catalog curation, probably, but worth stating.
- **Quality floor for greenfield publication.** When a user flips a scaffolded project into the public catalog, what's the minimum bar? A README and license is probably enough; heavier gates risk discouraging publication at all.

## 12. Phased Roadmap

**Phase 0 — Component spike (3–4 weeks).** Ship `bf-build-cargo`, `bf-install`, and a minimal `bf` orchestrator *as separate binaries from day one*. Also ship `bf-forge-github` as a thin `gh` wrapper. Prove the fork → build → install → rollback loop end-to-end on `ripgrep`, `fd`, `bat`. No daemon, no agent, no UI. Test: every high-level flow has a shell-script equivalent and CI verifies parity.

**Phase 1 — Agent and self-hosting.** Add `bf-sandbox`, `bf-index`, `bf-agent` (Claude backend). `bf-bootstrap` ships; Butterfork installs Butterfork; the core team switches to the Butterfork-installed `bf` as daily driver. End-to-end test: a real PR opened against a test repo, and the first Butterfork → Butterfork PR. PR-etiquette gate lands in `bf-forge`. A minimal `bf-scaffold` in design-doc-only mode lands here too — the smallest possible scaffolder is a template that seeds a repo with `docs/DESIGN.md`, enough to let the agent operate on a brand-new project artifact.

**Phase 2 — Breadth.** `bf-build-cmake`, `bf-build-meson`, `bf-build-npm`. `bf-forge-gitlab`. Fat-binary packaging as the default end-user artifact. Full `bf-scaffold` with hello-world and POC modes, plus the first round of community-contributable templates. Alternative agent backends (`bf-agent-ollama`) land as proof of swappability. Beta-ready for a small group of friendly developers.

**Phase 3 — Polish.** `butterfork-ui` Tauri app (catalog, diff viewer, PR tracking). macOS and Windows support. `bf-daemon` for background PR watching and long-run continuity.

**Phase 4 — Public launch.** Curated catalog, quality guardrails, community reviewer mode, opt-in telemetry on the *good to contribute to* signal. First component spinoff candidate identified and begun.

---

*Next step:* cut Phase 0 scope down to a single command (`bf install ripgrep`) that is literally a shell script over `bf-forge-github`, `bf-build-cargo`, and `bf-install`. Prove the install → rebuild-from-fork → rollback loop works as a pipeline of separate processes before any of it is wrapped in a Rust orchestrator. Add `bf-bootstrap` and a Butterfork-on-Butterfork generation immediately after. The hardest risk in this design is not the LLM — it is making the build/install/generation plumbing boring and reliable enough, through clean seams, that the team trusts it with its own dev loop and outside contributors can meaningfully own any single component.
