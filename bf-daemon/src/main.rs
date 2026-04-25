use anyhow::{Context, Result};
use bf_common::{emit, Event};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

#[derive(Parser)]
#[command(
    name = "bf-daemon",
    about = "Optional supervisor for long-running builds, agent sessions, and PR watchers",
    long_about = "bf-daemon shells out to the same component binaries a user would invoke\n\
                  by hand; it owns no private capability. Users who don't want a daemon\n\
                  can script their workflows directly. The daemon is a convenience, not\n\
                  a chokepoint — Butterfork without a daemon is still Butterfork.\n\n\
                  IPC: Unix-domain socket at $XDG_RUNTIME_DIR/butterfork.sock\n\
                  Log: ~/.butterfork/logs/daemon.log",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the daemon (background by default; use --foreground to stay attached)
    Start {
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the running daemon gracefully
    Stop,
    /// Show daemon status and active task list
    Status,
    /// Print daemon log output
    Log {
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// Watch a pull request URL for state changes
    WatchPr {
        /// Full PR URL (e.g. https://github.com/owner/repo/pull/42)
        url: String,
        /// Project slug for notification labels
        #[arg(long, default_value = "unknown")]
        slug: String,
    },
}

// ── paths ─────────────────────────────────────────────────────────────────────

fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(runtime_dir).join("butterfork.sock")
}

fn log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.butterfork/logs/daemon.log"))
}

fn pid_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.butterfork/daemon.pid"))
}

// ── task model ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    PrWatch,
    Build,
    AgentRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub label: String,
    pub started_at: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn new_task_id() -> String {
    format!("{:x}", now_secs() ^ (std::process::id() as u64))
}

// ── shared daemon state ───────────────────────────────────────────────────────

#[derive(Default)]
struct DaemonState {
    tasks: HashMap<String, Task>,
    shutdown: bool,
}

type SharedState = Arc<Mutex<DaemonState>>;

// ── socket protocol ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum SocketCommand {
    Status,
    Shutdown,
    WatchPr { url: String, slug: Option<String> },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SocketResponse {
    Status { tasks: Vec<Task> },
    TaskCreated { id: String },
    Ok,
    Error { message: String },
}

fn write_response(resp: &SocketResponse) -> String {
    serde_json::to_string(resp).unwrap_or_else(|_| r#"{"type":"error","message":"serialize error"}"#.to_owned())
}

// ── connection handler ────────────────────────────────────────────────────────

async fn handle_connection(
    stream: UnixStream,
    state: SharedState,
    log_tx: tokio::sync::mpsc::Sender<String>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let cmd: SocketCommand = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                let resp = write_response(&SocketResponse::Error { message: e.to_string() });
                let _ = writer.write_all(format!("{resp}\n").as_bytes()).await;
                continue;
            }
        };

        match cmd {
            SocketCommand::Status => {
                let tasks: Vec<Task> = state.lock().unwrap().tasks.values().cloned().collect();
                let resp = write_response(&SocketResponse::Status { tasks });
                let _ = writer.write_all(format!("{resp}\n").as_bytes()).await;
            }

            SocketCommand::Shutdown => {
                state.lock().unwrap().shutdown = true;
                let resp = write_response(&SocketResponse::Ok);
                let _ = writer.write_all(format!("{resp}\n").as_bytes()).await;
                break;
            }

            SocketCommand::WatchPr { url, slug } => {
                let id = new_task_id();
                let label = format!("pr-watch: {}", slug.as_deref().unwrap_or("unknown"));
                let task = Task {
                    id: id.clone(),
                    kind: TaskKind::PrWatch,
                    status: TaskStatus::Running,
                    label: label.clone(),
                    started_at: now_secs(),
                };
                state.lock().unwrap().tasks.insert(id.clone(), task);

                let resp = write_response(&SocketResponse::TaskCreated { id: id.clone() });
                let _ = writer.write_all(format!("{resp}\n").as_bytes()).await;

                // Spawn PR watcher as a background task.
                let state2 = state.clone();
                let log_tx2 = log_tx.clone();
                tokio::spawn(async move {
                    pr_watcher(id, url, state2, log_tx2).await;
                });
            }
        }
    }
}

// ── PR watcher ────────────────────────────────────────────────────────────────

