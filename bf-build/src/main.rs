use anyhow::Result;
use bf_common::{emit, BuildDetection, Event};
use clap::Parser;
use clap::Subcommand;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-build",
    about = "Build system detection, planning, and execution dispatcher",
    long_about = "bf-build discovers the right adapter by trying each bf-build-<name> binary\n\
                  in order of confidence score. Set BF_BUILD to force a specific adapter.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: BuildCommand,
}

#[derive(Subcommand)]
enum BuildCommand {
    /// Detect which build system a repository uses
    Detect {
        /// Path to the repository root
        repo: String,
    },
    /// Produce a concrete BuildPlan JSON for a repository
    Plan {
        /// Path to the repository root
        repo: String,
    },
    /// Build a repository (detect adapter if needed, then run)
    Run {
        /// Path to the repository root
        repo: String,
        /// Use a pre-computed build plan instead of detecting
        #[arg(long)]
        plan: Option<String>,
        /// Build in release mode
        #[arg(long)]
        release: bool,
    },
}

/// Well-known build adapters in preference order for auto-detection.
const ADAPTERS: &[&str] = &[
    "bf-build-cargo",
    "bf-build-cmake",
    "bf-build-meson",
    "bf-build-make",
    "bf-build-go",
    "bf-build-npm",
    "bf-build-python",
    "bf-build-gradle",
];

fn best_adapter(repo: &str) -> Option<(String, BuildDetection)> {
    if let Ok(forced) = std::env::var("BF_BUILD") {
        let detection = BuildDetection {
            adapter: forced.clone(),
            confidence: 1.0,
            hints: vec!["forced via BF_BUILD env var".to_owned()],
        };
        return Some((forced, detection));
    }

    let mut best: Option<(String, BuildDetection)> = None;
    for adapter in ADAPTERS {
        let Ok(out) = Command::new(adapter).args(["detect", repo]).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(det) = serde_json::from_str::<BuildDetection>(line) {
                let is_better = best
                    .as_ref()
                    .map(|(_, b)| det.confidence > b.confidence)
                    .unwrap_or(true);
                if is_better {
                    best = Some((adapter.to_string(), det));
                }
            }
        }
    }
    best
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        BuildCommand::Detect { repo } => {
            eprintln!("bf-build: detecting build system in '{repo}'");
            match best_adapter(&repo) {
                Some((_, det)) => {
                    println!("{}", serde_json::to_string(&det)?);
                }
                None => {
                    eprintln!("bf-build: no adapter could detect a build system in '{repo}'");
                    std::process::exit(bf_common::exit::DATAERR);
                }
            }
        }

        BuildCommand::Plan { repo } => {
            eprintln!("bf-build: planning build for '{repo}'");
            let Some((adapter, _)) = best_adapter(&repo) else {
                eprintln!("bf-build: no adapter detected for '{repo}'");
                std::process::exit(bf_common::exit::DATAERR);
            };
            let status = Command::new(&adapter).args(["plan", &repo]).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }

        BuildCommand::Run {
            repo,
            plan,
            release,
        } => {
            eprintln!("bf-build: building '{repo}'");
            let adapter = match std::env::var("BF_BUILD") {
                Ok(v) => v,
                Err(_) => match best_adapter(&repo) {
                    Some((a, det)) => {
                        eprintln!(
                            "bf-build: selected adapter '{}' (confidence {:.2})",
                            a, det.confidence
                        );
                        a
                    }
                    None => {
                        eprintln!("bf-build: could not detect build system for '{repo}'");
                        std::process::exit(bf_common::exit::DATAERR);
                    }
                },
            };

            let mut args = vec!["run".to_owned(), repo];
            if let Some(p) = plan {
                args.extend(["--plan".to_owned(), p]);
            }
            if release {
                args.push("--release".to_owned());
            }

            emit(&Event::Plan {
                steps: vec![
                    format!("detect adapter: {adapter}"),
                    "run build".to_owned(),
                    "write artifact manifest".to_owned(),
                ],
            });

            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let status = Command::new(&adapter).args(&arg_refs).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
