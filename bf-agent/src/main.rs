use anyhow::{Context, Result};
use bf_common::{emit, exit, Event};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-agent",
    about = "LLM tool-use loop: reads a prompt and tool manifest, streams NDJSON events",
    long_about = "Default backend calls the Claude API (ANTHROPIC_API_KEY required).\n\
                  Swap the backend by replacing this binary or setting BF_AGENT.\n\
                  Tool manifest JSON declares external commands the agent may invoke.\n\
                  Events stream as NDJSON on stdout; progress goes to stderr.",
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
    #[arg(
        long,
        default_value = "claude-opus-4-7-20251101",
        env = "BF_AGENT_MODEL"
    )]
    model: String,
}

// ── tool manifest ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// `["__builtin__"]` for built-in tools; otherwise `["bin", "arg", …]`
    pub command: Vec<String>,
    pub schema: Value,
}

#[derive(Debug, Deserialize)]
pub struct ToolManifest {
    pub tools: Vec<ToolSpec>,
}

fn load_manifest(path: &str) -> Result<ToolManifest> {
    let s =
        std::fs::read_to_string(path).with_context(|| format!("reading tool manifest: {path}"))?;
    serde_json::from_str(&s).context("parsing tool manifest JSON")
}

// ── Claude API client ─────────────────────────────────────────────────────────

struct ClaudeClient {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

impl ClaudeClient {
    fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            api_key,
            model,
        }
    }

    fn send(&self, system: &str, messages: &[Value], tools: &[Value]) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "max_tokens": 8192,
            "system": system,
            "tools": tools,
            "messages": messages,
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .context("calling Claude API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            anyhow::bail!("Claude API {status}: {text}");
        }

        resp.json::<Value>().context("parsing Claude API response")
    }
}

// ── tool execution ────────────────────────────────────────────────────────────

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
            let full = repo_path.join(path);
            std::fs::read_to_string(&full).with_context(|| format!("reading {}", full.display()))
        }

        "write_file" => {
            let path = input["path"].as_str().context("path required")?;
            let content = input["content"].as_str().context("content required")?;
            let full = repo_path.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, content)
                .with_context(|| format!("writing {}", full.display()))?;
            Ok(format!("Wrote {} bytes to {path}", content.len()))
        }

        "list_files" => {
            let rel = input["path"].as_str().unwrap_or(".");
            let full = repo_path.join(rel);
            let mut entries: Vec<String> = std::fs::read_dir(&full)
                .with_context(|| format!("listing {}", full.display()))?
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        format!("{name}/")
                    } else {
                        name
                    }
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
            let out = Command::new(cmd)
                .args(&args)
                .current_dir(repo)
                .output()
                .with_context(|| format!("running {cmd}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let combined = if stderr.is_empty() {
                stdout
            } else if stdout.is_empty() {
                stderr
            } else {
                format!("{stdout}\n{stderr}")
            };
            if !out.status.success() {
                anyhow::bail!("exited {}: {combined}", out.status);
            }
            Ok(combined)
        }

        "git_add" => {
            let paths: Vec<&str> = input["paths"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_else(|| vec!["."]);
            let mut args = vec!["add", "--"];
            args.extend(paths.iter().copied());
            let status = Command::new("git").args(&args).current_dir(repo).status()?;
            if !status.success() {
                anyhow::bail!("git add failed");
            }
            Ok("Staged changes".to_owned())
        }

        "git_commit" => {
            let message = input["message"].as_str().context("message required")?;
            let out = Command::new("git")
                .args(["commit", "-s", "-m", message])
                .current_dir(repo)
                .output()?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                anyhow::bail!("git commit failed: {stderr}");
            }
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        }

        "git_diff" => {
            let staged = input["staged"].as_bool().unwrap_or(false);
            let mut args = vec!["diff"];
            if staged {
                args.push("--staged");
            }
            let out = Command::new("git").args(&args).current_dir(repo).output()?;
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        }

        _ => {
            let spec = specs
                .iter()
                .find(|s| s.name == name)
                .with_context(|| format!("unknown tool: {name}"))?;

            if spec.command.first().map(|s| s.as_str()) == Some("__builtin__") {
                anyhow::bail!("unimplemented builtin: {name}");
            }

            // Build argv: base command + --key value for each input field
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

            let (bin, rest) = argv.split_first().context("empty command in manifest")?;
            let out = Command::new(bin)
                .args(rest)
                .current_dir(repo)
                .output()
                .with_context(|| format!("running {bin}"))?;

            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let combined = format!("{stdout}{stderr}");
            if !out.status.success() {
                anyhow::bail!("tool failed ({}): {combined}", out.status);
            }
            Ok(combined)
        }
    }
}

// ── agent loop ────────────────────────────────────────────────────────────────

fn system_prompt(repo: &str, prompt: &str) -> String {
    format!(
        "You are Butterfork's coding agent. Make the following change to the repository at \
         `{repo}`:\n\n{prompt}\n\n\
         Guidelines:\n\
         - Read relevant source files before modifying them.\n\
         - Make minimal, focused changes that address only the stated task.\n\
         - After making changes, run tests with run_shell (e.g. cargo test).\n\
         - Stage changed files with git_add, then commit with git_commit.\n\
         - Commit messages must be concise and include a DCO Signed-off-by line \
           (git_commit does this automatically with -s).\n\
         - Do not access paths outside the repository.\n\
         - When done, summarize what you changed and why."
    )
}

