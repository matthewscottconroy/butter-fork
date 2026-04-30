use anyhow::{Context, Result};
use bf_common::{emit, exit, Event, TelemetryEvent, TelemetryRecord};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
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
    Request { slug: String, description: String },
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
    /// Manage opt-in local telemetry (never transmitted automatically)
    Telemetry {
        #[command(subcommand)]
        cmd: TelemetryCommand,
    },
    /// Run an integration self-test of the installed bf-* components
    SelfTest {
        /// Run against a specific local repo path instead of the workspace default
        #[arg(long)]
        repo: Option<String>,
        /// Skip sandbox test (useful in CI without bubblewrap/podman)
        #[arg(long)]
        no_sandbox: bool,
    },
}

#[derive(Subcommand)]
enum RescueCommand {
    List { slug: String },
    Activate { slug: String, generation_id: String },
}

#[derive(Subcommand)]
enum TelemetryCommand {
    /// Show whether telemetry is enabled and how many records are stored
    Status,
    /// Enable local telemetry recording
    Enable,
    /// Disable local telemetry recording and stop new records being written
    Disable,
    /// Print all stored telemetry records as NDJSON
    Show,
    /// Delete all stored telemetry records
    Clear,
}

// ── telemetry helpers ─────────────────────────────────────────────────────────

fn telemetry_opt_in_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    PathBuf::from(format!("{bf_home}/telemetry-enabled"))
}

fn telemetry_log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    PathBuf::from(format!("{bf_home}/telemetry.jsonl"))
}

fn telemetry_enabled() -> bool {
    // Also honour BF_TELEMETRY=1 env var.
    if std::env::var("BF_TELEMETRY").as_deref() == Ok("1") {
        return true;
    }
    telemetry_opt_in_path().exists()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn record_telemetry(event: TelemetryEvent) {
    if !telemetry_enabled() {
        return;
    }
    let record = TelemetryRecord {
        timestamp: now_secs(),
        event,
    };
    let Ok(line) = serde_json::to_string(&record) else {
        return;
    };
    let path = telemetry_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

// ── self-test helpers ─────────────────────────────────────────────────────────

struct SelfTestResult {
    name: &'static str,
    passed: bool,
    note: String,
}

fn self_test_check(
    results: &mut Vec<SelfTestResult>,
    name: &'static str,
    passed: bool,
    note: impl Into<String>,
) {
    let note = note.into();
    let icon = if passed { "[ok]  " } else { "[FAIL]" };
    eprintln!(
        "bf self-test: {icon} {name}{}",
        if note.is_empty() {
            String::new()
        } else {
            format!(" — {note}")
        }
    );
    results.push(SelfTestResult { name, passed, note });
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
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));

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

    let upstream_url = extract_upstream_url(&cat_out.stdout).unwrap_or_else(|| slug.to_owned());
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
    let dest = dest_override.unwrap_or_else(|| format!("{bf_home}/repos/{project_slug}"));

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
    let t0 = now_secs();
    let add_status = spawn_inherit("bf-install", &["add", &project_slug, &manifest_path])?;
    if !add_status.success() {
        record_telemetry(TelemetryEvent::Install {
            slug: project_slug.clone(),
            success: false,
            duration_secs: now_secs() - t0,
        });
        anyhow::bail!("bf-install add failed");
    }
    let act_status = spawn_inherit("bf-install", &["activate", &project_slug, "latest"])?;
    if !act_status.success() {
        record_telemetry(TelemetryEvent::Install {
            slug: project_slug.clone(),
            success: false,
            duration_secs: now_secs() - t0,
        });
        anyhow::bail!("bf-install activate failed");
    }

    record_telemetry(TelemetryEvent::Install {
        slug: project_slug.clone(),
        success: true,
        duration_secs: now_secs() - t0,
    });

    eprintln!("bf: '{project_slug}' installed — binaries under {bf_home}/bin/");
    eprintln!("bf: add {bf_home}/bin to your PATH if not already there");
    eprintln!("bf: to roll back: bf rescue activate {project_slug} <previous-generation-id>");
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

/// Extract `owner/repo` from a GitHub HTTPS URL.
fn github_slug_from_url(url: &str) -> Option<String> {
    let stripped = url.trim_end_matches('/').trim_end_matches(".git");
    stripped
        .split_once("github.com/")
        .map(|(_, slug)| slug.to_owned())
}

// ── request pipeline ──────────────────────────────────────────────────────────

/// Tool manifest passed to bf-agent for change requests.
fn generate_tool_manifest() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "read_file",
                "description": "Read the contents of a file in the repository.",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path relative to repo root"}
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "write_file",
                "description": "Write (or overwrite) a file in the repository.",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }
            },
            {
                "name": "list_files",
                "description": "List files and directories at a path in the repository.",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path relative to repo root (default: .)"}
                    }
                }
            },
            {
                "name": "run_shell",
                "description": "Run a shell command in the repository directory (e.g. cargo test, cargo fmt).",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "args": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "git_diff",
                "description": "Show uncommitted or staged changes.",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "staged": {"type": "boolean", "description": "Show staged changes (default false)"}
                    }
                }
            },
            {
                "name": "git_add",
                "description": "Stage files for the next commit.",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "paths": {"type": "array", "items": {"type": "string"}, "description": "Paths to stage (default: [\".\"])"}
                    }
                }
            },
            {
                "name": "git_commit",
                "description": "Create a git commit with staged changes. A DCO Signed-off-by is added automatically.",
                "command": ["__builtin__"],
                "schema": {
                    "type": "object",
                    "properties": {
                        "message": {"type": "string"}
                    },
                    "required": ["message"]
                }
            }
        ]
    })
}

