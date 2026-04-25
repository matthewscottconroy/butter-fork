use anyhow::{Context, Result};
use bf_common::{emit, AiFooterPolicy, Event, PolicyConfig};
use clap::{Args, Parser, Subcommand};
use std::path::Path;
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

/// Attempt to find the local checkout for a repo slug under BF_HOME.
fn find_local_checkout(repo_slug: &str) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    let name = repo_slug.rsplit('/').next()?;
    let path = format!("{bf_home}/repos/{name}");
    Path::new(&path).exists().then_some(path)
}

/// Load per-project policy from `~/.butterfork/pr-policy/<slug>.toml`.
/// Falls back to `PolicyConfig::default()` if the file is absent or malformed.
fn load_policy(repo_slug: &str) -> PolicyConfig {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    // Slug may be "owner/repo" — use just the repo name for the filename.
    let name = repo_slug.rsplit('/').next().unwrap_or(repo_slug);
    let path = format!("{bf_home}/pr-policy/{name}.toml");

    #[derive(serde::Deserialize, Default)]
    struct PolicyFile { #[serde(default)] policy: PolicyConfig }

    if let Ok(s) = std::fs::read_to_string(&path) {
        match toml::from_str::<PolicyFile>(&s) {
            Ok(f) => {
                eprintln!("bf-forge-github: loaded PR policy from {path}");
                return f.policy;
            }
            Err(e) => eprintln!("bf-forge-github: warning: could not parse {path}: {e}"),
        }
    }
    PolicyConfig::default()
}

/// Detect SPDX license via GitHub API. Returns (spdx_id, is_copyleft).
fn detect_spdx(repo_slug: &str) -> Option<(String, bool)> {
    let api_path = format!("repos/{repo_slug}/license");
    let out = Command::new("gh")
        .args(["api", &api_path, "--jq", ".license.spdxId"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let spdx = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if spdx.is_empty() || spdx == "NOASSERTION" {
        return None;
    }
    let s = spdx.to_uppercase();
    let is_copyleft = s.contains("GPL") || s.contains("AGPL") || s.contains("LGPL")
        || s.contains("EUPL") || s.contains("OSL") || s.contains("MPL");
    Some((spdx, is_copyleft))
}

/// Sum numeric tokens from `git diff --shortstat` output.
fn parse_shortstat_total(output: &str) -> u64 {
    output
        .split_whitespace()
        .filter_map(|w| w.parse::<u64>().ok())
        .sum()
}

fn preflight_pr(repo_slug: &str, head: &str) -> Result<()> {
    eprintln!("bf-forge-github: pre-flight checks for {repo_slug}@{head}");
    let local = find_local_checkout(repo_slug);
    let policy = load_policy(repo_slug);
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // ── 1. CONTRIBUTING.md in target repo ────────────────────────────────────
    let api_path = format!("repos/{repo_slug}/contents/CONTRIBUTING.md");
    match Command::new("gh").args(["api", &api_path]).output() {
        Ok(out) if out.status.success() => {
            eprintln!("bf-forge-github: [ok] CONTRIBUTING.md found");
        }
        _ => {
            warnings.push(
                "No CONTRIBUTING.md found in target repo — \
                 review contribution guidelines on the project website"
                    .to_owned(),
            );
        }
    }

    // ── 2. SPDX license check ─────────────────────────────────────────────────
    if let Some((spdx, copyleft)) = detect_spdx(repo_slug) {
        if copyleft {
            warnings.push(format!(
                "License is {spdx} (copyleft) — redistribution of your modifications \
                 may require releasing them under the same terms"
            ));
        } else {
            eprintln!("bf-forge-github: [ok] license: {spdx} (permissive)");
        }
    }

    // ── 3. DCO: Signed-off-by on every commit ────────────────────────────────
    if policy.require_dco {
        if let Some(ref local_path) = local {
            let log = Command::new("git")
                .args(["log", "origin/main..HEAD", "--format=%B---COMMIT---"])
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
                    eprintln!("bf-forge-github: [ok] all commits have DCO Signed-off-by");
                }
            }
        }
    }

    // ── 4. Diff size + whitespace churn ──────────────────────────────────────
    if let Some(ref local_path) = local {
        let stat = Command::new("git")
            .args(["diff", "origin/main..HEAD", "--shortstat"])
            .current_dir(local_path)
            .output();
        if let Ok(out) = stat {
            let total = parse_shortstat_total(&String::from_utf8_lossy(&out.stdout));
            if total > policy.max_diff_lines {
                warnings.push(format!(
                    "Large diff (~{total} changes, limit {}) — \
                     consider splitting into smaller PRs",
                    policy.max_diff_lines
                ));
            } else {
                eprintln!("bf-forge-github: [ok] diff size within limit ({total})");
            }

            // Whitespace churn: compare full diff vs ignore-whitespace diff.
            if policy.warn_whitespace_churn && total > 0 {
                let ws_stat = Command::new("git")
                    .args(["diff", "origin/main..HEAD", "--ignore-all-space", "--shortstat"])
                    .current_dir(local_path)
                    .output();
                if let Ok(ws_out) = ws_stat {
                    let ws_total = parse_shortstat_total(
                        &String::from_utf8_lossy(&ws_out.stdout),
                    );
                    // If ignoring whitespace drops >80% of the diff, flag it.
                    if total > 10 && ws_total < total / 5 {
                        warnings.push(format!(
                            "Diff appears to be mostly whitespace changes \
                             ({total} total, {ws_total} substantive) — \
                             strip whitespace-only changes to reduce reviewer noise"
                        ));
                    }
                }
            }
        }
    }

    // ── 5. Build-system tests ─────────────────────────────────────────────────
    if policy.require_tests {
        if let Some(ref local_path) = local {
            let has_cargo = Path::new(local_path).join("Cargo.toml").exists();
            if has_cargo {
                eprintln!("bf-forge-github: running `cargo test --quiet`");
                let test = Command::new("cargo")
                    .args(["test", "--quiet"])
                    .current_dir(local_path)
                    .status();
                match test {
                    Ok(s) if s.success() => {
                        eprintln!("bf-forge-github: [ok] cargo test passed");
                    }
                    Ok(_) => errors.push(
                        "cargo test failed — fix failing tests before opening a PR".to_owned(),
                    ),
                    Err(e) => warnings.push(format!("could not run cargo test: {e}")),
                }
            }
        }
    }

    // ── 6. Format check ──────────────────────────────────────────────────────
    if policy.require_format_check {
        if let Some(ref local_path) = local {
            if Path::new(local_path).join("Cargo.toml").exists() {
                let fmt = Command::new("cargo")
                    .args(["fmt", "--all", "--", "--check"])
                    .current_dir(local_path)
                    .status();
                match fmt {
                    Ok(s) if s.success() => {
                        eprintln!("bf-forge-github: [ok] cargo fmt check passed");
                    }
                    Ok(_) => warnings.push(
                        "cargo fmt check failed — run `cargo fmt --all` before opening a PR"
                            .to_owned(),
                    ),
                    Err(e) => warnings.push(format!("could not run cargo fmt: {e}")),
                }
            }
        }
    }

    // ── 7. AI-assistance footer notice ───────────────────────────────────────
    match policy.ai_footer {
        AiFooterPolicy::Include => {
            eprintln!("bf-forge-github: [note] AI-assistance footer will be included in PR body");
        }
        AiFooterPolicy::Exclude => {
            eprintln!(
                "bf-forge-github: [note] AI-assistance footer suppressed per project policy"
            );
        }
        AiFooterPolicy::Ask => {
            eprintln!(
                "bf-forge-github: [note] AI-assistance footer: policy=ask (falling back to include)"
            );
        }
    }

    // ── report ────────────────────────────────────────────────────────────────
    for w in &warnings {
        eprintln!("bf-forge-github: [warn] {w}");
    }
    for e in &errors {
        eprintln!("bf-forge-github: [error] {e}");
    }

    if !errors.is_empty() {
        anyhow::bail!("pre-flight failed ({} error(s))", errors.len());
    }
    Ok(())
}

// ── main ─────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
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

            // SPDX license detection — warn on copyleft before user proceeds.
            if let Some(slug) = github_slug(&upstream_url) {
                if let Some((spdx, copyleft)) = detect_spdx(&slug) {
                    if copyleft {
                        eprintln!(
                            "bf-forge-github: [warn] license is {spdx} (copyleft) — \
                             redistribution of modifications may require releasing them \
                             under the same terms. Review the license before contributing."
                        );
                    } else {
                        eprintln!("bf-forge-github: [ok] license: {spdx} (permissive)");
                    }
                }
            }

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
            // Capture stdout to extract the issue URL that `gh issue create` prints.
            let out = Command::new("gh")
                .args([
                    "issue", "create",
                    "--repo", &slug,
                    "--title", &args.title,
                    "--body", &args.body,
                ])
                .stderr(std::process::Stdio::inherit())
                .output()
                .context("running `gh issue create`")?;
            if !out.status.success() {
                std::process::exit(out.status.code().unwrap_or(1));
            }
            let issue_url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !issue_url.is_empty() {
                eprintln!("bf-forge-github: issue created: {issue_url}");
                emit(&Event::IssueCreated {
                    issue_url: issue_url.clone(),
                });
            }
            emit(&Event::Done { exit_code: 0 });
        }

        ForgeCommand::Pr { cmd } => match cmd {
            PrCommand::Open(args) => {
                require_gh()?;
                let slug = github_slug(&args.repo).unwrap_or(args.repo.clone());
                preflight_pr(&slug, &args.head)?;

                // Append AI-assistance footer per project policy.
                let policy = load_policy(&slug);
                let body = if policy.ai_footer != AiFooterPolicy::Exclude
                    && std::env::var("BF_NO_AI_FOOTER").as_deref() != Ok("1")
                {
                    format!(
                        "{}\n\n---\n*This change was drafted with AI assistance \
                         via [Butterfork](https://github.com/matthewscottconroy/butter-fork). \
                         The author reviewed and is responsible for all content.*",
                        args.body
                    )
                } else {
                    args.body.clone()
                };

                // Capture stdout to get the PR URL printed by `gh pr create`.
                let out = Command::new("gh")
                    .args([
                        "pr", "create",
                        "--repo", &slug,
                        "--head", &args.head,
                        "--base", &args.base,
                        "--title", &args.title,
                        "--body", &body,
                    ])
                    .stderr(std::process::Stdio::inherit())
                    .output()
                    .context("running `gh pr create`")?;
                if !out.status.success() {
                    std::process::exit(out.status.code().unwrap_or(1));
                }
                let pr_url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !pr_url.is_empty() {
                    eprintln!("bf-forge-github: PR created: {pr_url}");
                    emit(&Event::PrCreated { pr_url });
                }
                emit(&Event::Done { exit_code: 0 });
            }
            PrCommand::Status { url } => {
                require_gh()?;
                let status = gh_status(&["pr", "view", &url])?;
                std::process::exit(status.code().unwrap_or(1));
            }
            PrCommand::Watch { url } => {
                require_gh()?;
                eprintln!("bf-forge-github: delegating PR watch to bf-daemon");
                // Prefer the daemon for persistent watching; fall back to in-process poll.
                let status = std::process::Command::new("bf-daemon")
                    .args(["watch-pr", &url])
                    .status();
                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                    Err(_) => {
                        // bf-daemon not installed — run a simple blocking poll loop.
                        eprintln!("bf-forge-github: bf-daemon not found, polling directly");
                        let mut last_state = String::new();
                        loop {
                            let out = std::process::Command::new("gh")
                                .args(["pr", "view", &url, "--json", "state,title"])
                                .output();
                            if let Ok(o) = out {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(
                                    &String::from_utf8_lossy(&o.stdout),
                                ) {
                                    let pr_state = v["state"].as_str().unwrap_or("UNKNOWN").to_owned();
                                    let title = v["title"].as_str().unwrap_or("").to_owned();
                                    if pr_state != last_state {
                                        eprintln!("bf-forge-github: PR '{title}' → {pr_state}");
                                        emit(&Event::Message {
                                            text: format!("PR '{title}' state → {pr_state}"),
                                        });
                                        last_state = pr_state.clone();
                                    }
                                    if matches!(pr_state.as_str(), "MERGED" | "CLOSED") {
                                        return Ok(());
                                    }
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_secs(60));
                        }
                    }
                }
            }
        },
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
