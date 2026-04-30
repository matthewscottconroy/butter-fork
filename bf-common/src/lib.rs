use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// sysexits.h exit code constants used by all bf-* components.
pub mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 64;
    pub const DATAERR: i32 = 65;
    pub const NOINPUT: i32 = 66;
    pub const UNAVAILABLE: i32 = 69;
    pub const SOFTWARE: i32 = 70;
    pub const OSERR: i32 = 71;
    pub const CANTCREAT: i32 = 73;
    pub const IOERR: i32 = 74;
    pub const TEMPFAIL: i32 = 75;
    pub const NOPERM: i32 = 77;
    pub const CONFIG: i32 = 78;
}

/// NDJSON event emitted to stdout by all long-running bf-* commands.
///
/// One JSON object per line; shell consumers can pipe into `jq` live.
/// The `type` field is the discriminant (kebab-case).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    /// The agent has laid out a plan for the task.
    Plan { steps: Vec<String> },
    /// The agent is invoking an external tool.
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// Result of a tool invocation.
    ToolResult {
        id: String,
        output: serde_json::Value,
        is_error: bool,
    },
    /// A text message from the agent or component.
    Message { text: String },
    /// bf-forge fork completed; the fork URL is available for cloning.
    ForkCreated { fork_url: String },
    /// bf-build run completed; the artifact manifest is ready for bf-install.
    BuildComplete { manifest_path: String },
    /// bf-install activate completed; the project is now on PATH.
    InstallComplete {
        project: String,
        generation_id: String,
        bin_dir: String,
    },
    /// An issue was opened in the fork repository.
    IssueCreated { issue_url: String },
    /// A feature branch was created in the local checkout.
    BranchCreated { branch: String },
    /// A pull request was opened on the forge.
    PrCreated { pr_url: String },
    /// The operation has finished.
    Done { exit_code: i32 },
}

/// Write a single NDJSON event to stdout.
pub fn emit(event: &Event) {
    println!(
        "{}",
        serde_json::to_string(event).expect("event serialization must not fail")
    );
}

/// A catalog entry describing an OSS project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub upstream_url: String,
    pub license: String,
    pub stars: u64,
    pub has_contributing: bool,
    pub has_code_of_conduct: bool,
    /// Median days from PR open to first maintainer response, if known.
    pub pr_response_latency_days: Option<f64>,
    /// 0.0–1.0 composite contribution-friendliness score (Phase 4+).
    #[serde(default)]
    pub contribution_score: Option<f64>,
    /// SPDX expression detected at fork time (e.g. "MIT", "GPL-3.0-only").
    #[serde(default)]
    pub spdx_id: Option<String>,
    /// Whether the license is copyleft (GPL/LGPL/AGPL family).
    #[serde(default)]
    pub is_copyleft: bool,
}

/// Per-project PR policy loaded from `~/.butterfork/pr-policy/<slug>.toml`.
///
/// Controls which pre-flight checks run before opening a PR and how the
/// AI-assistance footer is handled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    /// Require DCO Signed-off-by on every commit (default: true).
    pub require_dco: bool,
    /// Require `cargo test` (or equivalent) to pass (default: true).
    pub require_tests: bool,
    /// Run a format check (`cargo fmt --check`) before opening a PR (default: false).
    pub require_format_check: bool,
    /// How to handle the AI-assistance footer in the PR body.
    pub ai_footer: AiFooterPolicy,
    /// Warn when the diff exceeds this many total line/file changes (default: 1000).
    pub max_diff_lines: u64,
    /// Block PRs that add undeclared new dependencies (default: false).
    pub block_new_dependencies: bool,
    /// Warn when > 80 % of the diff is whitespace changes (default: true).
    pub warn_whitespace_churn: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            require_dco: true,
            require_tests: true,
            require_format_check: false,
            ai_footer: AiFooterPolicy::Include,
            max_diff_lines: 1000,
            block_new_dependencies: false,
            warn_whitespace_churn: true,
        }
    }
}

/// How the AI-assistance footer is included in PR bodies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiFooterPolicy {
    /// Always include the footer (default).
    #[default]
    Include,
    /// Never include the footer (user has verified the project prohibits it).
    Exclude,
    /// Prompt the user interactively (not yet implemented, falls back to Include).
    Ask,
}

/// A single telemetry event recorded to `~/.butterfork/telemetry.jsonl`.
///
/// All fields are local-only. Nothing is transmitted automatically.
/// Users opt in with `bf telemetry enable` and export with `bf telemetry show`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryRecord {
    /// Unix timestamp of the event.
    pub timestamp: u64,
    pub event: TelemetryEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelemetryEvent {
    Install {
        slug: String,
        success: bool,
        duration_secs: u64,
    },
    Build {
        slug: String,
        adapter: String,
        success: bool,
        duration_secs: u64,
    },
    AgentRun {
        slug: String,
        success: bool,
        iterations: u32,
    },
    PrOpened {
        slug: String,
    },
    PrMerged {
        slug: String,
    },
}

/// Result of `bf-build detect`: which adapter should build this repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildDetection {
    pub adapter: String,
    /// 0.0–1.0 confidence score.
    pub confidence: f64,
    pub hints: Vec<String>,
}

/// A concrete, ordered plan for building a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildPlan {
    pub adapter: String,
    pub steps: Vec<BuildStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStep {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// A single install generation record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generation {
    pub id: String,
    pub project: String,
    pub git_ref: String,
    pub built_at: String,
    pub artifact_paths: Vec<String>,
    pub active: bool,
}

/// Artifact manifest produced by a build and consumed by bf-install.
///
/// `artifact.src`  — absolute path to the built binary.
/// `artifact.dest` — path relative to the generation directory
///                   (e.g. `bin/rg`, `lib/libfoo.so`).
/// `bf-install add` copies each artifact to:
///   `~/.butterfork/generations/<project>/<id>/<artifact.dest>`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactManifest {
    pub project: String,
    pub git_ref: String,
    pub built_at: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Absolute path to the compiled output.
    pub src: String,
    /// Relative destination inside the generation directory (e.g. `bin/rg`).
    pub dest: String,
}
