use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-sandbox",
    about = "Run a command under a named sandbox profile (wraps bubblewrap on Linux)",
    long_about = "Named profiles (build, agent, run) ship with sensible defaults.\n\
                  Override by placing a TOML file at\n\
                  ~/.butterfork/sandbox-profiles/<name>.toml.\n\
                  Falls back to unsandboxed execution if bwrap is not installed,\n\
                  with a clear warning. Standalone useful without the rest of Butterfork.",
    version
)]
struct Cli {
    /// Named sandbox profile (build | agent | run | custom)
    #[arg(long, default_value = "build")]
    profile: String,

    /// Additional network hosts to allow (comma-separated, not yet enforced at bwrap level)
    #[arg(long, value_delimiter = ',')]
    allow_net: Vec<String>,

    /// Additional bind mounts in path[:ro|rw] format
    #[arg(long)]
    bind: Vec<String>,

    /// Command and arguments to run inside the sandbox
    #[arg(last = true, required = true)]
    cmd: Vec<String>,
}

// ── profile TOML ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProfileConfig {
    /// Isolate network namespace (default: true for build, false for run)
    unshare_net: Option<bool>,
    /// Additional read-only bind mounts (absolute paths)
    ro_binds: Vec<String>,
    /// Additional read-write bind mounts (absolute paths)
    rw_binds: Vec<String>,
}

fn load_profile(name: &str) -> ProfileConfig {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home =
        std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    let path = format!("{bf_home}/sandbox-profiles/{name}.toml");

    if let Ok(s) = std::fs::read_to_string(&path) {
        match toml::from_str::<ProfileConfig>(&s) {
            Ok(cfg) => {
                eprintln!("bf-sandbox: loaded profile from {path}");
                return cfg;
            }
            Err(e) => {
                eprintln!("bf-sandbox: warning: could not parse {path}: {e}");
            }
        }
    }

    // Built-in profile defaults
    match name {
        "build" => ProfileConfig {
            unshare_net: Some(true),
            ro_binds: vec![],
            rw_binds: vec![],
        },
        "agent" => ProfileConfig {
            // Agent needs network to reach the forge
            unshare_net: Some(false),
            ro_binds: vec![],
            rw_binds: vec![],
        },
        "run" => ProfileConfig {
            unshare_net: Some(false),
            ro_binds: vec![],
            rw_binds: vec![],
        },
        _ => {
            eprintln!("bf-sandbox: unknown profile '{name}'; using build defaults");
            ProfileConfig {
                unshare_net: Some(true),
                ro_binds: vec![],
                rw_binds: vec![],
            }
        }
    }
}

// ── bubblewrap invocation ─────────────────────────────────────────────────────

fn bubblewrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn build_bwrap_args(profile: &ProfileConfig, extra_binds: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--proc".to_owned(),
        "/proc".to_owned(),
        "--dev".to_owned(),
        "/dev".to_owned(),
    ];

    // Read-only system paths
    for sys in &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
        let p = Path::new(sys);
        if p.exists() {
            args.extend([
                "--ro-bind".to_owned(),
                sys.to_string(),
                sys.to_string(),
            ]);
        }
    }

    // Network isolation
    if profile.unshare_net.unwrap_or(true) {
        args.push("--unshare-net".to_owned());
    }

    // Profile ro_binds
    for path in &profile.ro_binds {
        args.extend(["--ro-bind".to_owned(), path.clone(), path.clone()]);
    }

    // Profile rw_binds
    for path in &profile.rw_binds {
        args.extend(["--bind".to_owned(), path.clone(), path.clone()]);
    }

    // Extra --bind args from CLI
    for bind_spec in extra_binds {
        let (path, mode) = bind_spec.split_once(':').unwrap_or((bind_spec, "rw"));
        let flag = if mode == "ro" { "--ro-bind" } else { "--bind" };
        if Path::new(path).exists() {
            args.extend([flag.to_owned(), path.to_owned(), path.to_owned()]);
        } else {
            eprintln!("bf-sandbox: warning: bind path does not exist: {path}");
        }
    }

    args
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    eprintln!("bf-sandbox: profile={} command={}", cli.profile, cli.cmd.join(" "));

    if !bubblewrap_available() {
        eprintln!(
            "bf-sandbox: bubblewrap (bwrap) not found — \
             install with: apt install bubblewrap  OR  dnf install bubblewrap"
        );
        eprintln!("bf-sandbox: WARNING — running unsandboxed");
        let (bin, rest) = cli
            .cmd
            .split_first()
            .context("empty command")?;
        let status = Command::new(bin).args(rest).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    let profile = load_profile(&cli.profile);
    let mut bwrap_args = build_bwrap_args(&profile, &cli.bind);

    // Warn if --allow-net was given but net-isolation is active
    if !cli.allow_net.is_empty() && profile.unshare_net.unwrap_or(true) {
        eprintln!(
            "bf-sandbox: note: --allow-net is advisory only; \
             per-host filtering requires a user-space proxy and is not yet implemented"
        );
    }

    bwrap_args.push("--".to_owned());
    bwrap_args.extend(cli.cmd.iter().cloned());

    let refs: Vec<&str> = bwrap_args.iter().map(String::as_str).collect();
    let status = Command::new("bwrap").args(&refs).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
