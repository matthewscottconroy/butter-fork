use anyhow::Result;
use bf_common::{emit, Event};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "bf-daemon",
    about = "Optional supervisor for long-running builds, agent sessions, and PR watchers",
    long_about = "bf-daemon shells out to the same component binaries a user would invoke\n\
                  by hand; it owns no private capability. Users who don't want a daemon\n\
                  can script their workflows directly. The daemon is a convenience, not\n\
                  a chokepoint — Butterfork without a daemon is still Butterfork.\n\n\
                  IPC: Unix-domain socket at $XDG_RUNTIME_DIR/butterfork.sock",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the daemon in the background
    Start {
        /// Run in the foreground instead of daemonizing
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the running daemon gracefully
    Stop,
    /// Show daemon status and active task list
    Status,
    /// Print daemon log output
    Log {
        /// Follow (tail -f) the log output
        #[arg(long, short = 'f')]
        follow: bool,
    },
}

fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(runtime_dir).join("butterfork.sock")
}

fn log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.butterfork/logs/daemon.log"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        DaemonCommand::Start { foreground } => {
            let sock = socket_path();
            eprintln!("bf-daemon: socket will be at {}", sock.display());
            if foreground {
                eprintln!("bf-daemon: running in foreground (--foreground)");
            } else {
                eprintln!("bf-daemon: daemonizing");
            }
            // TODO (Phase 3): implement the actual supervisor loop.
            // - Listen on Unix socket at socket_path()
            // - Accept task submissions (long builds, agent runs, PR watches)
            // - Write checkpoints to ~/.butterfork/state.db
            // - Support graceful shutdown and task draining
            emit(&Event::Message {
                text: "Daemon not yet implemented (Phase 3)".to_owned(),
            });
            std::process::exit(bf_common::exit::UNAVAILABLE);
        }

        DaemonCommand::Stop => {
            let sock = socket_path();
            if !sock.exists() {
                eprintln!("bf-daemon: no socket found at {} — daemon may not be running", sock.display());
                std::process::exit(bf_common::exit::UNAVAILABLE);
            }
            eprintln!("bf-daemon: sending stop signal");
            // TODO: connect to socket and send a graceful shutdown message.
            eprintln!("bf-daemon: stop not yet implemented");
            std::process::exit(bf_common::exit::UNAVAILABLE);
        }

        DaemonCommand::Status => {
            let sock = socket_path();
            if sock.exists() {
                eprintln!("bf-daemon: socket found at {}", sock.display());
                // TODO: query daemon for active task list.
            } else {
                eprintln!("bf-daemon: not running (no socket at {})", sock.display());
            }
        }

        DaemonCommand::Log { follow } => {
            let log = log_path();
            if !log.exists() {
                eprintln!("bf-daemon: no log file found at {}", log.display());
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
    }

    Ok(())
}

