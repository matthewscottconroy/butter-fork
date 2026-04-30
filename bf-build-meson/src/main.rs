use anyhow::{Context, Result};
use bf_common::{emit, Artifact, ArtifactManifest, BuildDetection, BuildPlan, BuildStep, Event};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build-meson",
    about = "Meson build adapter for bf-build",
    long_about = "Detects Meson projects by the presence of meson.build.\n\
                  Builds with: meson setup build && meson compile -C build\n\
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

fn is_meson_repo(repo: &str) -> bool {
    Path::new(repo).join("meson.build").exists()
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

fn find_artifacts(repo: &str) -> Vec<Artifact> {
    let build_dir = Path::new(repo).join("build");
    let mut out = Vec::new();
    collect_executables(&build_dir, &mut out);
    out
}

fn collect_executables(dir: &Path, out: &mut Vec<Artifact>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_executables(&path, out);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(
            ext,
            "so" | "a" | "dylib" | "dll" | "o" | "ninja" | "log" | "txt"
        ) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = path.metadata() {
                if meta.permissions().mode() & 0o111 != 0 && meta.is_file() {
                    out.push(Artifact {
                        src: path.to_string_lossy().to_string(),
                        dest: format!("bin/{name}"),
                    });
                }
            }
        }
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        BuildCommand::Detect { repo } => {
            if !is_meson_repo(&repo) {
                std::process::exit(1);
            }
            let det = BuildDetection {
                adapter: "bf-build-meson".to_owned(),
                confidence: 0.95,
                hints: vec!["meson.build found".to_owned()],
            };
            println!("{}", serde_json::to_string(&det)?);
        }

        BuildCommand::Plan { repo } => {
            if !is_meson_repo(&repo) {
                anyhow::bail!("no meson.build in {repo}");
            }
            let plan = BuildPlan {
                adapter: "bf-build-meson".to_owned(),
                steps: vec![
                    BuildStep {
                        name: "setup".to_owned(),
                        command: vec!["meson".to_owned(), "setup".to_owned(), "build".to_owned()],
                        env: Default::default(),
                    },
                    BuildStep {
                        name: "compile".to_owned(),
                        command: vec![
                            "meson".to_owned(),
                            "compile".to_owned(),
                            "-C".to_owned(),
                            "build".to_owned(),
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
            if !is_meson_repo(&repo) {
                anyhow::bail!("no meson.build in {repo}");
            }
            let build_type = if release { "release" } else { "debug" };
            let build_dir = Path::new(&repo).join("build");

            if !build_dir.exists() {
                eprintln!("bf-build-meson: meson setup build --buildtype={build_type}");
                let status = Command::new("meson")
                    .args(["setup", "build", &format!("--buildtype={build_type}")])
                    .current_dir(&repo)
                    .status()
                    .context("meson setup")?;
                if !status.success() {
                    anyhow::bail!("meson setup failed");
                }
            }

            eprintln!("bf-build-meson: compiling");
            let status = Command::new("meson")
                .args(["compile", "-C", "build"])
                .current_dir(&repo)
                .status()
                .context("meson compile")?;
            if !status.success() {
                anyhow::bail!("meson compile failed");
            }

            let artifacts = find_artifacts(&repo);
            eprintln!("bf-build-meson: found {} artifact(s)", artifacts.len());

            let manifest = ArtifactManifest {
                project: Path::new(&repo)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_owned()),
                git_ref: git_ref(&repo),
                built_at: now_iso(),
                artifacts,
            };

            let manifest_path = build_dir.join("bf-artifact-manifest.json");
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
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
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn detects_meson_project() {
        let dir = tmp();
        std::fs::write(dir.path().join("meson.build"), "project('foo', 'c')\n").unwrap();
        assert!(is_meson_repo(dir.path().to_str().unwrap()));
    }

    #[test]
    fn rejects_non_meson_dir() {
        let dir = tmp();
        assert!(!is_meson_repo(dir.path().to_str().unwrap()));
    }
}
