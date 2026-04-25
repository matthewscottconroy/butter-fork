//! Minimal bootstrap binary.
//!
//! Does exactly one thing: fork → clone → build → install Butterfork from its
//! canonical upstream repo, then exits. After this, `bf` is on PATH and every
//! future update flows through `bf`.
//!
//! Ships as a small static binary from the project's GitHub releases.
//! No plugins, no agent, no UI, no daemon.

use anyhow::{Context, Result};
use bf_common::{emit, Event};
use clap::Parser;
use std::process::Command;

const CANONICAL_REPO: &str = "https://github.com/matthewscottconroy/butter-fork";

#[derive(Parser)]
#[command(
    name = "bf-bootstrap",
    about = "Bootstrap Butterfork from source onto this machine",
    long_about = "Forks the canonical Butterfork repo to your GitHub, builds it with Cargo,\n\
                  and installs it under ~/.butterfork/. After this, use `bf` for everything.",
    version
)]
struct Cli {
    /// Override the upstream repository URL
    #[arg(long, default_value = CANONICAL_REPO)]
    upstream: String,

    /// Override the install prefix (default: ~/.butterfork)
    #[arg(long)]
    prefix: Option<String>,

    /// Skip forking — clone upstream directly (for contributors or CI)
    #[arg(long, env = "BF_NO_FORK")]
    no_fork: bool,
}

fn run_ok(bin: &str, args: &[&str]) -> Result<()> {
    eprintln!("bf-bootstrap: {bin} {}", args.join(" "));
    let status = Command::new(bin)
        .args(args)
        .status()
        .with_context(|| format!("launching {bin}"))?;
    if !status.success() {
        anyhow::bail!("{bin} exited with {status}");
    }
    Ok(())
}

/// Capture stdout (stderr inherited); return trimmed stdout string.
fn capture_stdout(bin: &str, args: &[&str]) -> Result<String> {
    eprintln!("bf-bootstrap: {bin} {}", args.join(" "));
    let out = Command::new(bin)
        .args(args)
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("launching {bin}"))?;
    if !out.status.success() {
        anyhow::bail!("{bin} exited with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Parse a `fork-created` NDJSON event and return the fork URL.
fn extract_fork_url(ndjson: &str) -> Option<String> {
    ndjson.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v["type"].as_str()? == "fork-created" {
            v["fork_url"].as_str().map(str::to_owned)
        } else {
            None
        }
    })
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    let prefix = cli
        .prefix
        .unwrap_or_else(|| format!("{home}/.butterfork"));
    let dest = format!("{prefix}/repos/butterfork");

    eprintln!("bf-bootstrap: installing Butterfork into {prefix}");

    // ── step 1: fork (or skip) ────────────────────────────────────────────────
    let clone_url = if cli.no_fork {
        eprintln!("bf-bootstrap: step 1/4 — skipping fork (--no-fork)");
        cli.upstream.clone()
    } else {
        eprintln!("bf-bootstrap: step 1/4 — fork {}", cli.upstream);
        let ndjson = capture_stdout("bf-forge-github", &["fork", &cli.upstream])?;
        if let Some(fork_url) = extract_fork_url(&ndjson) {
            eprintln!("bf-bootstrap: fork ready at {fork_url}");
            emit(&Event::ForkCreated {
                fork_url: fork_url.clone(),
            });
            fork_url
        } else {
            eprintln!(
                "bf-bootstrap: could not extract fork URL; \
                 cloning upstream directly"
            );
            cli.upstream.clone()
        }
    };

    // ── step 2: clone ─────────────────────────────────────────────────────────
    eprintln!("bf-bootstrap: step 2/4 — clone {clone_url} → {dest}");
    run_ok("git", &["clone", "--", &clone_url, &dest])?;

    // Add upstream remote when we cloned a fork.
    if clone_url != cli.upstream {
        eprintln!("bf-bootstrap: adding upstream remote");
        let _ = Command::new("git")
            .args(["remote", "add", "upstream", &cli.upstream])
            .current_dir(&dest)
            .status();
    }

    // ── step 3: build ─────────────────────────────────────────────────────────
    eprintln!("bf-bootstrap: step 3/4 — build (release)");
    run_ok("bf-build-cargo", &["run", &dest, "--release"])?;

    // ── step 4: install ───────────────────────────────────────────────────────
    eprintln!("bf-bootstrap: step 4/4 — install generation");
    let manifest = format!("{dest}/target/bf-artifact-manifest.json");
    run_ok("bf-install", &["add", "butterfork", &manifest])?;
    run_ok("bf-install", &["activate", "butterfork", "latest"])?;

    eprintln!("bf-bootstrap: done — `bf` is installed at {prefix}/bin/bf");
    eprintln!("bf-bootstrap: add {prefix}/bin to your PATH if it isn't already");
    eprintln!("bf-bootstrap: run `bf doctor` to verify all components are present");
    Ok(())
}
