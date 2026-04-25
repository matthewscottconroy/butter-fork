//! Busybox-style fat binary for Butterfork.
//!
//! Dispatch is determined by `argv[0]` (the symlink name used to invoke the binary).
//! Install with `scripts/fat-install.sh`, which builds this binary and creates
//! one symlink per component under `~/.butterfork/bin/`.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    // argv[0] may be a full path or just the program name; take the final component.
    let argv0 = std::env::args().next().unwrap_or_default();
    let name = Path::new(&argv0)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    match name.as_str() {
        "bf" => bf::run(),
        "bf-agent" => bf_agent::run(),
        "bf-agent-ollama" => bf_agent_ollama::run(),
        "bf-bootstrap" => bf_bootstrap::run(),
        "bf-build" => bf_build::run(),
        "bf-build-cargo" => bf_build_cargo::run(),
        "bf-build-cmake" => bf_build_cmake::run(),
        "bf-build-meson" => bf_build_meson::run(),
        "bf-build-npm" => bf_build_npm::run(),
        "bf-catalog" => bf_catalog::run(),
        "bf-forge" => bf_forge::run(),
        "bf-forge-github" => bf_forge_github::run(),
        "bf-forge-gitlab" => bf_forge_gitlab::run(),
        "bf-index" => bf_index::run(),
        "bf-install" => bf_install::run(),
        "bf-sandbox" => bf_sandbox::run(),
        "bf-scaffold" => bf_scaffold::run(),
        "bf-fat" | "" => {
            // Invoked directly — print a usage summary.
            eprintln!("bf-fat: Butterfork fat binary");
            eprintln!("Usage: invoke via a component symlink, e.g.:");
            eprintln!("  ln -s bf-fat ~/.butterfork/bin/bf");
            eprintln!("  ln -s bf-fat ~/.butterfork/bin/bf-agent");
            eprintln!("");
            eprintln!("Available components:");
            for comp in COMPONENTS {
                eprintln!("  {comp}");
            }
            std::process::exit(64); // EX_USAGE
        }
        other => {
            eprintln!("bf-fat: unknown component '{other}'");
            eprintln!("Run bf-fat directly to see available components.");
            std::process::exit(64);
        }
    }
}

const COMPONENTS: &[&str] = &[
    "bf",
    "bf-agent",
    "bf-agent-ollama",
    "bf-bootstrap",
    "bf-build",
    "bf-build-cargo",
    "bf-build-cmake",
    "bf-build-meson",
    "bf-build-npm",
    "bf-catalog",
    "bf-forge",
    "bf-forge-github",
    "bf-forge-gitlab",
    "bf-index",
    "bf-install",
    "bf-sandbox",
    "bf-scaffold",
];
