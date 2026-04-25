use anyhow::{Context, Result};
use bf_common::{emit, Event};
use clap::{Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf",
    about = "Butterfork — fork, build, install, and improve open source software",
    version,
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: BfCommand,
}

#[derive(Subcommand)]
enum BfCommand {
    /// Fork, build, and install an OSS project (the Phase 0 core loop)
    Install {
        /// Project slug (e.g. "ripgrep") or upstream URL
        slug: String,
        /// Override the local clone destination
        #[arg(long)]
        dest: Option<String>,
        /// Skip forking — clone upstream directly (useful without a forge account)
        #[arg(long, env = "BF_NO_FORK")]
        no_fork: bool,
        /// Build in debug mode instead of release
        #[arg(long)]
        debug: bool,
    },
    /// Submit a natural-language change request to the agent for a project (Phase 1)
    Request {
        slug: String,
        description: String,
    },
    /// Open an upstream PR for a completed change (Phase 1)
    Submit { slug: String },
    /// Scaffold a new OSS project from an idea
    New {
        path: String,
        #[arg(long, short = 'd')]
        description: String,
        #[arg(long)]
        spec: Option<String>,
        #[arg(long, default_value = "design-doc", value_parser = ["hello-world", "poc", "design-doc"])]
        mode: String,
        #[arg(long)]
        language: Option<String>,
    },
    /// Check that all required bf-* components are installed and healthy
    Doctor,
    /// Emergency recovery: list or activate a previous install generation
    Rescue {
        #[command(subcommand)]
        cmd: RescueCommand,
    },
    /// Discover bf-* components on PATH and print their --help
    HelpAll,
}

#[derive(Subcommand)]
enum RescueCommand {
    List { slug: String },
    Activate { slug: String, generation_id: String },
}

// ── low-level process helpers ─────────────────────────────────────────────────

/// Run a component binary and inherit its stdout/stderr (fire-and-forget style).
fn spawn_inherit(bin: &str, args: &[&str]) -> Result<std::process::ExitStatus> {
    Command::new(bin)
        .args(args)
        .status()
        .with_context(|| format!("launching {bin}"))
}

/// Run a component binary and CAPTURE its stdout (for NDJSON parsing).
/// Stderr is still inherited so the user sees progress messages live.
fn spawn_capture(bin: &str, args: &[&str]) -> Result<std::process::Output> {
    Command::new(bin)
        .args(args)
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("launching {bin}"))
}

/// Parse NDJSON lines from a byte slice, yield each successfully parsed Event.
fn parse_events(stdout: &[u8]) -> Vec<Event> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Event>(line).ok())
        .collect()
}

/// Extract a field from a catalog entry emitted as a plain JSON object on stdout.
fn extract_upstream_url(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|v| v["upstream_url"].as_str().map(str::to_owned))
}

/// Extract the fork URL from the events emitted by `bf-forge fork`.
fn extract_fork_url(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::ForkCreated { fork_url } => Some(fork_url.clone()),
        _ => None,
    })
}

/// Extract the manifest path from the events emitted by `bf-build run`.
fn extract_manifest_path(events: &[Event]) -> Option<String> {
    events.iter().find_map(|e| match e {
        Event::BuildComplete { manifest_path } => Some(manifest_path.clone()),
        _ => None,
    })
}

// ── install pipeline ──────────────────────────────────────────────────────────

