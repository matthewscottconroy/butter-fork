use anyhow::{Context, Result};
use bf_common::{emit, Artifact, ArtifactManifest, BuildDetection, BuildPlan, BuildStep, Event};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build-npm",
    about = "npm/pnpm build adapter for bf-build",
    long_about = "Detects Node.js projects by the presence of package.json.\n\
                  Uses pnpm if available, otherwise npm.\n\
                  Runs `install` then `build` script if present.\n\
                  Artifacts: executables declared in package.json `bin` field.",
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

fn is_npm_repo(repo: &str) -> bool {
    Path::new(repo).join("package.json").exists()
}

fn package_json(repo: &str) -> Result<serde_json::Value> {
    let s = std::fs::read_to_string(Path::new(repo).join("package.json"))
        .context("reading package.json")?;
    serde_json::from_str(&s).context("parsing package.json")
}

/// Prefer pnpm if on PATH, else npm.
fn package_manager() -> &'static str {
    if Command::new("pnpm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "pnpm"
    } else {
        "npm"
    }
}

fn has_build_script(pkg: &serde_json::Value) -> bool {
    pkg["scripts"]["build"].is_string()
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

/// Collect artifacts from the `bin` field of package.json.
fn artifacts_from_pkg(repo: &str, pkg: &serde_json::Value) -> Vec<Artifact> {
    let mut out = Vec::new();
    let bin = &pkg["bin"];
    match bin {
        serde_json::Value::String(path) => {
            let name = pkg["name"].as_str().unwrap_or("index").to_owned();
            out.push(Artifact {
                src: Path::new(repo).join(path).to_string_lossy().to_string(),
                dest: format!("bin/{name}"),
            });
        }
        serde_json::Value::Object(map) => {
            for (name, path_val) in map {
                if let Some(path) = path_val.as_str() {
                    out.push(Artifact {
                        src: Path::new(repo).join(path).to_string_lossy().to_string(),
                        dest: format!("bin/{name}"),
                    });
                }
            }
        }
        _ => {
            // No bin field: look for node_modules/.bin entries that match package name
            let nm_bin = Path::new(repo).join("node_modules/.bin");
            if let Ok(entries) = std::fs::read_dir(&nm_bin) {
                let pkg_name = pkg["name"]
                    .as_str()
                    .unwrap_or("")
                    .replace('@', "")
                    .replace('/', "-");
                for entry in entries.flatten() {
                    let n = entry.file_name().to_string_lossy().to_string();
                    if n == pkg_name {
                        out.push(Artifact {
                            src: entry.path().to_string_lossy().to_string(),
                            dest: format!("bin/{n}"),
                        });
                    }
                }
            }
        }
    }
    out
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        BuildCommand::Detect { repo } => {
            if !is_npm_repo(&repo) {
                std::process::exit(1);
            }
            let mut hints = vec!["package.json found".to_owned()];
            let pm = package_manager();
            hints.push(format!("package manager: {pm}"));
            if let Ok(pkg) = package_json(&repo) {
                if has_build_script(&pkg) {
                    hints.push("build script present".to_owned());
                }
            }
            let det = BuildDetection {
                adapter: "bf-build-npm".to_owned(),
                // Lower confidence than Cargo/CMake/Meson: package.json is very common
                confidence: 0.80,
                hints,
            };
            println!("{}", serde_json::to_string(&det)?);
        }

        BuildCommand::Plan { repo } => {
            if !is_npm_repo(&repo) {
                anyhow::bail!("no package.json in {repo}");
            }
            let pm = package_manager();
            let pkg = package_json(&repo).unwrap_or(serde_json::Value::Null);
            let mut steps = vec![BuildStep {
                name: "install".to_owned(),
                command: vec![pm.to_owned(), "install".to_owned()],
                env: Default::default(),
            }];
            if has_build_script(&pkg) {
                steps.push(BuildStep {
                    name: "build".to_owned(),
                    command: vec![pm.to_owned(), "run".to_owned(), "build".to_owned()],
                    env: Default::default(),
                });
            }
            let plan = BuildPlan {
                adapter: "bf-build-npm".to_owned(),
                steps,
            };
            println!("{}", serde_json::to_string(&plan)?);
        }

        BuildCommand::Run {
            repo,
            plan: _,
            release: _,
        } => {
            if !is_npm_repo(&repo) {
                anyhow::bail!("no package.json in {repo}");
            }
            let pm = package_manager();
            let pkg = package_json(&repo).context("reading package.json")?;

            eprintln!("bf-build-npm: {pm} install");
            let install = Command::new(pm)
                .arg("install")
                .current_dir(&repo)
                .status()
                .context("npm install")?;
            if !install.success() {
                anyhow::bail!("{pm} install failed");
            }

            if has_build_script(&pkg) {
                eprintln!("bf-build-npm: {pm} run build");
                let build = Command::new(pm)
                    .args(["run", "build"])
                    .current_dir(&repo)
                    .status()
                    .context("npm run build")?;
                if !build.success() {
                    anyhow::bail!("{pm} run build failed");
                }
            }

            let artifacts = artifacts_from_pkg(&repo, &pkg);
            eprintln!("bf-build-npm: found {} artifact(s)", artifacts.len());

            let project = pkg["name"]
                .as_str()
                .unwrap_or("unknown")
                .replace('@', "")
                .replace('/', "-");

            let manifest = ArtifactManifest {
                project,
                git_ref: git_ref(&repo),
                built_at: now_iso(),
                artifacts,
            };
            let manifest_path = Path::new(&repo).join("bf-artifact-manifest.json");
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
    fn detects_npm_project() {
        let dir = tmp();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"foo","version":"1.0.0"}"#,
        )
        .unwrap();
        assert!(is_npm_repo(dir.path().to_str().unwrap()));
    }

    #[test]
    fn rejects_non_npm_dir() {
        let dir = tmp();
        assert!(!is_npm_repo(dir.path().to_str().unwrap()));
    }

    #[test]
    fn has_build_script_true() {
        let pkg = serde_json::json!({"scripts":{"build":"webpack"}});
        assert!(has_build_script(&pkg));
    }

    #[test]
    fn has_build_script_false() {
        let pkg = serde_json::json!({"scripts":{"test":"jest"}});
        assert!(!has_build_script(&pkg));
    }

    #[test]
    fn string_bin_field() {
        let dir = tmp();
        let repo = dir.path().to_str().unwrap();
        let pkg = serde_json::json!({"name":"mytool","bin":"./bin/mytool.js"});
        let arts = artifacts_from_pkg(repo, &pkg);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].dest, "bin/mytool");
    }
}