async fn pr_watcher(
    task_id: String,
    url: String,
    state: SharedState,
    log_tx: tokio::sync::mpsc::Sender<String>,
) {
    let _ = log_tx
        .send(format!("pr-watcher [{task_id}]: watching {url}"))
        .await;

    let mut last_state = String::new();
    let poll_interval = tokio::time::Duration::from_secs(60);

    loop {
        // Poll with `gh pr view`
        let output = tokio::process::Command::new("gh")
            .args(["pr", "view", &url, "--json", "state,title,isDraft"])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    let pr_state = v["state"].as_str().unwrap_or("UNKNOWN").to_owned();
                    let title = v["title"].as_str().unwrap_or("").to_owned();

                    if pr_state != last_state {
                        let msg = format!("PR '{}' state → {} ({})", title, pr_state, url);
                        let _ = log_tx.send(format!("pr-watcher [{task_id}]: {msg}")).await;
                        emit(&Event::Message { text: msg });
                        last_state = pr_state.clone();
                    }

                    // Terminal states — stop polling.
                    if matches!(pr_state.as_str(), "MERGED" | "CLOSED") {
                        // Drop the guard before any await point.
                        {
                            let mut s = state.lock().unwrap();
                            if let Some(t) = s.tasks.get_mut(&task_id) {
                                t.status = TaskStatus::Done;
                            }
                        }
                        let _ = log_tx
                            .send(format!("pr-watcher [{task_id}]: done ({pr_state})"))
                            .await;
                        return;
                    }
                }
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let _ = log_tx
                    .send(format!("pr-watcher [{task_id}]: gh error: {}", err.trim()))
                    .await;
            }
            Err(e) => {
                let _ = log_tx
                    .send(format!("pr-watcher [{task_id}]: poll error: {e}"))
                    .await;
            }
        }

        // Check if daemon is shutting down before sleeping.
        if state.lock().unwrap().shutdown {
            return;
        }
        tokio::time::sleep(poll_interval).await;
        if state.lock().unwrap().shutdown {
            return;
        }
    }
}

// ── logger task ───────────────────────────────────────────────────────────────

async fn logger_task(log: PathBuf, mut rx: tokio::sync::mpsc::Receiver<String>) {
    use tokio::io::AsyncWriteExt as _;
    if let Some(parent) = log.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .await;
    let mut file = match file {
        Ok(f) => f,
        Err(e) => {
            eprintln!("bf-daemon: cannot open log file {}: {e}", log.display());
            return;
        }
    };
    while let Some(line) = rx.recv().await {
        let ts = now_secs();
        let entry = format!("[{ts}] {line}\n");
        eprintln!("bf-daemon: {line}");
        let _ = file.write_all(entry.as_bytes()).await;
    }
}

// ── server loop ───────────────────────────────────────────────────────────────

async fn serve(foreground: bool) -> Result<()> {
    let sock = socket_path();

    if sock.exists() {
        // Stale socket check — try to connect; if it fails the daemon is dead.
        if UnixStream::connect(&sock).await.is_ok() {
            anyhow::bail!(
                "daemon already running (socket at {})",
                sock.display()
            );
        }
        std::fs::remove_file(&sock)?;
    }

    let listener = UnixListener::bind(&sock)
        .with_context(|| format!("binding socket at {}", sock.display()))?;

    // Write PID file.
    let pid = std::process::id();
    let pidfile = pid_path();
    if let Some(p) = pidfile.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&pidfile, pid.to_string())?;

    // Logger channel.
    let (log_tx, log_rx) = tokio::sync::mpsc::channel::<String>(256);
    tokio::spawn(logger_task(log_path(), log_rx));

    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));

    eprintln!(
        "bf-daemon: listening on {} (pid {pid}, foreground={foreground})",
        sock.display()
    );
    let _ = log_tx
        .send(format!("daemon started (pid {pid})"))
        .await;

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let st = state.clone();
                        let tx = log_tx.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, st, tx).await;
                        });
                    }
                    Err(e) => {
                        eprintln!("bf-daemon: accept error: {e}");
                    }
                }
            }
        }

        if state.lock().unwrap().shutdown {
            eprintln!("bf-daemon: shutdown requested — stopping");
            break;
        }
    }

    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_file(&pid_path());
    let _ = log_tx.send("daemon stopped".to_owned()).await;
    Ok(())
}

// ── background start ──────────────────────────────────────────────────────────