fn install(slug: &str, dest_override: Option<String>, no_fork: bool, release: bool) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME")
        .unwrap_or_else(|_| format!("{home}/.butterfork"));

    // ── step 1: resolve upstream URL from catalog ─────────────────────────────
    eprintln!("bf: step 1/5 — catalog lookup for '{slug}'");
    let cat_out = spawn_capture("bf-catalog", &["show", slug])?;
    if !cat_out.status.success() {
        // Treat the slug as a raw URL if it starts with https://.
        if slug.starts_with("https://") || slug.starts_with("http://") {
            eprintln!("bf: '{slug}' not in catalog; treating as upstream URL");
        } else {
            eprintln!(
                "bf: '{slug}' not found — add it first with `bf-catalog add <url>` or \
                 pass a full URL directly"
            );
            std::process::exit(bf_common::exit::NOINPUT);
        }
    }

    let upstream_url = extract_upstream_url(&cat_out.stdout)
        .unwrap_or_else(|| slug.to_owned());
    eprintln!("bf: upstream: {upstream_url}");

    // ── step 2: fork (or skip) ────────────────────────────────────────────────
    let fork_url = if no_fork {
        eprintln!("bf: step 2/5 — skipping fork (--no-fork)");
        upstream_url.clone()
    } else {
        eprintln!("bf: step 2/5 — forking on GitHub");
        // If BF_NO_FORK is set in env, bf-forge-github will also skip the fork.
        let forge_out = spawn_capture("bf-forge", &["fork", &upstream_url])?;
        if !forge_out.status.success() {
            anyhow::bail!("bf-forge fork failed");
        }
        let events = parse_events(&forge_out.stdout);
        extract_fork_url(&events).with_context(|| {
            "bf-forge fork did not emit a fork URL — is `gh` installed and authenticated?"
        })?
    };
    eprintln!("bf: fork: {fork_url}");

    // ── step 3: clone ─────────────────────────────────────────────────────────
    let project_slug = slug_from_url(&fork_url);
    let dest = dest_override
        .unwrap_or_else(|| format!("{bf_home}/repos/{project_slug}"));

    if std::path::Path::new(&dest).exists() {
        eprintln!("bf: step 3/5 — destination already exists, pulling latest");
        let status = Command::new("git")
            .current_dir(&dest)
            .args(["pull", "--ff-only"])
            .status()?;
        if !status.success() {
            eprintln!("bf: git pull failed — continuing with existing checkout");
        }
    } else {
        eprintln!("bf: step 3/5 — cloning {fork_url} → {dest}");
        let status = spawn_inherit("bf-forge", &["clone", &fork_url, &dest])?;
        if !status.success() {
            anyhow::bail!("bf-forge clone failed");
        }
    }

    // ── step 4: build ─────────────────────────────────────────────────────────
    eprintln!("bf: step 4/5 — building");
    let mut build_args = vec!["run", dest.as_str()];
    if release {
        build_args.push("--release");
    }
    let build_out = spawn_capture("bf-build", &build_args)?;
    if !build_out.status.success() {
        anyhow::bail!("bf-build run failed");
    }
    let build_events = parse_events(&build_out.stdout);
    let manifest_path = extract_manifest_path(&build_events)
        .unwrap_or_else(|| format!("{dest}/target/bf-artifact-manifest.json"));
    eprintln!("bf: manifest: {manifest_path}");

    // ── step 5: install generation ────────────────────────────────────────────
    eprintln!("bf: step 5/5 — installing generation");
    let add_status = spawn_inherit("bf-install", &["add", &project_slug, &manifest_path])?;
    if !add_status.success() {
        anyhow::bail!("bf-install add failed");
    }
    let act_status = spawn_inherit("bf-install", &["activate", &project_slug, "latest"])?;
    if !act_status.success() {
        anyhow::bail!("bf-install activate failed");
    }

    eprintln!("bf: '{project_slug}' installed — binaries under {bf_home}/bin/");
    eprintln!("bf: add {bf_home}/bin to your PATH if not already there");
    eprintln!(
        "bf: to roll back: bf rescue activate {project_slug} <previous-generation-id>"
    );
    emit(&Event::InstallComplete {
        project: project_slug.clone(),
        generation_id: "latest".to_owned(),
        bin_dir: format!("{bf_home}/bin"),
    });

    Ok(())
}

/// Derive a clean slug from a URL (e.g. "https://github.com/foo/bar.git" → "bar").
fn slug_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

