/// Integration tests for bf-build-cargo.
///
/// These tests create real temporary Cargo projects and exercise the adapter's
/// detect/plan/run pipeline. They require the `cargo` binary on PATH — which is
/// always true in a normal Rust development environment.
use std::fs;
use std::process::Command;
use tempfile::TempDir;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Return the path to the bf-build-cargo binary built by this test run.
fn bin_path() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // strip test binary name
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("bf-build-cargo")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin_path())
        .args(args)
        .output()
        .expect("bf-build-cargo should be runnable")
}

/// Create a minimal Cargo project with a single binary in `dir`.
fn make_toy_crate(dir: &TempDir, name: &str) -> String {
    let root = dir.path().join(name);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [[bin]]\nname = \"{name}\"\npath = \"src/main.rs\"\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"hello from test binary\"); }\n",
    )
    .unwrap();
    root.to_string_lossy().to_string()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
fn detect_returns_0_for_cargo_repo() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_toy_crate(&dir, "myapp");
    let out = run(&["detect", &path]);
    assert!(
        out.status.success(),
        "detect should exit 0 for a Cargo project"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let det: serde_json::Value = serde_json::from_str(
        stdout.lines().next().unwrap_or("{}"),
    )
    .unwrap();
    assert_eq!(det["adapter"].as_str(), Some("bf-build-cargo"));
    assert!(det["confidence"].as_f64().unwrap_or(0.0) > 0.9);
}

#[test]
fn detect_returns_nonzero_for_non_cargo_dir() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["detect", &dir.path().to_string_lossy()]);
    assert!(
        !out.status.success(),
        "detect should exit non-zero for a non-Cargo directory"
    );
}

#[test]
fn plan_emits_build_plan_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_toy_crate(&dir, "planned");
    let out = run(&["plan", &path]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let plan: serde_json::Value =
        serde_json::from_str(stdout.lines().next().unwrap_or("{}")).unwrap();
    assert_eq!(plan["adapter"].as_str(), Some("bf-build-cargo"));
    assert!(plan["steps"].as_array().map(|s| !s.is_empty()).unwrap_or(false));
}

#[test]
fn run_builds_and_writes_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_toy_crate(&dir, "helloworld");

    let out = run(&["run", &path]); // debug mode is fine for the test
    assert!(
        out.status.success(),
        "bf-build-cargo run should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let manifest_path = format!("{path}/target/bf-artifact-manifest.json");
    assert!(
        std::path::Path::new(&manifest_path).exists(),
        "artifact manifest should be written to {manifest_path}"
    );

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["project"].as_str(), Some("helloworld"));
    let artifacts = manifest["artifacts"].as_array().unwrap();
    assert!(!artifacts.is_empty(), "at least one artifact expected");
    assert_eq!(
        artifacts[0]["dest"].as_str(),
        Some("bin/helloworld"),
        "dest should be relative (bin/<name>)"
    );
}

#[test]
fn run_emits_build_complete_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_toy_crate(&dir, "evented");

    let out = run(&["run", &path]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let has_build_complete = stdout.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .map(|v| v["type"].as_str() == Some("build-complete"))
            .unwrap_or(false)
    });
    assert!(has_build_complete, "expected a build-complete NDJSON event on stdout");
}
