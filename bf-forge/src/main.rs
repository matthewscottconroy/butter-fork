use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-forge",
    about = "Forge operations: fork, clone, open issues and PRs across hosting backends",
    long_about = "bf-forge dispatches to a backend binary discovered on PATH by URL pattern.\n\
                  github.com URLs → bf-forge-github (wraps gh)\n\
                  gitlab.com URLs → bf-forge-gitlab (wraps glab)\n\
                  Override with BF_FORGE env var.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: ForgeCommand,
}

#[derive(Subcommand)]
enum ForgeCommand {
    /// Fork an upstream repository to the authenticated user's account
    Fork {
        /// Upstream repository URL
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
    /// Open a new issue
    Open(IssueOpenArgs),
}

#[derive(Args)]
struct IssueOpenArgs {
    /// Repository slug (owner/repo)
    #[arg(long)]
    repo: String,
    /// Issue title
    #[arg(long)]
    title: String,
    /// Issue body (markdown)
    #[arg(long)]
    body: String,
}

#[derive(Subcommand)]
enum PrCommand {
    /// Open a new pull request
    Open(PrOpenArgs),
    /// Show the current status of a PR
    Status {
        /// PR URL
        url: String,
    },
    /// Watch a PR for new events, streaming NDJSON
    Watch {
        /// PR URL
        url: String,
    },
}

#[derive(Args)]
struct PrOpenArgs {
    /// Repository slug (owner/repo)
    #[arg(long)]
    repo: String,
    /// Head branch
    #[arg(long)]
    head: String,
    /// Base branch
    #[arg(long)]
    base: String,
    /// PR title
    #[arg(long)]
    title: String,
    /// PR body (markdown)
    #[arg(long)]
    body: String,
}

/// Select the backend binary for a given URL.
fn backend_for(url: &str) -> &'static str {
    if let Ok(v) = std::env::var("BF_FORGE") {
        // SAFETY: the env var may outlive this function; leak is intentional for static lifetime.
        return Box::leak(v.into_boxed_str());
    }
    if url.contains("github.com") {
        "bf-forge-github"
    } else if url.contains("gitlab.com") {
        "bf-forge-gitlab"
    } else {
        "bf-forge-github" // default
    }
}

fn delegate(backend: &str, args: &[&str]) -> Result<()> {
    eprintln!("bf-forge: delegating to {backend}");
    let status = Command::new(backend).args(args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        ForgeCommand::Fork { upstream_url } => {
            let backend = backend_for(&upstream_url);
            delegate(backend, &["fork", &upstream_url])?;
        }

        ForgeCommand::Clone { fork_url, dest } => {
            let backend = backend_for(&fork_url);
            delegate(backend, &["clone", &fork_url, &dest])?;
        }

        ForgeCommand::Issue {
            cmd: IssueCommand::Open(args),
        } => {
            let backend = backend_for(&args.repo);
            delegate(
                backend,
                &[
                    "issue", "open",
                    "--repo", &args.repo,
                    "--title", &args.title,
                    "--body", &args.body,
                ],
            )?;
        }

        ForgeCommand::Pr { cmd } => match cmd {
            PrCommand::Open(args) => {
                let backend = backend_for(&args.repo);
                delegate(
                    backend,
                    &[
                        "pr", "open",
                        "--repo", &args.repo,
                        "--head", &args.head,
                        "--base", &args.base,
                        "--title", &args.title,
                        "--body", &args.body,
                    ],
                )?;
            }
            PrCommand::Status { url } => {
                let backend = backend_for(&url);
                delegate(backend, &["pr", "status", &url])?;
            }
            PrCommand::Watch { url } => {
                let backend = backend_for(&url);
                delegate(backend, &["pr", "watch", &url])?;
            }
        },
    }

    Ok(())
}
