use anyhow::{Context, Result};
use bf_common::{emit, Event};
use clap::{Args, Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-forge-github",
    about = "GitHub backend for bf-forge: thin wrapper over the gh CLI",
    long_about = "Requires `gh` on PATH and `gh auth login` to have been run.\n\
                  Set BF_GITHUB_USER to skip the `gh api user` round-trip.\n\
                  Set BF_NO_FORK=1 to clone upstream directly without forking\n\
                  (useful for testing or when you already own the repo).",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: ForgeCommand,
}

#[derive(Subcommand)]
enum ForgeCommand {
    /// Fork an upstream GitHub repository to the authenticated user's account
    Fork {
        /// Upstream repository URL (must be on github.com)
        upstream_url: String,
    },
    /// Clone a repository to a local path
    Clone {
        /// Repository URL to clone
        fork_url: String,
        /// Destination directory
        dest: String,
    },
    /// Issue management
    Issue {
        #[command(subcommand)]
        cmd: IssueCommand,
    },
    /// Pull request management
    Pr {
        #[command(subcommand)]
        cmd: PrCommand,
    },
}

#[derive(Subcommand)]
enum IssueCommand {
    Open(IssueOpenArgs),
}

#[derive(Args)]
struct IssueOpenArgs {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    title: String,
    #[arg(long)]
    body: String,
}

#[derive(Subcommand)]
enum PrCommand {
    Open(PrOpenArgs),
    Status { url: String },
    Watch { url: String },
}

#[derive(Args)]
struct PrOpenArgs {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    head: String,
    #[arg(long)]
    base: String,
    #[arg(long)]
    title: String,
    #[arg(long)]
    body: String,
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn require_gh() -> Result<()> {
    let ok = Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!(
            "gh CLI not found on PATH — install from https://cli.github.com/ and run `gh auth login`"
        );
    }
    Ok(())
}

/// Return the slug `owner/repo` from a GitHub URL.
fn github_slug(url: &str) -> Option<String> {
    // Handles https://github.com/owner/repo and https://github.com/owner/repo.git
    let stripped = url
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<&str> = stripped.splitn(2, "github.com/").collect();
    parts.get(1).map(|s| s.to_string())
}

/// Return the repo name portion of a URL or slug.
fn repo_name(url: &str) -> String {
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

/// Determine the authenticated GitHub username.
/// Prefers BF_GITHUB_USER env var to skip the API call.
fn github_username() -> Result<String> {
    if let Ok(u) = std::env::var("BF_GITHUB_USER") {
        return Ok(u);
    }
    let out = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .context("running `gh api user`")?;
    if !out.status.success() {
        anyhow::bail!(
            "could not determine GitHub username — run `gh auth login` first\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn gh_status(args: &[&str]) -> Result<std::process::ExitStatus> {
    eprintln!("bf-forge-github: gh {}", args.join(" "));
    Ok(Command::new("gh").args(args).status()?)
}

// ── pre-flight gate ───────────────────────────────────────────────────────────

fn preflight_pr(repo: &str, head: &str) -> Result<()> {
    eprintln!("bf-forge-github: pre-flight checks for {repo}@{head}");
    // TODO (Phase 1): run full gate — tests, lint, fmt, CONTRIBUTING.md,
    // CLA/DCO detection, anti-spam heuristics (giant diff, whitespace churn).
    eprintln!("bf-forge-github: [stub] pre-flight not yet implemented");
    Ok(())
}

// ── main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        ForgeCommand::Fork { upstream_url } => {
            // BF_NO_FORK=1 is used in tests and for repos the user already owns.
            if std::env::var("BF_NO_FORK").as_deref() == Ok("1") {
                eprintln!("bf-forge-github: BF_NO_FORK=1, skipping fork; treating upstream as fork");
                let fork_url = upstream_url.clone();
                emit(&Event::ForkCreated { fork_url });
                emit(&Event::Done { exit_code: 0 });
                return Ok(());
            }

            require_gh()?;
            eprintln!("bf-forge-github: forking {upstream_url}");

            // `gh repo fork` forks and prints the fork URL.
            // --clone=false: we clone separately so we control the destination.
            let status = Command::new("gh")
                .args(["repo", "fork", &upstream_url, "--clone=false"])
                .status()
                .context("running `gh repo fork`")?;

            if !status.success() {
                anyhow::bail!("`gh repo fork` failed with status {status}");
            }

            // Construct the fork URL from the authenticated username + repo name.
            let username = github_username()?;
            let name = repo_name(&upstream_url);
            let fork_url = format!("https://github.com/{username}/{name}");

            eprintln!("bf-forge-github: fork ready at {fork_url}");
            emit(&Event::ForkCreated {
                fork_url: fork_url.clone(),
            });
            emit(&Event::Done { exit_code: 0 });
        }

        ForgeCommand::Clone { fork_url, dest } => {
            eprintln!("bf-forge-github: cloning {fork_url} → {dest}");
            if let Some(parent) = std::path::Path::new(&dest).parent() {
                std::fs::create_dir_all(parent)?;
            }
            let status = Command::new("git")
                .args(["clone", "--", &fork_url, &dest])
                .status()
                .context("running `git clone`")?;
            if !status.success() {
                anyhow::bail!("`git clone` failed with status {status}");
            }
            emit(&Event::Message {
                text: format!("Cloned to {dest}"),
            });
            emit(&Event::Done { exit_code: 0 });
        }

        ForgeCommand::Issue {
            cmd: IssueCommand::Open(args),
        } => {
            require_gh()?;
            let slug = github_slug(&args.repo).unwrap_or(args.repo.clone());
            eprintln!("bf-forge-github: opening issue on {slug}");
            let status = gh_status(&[
                "issue", "create",
                "--repo", &slug,
                "--title", &args.title,
                "--body", &args.body,
            ])?;
            std::process::exit(status.code().unwrap_or(1));
        }

        ForgeCommand::Pr { cmd } => match cmd {
            PrCommand::Open(args) => {
                require_gh()?;
                let slug = github_slug(&args.repo).unwrap_or(args.repo.clone());
                preflight_pr(&slug, &args.head)?;
                let status = gh_status(&[
                    "pr", "create",
                    "--repo", &slug,
                    "--head", &args.head,
                    "--base", &args.base,
                    "--title", &args.title,
                    "--body", &args.body,
                ])?;
                std::process::exit(status.code().unwrap_or(1));
            }
            PrCommand::Status { url } => {
                require_gh()?;
                let status = gh_status(&["pr", "view", &url])?;
                std::process::exit(status.code().unwrap_or(1));
            }
            PrCommand::Watch { url } => {
                require_gh()?;
                eprintln!("bf-forge-github: watching {url} (Phase 1)");
                // TODO: poll GitHub API and emit Event::Message for each new event.
                emit(&Event::Message {
                    text: format!("PR watch not yet implemented: {url}"),
                });
                std::process::exit(bf_common::exit::UNAVAILABLE);
            }
        },
    }

    Ok(())
}
