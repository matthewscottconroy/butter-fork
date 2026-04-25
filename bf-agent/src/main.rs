use anyhow::Result;
use bf_common::{emit, Event};
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(
    name = "bf-agent",
    about = "LLM tool-use loop: reads a prompt and tool manifest, streams NDJSON events",
    long_about = "The tool manifest declares external commands the agent may invoke.\n\
                  Swap the model backend by replacing the bf-agent binary with any binary\n\
                  that speaks the same NDJSON event protocol (plan/tool-call/tool-result/\n\
                  message/done). The model vendor is not a Butterfork concern.\n\
                  Override with BF_AGENT env var.",
    version
)]
struct Cli {
    /// Path to the repository the agent will operate on
    #[arg(long)]
    repo: String,

    /// Natural-language prompt describing the task
    #[arg(long)]
    prompt: String,

    /// Path to the tool manifest JSON file
    #[arg(long)]
    tools: String,

    /// Maximum number of tool-call iterations before the agent gives up
    #[arg(long, default_value = "50")]
    max_iterations: u32,
}

/// An entry in the tool manifest: a named external command the agent may invoke.
#[derive(Debug, Deserialize, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub command: Vec<String>,
    pub schema: serde_json::Value,
}

/// The full tool manifest passed to bf-agent via --tools.
#[derive(Debug, Deserialize)]
pub struct ToolManifest {
    pub tools: Vec<ToolSpec>,
}

fn load_manifest(path: &str) -> Result<ToolManifest> {
    let s = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&s)?)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    eprintln!("bf-agent: repo={}", cli.repo);
    eprintln!("bf-agent: tools={}", cli.tools);
    eprintln!("bf-agent: prompt={}", cli.prompt);

    let manifest = load_manifest(&cli.tools)?;
    eprintln!(
        "bf-agent: loaded {} tool(s): {}",
        manifest.tools.len(),
        manifest.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
    );

    // Emit a plan event so downstream consumers know the agent is starting.
    emit(&Event::Plan {
        steps: vec![
            "analyze task".to_owned(),
            "call tools iteratively".to_owned(),
            "write result".to_owned(),
        ],
    });

    // TODO (Phase 1): implement the actual LLM tool-use loop.
    // The default backend will be bf-agent-claude; swap by replacing this binary
    // or setting BF_AGENT in the environment.
    emit(&Event::Message {
        text: "Agent loop not yet implemented. \
               Implement bf-agent-claude (Phase 1) or set BF_AGENT to a custom backend."
            .to_owned(),
    });

    emit(&Event::Done {
        exit_code: bf_common::exit::UNAVAILABLE,
    });
    std::process::exit(bf_common::exit::UNAVAILABLE);
}
