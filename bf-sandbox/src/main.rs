use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-sandbox",
    about = "Run a command under a named sandbox profile",
    long_about = "Named profiles (build, agent, run) ship with sensible defaults.\n\
                  Override by placing a TOML file at\n\
                  ~/.butterfork/sandbox-profiles/<name>.toml.\n\n\
                  Backend selection (in order of preference):\n\
                  1. bubblewrap (bwrap) — Linux kernel namespaces, lowest overhead\n\
                  2. Podman       — rootless container, cross-platform\n\
                  3. Docker       — container runtime fallback\n\
                  4. Unsandboxed  — explicit warning, for CI or trusted environments\n\n\
                  Set BF_SANDBOX_IMAGE to override the container image used by Podman/Docker.\n\
                  Set BF_SANDBOX=none to force unsandboxed execution.",
    version
)]
struct Cli {
    /// Named sandbox profile (build | agent | run | custom)
    #[arg(long, default_value = "build")]
    profile: String,

    /// Additional network hosts to allow (comma-separated, advisory)
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
    unshare_net: Option<bool>,
    ro_binds: Vec<String>,
    rw_binds: Vec<String>,
}

fn load_profile(name: &str) -> ProfileConfig {
    let home = std::env::var("HOME").unwrap_or_default();
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    let path = format!("{bf_home}/sandbox-profiles/{name}.toml");

    if let Ok(s) = std::fs::read_to_string(&path) {
        match toml::from_str::<ProfileConfig>(&s) {
            Ok(cfg) => {
                eprintln!("bf-sandbox: loaded profile from {path}");
                return cfg;
            }
            Err(e) => eprintln!("bf-sandbox: warning: could not parse {path}: {e}"),
        }
    }

    match name {
        "build" => ProfileConfig {
            unshare_net: Some(true),
            ..Default::default()
        },
        "agent" => ProfileConfig {
            unshare_net: Some(false),
            ..Default::default()
        },
        "run" => ProfileConfig {
            unshare_net: Some(false),
            ..Default::default()
        },
        _ => {
            eprintln!("bf-sandbox: unknown profile '{name}'; using build defaults");
            ProfileConfig {
                unshare_net: Some(true),
                ..Default::default()
            }
        }
    }
}

// ── backend detection ─────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum Backend {
    Bwrap,
    Podman,
    Docker,
    None,
}

fn detect_backend() -> Backend {
    if std::env::var("BF_SANDBOX").as_deref() == Ok("none") {
        return Backend::None;
    }

    // bubblewrap is Linux-only and the lightest-weight option.
    if cfg!(target_os = "linux") && tool_available("bwrap") {
        return Backend::Bwrap;
    }

    if tool_available("podman") {
        return Backend::Podman;
    }
    if tool_available("docker") {
        return Backend::Docker;
    }
    Backend::None
}

fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── bubblewrap ────────────────────────────────────────────────────────────────

fn build_bwrap_args(profile: &ProfileConfig, extra_binds: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--proc".to_owned(),
        "/proc".to_owned(),
        "--dev".to_owned(),
        "/dev".to_owned(),
    ];

    for sys in &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
        if Path::new(sys).exists() {
            args.extend(["--ro-bind".to_owned(), sys.to_string(), sys.to_string()]);
        }
    }

    if profile.unshare_net.unwrap_or(true) {
        args.push("--unshare-net".to_owned());
    }

    for path in &profile.ro_binds {
        args.extend(["--ro-bind".to_owned(), path.clone(), path.clone()]);
    }
    for path in &profile.rw_binds {
        args.extend(["--bind".to_owned(), path.clone(), path.clone()]);
    }

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

