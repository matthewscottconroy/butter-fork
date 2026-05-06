use anyhow::{Context, Result};
use bf_common::{emit, Event, PolicyConfig};
use clap::{Args, Parser, Subcommand};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-forge-gitlab",
    about = "GitLab backend for bf-forge: thin wrapper over the glab CLI",
    long_about = "Requires `glab` on PATH and `glab auth login` to have been run.\n\
                  Set BF_GITLAB_USER to skip the `glab api user` round-trip.\n\
                  Set BF_NO_FORK=1 to clone upstream directly without forking.\n\
                  Note: GitLab uses 'merge requests' (MR); this adapter maps\n\
                  the bf-forge 'pr' contract to MRs transparently.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: ForgeCommand,
}

#[derive(Subcommand)]
enum ForgeCommand {
    /// Fork an upstream GitLab repository
    Fork { upstream_url: String },
    /// Clone a repository to a local path
    Clone { fork_url: String, dest: String },
    /// Issue management
    Issue {
        #[command(subcommand)]
        cmd: IssueCommand,
    },
    /// Merge-request (PR) management
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

fn require_glab() -> Result<()> {
    let ok = Command::new("glab")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!(
            "glab CLI not found on PATH — install from https://gitlab.com/gitlab-org/cli \
             and run `glab auth login`"
        );
    }
    Ok(())
}

fn gitlab_slug(url: &str) -> Option<String> {
    let stripped = url.trim_end_matches('/').trim_end_matches(".git");
    stripped
        .split_once("gitlab.com/")
        .map(|(_, s)| s.to_owned())
}

