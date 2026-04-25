use anyhow::{Context, Result};
use bf_common::{emit, exit, Event};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-agent-ollama",
    about = "Ollama agent backend for bf-agent: same protocol, local LLM",
    long_about = "Drop-in replacement for bf-agent using a locally running Ollama model.\n\
                  Set BF_AGENT=bf-agent-ollama to use this instead of the Claude backend.\n\
                  Requires Ollama running at OLLAMA_HOST (default: http://localhost:11434).\n\
                  Model: OLLAMA_MODEL env var (default: llama3.1).",
    version
)]
struct Cli {
    #[arg(long)]
    repo: String,
    #[arg(long)]
    prompt: String,
    #[arg(long)]
    tools: String,
    #[arg(long, default_value = "50")]
    max_iterations: u32,
    #[arg(long, env = "OLLAMA_MODEL", default_value = "llama3.1")]
    model: String,
}

// ── tool manifest (shared shape with bf-agent) ────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub command: Vec<String>,
    pub schema: Value,
}

#[derive(Debug, Deserialize)]
pub struct ToolManifest {
    pub tools: Vec<ToolSpec>,
}

fn load_manifest(path: &str) -> Result<ToolManifest> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("reading tool manifest: {path}"))?;
    serde_json::from_str(&s).context("parsing tool manifest JSON")
}

// ── Ollama API client ─────────────────────────────────────────────────────────

struct OllamaClient {
    client: reqwest::blocking::Client,
    base_url: String,
    model: String,
}

impl OllamaClient {
    fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            base_url,
            model,
        }
    }

    fn chat(&self, messages: &[Value], tools: &[Value]) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "stream": false,
        });

        let url = format!("{}/api/chat", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .context("calling Ollama API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            anyhow::bail!("Ollama API {status}: {text}");
        }

        resp.json::<Value>().context("parsing Ollama response")
    }
}

// ── tool execution (same logic as bf-agent) ───────────────────────────────────

fn execute_tool(name: &str, input: &Value, repo: &str, specs: &[ToolSpec]) -> (String, bool) {
    match run_tool(name, input, repo, specs) {
        Ok(out) => (out, false),
        Err(e) => (format!("Error: {e:#}"), true),
    }
}

fn run_tool(name: &str, input: &Value, repo: &str, specs: &[ToolSpec]) -> Result<String> {
    let repo_path = Path::new(repo);
    match name {
        "read_file" => {
            let path = input["path"].as_str().context("path required")?;
            std::fs::read_to_string(repo_path.join(path))
                .with_context(|| format!("reading {path}"))
        }
        "write_file" => {
            let path = input["path"].as_str().context("path required")?;
            let content = input["content"].as_str().context("content required")?;
            let full = repo_path.join(path);
            if let Some(p) = full.parent() { std::fs::create_dir_all(p)?; }
            std::fs::write(&full, content)?;
            Ok(format!("Wrote {} bytes to {path}", content.len()))
        }
        "list_files" => {
            let rel = input["path"].as_str().unwrap_or(".");
            let mut entries: Vec<String> = std::fs::read_dir(repo_path.join(rel))
                .with_context(|| format!("listing {rel}"))?
                .filter_map(|e| e.ok())
                .map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) { format!("{n}/") } else { n }
                })
                .collect();
            entries.sort();
            Ok(entries.join("\n"))
        }
        "run_shell" => {
            let cmd = input["command"].as_str().context("command required")?;
            let args: Vec<&str> = input["args"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let out = Command::new(cmd).args(&args).current_dir(repo).output()
                .with_context(|| format!("running {cmd}"))?;
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if !out.status.success() { anyhow::bail!("exited {}: {combined}", out.status); }
            Ok(combined)
        }
        "git_add" => {
            let paths: Vec<&str> = input["paths"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_else(|| vec!["."]);
            let mut args = vec!["add", "--"];
            args.extend(paths.iter().copied());
            let s = Command::new("git").args(&args).current_dir(repo).status()?;
            if !s.success() { anyhow::bail!("git add failed"); }
            Ok("Staged changes".to_owned())
        }
        "git_commit" => {
            let message = input["message"].as_str().context("message required")?;
            let out = Command::new("git")
                .args(["commit", "-s", "-m", message])
                .current_dir(repo).output()?;
            if !out.status.success() {
                anyhow::bail!("git commit failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        }
        "git_diff" => {
            let staged = input["staged"].as_bool().unwrap_or(false);
            let mut args = vec!["diff"];
            if staged { args.push("--staged"); }
            let out = Command::new("git").args(&args).current_dir(repo).output()?;
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        }
        _ => {
            let spec = specs.iter().find(|s| s.name == name)
                .with_context(|| format!("unknown tool: {name}"))?;
            if spec.command.first().map(|s| s.as_str()) == Some("__builtin__") {
                anyhow::bail!("unimplemented builtin: {name}");
            }
            let mut argv: Vec<String> = spec.command.clone();
            if let Some(obj) = input.as_object() {
                for (key, val) in obj {
                    argv.push(format!("--{}", key.replace('_', "-")));
                    argv.push(match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    });
                }
            }
            let (bin, rest) = argv.split_first().context("empty command")?;
            let out = Command::new(bin).args(rest).current_dir(repo).output()
                .with_context(|| format!("running {bin}"))?;
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if !out.status.success() { anyhow::bail!("tool failed: {combined}"); }
            Ok(combined)
        }
    }
}

// ── Ollama tool format ────────────────────────────────────────────────────────

fn tool_to_ollama(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.schema,
        }
    })
}

