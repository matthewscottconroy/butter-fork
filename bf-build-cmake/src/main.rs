use anyhow::{Context, Result};
use bf_common::{emit, Artifact, ArtifactManifest, BuildDetection, BuildPlan, BuildStep, Event};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build-cmake",
    about = "CMake build adapter for bf-build",
    long_about = "Detects CMake projects by the presence of CMakeLists.txt.\n\
                  Builds with: cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build\n\
                  Artifacts: executables found in the build/ directory.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: BuildCommand,
}

#[derive(Subcommand)]
enum BuildCommand {
    Detect {
        repo: String,
    },
    Plan {
        repo: String,
    },
    Run {
        repo: String,
        #[arg(long)]
        plan: Option<String>,
        #[arg(long)]
        release: bool,
    },
}

fn is_cmake_repo(repo: &str) -> bool {
    Path::new(repo).join("CMakeLists.txt").exists()
}

fn git_ref(repo: &str) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// Find executables in `build/` that look like project binaries.
fn find_artifacts(repo: &str) -> Vec<Artifact> {
    let build_dir = Path::new(repo).join("build");
    let mut artifacts = Vec::new();
    collect_executables(&build_dir, &build_dir, &mut artifacts);
    artifacts
}

fn collect_executables(dir: &Path, build_root: &Path, out: &mut Vec<Artifact>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip CMake internals, libraries, and object files.
        if name.starts_with("CMake") || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_executables(&path, build_root, out);
            continue;
        }
        // Skip known non-binary extensions.
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(
            ext,
            "so" | "a" | "dylib" | "dll" | "o" | "cmake" | "txt" | "make"
        ) {
            continue;
        }
        // Check executable bit on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = path.metadata() {
                if meta.permissions().mode() & 0o111 != 0 && meta.is_file() {
                    let rel = path
                        .strip_prefix(build_root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    out.push(Artifact {
                        src: path.to_string_lossy().to_string(),
                        dest: format!("bin/{name}"),
                    });
                    let _ = rel; // suppress unused warning
                }
            }
        }
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        BuildCommand::Detect { repo } => {
            if !is_cmake_repo(&repo) {
                std::process::exit(1);
            }
            let det = BuildDetection {
                adapter: "bf-build-cmake".to_owned(),
                confidence: 0.95,
                hints: vec!["CMakeLists.txt found".to_owned()],
            };
            println!("{}", serde_json::to_string(&det)?);
        }

        BuildCommand::Plan { repo } => {
            if !is_cmake_repo(&repo) {
                anyhow::bail!("no CMakeLists.txt found in {repo}");
            }
            let build_type = "Release";
            let plan = BuildPlan {
                adapter: "bf-build-cmake".to_owned(),
                steps: vec![
                    BuildStep {
                        name: "configure".to_owned(),
                        command: vec![
                            "cmake".to_owned(),
                            "-B".to_owned(),
                            "build".to_owned(),
                            format!("-DCMAKE_BUILD_TYPE={build_type}"),
                        ],
                        env: Default::default(),
                    },
                    BuildStep {
                        name: "compile".to_owned(),
                        command: vec![
                            "cmake".to_owned(),
                            "--build".to_owned(),
                            "build".to_owned(),
                            "--parallel".to_owned(),
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
            if !is_cmake_repo(&repo) {
                anyhow::bail!("no CMakeLists.txt found in {repo}");
            }
            let build_type = if release { "Release" } else { "Debug" };
            eprintln!("bf-build-cmake: configuring ({build_type})");

            let cfg = Command::new("cmake")
                .args(["-B", "build", &format!("-DCMAKE_BUILD_TYPE={build_type}")])
                .current_dir(&repo)
                .status()
                .context("running cmake configure")?;
            if !cfg.success() {
                anyhow::bail!("cmake configure failed");
            }

            eprintln!("bf-build-cmake: compiling");
            let build = Command::new("cmake")
                .args(["--build", "build", "--parallel"])
                .current_dir(&repo)
                .status()
                .context("running cmake build")?;
            if !build.success() {
                anyhow::bail!("cmake build failed");
            }

            let ref_str = git_ref(&repo);
            let artifacts = find_artifacts(&repo);
            eprintln!("bf-build-cmake: found {} artifact(s)", artifacts.len());

            let manifest = ArtifactManifest {
                project: Path::new(&repo)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_owned()),
                git_ref: ref_str,
                built_at: now_iso(),
                artifacts,
            };

            let manifest_path = Path::new(&repo).join("build/bf-artifact-manifest.json");
            std::fs::write(
                &manifest_path,
                serde_json::to_string_pretty(&manifest).context("serializing manifest")?,
            )?;

            emit(&Event::BuildComplete {
                manifest_path: manifest_path.to_string_lossy().to_string(),
            });
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn detects_cmake_project() {
        let dir = tmp();
        fs::write(
            dir.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.10)\n",
        )
        .unwrap();
        assert!(is_cmake_repo(dir.path().to_str().unwrap()));
    }

    #[test]
    fn rejects_non_cmake_dir() {
        let dir = tmp();
        assert!(!is_cmake_repo(dir.path().to_str().unwrap()));
    }
}