fn repo_name(url: &str) -> String {
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

fn gitlab_username() -> Result<String> {
    if let Ok(u) = std::env::var("BF_GITLAB_USER") {
        return Ok(u);
    }
    let out = Command::new("glab")
        .args(["api", "user", "--field", "username"])
        .output()
        .context("running `glab api user`")?;
    if !out.status.success() {
        anyhow::bail!(
            "could not determine GitLab username — run `glab auth login` first\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // glab returns the raw value; trim quotes if JSON string.
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(raw.trim_matches('"').to_owned())
}

// ── pre-flight helpers ────────────────────────────────────────────────────────

fn bf_home() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"))
}

fn find_local_checkout(slug: &str) -> Option<String> {
    let name = slug.rsplit('/').next()?;
    let path = format!("{}/repos/{}", bf_home(), name);
    Path::new(&path).exists().then_some(path)
}

fn load_policy(slug: &str) -> PolicyConfig {
    let name = slug.rsplit('/').next().unwrap_or(slug);
    let path = format!("{}/pr-policy/{}.toml", bf_home(), name);
    if let Ok(s) = std::fs::read_to_string(&path) {
        match toml::from_str::<PolicyConfig>(&s) {
            Ok(cfg) => {
                eprintln!("bf-forge-gitlab: loaded PR policy from {path}");
                return cfg;
            }
            Err(e) => eprintln!("bf-forge-gitlab: warning: could not parse {path}: {e}"),
        }
    }
    PolicyConfig::default()
}

fn detect_default_branch(local_path: &str) -> String {
    let out = Command::new("git")
        .args(["remote", "show", "origin"])
        .current_dir(local_path)
        .output();
    if let Ok(o) = out {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("HEAD branch:") {
                let branch = rest.trim().to_owned();
                if !branch.is_empty() && branch != "(unknown)" {
                    return branch;
                }
            }
        }
    }
    "main".to_owned()
}

fn parse_shortstat_total(output: &str) -> u64 {
    output
        .split_whitespace()
        .filter_map(|w| w.parse::<u64>().ok())
        .sum()
}

fn preflight_mr(slug: &str, head: &str) -> Result<()> {
    eprintln!("bf-forge-gitlab: pre-flight checks for {slug}@{head}");
    let local = find_local_checkout(slug);
    let policy = load_policy(slug);
    let mut warnings: Vec<String> = Vec::new();
    let errors: Vec<String> = Vec::new();

    let default_branch = local
        .as_deref()
        .map(detect_default_branch)
        .unwrap_or_else(|| "main".to_owned());
    let range = format!("origin/{default_branch}..HEAD");

    // ── 1. DCO check ─────────────────────────────────────────────────────────
    if policy.require_dco {
        if let Some(ref local_path) = local {
            let log = Command::new("git")
                .args(["log", &range, "--format=%B---COMMIT---"])
                .current_dir(local_path)
                .output();
            if let Ok(out) = log {
                let text = String::from_utf8_lossy(&out.stdout);
                let missing = text
                    .split("---COMMIT---")
                    .filter(|msg| {
                        let m = msg.trim();
                        !m.is_empty() && !m.contains("Signed-off-by:")
                    })
                    .count();
                if missing > 0 {
                    warnings.push(format!(
                        "{missing} commit(s) missing DCO Signed-off-by — \
                         amend with `git commit -s --amend`"
                    ));
                } else {
                    eprintln!("bf-forge-gitlab: [ok] all commits have DCO Signed-off-by");
                }
            }
        }
    }

    // ── 2. Diff size ─────────────────────────────────────────────────────────
    if let Some(ref local_path) = local {
        let stat = Command::new("git")
            .args(["diff", &range, "--shortstat"])
            .current_dir(local_path)
            .output();
        if let Ok(out) = stat {
            let total = parse_shortstat_total(&String::from_utf8_lossy(&out.stdout));
            if total > policy.max_diff_lines {
                warnings.push(format!(
                    "Large diff (~{total} changes, limit {}) — \
                     consider splitting into smaller MRs",
                    policy.max_diff_lines
                ));
            } else {
                eprintln!("bf-forge-gitlab: [ok] diff size within limit ({total})");
            }
        }
    }

    // ── report ────────────────────────────────────────────────────────────────
    for w in &warnings {
        eprintln!("bf-forge-gitlab: [warn] {w}");
    }
    for e in &errors {
        eprintln!("bf-forge-gitlab: [error] {e}");
    }
    if !errors.is_empty() {
        anyhow::bail!("pre-flight failed ({} error(s))", errors.len());
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        ForgeCommand::Fork { upstream_url } => {
            if std::env::var("BF_NO_FORK").as_deref() == Ok("1") {
                eprintln!("bf-forge-gitlab: BF_NO_FORK=1, skipping fork");
                emit(&Event::ForkCreated {
                    fork_url: upstream_url,
                });
                emit(&Event::Done { exit_code: 0 });
                return Ok(());
            }
            require_glab()?;
            eprintln!("bf-forge-gitlab: forking {upstream_url}");
            let status = Command::new("glab")
                .args(["repo", "fork", &upstream_url, "--clone=false"])
                .status()
                .context("glab repo fork")?;
            if !status.success() {
                anyhow::bail!("glab repo fork failed");
            }
            let username = gitlab_username()?;
            let name = repo_name(&upstream_url);
            let fork_url = format!("https://gitlab.com/{username}/{name}");
            eprintln!("bf-forge-gitlab: fork ready at {fork_url}");
            emit(&Event::ForkCreated { fork_url });
            emit(&Event::Done { exit_code: 0 });
        }

        ForgeCommand::Clone { fork_url, dest } => {
            eprintln!("bf-forge-gitlab: cloning {fork_url} → {dest}");
            if let Some(parent) = std::path::Path::new(&dest).parent() {
                std::fs::create_dir_all(parent)?;
            }
            let status = Command::new("git")
                .args(["clone", "--", &fork_url, &dest])
                .status()
                .context("git clone")?;
            if !status.success() {
                anyhow::bail!("git clone failed");
            }
            emit(&Event::Message {
                text: format!("Cloned to {dest}"),
            });
            emit(&Event::Done { exit_code: 0 });
        }

        ForgeCommand::Issue {
            cmd: IssueCommand::Open(args),
        } => {
            require_glab()?;
            let slug = gitlab_slug(&args.repo).unwrap_or(args.repo.clone());
            eprintln!("bf-forge-gitlab: opening issue on {slug}");
            let out = Command::new("glab")
                .args([
                    "issue",
                    "create",
                    "--repo",
                    &slug,
                    "--title",
                    &args.title,
                    "--description",
                    &args.body,
                    "--yes",
                ])
                .stderr(std::process::Stdio::inherit())
                .output()
                .context("glab issue create")?;
            if !out.status.success() {
                std::process::exit(out.status.code().unwrap_or(1));
            }
            let issue_url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !issue_url.is_empty() {
                eprintln!("bf-forge-gitlab: issue created: {issue_url}");
                emit(&Event::IssueCreated { issue_url });
            }
            emit(&Event::Done { exit_code: 0 });
        }

        ForgeCommand::Pr { cmd } => match cmd {
            PrCommand::Open(args) => {
                require_glab()?;
                let slug = gitlab_slug(&args.repo).unwrap_or(args.repo.clone());
                preflight_mr(&slug, &args.head)?;
                eprintln!(
                    "bf-forge-gitlab: opening MR on {slug} ({} → {})",
                    args.head, args.base
                );
                let out = Command::new("glab")
                    .args([
                        "mr",
                        "create",
                        "--repo",
                        &slug,
                        "--source-branch",
                        &args.head,
                        "--target-branch",
                        &args.base,
                        "--title",
                        &args.title,
                        "--description",
                        &args.body,
                        "--yes",
                    ])
                    .stderr(std::process::Stdio::inherit())
                    .output()
                    .context("glab mr create")?;
                if !out.status.success() {
                    std::process::exit(out.status.code().unwrap_or(1));
                }
                let pr_url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !pr_url.is_empty() {
                    eprintln!("bf-forge-gitlab: MR created: {pr_url}");
                    emit(&Event::PrCreated { pr_url });
                }
                emit(&Event::Done { exit_code: 0 });
            }
            PrCommand::Status { url } => {
                require_glab()?;
                let status = Command::new("glab").args(["mr", "view", &url]).status()?;
                std::process::exit(status.code().unwrap_or(1));
            }
            PrCommand::Watch { url } => {
                require_glab()?;
                eprintln!("bf-forge-gitlab: watching {url}");
                eprintln!("bf-forge-gitlab: will time out after 4 hours");
                const MAX_POLLS: u32 = 480; // 480 * 30s = 4 hours
                for poll in 0..MAX_POLLS {
                    let out = Command::new("glab")
                        .args(["mr", "view", &url, "--output", "json"])
                        .output();
                    match out {
                        Ok(o) if o.status.success() => {
                            let v: serde_json::Value =
                                serde_json::from_slice(&o.stdout).unwrap_or_default();
                            let state = v["state"].as_str().unwrap_or("unknown");
                            emit(&Event::Message {
                                text: format!("MR state: {state}"),
                            });
                            if matches!(state, "merged" | "closed") {
                                emit(&Event::Done { exit_code: 0 });
                                return Ok(());
                            }
                        }
                        _ => {
                            emit(&Event::Message {
                                text: "could not poll MR state".to_owned(),
                            });
                        }
                    }
                    if poll + 1 < MAX_POLLS {
                        std::thread::sleep(std::time::Duration::from_secs(30));
                    }
                }
                eprintln!(
                    "bf-forge-gitlab: watch timed out after 4 hours — \
                     check MR status manually"
                );
                std::process::exit(bf_common::exit::TEMPFAIL);
            }
        },
    }
    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