fn system_prompt(repo: &str, prompt: &str) -> String {
    format!(
        "You are Butterfork's coding agent running on a local Ollama model. \
         Make the following change to the repository at `{repo}`:\n\n{prompt}\n\n\
         Read relevant files first, make minimal changes, run tests, then commit."
    )
}

// ── agent loop ────────────────────────────────────────────────────────────────

fn run_agent_loop(
    client: &OllamaClient,
    repo: &str,
    prompt: &str,
    tools: &[ToolSpec],
    max_iterations: u32,
) -> Result<()> {
    let ollama_tools: Vec<Value> = tools.iter().map(tool_to_ollama).collect();
    let system = system_prompt(repo, prompt);

    let mut messages: Vec<Value> = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": prompt}),
    ];

    emit(&Event::Plan {
        steps: vec![
            "understand codebase".to_owned(),
            "apply changes".to_owned(),
            "run tests".to_owned(),
            "commit".to_owned(),
        ],
    });

    for iteration in 0..max_iterations {
        eprintln!("bf-agent-ollama: iteration {}/{max_iterations}", iteration + 1);

        let response = client.chat(&messages, &ollama_tools)?;

        let message = &response["message"];
        let content = message["content"].as_str().unwrap_or("").to_owned();
        let tool_calls = message["tool_calls"].as_array().cloned().unwrap_or_default();

        if !content.is_empty() {
            emit(&Event::Message { text: content.clone() });
        }

        // Push assistant message
        messages.push(json!({
            "role": "assistant",
            "content": content,
            "tool_calls": tool_calls,
        }));

        if tool_calls.is_empty() {
            eprintln!("bf-agent-ollama: done (no tool calls)");
            emit(&Event::Done { exit_code: 0 });
            return Ok(());
        }

        // Execute each tool call
        for call in &tool_calls {
            let func = &call["function"];
            let name = func["name"].as_str().unwrap_or("").to_owned();
            let args_raw = &func["arguments"];
            let input: Value = if args_raw.is_string() {
                serde_json::from_str(args_raw.as_str().unwrap_or("{}")).unwrap_or(json!({}))
            } else {
                args_raw.clone()
            };
            let call_id = call["id"].as_str().unwrap_or("").to_owned();

            emit(&Event::ToolCall {
                id: call_id.clone(),
                name: name.clone(),
                args: input.clone(),
            });

            let (output, is_error) = execute_tool(&name, &input, repo, tools);

            emit(&Event::ToolResult {
                id: call_id.clone(),
                output: Value::String(output.clone()),
                is_error,
            });

            messages.push(json!({
                "role": "tool",
                "content": output,
            }));
        }
    }

    eprintln!("bf-agent-ollama: reached max_iterations ({max_iterations})");
    emit(&Event::Done { exit_code: exit::TEMPFAIL });
    std::process::exit(exit::TEMPFAIL);
}

// ── main ─────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    let base_url = std::env::var("OLLAMA_HOST")
        .unwrap_or_else(|_| "http://localhost:11434".to_owned());

    eprintln!("bf-agent-ollama: model={} host={}", cli.model, base_url);

    let manifest = load_manifest(&cli.tools)?;
    eprintln!(
        "bf-agent-ollama: loaded {} tool(s): {}",
        manifest.tools.len(),
        manifest.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
    );

    let client = OllamaClient::new(base_url, cli.model);
    run_agent_loop(&client, &cli.repo, &cli.prompt, &manifest.tools, cli.max_iterations)
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git").args(["init", "-q"]).current_dir(dir.path()).status().unwrap();
        Command::new("git").args(["config", "user.email", "t@t.com"]).current_dir(dir.path()).status().unwrap();
        Command::new("git").args(["config", "user.name", "T"]).current_dir(dir.path()).status().unwrap();
        dir
    }

    #[test]
    fn tool_read_write() {
        let dir = tmp_repo();
        let repo = dir.path().to_str().unwrap();
        let (_, err) = execute_tool("write_file", &json!({"path":"hi.txt","content":"hello"}), repo, &[]);
        assert!(!err);
        let (content, err) = execute_tool("read_file", &json!({"path":"hi.txt"}), repo, &[]);
        assert!(!err);
        assert_eq!(content, "hello");
    }

    #[test]
    fn tool_to_ollama_format() {
        let spec = ToolSpec {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            command: vec!["__builtin__".to_owned()],
            schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        };
        let v = tool_to_ollama(&spec);
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "read_file");
    }
}
