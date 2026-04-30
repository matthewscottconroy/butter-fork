use anyhow::{Context, Result};
use bf_common::{emit, Artifact, ArtifactManifest, BuildDetection, BuildPlan, BuildStep, Event};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build-cargo",
    about = "Cargo build adapter for bf-build",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: BuildCommand,
}

#[derive(Subcommand)]
enum BuildCommand {
    /// Report whether this repo uses Cargo (exit 0) or not (exit 1)
    Detect { repo: String },
    /// Emit a BuildPlan JSON for a Cargo workspace or crate
    Plan { repo: String },
    /// Build the project and write target/bf-artifact-manifest.json
    Run {
        repo: String,
        #[arg(long)]
        plan: Option<String>,
        /// Build in release mode
        #[arg(long)]
        release: bool,
    },
}

// ── detection ─────────────────────────────────────────────────────────────────

fn is_cargo_repo(repo: &str) -> bool {
    Path::new(repo).join("Cargo.toml").exists()
}

// ── cargo metadata helpers ────────────────────────────────────────────────────

/// Returns the names of all `[[bin]]` targets in the workspace.
pub fn binary_targets(repo: &str) -> Result<Vec<String>> {
    let out = Command::new("cargo")
        .current_dir(repo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running `cargo metadata`")?;

    if !out.status.success() {
        anyhow::bail!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing cargo metadata")?;

    let mut bins = Vec::new();
    if let Some(packages) = meta["packages"].as_array() {
        for pkg in packages {
            if let Some(targets) = pkg["targets"].as_array() {
                for target in targets {
                    let is_bin = target["kind"]
                        .as_array()
                        .map(|kinds| kinds.iter().any(|k| k.as_str() == Some("bin")))
                        .unwrap_or(false);
                    if is_bin {
                        if let Some(name) = target["name"].as_str() {
                            bins.push(name.to_owned());
                        }
                    }
                }
            }
        }
    }

    Ok(bins)
}

/// Returns the primary package name (first `[package]` in workspace root Cargo.toml,
/// or the root package name from cargo metadata).
pub fn root_package_name(repo: &str) -> Result<String> {
    let out = Command::new("cargo")
        .current_dir(repo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running `cargo metadata`")?;

    if !out.status.success() {
        anyhow::bail!("`cargo metadata` failed");
    }

    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)?;

    // workspace_default_members lists the "root" packages (e.g. virtual workspaces use all members).
    // Prefer the first default member's name.
    if let Some(members) = meta["workspace_default_members"].as_array() {
        if let Some(first_id) = members.first().and_then(|v| v.as_str()) {
            if let Some(pkgs) = meta["packages"].as_array() {
                for pkg in pkgs {
                    if pkg["id"].as_str() == Some(first_id) {
                        if let Some(name) = pkg["name"].as_str() {
                            return Ok(name.to_owned());
                        }
                    }
                }
            }
        }
    }

    // Fallback: first package in the list.
    if let Some(pkgs) = meta["packages"].as_array() {
        if let Some(name) = pkgs.first().and_then(|p| p["name"].as_str()) {
            return Ok(name.to_owned());
        }
    }

    // Last resort: directory name.
    Ok(Path::new(repo)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_owned()))
}

/// Return the short git SHA of HEAD, or "unknown" if git is unavailable.
pub fn git_ref(repo: &str) -> String {
    Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Milliseconds since UNIX epoch — used as a monotonic generation ID.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── main ─────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        BuildCommand::Detect { repo } => {
            if is_cargo_repo(&repo) {
                let det = BuildDetection {
                    adapter: "bf-build-cargo".to_owned(),
                    confidence: 0.95,
                    hints: vec!["Cargo.toml present".to_owned()],
                };
                println!("{}", serde_json::to_string(&det)?);
                emit(&Event::Done { exit_code: 0 });
            } else {
                // Non-zero exit signals "not applicable" to bf-build's dispatcher.
                std::process::exit(1);
            }
        }

        BuildCommand::Plan { repo } => {
            if !is_cargo_repo(&repo) {
                eprintln!("bf-build-cargo: no Cargo.toml in '{repo}'");
                std::process::exit(bf_common::exit::DATAERR);
            }
            let plan = BuildPlan {
                adapter: "bf-build-cargo".to_owned(),
                steps: vec![
                    BuildStep {
                        name: "cargo build --release".to_owned(),
                        command: vec![
                            "cargo".to_owned(),
                            "build".to_owned(),
                            "--release".to_owned(),
                        ],
                        env: Default::default(),
                    },
                    BuildStep {
                        name: "write artifact manifest".to_owned(),
                        command: vec![
                            "bf-build-cargo".to_owned(),
                            "run".to_owned(),
                            "--release".to_owned(),
                        ],
                        env: Default::default(),
                    },
                ],
            };
            println!("{}", serde_json::to_string(&plan)?);
        }

        BuildCommand::Run {
            repo,
            plan: _,
            release,
        } => {
            if !is_cargo_repo(&repo) {
                eprintln!("bf-build-cargo: no Cargo.toml in '{repo}'");
                std::process::exit(bf_common::exit::DATAERR);
            }

            // Discover binary targets before building.
            let bins = binary_targets(&repo).unwrap_or_else(|e| {
                eprintln!("bf-build-cargo: cargo metadata warning: {e}");
                // Fall back to directory name as the single binary.
                vec![Path::new(&repo)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_owned())]
            });
            eprintln!("bf-build-cargo: binary targets: {}", bins.join(", "));

            let profile = if release { "release" } else { "debug" };
            emit(&Event::Plan {
                steps: vec![
                    format!("cargo build{}", if release { " --release" } else { "" }),
                    "collect artifacts".to_owned(),
                    "write target/bf-artifact-manifest.json".to_owned(),
                ],
            });

            // Run the build.
            eprintln!(
                "bf-build-cargo: running cargo build{}",
                if release { " --release" } else { "" }
            );
            let mut cmd = Command::new("cargo");
            cmd.current_dir(&repo).arg("build");
            if release {
                cmd.arg("--release");
            }
            let status = cmd.status().context("running `cargo build`")?;
            if !status.success() {
                emit(&Event::Done {
                    exit_code: status.code().unwrap_or(1),
                });
                std::process::exit(status.code().unwrap_or(1));
            }

            // Collect built artifacts.
            let mut artifacts: Vec<Artifact> = Vec::new();
            for bin_name in &bins {
                let src = format!("{repo}/target/{profile}/{bin_name}");
                if Path::new(&src).exists() {
                    artifacts.push(Artifact {
                        src,
                        // Relative dest inside the generation dir.
                        dest: format!("bin/{bin_name}"),
                    });
                } else {
                    eprintln!("bf-build-cargo: warning: expected binary not found: {src}");
                }
            }

            if artifacts.is_empty() {
                eprintln!(
                    "bf-build-cargo: no binaries found in {repo}/target/{profile}/ — \
                     check `cargo metadata` output"
                );
                std::process::exit(bf_common::exit::SOFTWARE);
            }

            let pkg_name = root_package_name(&repo).unwrap_or_else(|_| {
                Path::new(&repo)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_owned())
            });

            let manifest = ArtifactManifest {
                project: pkg_name,
                git_ref: git_ref(&repo),
                built_at: now_ms().to_string(),
                artifacts,
            };

            let manifest_path = format!("{repo}/target/bf-artifact-manifest.json");
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
            eprintln!("bf-build-cargo: manifest written → {manifest_path}");

            emit(&Event::BuildComplete {
                manifest_path: manifest_path.clone(),
            });
            emit(&Event::Done { exit_code: 0 });
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_this_workspace() {
        // The workspace root has a Cargo.toml — detection should succeed.
        let workspace = std::env::var("CARGO_MANIFEST_DIR")
            .map(|p| {
                // CARGO_MANIFEST_DIR is bf-build-cargo; go up one to the workspace root.
                std::path::PathBuf::from(p)
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|_| ".".to_owned());
        assert!(is_cargo_repo(&workspace));
    }

    #[test]
    fn rejects_non_cargo_dir() {
        let tmp = std::env::temp_dir();
        // The system temp dir almost certainly has no Cargo.toml.
        // If it somehow does, skip this test rather than fail.
        if !Path::new(&tmp).join("Cargo.toml").exists() {
            assert!(!is_cargo_repo(&tmp.to_string_lossy()));
        }
    }

    #[test]
    fn binary_targets_finds_bins_in_workspace() {
        let workspace = std::env::var("CARGO_MANIFEST_DIR")
            .map(|p| {
                std::path::PathBuf::from(p)
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|_| ".".to_owned());
        let bins = binary_targets(&workspace).expect("cargo metadata should succeed");
        // The workspace has at least `bf`, `bf-catalog`, `bf-build-cargo`, etc.
        assert!(
            !bins.is_empty(),
            "expected at least one binary target in workspace"
        );
        assert!(
            bins.iter().any(|b| b == "bf"),
            "expected `bf` binary in workspace"
        );
    }

    #[test]
    fn git_ref_returns_something() {
        let workspace = std::env::var("CARGO_MANIFEST_DIR")
            .map(|p| {
                std::path::PathBuf::from(p)
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|_| ".".to_owned());
        let r = git_ref(&workspace);
        assert!(!r.is_empty());
    }
}