fn start_background() -> Result<()> {
    let exe = std::env::current_exe().context("resolving own executable path")?;
    let log = log_path();
    if let Some(p) = log.parent() {
        std::fs::create_dir_all(p)?;
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("opening log {}", log.display()))?;
    let child = std::process::Command::new(exe)
        .args(["start", "--foreground"])
        .stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()
        .context("spawning background daemon")?;
    eprintln!("bf-daemon: started in background (pid {})", child.id());
    eprintln!("bf-daemon: log → {}", log.display());
    Ok(())
}

// ── client helpers ────────────────────────────────────────────────────────────

async fn send_command(cmd: &serde_json::Value) -> Result<String> {
    let sock = socket_path();
    let stream = UnixStream::connect(&sock)
        .await
        .with_context(|| format!("connecting to {} — is the daemon running?", sock.display()))?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(format!("{}\n", serde_json::to_string(cmd)?).as_bytes())
        .await?;
    let mut lines = BufReader::new(reader).lines();
    Ok(lines.next_line().await?.unwrap_or_default())
}

// ── entry point ───────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        DaemonCommand::Start { foreground } => {
            if foreground {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?
                    .block_on(serve(true))?;
            } else {
                start_background()?;
            }
        }

        DaemonCommand::Stop => {
            let resp = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(send_command(&serde_json::json!({"cmd": "shutdown"})))?;
            eprintln!("bf-daemon: {resp}");
        }

        DaemonCommand::Status => {
            let resp = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(send_command(&serde_json::json!({"cmd": "status"})))?;

            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp) {
                let tasks = v["tasks"].as_array().cloned().unwrap_or_default();
                if tasks.is_empty() {
                    eprintln!("bf-daemon: running, no active tasks");
                } else {
                    eprintln!("bf-daemon: {} active task(s)", tasks.len());
                    for t in &tasks {
                        println!("{}", serde_json::to_string_pretty(t).unwrap_or_default());
                    }
                }
            } else {
                eprintln!("bf-daemon: not running (could not reach socket)");
                eprintln!("bf-daemon: start with `bf-daemon start`");
                std::process::exit(bf_common::exit::UNAVAILABLE);
            }
        }

        DaemonCommand::Log { follow } => {
            let log = log_path();
            if !log.exists() {
                eprintln!("bf-daemon: no log file at {} — daemon may never have run", log.display());
                return Ok(());
            }
            if follow {
                let status = std::process::Command::new("tail")
                    .args(["-f", &log.to_string_lossy()])
                    .status()?;
                std::process::exit(status.code().unwrap_or(1));
            } else {
                print!("{}", std::fs::read_to_string(&log)?);
            }
        }

        DaemonCommand::WatchPr { url, slug } => {
            let cmd = serde_json::json!({
                "cmd": "watch_pr",
                "url": url,
                "slug": slug,
            });
            let resp = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(send_command(&cmd))
                .unwrap_or_else(|_| String::new());

            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp) {
                if let Some(id) = v["id"].as_str() {
                    eprintln!("bf-daemon: PR watch task created (id: {id})");
                    emit(&Event::Message {
                        text: format!("Watching PR {url} (task {id})"),
                    });
                } else {
                    eprintln!("bf-daemon: daemon not running — start with `bf-daemon start`");
                    eprintln!("bf-daemon: falling back to direct poll (no persistence)");
                    // Run a simple synchronous poll loop in-process.
                    direct_pr_poll(&url)?;
                }
            } else {
                eprintln!("bf-daemon: daemon not reachable — running direct PR poll");
                direct_pr_poll(&url)?;
            }
        }
    }

    Ok(())
}

/// Synchronous PR poll used as a fallback when the daemon is not running.
fn direct_pr_poll(url: &str) -> Result<()> {
    eprintln!("bf-daemon: polling {url} every 60s (Ctrl-C to stop)");
    let mut last_state = String::new();
    loop {
        let out = std::process::Command::new("gh")
            .args(["pr", "view", url, "--json", "state,title"])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    let pr_state = v["state"].as_str().unwrap_or("UNKNOWN").to_owned();
                    let title = v["title"].as_str().unwrap_or("").to_owned();
                    if pr_state != last_state {
                        eprintln!("bf-daemon: PR '{}' → {}", title, pr_state);
                        emit(&Event::Message {
                            text: format!("PR '{}' state → {} ({})", title, pr_state, url),
                        });
                        last_state = pr_state.clone();
                    }
                    if matches!(pr_state.as_str(), "MERGED" | "CLOSED") {
                        eprintln!("bf-daemon: PR reached terminal state — done");
                        return Ok(());
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