// ── main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        BfCommand::Install {
            slug,
            dest,
            no_fork,
            debug,
        } => {
            install(&slug, dest, no_fork, !debug)?;
        }

        BfCommand::Request { slug, description } => {
            eprintln!("bf: request for '{slug}': {description}");
            eprintln!("bf: agent loop not yet implemented (Phase 1)");
            std::process::exit(bf_common::exit::UNAVAILABLE);
        }

        BfCommand::Submit { slug } => {
            eprintln!("bf: PR submission for '{slug}' not yet implemented (Phase 1)");
            std::process::exit(bf_common::exit::UNAVAILABLE);
        }

        BfCommand::New {
            path,
            description,
            spec,
            mode,
            language,
        } => {
            let mut args = vec![
                "new".to_owned(),
                path,
                "--description".to_owned(),
                description,
                "--mode".to_owned(),
                mode,
            ];
            if let Some(s) = spec {
                args.extend(["--spec".to_owned(), s]);
            }
            if let Some(l) = language {
                args.extend(["--language".to_owned(), l]);
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let status = spawn_inherit("bf-scaffold", &refs)?;
            std::process::exit(status.code().unwrap_or(1));
        }

        BfCommand::Doctor => {
            eprintln!("bf: checking component health");
            let components = [
                "bf-catalog",
                "bf-forge",
                "bf-forge-github",
                "bf-build",
                "bf-build-cargo",
                "bf-sandbox",
                "bf-install",
                "bf-index",
                "bf-agent",
                "bf-scaffold",
            ];
            let mut all_ok = true;
            for comp in &components {
                match Command::new(comp).arg("--version").output() {
                    Ok(out) if out.status.success() => {
                        let v = String::from_utf8_lossy(&out.stdout);
                        eprintln!("  [ok]      {comp}: {}", v.trim());
                    }
                    _ => {
                        eprintln!("  [missing] {comp}");
                        all_ok = false;
                    }
                }
            }
            if !all_ok {
                eprintln!("bf: one or more components are missing");
                eprintln!(
                    "bf: install with `cargo install --path <crate>` or use the fat binary"
                );
                std::process::exit(bf_common::exit::UNAVAILABLE);
            } else {
                eprintln!("bf: all components OK");
            }
        }

        BfCommand::Rescue { cmd } => match cmd {
            RescueCommand::List { slug } => {
                let status = spawn_inherit("bf-install", &["list", &slug])?;
                std::process::exit(status.code().unwrap_or(1));
            }
            RescueCommand::Activate {
                slug,
                generation_id,
            } => {
                let status =
                    spawn_inherit("bf-install", &["activate", &slug, &generation_id])?;
                std::process::exit(status.code().unwrap_or(1));
            }
        },

        BfCommand::HelpAll => {
            let path_var = std::env::var("PATH").unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for dir in path_var.split(':') {
                let dir_path = std::path::Path::new(dir);
                let Ok(entries) = std::fs::read_dir(dir_path) else {
                    continue;
                };
                let mut names: Vec<_> = entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("bf-"))
                    .collect();
                names.sort_by_key(|e| e.file_name());
                for entry in names {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if seen.insert(name.clone()) {
                        println!("\n=== {name} ===");
                        let _ = Command::new(entry.path()).arg("--help").status();
                    }
                }
            }
        }
    }

    Ok(())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_from_url_strips_dotgit() {
        assert_eq!(slug_from_url("https://github.com/BurntSushi/ripgrep"), "ripgrep");
        assert_eq!(slug_from_url("https://github.com/BurntSushi/ripgrep.git"), "ripgrep");
        assert_eq!(slug_from_url("https://github.com/sharkdp/fd/"), "fd");
    }

    #[test]
    fn parse_events_handles_mixed_lines() {
        let stdout = b"{\"type\":\"fork-created\",\"fork_url\":\"https://github.com/user/repo\"}\nnot-json\n{\"type\":\"done\",\"exit_code\":0}\n";
        let events = parse_events(stdout);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], Event::ForkCreated { fork_url } if fork_url == "https://github.com/user/repo"));
    }

    #[test]
    fn extract_fork_url_finds_event() {
        let events = vec![
            Event::Message { text: "forking".to_owned() },
            Event::ForkCreated { fork_url: "https://github.com/user/rg".to_owned() },
            Event::Done { exit_code: 0 },
        ];
        assert_eq!(
            extract_fork_url(&events),
            Some("https://github.com/user/rg".to_owned())
        );
    }

    #[test]
    fn extract_manifest_path_finds_event() {
        let events = vec![
            Event::BuildComplete { manifest_path: "/tmp/bf-artifact-manifest.json".to_owned() },
        ];
        assert_eq!(
            extract_manifest_path(&events),
            Some("/tmp/bf-artifact-manifest.json".to_owned())
        );
    }
}