fn request(slug: &str, description: &str) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    let repo_path = format!("{bf_home}/repos/{slug}");

    if !std::path::Path::new(&repo_path).exists() {
        eprintln!("bf: project '{slug}' not found at {repo_path}");
        eprintln!("bf: run `bf install {slug}` first");
        std::process::exit(exit::NOINPUT);
    }

    // Derive a URL-safe branch slug from the description.
    let branch_slug: String = description
        .to_lowercase()
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let branch = format!("bf/{branch_slug}-{ts}");

    eprintln!("bf: creating branch {branch}");
    let br_status = Command::new("git")
        .args(["checkout", "-b", &branch])
        .current_dir(&repo_path)
        .status()
        .context("git checkout -b")?;
    if !br_status.success() {
        anyhow::bail!("failed to create branch {branch}");
    }
    emit(&Event::BranchCreated {
        branch: branch.clone(),
    });

    // Write the tool manifest to a temp file.
    let manifest = generate_tool_manifest();
    let tmp = tempfile::NamedTempFile::new().context("creating temp manifest file")?;
    serde_json::to_writer_pretty(tmp.as_file(), &manifest).context("writing tool manifest")?;
    let manifest_path = tmp.path().to_string_lossy().to_string();

    eprintln!("bf: invoking agent — prompt: {description}");
    let agent_status = spawn_inherit(
        "bf-agent",
        &[
            "--repo",
            &repo_path,
            "--prompt",
            description,
            "--tools",
            &manifest_path,
        ],
    )?;

    if !agent_status.success() {
        anyhow::bail!("bf-agent exited with {agent_status}");
    }

    eprintln!("bf: agent done — review changes with `git log -1` in {repo_path}");
    eprintln!("bf: run `bf submit {slug}` when ready to open a PR");
    Ok(())
}

// ── submit pipeline ───────────────────────────────────────────────────────────

fn submit(slug: &str) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    let repo_path = format!("{bf_home}/repos/{slug}");

    if !std::path::Path::new(&repo_path).exists() {
        eprintln!("bf: project '{slug}' not found at {repo_path}");
        std::process::exit(exit::NOINPUT);
    }

    // Determine current branch.
    let br_out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&repo_path)
        .output()
        .context("git rev-parse")?;
    let branch = String::from_utf8_lossy(&br_out.stdout).trim().to_owned();
    if branch == "main" || branch == "master" {
        anyhow::bail!(
            "refusing to submit: on '{branch}'. \
             Switch to a feature branch (`bf request` creates one automatically)."
        );
    }

    // Get origin (fork) URL.
    let origin_out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&repo_path)
        .output()
        .context("git remote get-url origin")?;
    let fork_url = String::from_utf8_lossy(&origin_out.stdout)
        .trim()
        .to_owned();

    // Determine upstream repo slug (prefer `upstream` remote, fall back to origin).
    let upstream_out = Command::new("git")
        .args(["remote", "get-url", "upstream"])
        .current_dir(&repo_path)
        .output();
    let upstream_url = upstream_out
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| fork_url.clone());

    let pr_repo = github_slug_from_url(&upstream_url)
        .or_else(|| github_slug_from_url(&fork_url))
        .unwrap_or_else(|| upstream_url.clone());

    // Push the branch.
    eprintln!("bf: pushing {branch} → origin");
    let push_status = Command::new("git")
        .args(["push", "-u", "origin", &branch])
        .current_dir(&repo_path)
        .status()
        .context("git push")?;
    if !push_status.success() {
        anyhow::bail!("git push failed");
    }

    // Build PR title and body from the last commit message.
    let log_out = Command::new("git")
        .args(["log", "-1", "--pretty=%s%n%n%b"])
        .current_dir(&repo_path)
        .output()?;
    let commit_msg = String::from_utf8_lossy(&log_out.stdout).to_string();
    let pr_title = commit_msg.lines().next().unwrap_or(&branch).to_owned();
    let pr_body = format!(
        "{commit_msg}\n---\n\
         *Drafted with [Butterfork](https://github.com/matthewscottconroy/butter-fork) \
         and AI assistance.*"
    );

    eprintln!("bf: opening PR — {branch} → main on {pr_repo}");
    let pr_status = spawn_inherit(
        "bf-forge",
        &[
            "pr", "open", "--repo", &pr_repo, "--head", &branch, "--base", "main", "--title",
            &pr_title, "--body", &pr_body,
        ],
    )?;
    std::process::exit(pr_status.code().unwrap_or(1));
}

