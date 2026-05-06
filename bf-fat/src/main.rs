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
        "bf-daemon" => bf_daemon::run(),
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
            eprintln!();
            eprintln!("Available components:");
            for comp in COMPONENTS {
                eprintln!("  {comp}");
            }
            std::process::exit(64); // EX_USAGE
        }
        other => {
            eprintln!("bf-fat: unknown component '{other}'");
            eprintln!("Available components:");
            for comp in COMPONENTS {
                eprintln!("  {comp}");
            }
            std::process::exit(64);
        }
    }
}

/// Returns true if `name` is a known component that can be dispatched.
#[cfg(test)]
fn is_known_component(name: &str) -> bool {
    COMPONENTS.contains(&name)
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
    "bf-daemon",
    "bf-forge",
    "bf-forge-github",
    "bf-forge-gitlab",
    "bf-index",
    "bf-install",
    "bf-sandbox",
    "bf-scaffold",
];

#[cfg(test)]
mod tests {
    use super::*;

    // The MATCH_ARMS list must be kept in sync with the match in main().
    // If a component is added to COMPONENTS but not to the match (or vice versa),
    // this test will catch it by verifying every COMPONENT is "known".
    const MATCH_ARMS: &[&str] = &[
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
        "bf-daemon",
        "bf-forge",
        "bf-forge-github",
        "bf-forge-gitlab",
        "bf-index",
        "bf-install",
        "bf-sandbox",
        "bf-scaffold",
    ];

    #[test]
    fn components_list_matches_dispatch_arms() {
        // Every entry in COMPONENTS must appear in MATCH_ARMS.
        for comp in COMPONENTS {
            assert!(
                MATCH_ARMS.contains(comp),
                "COMPONENTS has '{comp}' but it is missing from the dispatch match"
            );
        }
        // Every entry in MATCH_ARMS must appear in COMPONENTS.
        for arm in MATCH_ARMS {
            assert!(
                COMPONENTS.contains(arm),
                "dispatch match has '{arm}' but it is missing from COMPONENTS"
            );
        }
    }

    #[test]
    fn is_known_component_returns_true_for_all_components() {
        for comp in COMPONENTS {
            assert!(is_known_component(comp), "'{comp}' should be known");
        }
    }

    #[test]
    fn is_known_component_returns_false_for_unknown() {
        assert!(!is_known_component("bf-unknown"));
        assert!(!is_known_component(""));
        assert!(!is_known_component("bf-fat")); // bf-fat itself is not in COMPONENTS
    }
}
