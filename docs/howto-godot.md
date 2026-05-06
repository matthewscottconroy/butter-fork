# Using Butterfork with Godot

This guide walks through the complete Butterfork workflow for
[Godot Engine](https://github.com/godotengine/godot) — from first fork to opening
a PR against upstream. Godot is a deliberately challenging example: it uses SCons
(a Python build system not covered by any built-in adapter), requires a dense set of
system libraries, produces a large binary, and has a well-defined PR etiquette that the
agent must respect. Working through Godot teaches every extensibility point Butterfork
exposes.

---

## Table of Contents

1. [What you'll end up with](#1-what-youll-end-up-with)
2. [System prerequisites](#2-system-prerequisites)
3. [Writing a `bf-build-scons` adapter](#3-writing-a-bf-build-scons-adapter)
4. [Writing a Godot sandbox profile](#4-writing-a-godot-sandbox-profile)
5. [Adding Godot to the catalog](#5-adding-godot-to-the-catalog)
6. [Installing Godot through Butterfork](#6-installing-godot-through-butterfork)
7. [Making a change request](#7-making-a-change-request)
8. [Iterating with the agent](#8-iterating-with-the-agent)
9. [Running your custom build](#9-running-your-custom-build)
10. [Submitting upstream](#10-submitting-upstream)
11. [Godot PR etiquette checklist](#11-godot-pr-etiquette-checklist)
12. [Rollback](#12-rollback)
13. [Keeping your fork in sync](#13-keeping-your-fork-in-sync)
14. [Common pitfalls](#14-common-pitfalls)

---

## 1. What you'll end up with

After following this guide:

- Your GitHub account has a fork of `godotengine/godot`.
- The fork is cloned to `~/.butterfork/repos/godot`.
- A Godot editor binary built from your fork is on your PATH via
  `~/.butterfork/bin/godot`.
- You understand how to ask the agent to make a source-level change, rebuild, test
  it, and open a PR against upstream.
- You know how to roll back to a previous build instantly if something goes wrong.

---

## 2. System prerequisites

Godot's Linux build requires several system libraries that your sandbox must be able
to see. Install them before doing anything else.

```sh
# Debian / Ubuntu / Pop!_OS
sudo apt install \
    build-essential \
    scons \
    python3 \
    pkg-config \
    libx11-dev \
    libxcursor-dev \
    libxrandr-dev \
    libxinerama-dev \
    libxi-dev \
    libxext-dev \
    libxrender-dev \
    libgl-dev \
    libglu-dev \
    libasound2-dev \
    libpulse-dev \
    libudev-dev \
    libdbus-1-dev \
    libfreetype-dev \
    libfontconfig-dev \
    libwayland-dev \
    libwayland-egl-backend-dev \
    libxkbcommon-dev \
    libvulkan-dev \
    glslang-tools \
    clang-format       # required by Godot's style gate

# Fedora / RHEL
sudo dnf install \
    scons python3 gcc gcc-c++ \
    libX11-devel libXcursor-devel libXrandr-devel libXinerama-devel \
    libXi-devel mesa-libGL-devel mesa-libGLU-devel \
    alsa-lib-devel pulseaudio-libs-devel libudev-devel \
    dbus-devel freetype-devel fontconfig-devel \
    wayland-devel libxkbcommon-devel vulkan-devel glslang \
    clang-tools-extra

# Arch / Manjaro
sudo pacman -S --needed \
    scons python base-devel \
    libx11 libxcursor libxrandr libxinerama libxi \
    mesa glu alsa-lib pulseaudio systemd \
    dbus freetype2 fontconfig wayland wayland-protocols \
    libxkbcommon vulkan-icd-loader glslang clang
```

Verify that `scons` is on your PATH:

```sh
scons --version
# SCons by Steven Knight et al.:
#     SCons: v4.x.x ...
```

Verify `gh` is authenticated (needed for forking):

```sh
gh auth status
```

---

## 3. Writing a `bf-build-scons` adapter

Butterfork ships adapters for Cargo, CMake, Meson, and npm. Godot uses
[SCons](https://scons.org/), so you need to write one adapter — a standalone Rust
binary that answers `detect`, `plan`, and `run`. This is the canonical way to extend
Butterfork's build system support, and the adapter you write here will work for any
SCons project, not just Godot.

### 3.1 Scaffold the crate

From your Butterfork workspace:

```sh
cargo new --name bf-build-scons bf-build-scons
```

Add it to the workspace:

```toml
# Cargo.toml (workspace root) — add to [workspace] members
members = [
    # ... existing members ...
    "bf-build-scons",
]
```

Add dependencies to `bf-build-scons/Cargo.toml`:

```toml
[package]
name = "bf-build-scons"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "bf-build-scons"
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

### 3.2 Implement `bf-build-scons/src/main.rs`

```rust
use anyhow::{Context, Result};
use bf_common::{emit, Artifact, ArtifactManifest, BuildDetection, BuildPlan, BuildStep, Event};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build-scons",
    about = "SCons build adapter for bf-build",
    long_about = "Detects SCons projects by the presence of SConstruct or SConscript.\n\
                  Builds the `editor` target on Linux by default.\n\
                  Override the SCons target with BF_SCONS_TARGET (e.g. template_release).\n\
                  Override extra SCons flags with BF_SCONS_FLAGS.",
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

fn is_scons_repo(repo: &str) -> bool {
    Path::new(repo).join("SConstruct").exists()
        || Path::new(repo).join("SConscript").exists()
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

/// Detect the platform SCons string for the current OS.
fn scons_platform() -> &'static str {
    if cfg!(target_os = "linux")   { "linuxbsd" }
    else if cfg!(target_os = "macos") { "macos" }
    else if cfg!(target_os = "windows") { "windows" }
    else { "linuxbsd" }
}

/// Return the SCons target (editor by default, overridden by BF_SCONS_TARGET).
fn scons_target() -> String {
    std::env::var("BF_SCONS_TARGET").unwrap_or_else(|_| "editor".to_owned())
}

/// Any extra flags passed verbatim to scons (space-separated).
fn scons_extra_flags() -> Vec<String> {
    std::env::var("BF_SCONS_FLAGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Locate the editor binary that SCons places in bin/ after a successful build.
fn find_built_binary(repo: &str) -> Option<std::path::PathBuf> {
    let bin_dir = Path::new(repo).join("bin");
    std::fs::read_dir(&bin_dir).ok()?.flatten().find_map(|e| {
        let path = e.path();
        // Godot names its editor binary godot.<platform>.<target>.<arch>[.exe]
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("godot") && !name.ends_with(".debug") {
            // Verify it's executable on Unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&path) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(path);
                    }
                }
            }
            #[cfg(not(unix))]
            return Some(path);
        }
        None
    })
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        BuildCommand::Detect { repo } => {
            if is_scons_repo(&repo) {
                let det = BuildDetection {
                    adapter: "bf-build-scons".to_owned(),
                    confidence: 0.90,
                    hints: vec!["SConstruct present".to_owned()],
                };
                println!("{}", serde_json::to_string(&det)?);
                emit(&Event::Done { exit_code: 0 });
            } else {
                std::process::exit(1);
            }
        }

        BuildCommand::Plan { repo } => {
            if !is_scons_repo(&repo) {
                eprintln!("bf-build-scons: no SConstruct in '{repo}'");
                std::process::exit(bf_common::exit::DATAERR);
            }
            let platform = scons_platform();
            let target   = scons_target();
            let mut cmd  = vec![
                "scons".to_owned(),
                format!("platform={platform}"),
                format!("target={target}"),
            ];
            cmd.extend(scons_extra_flags());

            let plan = BuildPlan {
                adapter: "bf-build-scons".to_owned(),
                steps: vec![BuildStep {
                    name: format!("scons platform={platform} target={target}"),
                    command: cmd,
                    env: Default::default(),
                }],
            };
            println!("{}", serde_json::to_string(&plan)?);
        }

        BuildCommand::Run { repo, plan: _, release } => {
            if !is_scons_repo(&repo) {
                eprintln!("bf-build-scons: no SConstruct in '{repo}'");
                std::process::exit(bf_common::exit::DATAERR);
            }

            let platform = scons_platform();
            let target   = if release { scons_target() } else { "editor".to_owned() };
            let mut args = vec![
                format!("platform={platform}"),
                format!("target={target}"),
                // Parallel jobs: use available CPUs.
                format!("-j{}", num_cpus()),
            ];
            args.extend(scons_extra_flags());

            eprintln!("bf-build-scons: running scons {}", args.join(" "));
            let status = std::process::Command::new("scons")
                .args(&args)
                .current_dir(&repo)
                .status()
                .context("running scons")?;

            if !status.success() {
                anyhow::bail!("scons exited with {status}");
            }

            let binary = find_built_binary(&repo).with_context(|| {
                format!("scons succeeded but no binary found in {repo}/bin/ — check BF_SCONS_TARGET")
            })?;

            let bin_name = binary
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "godot".to_owned());

            let manifest = ArtifactManifest {
                project: "godot".to_owned(),
                git_ref: git_ref(&repo),
                built_at: now_ms().to_string(),
                artifacts: vec![Artifact {
                    src:  binary.to_string_lossy().to_string(),
                    dest: format!("bin/{bin_name}"),
                }],
            };

            // Also write a stable `godot` symlink so users can just type `godot`.
            let godot_link = Artifact {
                src:  binary.to_string_lossy().to_string(),
                dest: "bin/godot".to_owned(),
            };
            let mut artifacts = manifest.artifacts;
            artifacts.push(godot_link);
            let manifest = ArtifactManifest { artifacts, ..manifest };

            let manifest_path = format!("{repo}/target/bf-artifact-manifest.json");
            std::fs::create_dir_all(format!("{repo}/target"))?;
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

            emit(&Event::BuildComplete { manifest_path });
            emit(&Event::Done { exit_code: 0 });
        }
    }

    Ok(())
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
```

### 3.3 Build and install the adapter

```sh
# From the workspace root
cargo build --release -p bf-build-scons

# Install it so bf-build can find it on PATH
cargo install --path bf-build-scons

# Verify
bf-build-scons --version
bf-build-scons detect ~/.butterfork/repos/godot   # exits 0 after the repo is cloned
```

Once `bf-build-scons` is on your PATH, `bf-build detect` will find it automatically
whenever it encounters a repo with a `SConstruct` file — no further configuration needed.

---

## 4. Writing a Godot sandbox profile

Godot's build needs access to system header paths and display services that the default
`build` profile (which has `unshare_net = true` and a minimal bind list) does not expose.
Create a custom profile at `~/.butterfork/sandbox-profiles/godot-build.toml`:

```toml
# ~/.butterfork/sandbox-profiles/godot-build.toml
#
# Used when building Godot from source. Network is isolated — all
# dependencies are already on disk. System library paths are bound
# read-only so SCons can find headers and pkg-config .pc files.

unshare_net = true

ro_binds = [
    "/usr/include",
    "/usr/lib",
    "/usr/lib/x86_64-linux-gnu",    # adjust for your arch
    "/usr/lib/pkgconfig",
    "/usr/share/pkgconfig",
    "/usr/bin/scons",
    "/usr/bin/python3",
    "/usr/bin/clang-format",
    "/usr/bin/pkg-config",
    "/usr/bin/gcc",
    "/usr/bin/g++",
    "/etc/alternatives",            # distro symlink farm for gcc/g++
    "/etc/ssl/certs",               # needed by Python's ssl module
]

rw_binds = [
    # The repo is bound rw automatically by bf-sandbox.
    # Add any additional writable paths here if your build needs them.
]
```

> **Note on Wayland/X11 headers:** if `pkg-config` is not finding headers you expect,
> add their specific `-dev` directories explicitly. Run
> `pkg-config --variable=includedir x11` to find the right path on your system.

To use this profile explicitly when building:

```sh
bf-sandbox --profile godot-build -- scons platform=linuxbsd target=editor -j$(nproc)
```

The `bf install godot` flow below will use it automatically because you'll set
`BF_SANDBOX_PROFILE=godot-build` in your shell or in a `.env` file next to the repo.

---

## 5. Adding Godot to the catalog

Godot is not in Butterfork's curated catalog yet (it will be once the catalog reaches
the game-engine category). Add it manually:

```sh
bf-catalog add https://github.com/godotengine/godot
```

Verify it landed:

```sh
bf-catalog show godot
```

Expected output (NDJSON):

```json
{
  "slug": "godot",
  "name": "Godot Engine",
  "description": "Multi-platform 2D and 3D game engine",
  "upstream_url": "https://github.com/godotengine/godot",
  "license": "MIT",
  "stars": 90000,
  "has_contributing": true,
  "has_code_of_conduct": true,
  "pr_response_latency_days": 3.4,
  "spdx_id": "MIT",
  "is_copyleft": false
}
```

---

## 6. Installing Godot through Butterfork

### 6.1 Set environment

```sh
# Tell bf-build to use the SCons adapter (overrides auto-detection for clarity)
export BF_BUILD=bf-build-scons

# Tell the sandbox which profile to load for the build step
export BF_SANDBOX_PROFILE=godot-build

# Optionally target a release template instead of the editor
# export BF_SCONS_TARGET=template_release

# Clang-format version check (Godot requires clang-format 14+)
clang-format --version
```

### 6.2 Run the install

```sh
bf install godot
```

What happens, step by step:

```
bf: step 1/5 — catalog lookup for 'godot'
bf: upstream: https://github.com/godotengine/godot
bf: step 2/5 — forking on GitHub
  → bf-forge forks godotengine/godot to <your-username>/godot
  → emits: {"type":"fork-created","fork_url":"https://github.com/<you>/godot"}
bf: step 3/5 — cloning https://github.com/<you>/godot → ~/.butterfork/repos/godot
  → full clone of ~2 GB (takes several minutes on first run)
bf: step 4/5 — building
  → bf-build dispatches to bf-build-scons (SConstruct detected)
  → scons platform=linuxbsd target=editor -j<N>
  → compiling ~1 500 C++ files (10–30 min depending on CPU)
  → binary written to ~/.butterfork/repos/godot/bin/godot.linuxbsd.editor.x86_64
  → manifest written to ~/.butterfork/repos/godot/target/bf-artifact-manifest.json
bf: step 5/5 — installing generation
  → bf-install add godot <manifest>
  → bf-install activate godot latest
  → symlink: ~/.butterfork/bin/godot → generation 1/bin/godot
bf: 'godot' installed — binaries under ~/.butterfork/bin/
```

### 6.3 Smoke test

```sh
godot --version
# 4.x.x.stable.custom_build
```

> **Tip:** Godot's build takes 10–30 minutes on a modern desktop for a first build.
> Incremental rebuilds (after small changes) take 1–5 minutes because SCons tracks
> dependencies correctly.

### 6.4 `--no-fork` mode (read-only exploration)

If you just want to build Godot without forking — to test the toolchain or explore the
code before committing to a fork — use:

```sh
bf install https://github.com/godotengine/godot --no-fork
```

This clones upstream directly. You can still use `bf request` in no-fork mode for
local experiments, but `bf submit` will refuse to push because you have no fork to
push to.

---

## 7. Making a change request

Once Godot is installed, describe what you want to change in plain English:

```sh
bf request godot "add a keyboard shortcut (Ctrl+Shift+R) to the Script editor \
    that runs the current scene without leaving the editor tab"
```

or a bug fix example:

```sh
bf request godot "the FileDialog remembers its last sort column but resets to \
    ascending order on reopen — it should also remember sort direction"
```

### What the agent does

`bf request` creates a feature branch, writes a tool manifest, and invokes `bf-agent`:

```
bf: creating branch bf/add-a-keyboard-shortcut-ctrl-shift-1714000000
bf: invoking agent — prompt: add a keyboard shortcut ...
```

The agent operates with these built-in tools scoped to the repo:

| Tool | What it does |
|------|-------------|
| `read_file` | Read any file in `~/.butterfork/repos/godot/` |
| `write_file` | Write or overwrite a file in the repo |
| `list_files` | List directory contents |
| `run_shell` | Run `scons`, `clang-format`, `python3`, etc. in the repo directory |
| `git_diff` | Show staged or unstaged changes |
| `git_add` | Stage files |
| `git_commit` | Create a commit with a DCO `Signed-off-by` trailer |

The agent cannot read your `~/.ssh`, your keychain, or any path outside the repo.
It cannot push anything. It works strictly within the sandbox.

### What to expect

A typical agent run for a Godot UI change looks like:

```
[agent] Reading editor/editor_node.cpp to understand scene run logic...
[agent] Reading editor/plugins/script_editor_plugin.cpp for shortcut registration...
[agent] Reading editor/input/input_map.cpp for shortcut definitions...
[agent] Drafting patch to editor/plugins/script_editor_plugin.cpp...
[agent] Writing editor/plugins/script_editor_plugin.cpp
[agent] Running clang-format on modified files...
[agent] Running: python3 misc/scripts/header_guards.py --check [file]...
[agent] Staging changes...
[agent] Committing: "Add Ctrl+Shift+R shortcut to run current scene from Script editor"
```

After the agent finishes, `bf request` rebuilds Godot automatically (a partial rebuild,
since only a few `.cpp` files changed) and activates the new generation:

```
bf: agent done — review changes with `git log -1` in ~/.butterfork/repos/godot
bf: run `bf submit godot` when ready to open a PR
```

---

## 8. Iterating with the agent

If the first attempt is not quite right, re-run `bf request` on the same branch:

```sh
bf request godot "the shortcut works but conflicts with the existing Ctrl+Shift+R \
    binding in the AnimationPlayer editor — resolve the conflict by remapping it \
    to Ctrl+Shift+F5 instead"
```

The agent will:
1. Read `git log --oneline` to understand what was already done.
2. Read the conflicting binding from `editor/plugins/animation_player_editor_plugin.cpp`.
3. Amend or add a follow-up commit on the existing branch.
4. Re-run `clang-format` and the header guard checker.
5. Rebuild and reinstall.

You can repeat this loop as many times as you want before submitting.

### Inspecting agent work

```sh
cd ~/.butterfork/repos/godot

# See the diff
git log -1 --stat
git diff HEAD~1 HEAD

# Verify clang-format compliance yourself
clang-format --dry-run --Werror editor/plugins/script_editor_plugin.cpp

# Run Godot's own style checks
python3 misc/scripts/header_guards.py editor/plugins/script_editor_plugin.cpp
python3 misc/scripts/copyright_headers.py editor/plugins/script_editor_plugin.cpp

# Open the editor in the modified build to test interactively
godot --editor
```

---

## 9. Running your custom build

Your modified Godot editor is on PATH as `godot`. Use it exactly like the upstream
release. Open a project, run scenes, try the new shortcut, observe the bug fix.

```sh
# Open the Godot project manager
godot

# Open a specific project directly
godot --path ~/my-game

# Headless (CI-style)
godot --headless --quit

# Check which generation you're running
bf rescue list godot
```

---

## 10. Submitting upstream

When you're happy with the change:

```sh
bf submit godot
```

Pre-flight checks that run before the PR opens:

| Check | Pass condition |
|-------|---------------|
| Branch is not `master` | Always true for `bf`-created branches |
| DCO `Signed-off-by` on every commit | `bf-agent` adds this automatically |
| `scons platform=linuxbsd target=editor` exits 0 | Build must be green |
| `clang-format` clean | No formatting drift |
| Header guards present | `misc/scripts/header_guards.py` passes |
| Diff < 1 000 lines (warning) | Your change gate — adjust in PR policy |
| No new dependencies without THIRDPARTY entry | Enforced by policy |

If any required check fails, `bf submit` exits with a non-zero code and prints the
failing check. Fix it and re-run.

The PR body is drafted automatically from the commit message, plus the AI-assistance
footer. Godot's `CONTRIBUTING.md` does not prohibit AI assistance disclosure, so the
default `ai_footer = "include"` applies. If Godot's policy changes, set:

```toml
# ~/.butterfork/pr-policy/godot.toml
ai_footer = "exclude"
```

`bf submit` pushes to your fork (e.g. `https://github.com/<you>/godot`) and opens
a PR from `bf/add-a-keyboard-shortcut-...` against `godotengine/godot:master`.

---

## 11. Godot PR etiquette checklist

Godot's [CONTRIBUTING.md](https://github.com/godotengine/godot/blob/master/CONTRIBUTING.md)
and its maintainers have specific expectations. Review these before `bf submit` — the
agent is instructed to follow them, but you should verify:

### Code style

- **clang-format 14+** enforced on all C++ files. Run:
  ```sh
  clang-format --dry-run --Werror <modified_file.cpp>
  ```
- **Header guards** must use the project's pattern (`EDITOR_PLUGINS_SCRIPT_EDITOR_PLUGIN_H`).
  Check with `python3 misc/scripts/header_guards.py <file.h>`.
- **Copyright headers** must be present on new files. Check with
  `python3 misc/scripts/copyright_headers.py <new_file.cpp>`.
- No trailing whitespace. `git diff --check HEAD~1` catches this.

### Commit message

- One-line subject in imperative mood: `Add Ctrl+Shift+R shortcut to Script editor`
- Subject must not end with a period.
- Body explains *why*, not just *what*. The agent writes this from the prompt; review it.
- DCO `Signed-off-by: Your Name <email>` is added automatically by the agent.

### PR scope

- One logical change per PR. The agent creates a branch per `bf request` invocation,
  which naturally enforces this.
- No unrelated reformatting. If you run `clang-format` on a file that was not logically
  changed by your patch, the style changes will appear in the diff and confuse reviewers.
  Limit formatting runs to files you actually modified.
- No new third-party libraries without a `THIRDPARTY/` entry and an explicit discussion.

### Test coverage

Godot does not use a traditional unit-test framework for editor code (most behavior is
tested through GDScript `assert()` in `.tscn`/`.gd` test scenes). If you are fixing
a bug that has a reproducible test case, add it to `tests/` and mention it in the PR
body.

### Platform considerations

If your change touches any platform-specific path (`platform/linuxbsd/`, `platform/macos/`,
etc.), mention which platforms you tested in the PR description. Reviewers may ask for
CI results from a second platform before merging.

---

## 12. Rollback

If the new build crashes, corrupts a project, or you simply want to go back:

```sh
# List your generations
bf rescue list godot

# Example output:
# generation 1  built 2026-05-04T10:12:33  active=false  ref=a3f1b9c
# generation 2  built 2026-05-04T14:55:02  active=true   ref=d7e2a01

# Roll back to generation 1 (the pre-patch build)
bf rescue activate godot 1

# Verify
godot --version
# The version string will show the ref from generation 1
```

The rollback is atomic — the symlink swap is instant and your running editor processes
are not affected (new launches pick up the new symlink).

---

## 13. Keeping your fork in sync

Godot moves fast. To sync your fork with upstream before making a new change:

```sh
cd ~/.butterfork/repos/godot

# Add upstream if not already there
git remote get-url upstream 2>/dev/null || \
    git remote add upstream https://github.com/godotengine/godot.git

# Fetch and fast-forward master
git fetch upstream
git checkout master
git merge --ff-only upstream/master
git push origin master

# Rebuild to get a fresh baseline generation
bf install godot
```

Or from outside the repo directory:

```sh
BF_NO_FORK=1 bf install godot   # pulls and rebuilds without re-forking
```

---

## 14. Common pitfalls

### `scons` not found inside sandbox

The default `build` sandbox profile does not bind `/usr/bin/scons`. Use the
`godot-build` profile described in section 4:

```sh
export BF_SANDBOX_PROFILE=godot-build
bf install godot
```

### `pkg-config` can't find Vulkan headers

Add the Vulkan SDK path to the sandbox profile's `ro_binds`. On Ubuntu the Vulkan
headers are in `/usr/include/vulkan`; if using the LunarG SDK they may be under
`/opt/vulkan/`. Check with:

```sh
pkg-config --cflags vulkan
```

and add the returned `-I/path/to/include` prefix to `ro_binds`.

### Build fails with "Python 3 is required"

SCons itself requires Python 3. Verify:

```sh
python3 --version       # must be 3.6+
which python3           # must be in a path bound by the sandbox
```

If your distro uses `/usr/bin/python3.11` as the real binary and symlinks
`/usr/bin/python3` to it, both paths need to be in the sandbox's `ro_binds`.

### Agent modifies too many files

Godot's codebase is large and the agent may wander into unrelated files during its
search phase. Bound the search by providing a more specific prompt:

```sh
# Too broad (agent may read dozens of files)
bf request godot "fix the FileDialog sort direction bug"

# Tighter (agent goes straight to the relevant subsystem)
bf request godot "in editor/gui/file_dialog.cpp: the _sort_items() method resets \
    sort_order_ascending to true on every call — preserve the value from the \
    previous session by reading/writing EditorSettings"
```

The more context you give about the file and the specific function, the fewer
exploratory reads the agent wastes and the faster the loop runs.

### clang-format version mismatch

Godot requires clang-format **14 or later**. Earlier versions produce slightly different
output and will fail the style CI:

```sh
clang-format --version
# Must be >= 14.0.0
```

On Ubuntu 22.04 LTS the default is clang-format-14. On 20.04 it is clang-format-10;
install a newer version via the LLVM apt repository.

### The PR is against the wrong base branch

Godot's default branch is `master`, not `main`. `bf submit` reads the upstream remote
and defaults the base branch to `main`. Override it temporarily:

```sh
# Until Butterfork learns Godot's preferred base branch from CONTRIBUTING.md:
bf-forge pr open \
    --repo godotengine/godot \
    --head bf/your-branch-name \
    --base master \
    --title "Your PR title" \
    --body "$(git log -1 --pretty=%B)"
```

> This is a known limitation tracked in the project's issue list. A future release of
> `bf-forge pr open` will read the upstream repo's default branch via the GitHub API
> and use it automatically.

### Shallow clone / missing history

If you previously cloned Godot as a shallow clone (e.g. `git clone --depth 1`), some
`git log` operations the agent relies on may fail. Check:

```sh
cd ~/.butterfork/repos/godot
git log --oneline | wc -l   # should be > 1 if not shallow
```

If shallow, unshallow:

```sh
git fetch --unshallow
```

---

*Next step: once your Godot PR is merged, use `bf install godot` to rebuild from
your updated fork and create a new generation that includes your own contribution.*
