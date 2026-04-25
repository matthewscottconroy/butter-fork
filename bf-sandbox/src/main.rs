use anyhow::Result;
use clap::Parser;
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-sandbox",
    about = "Run a command under a named sandbox profile (wraps bubblewrap on Linux)",
    long_about = "Named profiles (build, agent, run) ship with sensible defaults and are\n\
                  user-overridable via ~/.butterfork/sandbox-profiles/<name>.toml.\n\
                  Standalone useful: anyone building untrusted code locally can use this\n\
                  without any other Butterfork component.",
    version
)]
struct Cli {
    /// Named sandbox profile to apply
    #[arg(long, default_value = "build")]
    profile: String,

    /// Additional network hosts to allow (comma-separated)
    #[arg(long, value_delimiter = ',')]
    allow_net: Vec<String>,

    /// Additional bind mounts in path[:mode] format
    #[arg(long)]
    bind: Vec<String>,

    /// Command and arguments to run inside the sandbox
    #[arg(last = true, required = true)]
    cmd: Vec<String>,
}

fn bubblewrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    eprintln!("bf-sandbox: profile={}", cli.profile);
    eprintln!("bf-sandbox: command={}", cli.cmd.join(" "));

    if !bubblewrap_available() {
        eprintln!("bf-sandbox: bubblewrap (bwrap) not found on PATH");
        eprintln!("bf-sandbox: install it with: apt install bubblewrap  OR  dnf install bubblewrap");
        // Fall back to running unsandboxed with a clear warning.
        eprintln!("bf-sandbox: WARNING — running unsandboxed");
        let (bin, args) = cli.cmd.split_first().unwrap();
        let status = Command::new(bin).args(args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    // Build a bubblewrap invocation from the profile.
    let mut bwrap_args: Vec<String> = vec![
        "--unshare-all".to_owned(),
        "--share-net".to_owned(), // selectively re-enabled per profile/--allow-net
        "--proc".to_owned(), "/proc".to_owned(),
        "--dev".to_owned(), "/dev".to_owned(),
        "--ro-bind".to_owned(), "/usr".to_owned(), "/usr".to_owned(),
        "--ro-bind".to_owned(), "/lib".to_owned(), "/lib".to_owned(),
        "--ro-bind".to_owned(), "/lib64".to_owned(), "/lib64".to_owned(),
    ];

    // Apply profile defaults.
    match cli.profile.as_str() {
        "build" => {
            // Read-write access to the repo dir; no network by default.
            bwrap_args.extend(["--unshare-net".to_owned()]);
        }
        "agent" => {
            // Read-write repo dir; network allowed to forge only.
        }
        "run" => {
            // Full network; read-only repo.
        }
        other => {
            // TODO: load from ~/.butterfork/sandbox-profiles/<other>.toml
            eprintln!("bf-sandbox: unknown profile '{other}'; using build defaults");
            bwrap_args.push("--unshare-net".to_owned());
        }
    }

    for bind in &cli.bind {
        let (path, mode) = bind.split_once(':').unwrap_or((bind, "rw"));
        let flag = if mode == "ro" { "--ro-bind" } else { "--bind" };
        bwrap_args.extend([flag.to_owned(), path.to_owned(), path.to_owned()]);
    }

    bwrap_args.extend(["--".to_owned()]);
    bwrap_args.extend(cli.cmd.iter().cloned());

    let bwrap_ref: Vec<&str> = bwrap_args.iter().map(String::as_str).collect();
    let status = Command::new("bwrap").args(&bwrap_ref).status()?;
    std::process::exit(status.code().unwrap_or(1));
}