// ── main ─────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
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
            request(&slug, &description)?;
        }

        BfCommand::Submit { slug } => {
            submit(&slug)?;
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
            eprintln!("bf: checking system health");
            let mut warnings: Vec<String> = Vec::new();
            let mut errors: Vec<String> = Vec::new();

            // ── external tools ─────────────────────────────────────────────
            let ext_tools = [
                ("git", "version control"),
                ("gh", "GitHub CLI (for forge operations)"),
                ("cargo", "Rust build system"),
                ("rg", "ripgrep (faster grep for bf-index)"),
                ("bwrap", "bubblewrap sandbox"),
            ];
            eprintln!("bf: --- external tools ---");
            for (tool, purpose) in &ext_tools {
                match Command::new(tool).arg("--version").output() {
                    Ok(out) if out.status.success() => {
                        let v = String::from_utf8_lossy(&out.stdout);
                        eprintln!(
                            "  [ok]      {tool}: {}",
                            v.lines().next().unwrap_or("").trim()
                        );
                    }
                    _ => {
                        let msg = format!("{tool} not found — {purpose}");
                        if *tool == "git" || *tool == "cargo" {
                            errors.push(msg);
                            eprintln!("  [error]   {tool}: not found (required)");
                        } else {
                            warnings.push(msg);
                            eprintln!("  [warn]    {tool}: not found ({purpose})");
                        }
                    }
                }
            }

            // ── gh auth ────────────────────────────────────────────────────
            match Command::new("gh").args(["auth", "status"]).output() {
                Ok(out) if out.status.success() => {
                    eprintln!("  [ok]      gh auth: authenticated");
                }
                Ok(_) => {
                    warnings.push("gh is not authenticated — run `gh auth login`".to_owned());
                    eprintln!("  [warn]    gh auth: not authenticated — run `gh auth login`");
                }
                Err(_) => {} // gh already reported missing above
            }

            // ── API keys ───────────────────────────────────────────────────
            eprintln!("bf: --- environment ---");
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                eprintln!("  [ok]      ANTHROPIC_API_KEY: set");
            } else {
                warnings
                    .push("ANTHROPIC_API_KEY not set — bf-agent (Claude) will not work".to_owned());
                eprintln!("  [warn]    ANTHROPIC_API_KEY: not set (required for bf-agent)");
            }

            let ollama_ok = Command::new("curl")
                .args(["-sf", "http://localhost:11434/api/version"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ollama_ok {
                eprintln!("  [ok]      Ollama: reachable at localhost:11434");
            } else {
                eprintln!("  [info]    Ollama: not reachable (optional, for bf-agent-ollama)");
            }

            // ── bf components ──────────────────────────────────────────────
            eprintln!("bf: --- bf components ---");
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
                "bf-bootstrap",
            ];
            for comp in &components {
                match Command::new(comp).arg("--version").output() {
                    Ok(out) if out.status.success() => {
                        let v = String::from_utf8_lossy(&out.stdout);
                        eprintln!("  [ok]      {comp}: {}", v.trim());
                    }
                    _ => {
                        errors.push(format!("{comp}: not found on PATH"));
                        eprintln!("  [missing] {comp}");
                    }
                }
            }

            // ── summary ────────────────────────────────────────────────────
            eprintln!("bf: --- summary ---");
            for w in &warnings {
                eprintln!("  [warn]  {w}");
            }
            for e in &errors {
                eprintln!("  [error] {e}");
            }

            if errors.is_empty() && warnings.is_empty() {
                eprintln!("bf: all checks passed");
            } else if errors.is_empty() {
                eprintln!("bf: {} warning(s), no errors", warnings.len());
            } else {
                eprintln!(
                    "bf: {} error(s), {} warning(s)",
                    errors.len(),
                    warnings.len()
                );
                eprintln!("bf: install missing components with `cargo install --path <crate>` or `scripts/fat-install.sh`");
                std::process::exit(bf_common::exit::UNAVAILABLE);
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
                let status = spawn_inherit("bf-install", &["activate", &slug, &generation_id])?;
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

        BfCommand::Telemetry { cmd } => {
            let opt_in = telemetry_opt_in_path();
            let log = telemetry_log_path();
            match cmd {
                TelemetryCommand::Status => {
                    let enabled = telemetry_enabled();
                    let count = if log.exists() {
                        std::fs::read_to_string(&log)
                            .unwrap_or_default()
                            .lines()
                            .count()
                    } else {
                        0
                    };
                    eprintln!("bf telemetry: enabled={enabled}");
                    eprintln!(
                        "bf telemetry: {count} record(s) stored at {}",
                        log.display()
                    );
                    eprintln!(
                        "bf telemetry: records are local-only and never transmitted automatically"
                    );
                    println!(
                        "{}",
                        serde_json::json!({
                            "enabled": enabled,
                            "record_count": count,
                            "log_path": log.display().to_string(),
                        })
                    );
                }
                TelemetryCommand::Enable => {
                    if let Some(parent) = opt_in.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&opt_in, "")?;
                    eprintln!(
                        "bf telemetry: enabled — events will be recorded to {}",
                        log.display()
                    );
                    eprintln!("bf telemetry: disable at any time with `bf telemetry disable`");
                }
                TelemetryCommand::Disable => {
                    let _ = std::fs::remove_file(&opt_in);
                    eprintln!("bf telemetry: disabled — no new events will be recorded");
                    eprintln!("bf telemetry: existing records remain at {} (clear with `bf telemetry clear`)", log.display());
                }
                TelemetryCommand::Show => {
                    if !log.exists() {
                        eprintln!("bf telemetry: no records found");
                    } else {
                        print!("{}", std::fs::read_to_string(&log)?);
                    }
                }
                TelemetryCommand::Clear => {
                    if log.exists() {
                        std::fs::remove_file(&log)?;
                        eprintln!("bf telemetry: all records deleted");
                    } else {
                        eprintln!("bf telemetry: no records to delete");
                    }
                }
            }
        }

        BfCommand::SelfTest { repo, no_sandbox } => {
            eprintln!("bf self-test: running integration checks");
            let mut results: Vec<SelfTestResult> = Vec::new();

            // Resolve test repo — default to the current working directory.
            let test_repo = repo
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| ".".to_owned());
            let abs_repo = std::fs::canonicalize(&test_repo)
                .unwrap_or_else(|_| std::path::PathBuf::from(&test_repo))
                .to_string_lossy()
                .to_string();

            eprintln!("bf self-test: test repo = {abs_repo}");
            eprintln!("bf self-test: ---");

            // ── 1. bf-build detect ──────────────────────────────────────────
            let detect_out = Command::new("bf-build")
                .args(["detect", &abs_repo])
                .output();
            match &detect_out {
                Ok(o) if o.status.success() => {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    let adapter = serde_json::from_str::<serde_json::Value>(
                        stdout.lines().next().unwrap_or("{}"),
                    )
                    .ok()
                    .and_then(|v| v["adapter"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unknown".to_owned());
                    self_test_check(
                        &mut results,
                        "bf-build detect",
                        true,
                        format!("adapter={adapter}"),
                    );
                }
                Ok(o) => self_test_check(
                    &mut results,
                    "bf-build detect",
                    false,
                    format!("exit {}", o.status.code().unwrap_or(1)),
                ),
                Err(e) => self_test_check(&mut results, "bf-build detect", false, e.to_string()),
            }

            // ── 2. bf-build plan ────────────────────────────────────────────
            let plan_out = Command::new("bf-build").args(["plan", &abs_repo]).output();
            self_test_check(
                &mut results,
                "bf-build plan",
                plan_out
                    .as_ref()
                    .map(|o| o.status.success())
                    .unwrap_or(false),
                plan_out.err().map(|e| e.to_string()).unwrap_or_default(),
            );

            // ── 3. bf-index update (temp dir) ───────────────────────────────
            let tmpdir = tempfile::tempdir();
            match &tmpdir {
                Ok(d) => {
                    // Write a minimal Rust file to index.
                    let _ = std::fs::write(d.path().join("main.rs"), "pub fn hello() {}");
                    let idx_out = Command::new("bf-index")
                        .args(["update", &d.path().to_string_lossy()])
                        .output();
                    self_test_check(
                        &mut results,
                        "bf-index update",
                        idx_out
                            .as_ref()
                            .map(|o| o.status.success())
                            .unwrap_or(false),
                        idx_out.err().map(|e| e.to_string()).unwrap_or_default(),
                    );
                }
                Err(e) => {
                    self_test_check(&mut results, "bf-index update", false, e.to_string());
                }
            }

            // ── 4. bf-sandbox (echo test) ────────────────────────────────────
            if !no_sandbox {
                let sbox_out = Command::new("bf-sandbox")
                    .args(["--profile", "run", "--", "echo", "bf-sandbox-ok"])
                    .output();
                let passed = sbox_out
                    .as_ref()
                    .map(|o| {
                        o.status.success()
                            && String::from_utf8_lossy(&o.stdout).contains("bf-sandbox-ok")
                    })
                    .unwrap_or(false);
                self_test_check(
                    &mut results,
                    "bf-sandbox run",
                    passed,
                    if passed {
                        "echo test passed".to_owned()
                    } else {
                        "sandbox echo test failed (try --no-sandbox in CI)".to_owned()
                    },
                );
            }

            // ── 5. bf-catalog search ─────────────────────────────────────────
            let cat_out = Command::new("bf-catalog")
                .args(["search", "ripgrep"])
                .output();
            self_test_check(
                &mut results,
                "bf-catalog search",
                cat_out
                    .as_ref()
                    .map(|o| o.status.success())
                    .unwrap_or(false),
                cat_out.err().map(|e| e.to_string()).unwrap_or_default(),
            );

            // ── summary ──────────────────────────────────────────────────────
            eprintln!("bf self-test: ---");
            let passed = results.iter().filter(|r| r.passed).count();
            let total = results.len();
            let all_passed = passed == total;
            eprintln!("bf self-test: {passed}/{total} checks passed");

            let summary = serde_json::json!({
                "passed": passed,
                "total": total,
                "all_passed": all_passed,
                "checks": results.iter().map(|r| serde_json::json!({
                    "name": r.name,
                    "passed": r.passed,
                    "note": r.note,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&summary)?);

            if !all_passed {
                std::process::exit(exit::SOFTWARE);
            }
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_from_url_strips_dotgit() {
        assert_eq!(
            slug_from_url("https://github.com/BurntSushi/ripgrep"),
            "ripgrep"
        );
        assert_eq!(
            slug_from_url("https://github.com/BurntSushi/ripgrep.git"),
            "ripgrep"
        );
        assert_eq!(slug_from_url("https://github.com/sharkdp/fd/"), "fd");
    }

    #[test]
    fn parse_events_handles_mixed_lines() {
        let stdout = b"{\"type\":\"fork-created\",\"fork_url\":\"https://github.com/user/repo\"}\nnot-json\n{\"type\":\"done\",\"exit_code\":0}\n";
        let events = parse_events(stdout);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], Event::ForkCreated { fork_url } if fork_url == "https://github.com/user/repo")
        );
    }

    #[test]
    fn extract_fork_url_finds_event() {
        let events = vec![
            Event::Message {
                text: "forking".to_owned(),
            },
            Event::ForkCreated {
                fork_url: "https://github.com/user/rg".to_owned(),
            },
            Event::Done { exit_code: 0 },
        ];
        assert_eq!(
            extract_fork_url(&events),
            Some("https://github.com/user/rg".to_owned())
        );
    }

    #[test]
    fn extract_manifest_path_finds_event() {
        let events = vec![Event::BuildComplete {
            manifest_path: "/tmp/bf-artifact-manifest.json".to_owned(),
        }];
        assert_eq!(
            extract_manifest_path(&events),
            Some("/tmp/bf-artifact-manifest.json".to_owned())
        );
    }
}