fn tool_to_claude(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "input_schema": spec.schema,
    })
}

fn run_agent_loop(
    client: &ClaudeClient,
    repo: &str,
    prompt: &str,
    tools: &[ToolSpec],
    max_iterations: u32,
) -> Result<()> {
    let system = system_prompt(repo, prompt);
    let claude_tools: Vec<Value> = tools.iter().map(tool_to_claude).collect();

    let mut messages: Vec<Value> = vec![json!({
        "role": "user",
        "content": [{"type": "text", "text": prompt}]
    })];

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;

    emit(&Event::Plan {
        steps: vec![
            "understand codebase".to_owned(),
            "apply changes".to_owned(),
            "run tests".to_owned(),
            "commit".to_owned(),
        ],
    });

    for iteration in 0..max_iterations {
        eprintln!("bf-agent: iteration {}/{max_iterations}", iteration + 1);

        let response = client.send(&system, &messages, &claude_tools)?;

        if let Some(u) = response.get("usage") {
            total_in += u["input_tokens"].as_u64().unwrap_or(0);
            total_out += u["output_tokens"].as_u64().unwrap_or(0);
        }

        let content = response["content"].as_array().cloned().unwrap_or_default();
        let stop_reason = response["stop_reason"].as_str().unwrap_or("");

        // Collect tool_use blocks; emit text messages
        let mut tool_calls: Vec<(String, String, Value)> = Vec::new();
        for block in &content {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        emit(&Event::Message {
                            text: text.to_owned(),
                        });
                    }
                }
                Some("tool_use") => {
                    let id = block["id"].as_str().unwrap_or("").to_owned();
                    let name = block["name"].as_str().unwrap_or("").to_owned();
                    let input = block["input"].clone();
                    tool_calls.push((id, name, input));
                }
                _ => {}
            }
        }

        // Push assistant turn with the raw content blocks
        messages.push(json!({"role": "assistant", "content": content}));

        if tool_calls.is_empty() || stop_reason == "end_turn" {
            eprintln!("bf-agent: done ({stop_reason}) — {total_in} in / {total_out} out tokens");
            emit(&Event::Done { exit_code: 0 });
            return Ok(());
        }

        // Execute each tool and build tool_result blocks
        let mut results: Vec<Value> = Vec::new();
        for (id, name, input) in &tool_calls {
            emit(&Event::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args: input.clone(),
            });

            let (output, is_error) = execute_tool(name, input, repo, tools);

            emit(&Event::ToolResult {
                id: id.clone(),
                output: Value::String(output.clone()),
                is_error,
            });

            results.push(json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": output,
                "is_error": is_error,
            }));
        }

        messages.push(json!({"role": "user", "content": results}));
    }

    eprintln!("bf-agent: reached max_iterations ({max_iterations})");
    emit(&Event::Done {
        exit_code: exit::TEMPFAIL,
    });
    std::process::exit(exit::TEMPFAIL);
}

// ── main ─────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_else(|_| {
        // Try keyring in a future phase; for now exit with a clear message.
        eprintln!("bf-agent: ANTHROPIC_API_KEY not set — export it or configure the OS keychain");
        std::process::exit(exit::CONFIG);
    });

    let manifest = load_manifest(&cli.tools)?;
    eprintln!(
        "bf-agent: loaded {} tool(s): {}",
        manifest.tools.len(),
        manifest
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    eprintln!("bf-agent: model={}", cli.model);

    let client = ClaudeClient::new(api_key, cli.model);
    run_agent_loop(
        &client,
        &cli.repo,
        &cli.prompt,
        &manifest.tools,
        cli.max_iterations,
    )
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        dir
    }

    #[test]
    fn read_write_roundtrip() {
        let dir = tmp_repo();
        let repo = dir.path().to_str().unwrap();
        let input = json!({"path": "hello.txt", "content": "world\n"});
        let (out, err) = execute_tool("write_file", &input, repo, &[]);
        assert!(!err, "write failed: {out}");
        let (content, err) = execute_tool("read_file", &json!({"path": "hello.txt"}), repo, &[]);
        assert!(!err, "read failed: {content}");
        assert_eq!(content, "world\n");
    }

    #[test]
    fn list_files_returns_entries() {
        let dir = tmp_repo();
        let repo = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        let (out, err) = execute_tool("list_files", &json!({"path": "."}), repo, &[]);
        assert!(!err);
        assert!(out.contains("a.txt"));
        assert!(out.contains("b.txt"));
    }

    #[test]
    fn run_shell_captures_output() {
        let dir = tmp_repo();
        let repo = dir.path().to_str().unwrap();
        let input = json!({"command": "echo", "args": ["hello world"]});
        let (out, err) = execute_tool("run_shell", &input, repo, &[]);
        assert!(!err);
        assert!(out.trim() == "hello world");
    }

    #[test]
    fn run_shell_propagates_failure() {
        let dir = tmp_repo();
        let repo = dir.path().to_str().unwrap();
        let input = json!({"command": "false", "args": []});
        let (_out, is_err) = execute_tool("run_shell", &input, repo, &[]);
        assert!(is_err);
    }
}