fn run_bwrap(profile: &ProfileConfig, extra_binds: &[String], cmd: &[String]) -> Result<()> {
    let mut bwrap_args = build_bwrap_args(profile, extra_binds);
    bwrap_args.push("--".to_owned());
    bwrap_args.extend_from_slice(cmd);
    let refs: Vec<&str> = bwrap_args.iter().map(String::as_str).collect();
    let status = Command::new("bwrap").args(&refs).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

// ── container runtime (Podman / Docker) ──────────────────────────────────────

fn container_image() -> String {
    std::env::var("BF_SANDBOX_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".to_owned())
}

fn build_container_args(
    runtime: &str,
    profile: &ProfileConfig,
    extra_binds: &[String],
    cmd: &[String],
) -> Vec<String> {
    let mut args = vec![
        "run".to_owned(),
        "--rm".to_owned(),
        "--interactive".to_owned(),
    ];

    // Drop all capabilities and run as current user.
    args.extend(["--cap-drop=ALL".to_owned()]);
    if let Ok(uid) = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    {
        if let Ok(gid) = std::process::Command::new("id")
            .arg("-g")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        {
            args.push(format!("--user={uid}:{gid}"));
        }
    }

    // Network isolation.
    if profile.unshare_net.unwrap_or(true) {
        args.push("--network=none".to_owned());
    }

    // Working directory — use $HOME if available.
    if let Ok(home) = std::env::var("HOME") {
        args.extend([
            "-v".to_owned(),
            format!("{home}:{home}"),
            "-w".to_owned(),
            home,
        ]);
    }

    // Profile binds.
    for path in &profile.ro_binds {
        // Podman/Docker don't have separate ro-bind; append :ro option.
        args.extend(["-v".to_owned(), format!("{path}:{path}:ro")]);
    }
    for path in &profile.rw_binds {
        args.extend(["-v".to_owned(), format!("{path}:{path}")]);
    }

    // CLI extra binds.
    for bind_spec in extra_binds {
        let (path, mode) = bind_spec.split_once(':').unwrap_or((bind_spec, "rw"));
        if !Path::new(path).exists() {
            eprintln!("bf-sandbox: warning: bind path does not exist: {path}");
            continue;
        }
        let vol = if mode == "ro" {
            format!("{path}:{path}:ro")
        } else {
            format!("{path}:{path}")
        };
        args.extend(["-v".to_owned(), vol]);
    }

    // Podman-specific: use host's user namespace for seamless file ownership.
    if runtime == "podman" {
        args.push("--userns=keep-id".to_owned());
    }

    args.push(container_image());
    args.extend_from_slice(cmd);
    args
}

fn run_container(
    runtime: &str,
    profile: &ProfileConfig,
    extra_binds: &[String],
    cmd: &[String],
) -> Result<()> {
    let args = build_container_args(runtime, profile, extra_binds, cmd);
    eprintln!("bf-sandbox: using {runtime} (image: {})", container_image());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let status = Command::new(runtime).args(&refs).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

// ── entry point ───────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    eprintln!(
        "bf-sandbox: profile={} command={}",
        cli.profile,
        cli.cmd.join(" ")
    );

    let backend = detect_backend();
    eprintln!("bf-sandbox: backend={backend:?}");

    if !cli.allow_net.is_empty() {
        eprintln!(
            "bf-sandbox: note: --allow-net is advisory; \
             per-host filtering requires a proxy and is not yet enforced"
        );
    }

    let profile = load_profile(&cli.profile);

    match backend {
        Backend::Bwrap => run_bwrap(&profile, &cli.bind, &cli.cmd)?,
        Backend::Podman => run_container("podman", &profile, &cli.bind, &cli.cmd)?,
        Backend::Docker => run_container("docker", &profile, &cli.bind, &cli.cmd)?,
        Backend::None => {
            eprintln!(
                "bf-sandbox: WARNING — no sandbox backend found \
                 (no bwrap, podman, or docker on PATH)"
            );
            eprintln!(
                "bf-sandbox: running unsandboxed. \
                 Install bubblewrap (Linux), Podman, or Docker for isolation."
            );
            let (bin, rest) = cli.cmd.split_first().context("empty command")?;
            let status = Command::new(bin).args(rest).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
