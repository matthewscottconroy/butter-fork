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
