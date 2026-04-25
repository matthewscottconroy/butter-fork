//! Minimal bootstrap binary — does exactly one thing:
//! fork → clone → build → install Butterfork from its canonical upstream repo,
//! then exits. After this, `bf` is on PATH and every future update goes through `bf`.
//!
//! Ships as a small static binary from the project's GitHub releases.
//! No plugins, no agent, no UI, no daemon — just bf-forge-github and bf-build-cargo.

use anyhow::Result;
use clap::Parser;
use std::process::Command;

const CANONICAL_REPO: &str = "https://github.com/matthewscottconroy/butter-fork";

#[derive(Parser)]
#[command(
    name = "bf-bootstrap",
    about = "Bootstrap Butterfork from source onto this machine",
    long_about = "Forks the canonical Butterfork repo, builds it with Cargo, and installs it.\nAfter this completes, use `bf` for all future operations.",
    version
)]
struct Cli {
    /// Override the upstream repository URL
    #[arg(long, default_value = CANONICAL_REPO)]
    upstream: String,

    /// Override the install prefix (default: ~/.butterfork)
    #[arg(long)]
    prefix: Option<String>,

    /// Skip forking — clone upstream directly (useful if you don't have a forge account)
    #[arg(long)]
    no_fork: bool,
}

fn run(bin: &str, args: &[&str]) -> Result<()> {
    eprintln!("bf-bootstrap: {bin} {}", args.join(" "));
    let status = Command::new(bin).args(args).status()?;
    if !status.success() {
        anyhow::bail!("{bin} exited with status {status}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    let prefix = cli
        .prefix
        .unwrap_or_else(|| format!("{home}/.butterfork"));
    let dest = format!("{prefix}/repos/butterfork");

    eprintln!("bf-bootstrap: installing Butterfork into {prefix}");

    if cli.no_fork {
        eprintln!("bf-bootstrap: cloning upstream (--no-fork)");
        run("git", &["clone", &cli.upstream, &dest])?;
    } else {
        eprintln!("bf-bootstrap: step 1/4 — fork");
        run("bf-forge-github", &["fork", &cli.upstream])?;

        eprintln!("bf-bootstrap: step 2/4 — clone fork");
        // bf-forge-github prints the fork URL; a real impl would capture it.
        // For bootstrap, fall back to cloning upstream if fork URL is unknown.
        run("git", &["clone", &cli.upstream, &dest])?;
    }

    eprintln!("bf-bootstrap: step 3/4 — build");
    run(
        "bf-build-cargo",
        &["run", &dest, "--release"],
    )?;

    eprintln!("bf-bootstrap: step 4/4 — install");
    let manifest = format!("{dest}/target/bf-artifact-manifest.json");
    run("bf-install", &["add", "butterfork", &manifest])?;
    run("bf-install", &["activate", "butterfork", "latest"])?;

    eprintln!("bf-bootstrap: done — `bf` is now on PATH under {prefix}/bin");
    eprintln!("bf-bootstrap: add {prefix}/bin to your PATH if it isn't already");
    Ok(())
}
