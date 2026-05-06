# Using Butterfork with Qiskit

This guide covers the full Butterfork workflow for
[Qiskit](https://github.com/Qiskit/qiskit) — IBM's open-source quantum computing SDK.
Qiskit is a deliberately different kind of target than a compiled CLI tool like Godot:
it is a **Python library with Rust extensions**, installed into a virtual environment,
consumed via `import qiskit` rather than a standalone executable. Working through it
teaches the parts of Butterfork's adapter system that compiled-binary examples skip
entirely: how to model a venv-based install as a generation, how to wire a custom build
back into your own downstream Python projects, and how to handle a CLA-required upstream.

This guide also assumes you have your own quantum computing code in
`~/Development/Quantum-Computing/` that imports Qiskit — the workflow connects your
modified Qiskit build directly to those scripts.

---

## Table of Contents

1. [What you'll end up with](#1-what-youll-end-up-with)
2. [System prerequisites](#2-system-prerequisites)
3. [Writing a `bf-build-python` adapter](#3-writing-a-bf-build-python-adapter)
4. [Writing a Qiskit sandbox profile](#4-writing-a-qiskit-sandbox-profile)
5. [Adding Qiskit to the catalog](#5-adding-qiskit-to-the-catalog)
6. [Installing Qiskit through Butterfork](#6-installing-qiskit-through-butterfork)
7. [Using your custom Qiskit in your own projects](#7-using-your-custom-qiskit-in-your-own-projects)
8. [Making a change request](#8-making-a-change-request)
9. [Iterating with the agent](#9-iterating-with-the-agent)
10. [Submitting upstream](#10-submitting-upstream)
11. [Qiskit PR etiquette checklist](#11-qiskit-pr-etiquette-checklist)
12. [Rollback](#12-rollback)
13. [Keeping your fork in sync](#13-keeping-your-fork-in-sync)
14. [Common pitfalls](#14-common-pitfalls)

---

## 1. What you'll end up with

After following this guide:

- Your GitHub account has a fork of `Qiskit/qiskit`.
- The fork is cloned to `~/.butterfork/repos/qiskit`.
- A virtual environment containing your custom Qiskit build lives in a generation
  directory under `~/.butterfork/generations/qiskit/<id>/`.
- A `qiskit-python` wrapper script on your PATH activates the generation's venv
  and drops you into a Python interpreter with your custom Qiskit pre-imported.
- Your projects in `~/Development/Quantum-Computing/` can point at the generation venv
  to pick up your custom build with one environment variable change.
- You know how to ask the agent to fix a bug or add a feature, rebuild, validate with
  pytest, and open a PR against upstream including the IBM CLA requirement.

---

## 2. System prerequisites

### Python and Rust

Qiskit requires Python 3.8+ and uses [maturin](https://www.maturin.rs/) to compile its
Rust extensions (the statevector simulator and circuit optimizer are in Rust via PyO3).
Both the Python and Rust toolchains must be available.

```sh
# Verify Python
python3 --version      # must be >= 3.8
python3 -m pip --version

# Verify Rust (needed for maturin's Rust extensions)
rustc --version
cargo --version

# Install maturin (the build tool for Rust-backed Python extensions)
pip3 install --user maturin

# Install tox (used by Qiskit's test runner)
pip3 install --user tox

# Verify gh is authenticated
gh auth status
```

### IBM CLA

Qiskit is maintained by IBM and requires a **Contributor License Agreement** rather than
the DCO that most projects use. Before your PR can be merged, you must sign the IBM CLA.
Do this once before you start:

1. Visit [cla-assistant.io/Qiskit/qiskit](https://cla-assistant.io/Qiskit/qiskit).
2. Sign in with your GitHub account.
3. Accept the agreement.

The CLA bot checks automatically when a PR is opened. If you haven't signed it, the
bot will add a blocking comment and a `cla: not signed` label. Butterfork's pre-flight
gate detects this condition and warns you before `bf submit` runs.

### Linting tools

```sh
pip3 install --user black ruff pylint
```

Qiskit enforces:
- **black** (formatter) — any unformatted code is a CI failure.
- **ruff** (fast linter) — import ordering, style, and common bug patterns.
- **pylint** (extended linter) — run on modified files only for contributor PRs.

---

## 3. Writing a `bf-build-python` adapter

Butterfork has no built-in Python adapter. Qiskit uses `pyproject.toml` (PEP 517/518),
which `pip` can build directly — but the Rust extension layer means a plain `pip install`
triggers a full `maturin` compilation. The adapter below handles the full lifecycle:
creates an isolated virtual environment, installs the package in editable mode, captures
entry-point scripts as artifacts, and writes the artifact manifest.

### 3.1 Scaffold the crate

```sh
cargo new --name bf-build-python bf-build-python
```

Add to the workspace root `Cargo.toml`:

```toml
members = [
    # ... existing ...
    "bf-build-python",
]
```

`bf-build-python/Cargo.toml`:

```toml
[package]
name = "bf-build-python"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "bf-build-python"
path = "src/main.rs"

[lib]
path = "src/main.rs"

[dependencies]
anyhow.workspace = true
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
bf-common.workspace = true
```

### 3.2 Implement `bf-build-python/src/main.rs`

```rust
use anyhow::{Context, Result};
use bf_common::{emit, Artifact, ArtifactManifest, BuildDetection, BuildPlan, BuildStep, Event};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build-python",
    about = "Python build adapter for bf-build",
    long_about = "Detects Python projects by pyproject.toml, setup.cfg, or setup.py.\n\
                  Creates an isolated virtualenv and installs the package in editable mode.\n\
                  Captures console_scripts entry points as binary artifacts.\n\
                  For packages with Rust extensions (maturin), the Rust toolchain and\n\
                  maturin must be installed. Override the Python interpreter with\n\
                  BF_PYTHON (default: python3). Pass extra pip install flags via\n\
                  BF_PIP_FLAGS.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: BuildCommand,
}

#[derive(Subcommand)]
enum BuildCommand {
    Detect { repo: String },
    Plan   { repo: String },
    Run {
        repo: String,
        #[arg(long)]
        plan: Option<String>,
        #[arg(long)]
        release: bool,
    },
}

fn python() -> String {
    std::env::var("BF_PYTHON").unwrap_or_else(|_| "python3".to_owned())
}

fn extra_pip_flags() -> Vec<String> {
    std::env::var("BF_PIP_FLAGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn is_python_repo(repo: &str) -> bool {
    let root = Path::new(repo);
    root.join("pyproject.toml").exists()
        || root.join("setup.cfg").exists()
        || root.join("setup.py").exists()
}

/// Return true if the project uses maturin (Rust extensions).
fn has_maturin(repo: &str) -> bool {
    let ppt = Path::new(repo).join("pyproject.toml");
    if let Ok(s) = std::fs::read_to_string(&ppt) {
        return s.contains("maturin") || s.contains("[tool.maturin]");
    }
    false
}

/// Read [project.name] or [metadata] name from pyproject.toml / setup.cfg.
fn project_name(repo: &str) -> String {
    // Try pyproject.toml first.
    if let Ok(s) = std::fs::read_to_string(Path::new(repo).join("pyproject.toml")) {
        for line in s.lines() {
            let line = line.trim();
            if line.starts_with("name") {
                if let Some(val) = line.splitn(2, '=').nth(1) {
                    let name = val.trim().trim_matches('"').trim_matches('\'').to_owned();
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
        }
    }
    // Fall back to directory name.
    Path::new(repo)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn git_ref(repo: &str) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Venv lives inside the repo directory so it's next to the source.
fn venv_path(repo: &str) -> std::path::PathBuf {
    Path::new(repo).join(".bf-venv")
}

/// Read the console_scripts entry points from the installed dist-info.
/// Returns a list of (script_name, script_path) tuples.
fn find_entry_points(repo: &str, pkg_name: &str) -> Vec<(String, std::path::PathBuf)> {
    let venv = venv_path(repo);
    let scripts_dir = venv.join("bin");
    let mut found = Vec::new();

    // Parse the dist-info entry_points.txt for console_scripts.
    let site_packages = venv.join("lib");
    if let Ok(entries) = std::fs::read_dir(&site_packages) {
        for py_dir in entries.flatten() {
            let dist_info = py_dir.path().join(format!(
                "{}-{}.dist-info",
                pkg_name.replace('-', "_"),
                "*"
            ));
            // Walk for any dist-info that starts with the package name.
            if let Ok(sub) = std::fs::read_dir(py_dir.path()) {
                for item in sub.flatten() {
                    let name = item.file_name().to_string_lossy().to_string();
                    let norm = pkg_name.replace('-', "_").to_lowercase();
                    if name.to_lowercase().starts_with(&norm) && name.ends_with(".dist-info") {
                        let ep_file = item.path().join("entry_points.txt");
                        if let Ok(ep) = std::fs::read_to_string(&ep_file) {
                            let mut in_console = false;
                            for line in ep.lines() {
                                let line = line.trim();
                                if line == "[console_scripts]" {
                                    in_console = true;
                                    continue;
                                }
                                if line.starts_with('[') {
                                    in_console = false;
                                }
                                if in_console {
                                    if let Some(ep_name) = line.splitn(2, '=').next() {
                                        let ep_name = ep_name.trim().to_owned();
                                        let script = scripts_dir.join(&ep_name);
                                        if script.exists() {
                                            found.push((ep_name, script));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Also emit the venv's python3 binary itself as an artifact so users can
    // run `bf-python qiskit` to enter a Python shell with the package available.
    let python_bin = scripts_dir.join("python3");
    if python_bin.exists() {
        found.push(("python3-qiskit".to_owned(), python_bin));
    }

    // If we found nothing, at least expose the venv/bin/python3.
    if found.is_empty() {
        let fallback = scripts_dir.join("python3");
        if fallback.exists() {
            found.push(("python3".to_owned(), fallback));
        }
    }

    found
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        BuildCommand::Detect { repo } => {
            if !is_python_repo(&repo) {
                std::process::exit(1);
            }
            let mut hints = vec!["pyproject.toml / setup found".to_owned()];
            if has_maturin(&repo) {
                hints.push("maturin (Rust extensions) detected".to_owned());
            }
            let det = BuildDetection {
                adapter: "bf-build-python".to_owned(),
                confidence: 0.85,
                hints,
            };
            println!("{}", serde_json::to_string(&det)?);
            emit(&Event::Done { exit_code: 0 });
        }

        BuildCommand::Plan { repo } => {
            if !is_python_repo(&repo) {
                anyhow::bail!("no pyproject.toml or setup.py in {repo}");
            }
            let py = python();
            let steps = vec![
                BuildStep {
                    name: "create venv".to_owned(),
                    command: vec![py.clone(), "-m".to_owned(), "venv".to_owned(),
                                  ".bf-venv".to_owned()],
                    env: Default::default(),
                },
                BuildStep {
                    name: "pip install -e .[dev]".to_owned(),
                    command: vec![".bf-venv/bin/pip".to_owned(), "install".to_owned(),
                                  "-e".to_owned(), ".[dev]".to_owned()],
                    env: Default::default(),
                },
            ];
            let plan = BuildPlan {
                adapter: "bf-build-python".to_owned(),
                steps,
            };
            println!("{}", serde_json::to_string(&plan)?);
        }

        BuildCommand::Run { repo, plan: _, release: _ } => {
            if !is_python_repo(&repo) {
                anyhow::bail!("no pyproject.toml or setup.py in {repo}");
            }
            let py = python();
            let venv = venv_path(&repo);

            // Step 1: create (or reuse) the virtualenv.
            if !venv.join("bin/pip").exists() {
                eprintln!("bf-build-python: creating venv at {}", venv.display());
                let status = Command::new(&py)
                    .args(["-m", "venv", venv.to_str().unwrap_or(".bf-venv")])
                    .current_dir(&repo)
                    .status()
                    .context("python3 -m venv")?;
                if !status.success() {
                    anyhow::bail!("failed to create virtualenv");
                }
            } else {
                eprintln!("bf-build-python: reusing existing venv at {}", venv.display());
            }

            let pip = venv.join("bin/pip");

            // Step 2: upgrade pip and install build tools.
            eprintln!("bf-build-python: upgrading pip and installing build tools");
            let _ = Command::new(&pip)
                .args(["install", "--quiet", "--upgrade", "pip", "maturin", "setuptools", "wheel"])
                .current_dir(&repo)
                .status();

            // Step 3: install the package in editable mode with dev extras.
            let mut install_args = vec![
                "install".to_owned(),
                "-e".to_owned(),
                ".[dev]".to_owned(),
            ];
            install_args.extend(extra_pip_flags());
            eprintln!("bf-build-python: pip install -e .[dev] (this compiles Rust extensions)");
            let status = Command::new(&pip)
                .args(&install_args)
                .current_dir(&repo)
                .status()
                .context("pip install -e .[dev]")?;
            if !status.success() {
                // Retry without [dev] extras if the dev group is not defined.
                eprintln!("bf-build-python: [dev] extras failed, retrying without extras");
                let status2 = Command::new(&pip)
                    .args(["install", "-e", "."])
                    .current_dir(&repo)
                    .status()
                    .context("pip install -e .")?;
                if !status2.success() {
                    anyhow::bail!("pip install failed");
                }
            }

            // Step 4: collect entry-point scripts as artifacts.
            let pkg_name = project_name(&repo);
            let entry_points = find_entry_points(&repo, &pkg_name);
            eprintln!(
                "bf-build-python: found {} entry point(s): {}",
                entry_points.len(),
                entry_points.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
            );

            // Also write an `activate` wrapper script so users can source the venv.
            let activate_wrapper = format!(
                "#!/bin/sh\n# Butterfork: activate the {pkg_name} venv\n\
                 . {}/bin/activate\nexec \"$@\"\n",
                venv.display()
            );
            let wrapper_path = venv.join("bin/bf-activate");
            std::fs::write(&wrapper_path, &activate_wrapper)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut p = std::fs::metadata(&wrapper_path)?.permissions();
                p.set_mode(0o755);
                std::fs::set_permissions(&wrapper_path, p)?;
            }

            // Step 5: write the artifact manifest.
            let mut artifacts: Vec<Artifact> = entry_points
                .into_iter()
                .map(|(name, path)| Artifact {
                    src:  path.to_string_lossy().to_string(),
                    dest: format!("bin/{name}"),
                })
                .collect();

            // Include the activate wrapper.
            artifacts.push(Artifact {
                src:  wrapper_path.to_string_lossy().to_string(),
                dest: format!("bin/{pkg_name}-activate"),
            });

            let manifest = ArtifactManifest {
                project: pkg_name.clone(),
                git_ref: git_ref(&repo),
                built_at: now_ms().to_string(),
                artifacts,
            };

            let manifest_path = format!("{repo}/bf-artifact-manifest.json");
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
            emit(&Event::BuildComplete { manifest_path });
            emit(&Event::Done { exit_code: 0 });
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
```

### 3.3 Build and install the adapter

```sh
cargo build --release -p bf-build-python
cargo install --path bf-build-python

# Verify
bf-build-python --version
bf-build-python detect ~/.butterfork/repos/qiskit   # exits 0 after the clone
```

---

## 4. Writing a Qiskit sandbox profile

Qiskit's build needs outbound network access during the `pip install` phase (to download
dependencies from PyPI). After that, the tests run offline. The profile reflects this:

```toml
# ~/.butterfork/sandbox-profiles/qiskit-build.toml
#
# Network is allowed during install (pip needs PyPI).
# After the initial build, you can tighten this to unshare_net = true
# for test runs.

unshare_net = false

ro_binds = [
    "/usr/include",
    "/usr/lib",
    "/etc/ssl/certs",           # pip HTTPS certificate verification
    "/etc/ssl/private",
    "/etc/resolv.conf",         # DNS resolution
    "/etc/hosts",
    "/etc/ca-certificates",
]

rw_binds = [
    # pip writes to ~/.cache/pip — bind it rw so downloads are cached across builds.
]
```

> **Faster rebuilds:** After the first install, `pip install -e .` only recompiles
> Rust extension modules that changed. Subsequent `bf install qiskit` runs are fast
> (seconds to a few minutes) because the venv is reused and only the diff is compiled.

For test runs specifically, create a tighter `qiskit-test.toml` with `unshare_net = true`
so tests cannot make real network calls:

```toml
# ~/.butterfork/sandbox-profiles/qiskit-test.toml
unshare_net = true

ro_binds = [
    "/usr/include",
    "/usr/lib",
    "/etc/ssl/certs",
]
```

---

## 5. Adding Qiskit to the catalog

```sh
bf-catalog add https://github.com/Qiskit/qiskit
```

Verify:

```sh
bf-catalog show qiskit
```

Expected output:

```json
{
  "slug": "qiskit",
  "name": "Qiskit",
  "description": "Open-source SDK for working with quantum computers",
  "upstream_url": "https://github.com/Qiskit/qiskit",
  "license": "Apache-2.0",
  "stars": 14000,
  "has_contributing": true,
  "has_code_of_conduct": true,
  "pr_response_latency_days": 2.1,
  "spdx_id": "Apache-2.0",
  "is_copyleft": false
}
```

Note that Qiskit's `CONTRIBUTING.md` references the IBM CLA. Butterfork's catalog entry
surfaces this as a warning at install time:

```
bf: warning: 'qiskit' requires a CLA — see CONTRIBUTING.md before submitting a PR
bf: sign at: https://cla-assistant.io/Qiskit/qiskit
```

---

## 6. Installing Qiskit through Butterfork

### 6.1 Set environment

```sh
export BF_BUILD=bf-build-python
export BF_SANDBOX_PROFILE=qiskit-build

# Optional: use a specific Python version
export BF_PYTHON=python3.11
```

### 6.2 Run the install

```sh
bf install qiskit
```

Annotated step-by-step output:

```
bf: step 1/5 — catalog lookup for 'qiskit'
bf: upstream: https://github.com/Qiskit/qiskit
bf: step 2/5 — forking on GitHub
  → bf-forge forks Qiskit/qiskit to <your-username>/qiskit
  → emits: {"type":"fork-created","fork_url":"https://github.com/<you>/qiskit"}
bf: step 3/5 — cloning https://github.com/<you>/qiskit → ~/.butterfork/repos/qiskit
  → clone ~200 MB (significantly faster than Godot)
bf: step 4/5 — building
  → bf-build dispatches to bf-build-python (pyproject.toml detected, maturin detected)
  → python3 -m venv ~/.butterfork/repos/qiskit/.bf-venv
  → pip install --upgrade pip maturin setuptools wheel
  → pip install -e .[dev]
      → maturin compiles Rust extensions (rustworkx, qiskit._accelerate)
      → pip resolves: numpy, scipy, sympy, stevedore, rustworkx, dill, ...
      → editable install complete
  → entry points found: qiskit (CLI)
  → manifest written to ~/.butterfork/repos/qiskit/bf-artifact-manifest.json
bf: step 5/5 — installing generation
  → bf-install add qiskit <manifest>
  → bf-install activate qiskit latest
  → symlink: ~/.butterfork/bin/qiskit → generation 1/bin/qiskit
  → symlink: ~/.butterfork/bin/python3-qiskit → generation 1/bin/python3-qiskit
  → symlink: ~/.butterfork/bin/qiskit-activate → generation 1/bin/qiskit-activate
bf: 'qiskit' installed — binaries under ~/.butterfork/bin/
```

### 6.3 Smoke test

```sh
# Check the CLI
qiskit version

# Check the library import through the generation's venv Python
python3-qiskit -c "import qiskit; print(qiskit.__version__)"
# 1.x.x

# Check which commit you're on
cd ~/.butterfork/repos/qiskit && git log -1 --oneline
```

---

## 7. Using your custom Qiskit in your own projects

Your quantum computing projects in `~/Development/Quantum-Computing/` currently import
the system-wide Qiskit (from pip or conda). After the Butterfork install you have a
fork-backed build. Here's how to wire your projects to it.

### 7.1 Activate the generation venv for a single session

```sh
# The activate wrapper script is a Butterfork artifact
source ~/.butterfork/bin/qiskit-activate

# Now any python3 call in this shell uses the Qiskit generation venv
cd ~/Development/Quantum-Computing/circuit-trainer
python3 main.py
```

### 7.2 Point a project permanently at the generation venv

Add a `.env` file at the root of each project (most editors and test runners pick this
up via `python-dotenv`):

```sh
# ~/Development/Quantum-Computing/circuit-trainer/.env
PYTHONPATH=~/.butterfork/repos/qiskit
```

Or, if you use a project-level venv for your own code, install the Butterfork Qiskit
into it as an editable link:

```sh
cd ~/Development/Quantum-Computing/circuit-trainer
python3 -m venv .venv && source .venv/bin/activate

# Install your custom Qiskit into this project's venv as a link
pip install -e ~/.butterfork/repos/qiskit

# Now changes in ~/.butterfork/repos/qiskit show up here immediately
# because it's an editable install (no reinstall needed after source edits)
python3 main.py
```

### 7.3 Roll between Qiskit versions across projects

Because each `bf install` creates a new generation, you can maintain multiple Qiskit
builds simultaneously and switch per-project:

```sh
# Project A: use generation 1 (stable fork base)
export PYTHONPATH=$(readlink ~/.butterfork/generations/qiskit/1/bin/python3-qiskit | xargs dirname)/../../lib/python*/site-packages

# Project B: use generation 2 (your experimental patch)
# … activate the generation 2 venv instead
```

This is the primary workflow for testing whether your patch breaks any of your own
downstream projects before you submit it upstream.

---

## 8. Making a change request

### Example: fix a real papercut

Suppose while running your `vqa-trainer` you notice that `QuantumCircuit.draw()`
throws an unhelpful `KeyError` when a custom gate label contains a colon. Describe it:

```sh
bf request qiskit 'QuantumCircuit.draw() raises KeyError when a custom gate label
    contains a colon character — it should either escape the colon or raise a clear
    ValueError with the gate name and the problematic character'
```

Or a feature request:

```sh
bf request qiskit 'add a QuantumCircuit.depth_by_qubit() method that returns a dict
    mapping each qubit index to its individual circuit depth, so users can identify
    bottleneck qubits without parsing the full circuit structure manually'
```

### What the agent does

The agent creates a branch `bf/add-a-quantumcircuit-depth-by-qubit-<ts>`, reads
relevant source files in `~/.butterfork/repos/qiskit/`, and works through the change:

```
[agent] Reading qiskit/circuit/quantumcircuit.py to find the depth() method...
[agent] Reading qiskit/circuit/quantumcircuitdata.py for gate representation...
[agent] Drafting depth_by_qubit() implementation in quantumcircuit.py...
[agent] Writing qiskit/circuit/quantumcircuit.py
[agent] Writing test_depth_by_qubit in test/python/circuit/test_quantumcircuit.py
[agent] Running: black qiskit/circuit/quantumcircuit.py
[agent] Running: ruff check qiskit/circuit/quantumcircuit.py --fix
[agent] Running: python3 -m pytest test/python/circuit/test_quantumcircuit.py -x -q
    → collected 312 items ... 312 passed in 14.3s
[agent] Staging changes...
[agent] Committing: "Add QuantumCircuit.depth_by_qubit() for per-qubit depth analysis"
```

After the agent finishes, the adapter rebuilds Qiskit's editable install (fast, since
only `.py` files changed — no Rust recompilation), and `bf install` activates the new
generation. Your `~/Development/Quantum-Computing/` projects pick up the change on the
next import.

### Targeting Rust extensions

If your change requires modifying Qiskit's Rust-backed components (files under
`crates/` in the repo), the rebuild takes longer because maturin must recompile:

```sh
bf request qiskit 'the statevector simulator in crates/accelerate/src/statevector.rs
    allocates a new state vector on every measurement operation — cache the allocation
    across shots when the number of shots is fixed at QuantumInstance creation time'
```

In this case the agent will:
1. Locate the relevant Rust source under `crates/accelerate/`.
2. Make the change.
3. Run `maturin develop` (fast incremental Rust compile, not a full rebuild).
4. Run the relevant pytest suite.
5. Commit.

Expect 2–10 minutes for incremental Rust compilation.

---

## 9. Iterating with the agent

### Refining a change

```sh
bf request qiskit 'the depth_by_qubit() implementation is correct but it does not
    handle ClassicalRegister bits — add handling so that if include_clbits=True is
    passed, clbit contributions are counted per classical bit index too'
```

The agent reads `git log -1` to see the previous commit, extends the implementation,
and amends or appends a commit.

### Running the test suite yourself

```sh
cd ~/.butterfork/repos/qiskit

# Activate the generation venv
source .bf-venv/bin/activate

# Run the full circuit test suite
python3 -m pytest test/python/circuit/ -x -q

# Run a specific test file
python3 -m pytest test/python/circuit/test_quantumcircuit.py -x -v -k "depth"

# Run the style checks manually
black --check qiskit/
ruff check qiskit/
```

### Inspecting agent changes

```sh
cd ~/.butterfork/repos/qiskit

# Show the diff
git show HEAD --stat
git diff HEAD~1 HEAD qiskit/circuit/quantumcircuit.py

# Verify black compliance
black --check qiskit/circuit/quantumcircuit.py

# Verify ruff
ruff check qiskit/circuit/quantumcircuit.py
```

---

## 10. Submitting upstream

```sh
bf submit qiskit
```

Pre-flight checks before the PR opens:

| Check | Pass condition |
|-------|---------------|
| Branch is not `main` | Always true for `bf`-created branches |
| DCO Signed-off-by present | Added automatically by the agent |
| IBM CLA signed | Checked via `cla-assistant.io` API; warns if not signed |
| `pytest` exits 0 | Relevant test files must pass |
| `black --check` clean | No unformatted code |
| `ruff check` clean | No lint violations |
| Diff size < 1 000 lines | Warning only |
| No new files without Apache-2.0 header | Enforced by policy |

The PR body is auto-drafted from the commit message. Qiskit's `CONTRIBUTING.md`
does not prohibit mentioning AI assistance, so the default footer is included.
To disable it:

```toml
# ~/.butterfork/pr-policy/qiskit.toml
ai_footer = "exclude"
```

`bf submit` pushes to your fork and opens the PR against `Qiskit/qiskit:main`.

---

## 11. Qiskit PR etiquette checklist

Qiskit's [CONTRIBUTING.md](https://github.com/Qiskit/qiskit/blob/main/CONTRIBUTING.md)
and reviewer expectations:

### Code style

- **black** is non-negotiable. Run `black .` before committing. The agent does this
  automatically, but verify with `black --check qiskit/` before submitting.
- **ruff** for linting. Run `ruff check qiskit/ --fix`. The agent runs this, but
  check for any `# noqa` suppressions the agent may have added and confirm they are
  appropriate.
- **pylint** on modified files: `pylint qiskit/circuit/quantumcircuit.py`. Pylint
  failures are not always blocking but reviewers will call them out.
- **Type annotations** are expected on all new public methods. If your method has
  parameters or a return value, annotate them. Import types from `qiskit.circuit` or
  `typing` as needed.

### Docstrings

All public methods need a numpy-style docstring:

```python
def depth_by_qubit(
    self,
    gate_count_callback: Callable | None = None,
    include_clbits: bool = False,
) -> dict[Qubit, int]:
    """Return the depth of each qubit in the circuit.

    The qubit depth is the number of gates acting on that qubit
    in the critical path through the circuit.

    Args:
        gate_count_callback: Optional callable that takes a DAGOpNode
            and returns ``True`` if the gate should be counted.
            Defaults to counting all operations.
        include_clbits: If ``True``, also compute depths for classical bits
            and include them in the returned dict.

    Returns:
        A dict mapping each :class:`.Qubit` (and optionally :class:`.Clbit`)
        to its individual depth.

    Example::

        from qiskit.circuit import QuantumCircuit
        qc = QuantumCircuit(3)
        qc.h(0)
        qc.cx(0, 1)
        qc.cx(1, 2)
        print(qc.depth_by_qubit())
        # {Qubit(QuantumRegister(3, 'q'), 0): 2,
        #  Qubit(QuantumRegister(3, 'q'), 1): 2,
        #  Qubit(QuantumRegister(3, 'q'), 2): 1}
    """
```

The agent writes docstrings in this format, but review them for accuracy, especially
the `Example::` block — the agent may use a simplified circuit that doesn't reflect the
actual semantics of your method.

### Tests

- All new public methods **must have tests** in the `test/python/` tree. The agent
  writes these, but verify that the tests cover:
  - The happy path.
  - Edge cases: empty circuit, single-qubit circuit, circuit with measurements.
  - Invalid inputs if your method validates arguments.
- Tests must be class-based inheriting `QiskitTestCase`:
  ```python
  from test import QiskitTestCase

  class TestDepthByQubit(QiskitTestCase):
      def test_basic_circuit(self):
          ...
  ```
- Do not use bare `unittest.TestCase` — it skips Qiskit's test infrastructure.

### Commit messages

- Imperative subject, ≤ 72 characters: `Add QuantumCircuit.depth_by_qubit() method`
- Body explains motivation and links any related issues.
- No `Fixes #NNN` unless the issue is actually in `Qiskit/qiskit` (not a fork issue).
- DCO `Signed-off-by: Your Name <email>` on every commit.

### CLA reminder

The IBM CLA bot runs on every PR. If you see:

```
cla-assistant: Thank you for your submission! We really appreciate it.
Like many open source projects, we ask that you sign our Contributor License Agreement
before we can accept your contribution.
```

Go to [cla-assistant.io/Qiskit/qiskit](https://cla-assistant.io/Qiskit/qiskit) and
sign. The bot updates the PR status automatically within a minute.

### Release notes

For user-visible changes (new methods, behavior changes, deprecations), Qiskit uses
[Reno](https://docs.openstack.org/reno/latest/) for release notes. Add a note:

```sh
# From inside the repo with the venv activated
cd ~/.butterfork/repos/qiskit
source .bf-venv/bin/activate
reno new add-depth-by-qubit
```

Edit the generated file in `releasenotes/notes/` to describe the change. The agent
will do this if you include it in the prompt:

```sh
bf request qiskit '... also add a Reno release note for the new method'
```

---

## 12. Rollback

If a change breaks your own quantum computing projects:

```sh
# List generations
bf rescue list qiskit

# Example output:
# generation 1  built 2026-05-04T09:00:00  active=false  ref=abc1234
# generation 2  built 2026-05-04T15:30:00  active=true   ref=def5678

# Activate the previous generation
bf rescue activate qiskit 1
```

Because each generation has its own isolated venv, rollback is instant — no pip
operations, no recompilation. Projects that activated the venv directly will use the
previous generation on the next interpreter invocation.

After rollback, your `~/Development/Quantum-Computing/` projects that use `PYTHONPATH`
pointing at the Butterfork repo will need a refresh:

```sh
# If you used PYTHONPATH pointing at the repo itself (editable install)
cd ~/.butterfork/repos/qiskit
git checkout <generation-1-ref>
# The editable install in the repo dir is automatically back to gen-1
```

If you linked your project venvs to the repo via `pip install -e`, the rollback
is automatic because the editable install tracks the repo directory.

---

## 13. Keeping your fork in sync

Qiskit releases frequently. To sync before starting a new change:

```sh
cd ~/.butterfork/repos/qiskit

git remote get-url upstream 2>/dev/null || \
    git remote add upstream https://github.com/Qiskit/qiskit.git

git fetch upstream
git checkout main
git merge --ff-only upstream/main
git push origin main

# Rebuild so generation 3 reflects the current upstream main
BF_BUILD=bf-build-python bf install qiskit
```

---

## 14. Common pitfalls

### `pip install -e .[dev]` fails with "no such extra: dev"

Some Qiskit branches define development extras under a different name. Try:

```sh
# Inspect what extras are available
cd ~/.butterfork/repos/qiskit
grep -A 20 '\[project.optional-dependencies\]' pyproject.toml

# Then retry with the correct extra
BF_PIP_FLAGS="-e .[test]" bf install qiskit
```

Or set `BF_PIP_FLAGS=""` to install without extras and install test dependencies
manually afterwards.

### Rust extension compilation fails with `maturin not found`

The adapter installs maturin into the venv before the editable install, but if
`pip install maturin` itself fails (e.g. network issue), maturin won't be available.

```sh
# Install maturin system-wide as a fallback
pip3 install --user maturin

# Verify maturin is on PATH
maturin --version

# Re-run the install
BF_BUILD=bf-build-python bf install qiskit
```

### `import qiskit` fails in your project after install

The most common cause is that your project's Python interpreter is not the one inside
the Butterfork generation venv. Check:

```sh
# Which Python does your project use?
which python3

# Which Python is in the Butterfork venv?
readlink ~/.butterfork/bin/python3-qiskit

# Activate the Butterfork venv first, then run your project
source ~/.butterfork/bin/qiskit-activate
python3 ~/Development/Quantum-Computing/circuit-trainer/main.py
```

### `pytest` fails with `ImportError: cannot import name '_qasm3'`

This means the Rust extension module was not compiled for your current Python version.
The venv's Python and the Rust extension `.so` must match. If you upgraded Python
between installs:

```sh
cd ~/.butterfork/repos/qiskit
source .bf-venv/bin/activate
pip install --force-reinstall -e .
```

This recompiles the Rust extension for the current interpreter.

### Agent produces changes that break `black`

The agent runs `black` after writing files, but if `black` itself is not installed in
the generation venv, the run silently passes. Verify:

```sh
cd ~/.butterfork/repos/qiskit
source .bf-venv/bin/activate
black --version   # must print a version

# If missing
pip install black ruff
```

Then re-run the request and the agent will apply formatting correctly.

### CLA not signed — PR blocked

If `bf submit` opens a PR and the CLA bot immediately marks it as blocking:

1. Visit [cla-assistant.io/Qiskit/qiskit](https://cla-assistant.io/Qiskit/qiskit).
2. Sign in with the same GitHub account that owns your fork.
3. Accept the agreement.
4. Comment `/bot recheck` on the PR — the bot re-verifies within 60 seconds.

The PR does not need to be closed and reopened.

### `ruff` reports `E402` (module-level import not at top of file)

Qiskit's source has some `TYPE_CHECKING` import blocks. If the agent adds an import
outside this pattern, ruff will flag it:

```
E402 Module level import not at top of file
```

Fix:

```sh
cd ~/.butterfork/repos/qiskit
ruff check qiskit/circuit/quantumcircuit.py --fix
git add qiskit/circuit/quantumcircuit.py
git commit --amend --no-edit
```

Or ask the agent to fix it:

```sh
bf request qiskit 'ruff reports E402 in qiskit/circuit/quantumcircuit.py on the
    import I added — move it to the correct location at the top of the file
    respecting the existing TYPE_CHECKING guard block'
```

---

*Next step: once your Qiskit PR is merged, rebuild from your synced fork with
`BF_BUILD=bf-build-python bf install qiskit` to produce a new generation that includes
your contribution, and link it into your `~/Development/Quantum-Computing/` projects
as the stable baseline for your next change.*
