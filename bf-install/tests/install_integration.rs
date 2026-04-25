/// Integration tests for bf-install.
///
/// Each test sets BF_HOME to a temporary directory so nothing touches
/// ~/.butterfork on the developer's machine.
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn bin_path() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("bf-install")
}

fn run_with_home(home: &TempDir, args: &[&str]) -> std::process::Output {
    Command::new(bin_path())
        .args(args)
        .env("BF_HOME", home.path())
        .output()
        .expect("bf-install should be runnable")
}

/// Build a minimal ArtifactManifest JSON and a fake binary, return manifest path.
fn make_manifest(home: &TempDir, project: &str, bin_name: &str) -> String {
    // Create a fake source binary.
    let src_dir = home.path().join("fake-src");
    fs::create_dir_all(&src_dir).unwrap();
    let src_bin = src_dir.join(bin_name);
    fs::write(&src_bin, "#!/bin/sh\necho hello\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&src_bin).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&src_bin, p).unwrap();
    }

    let manifest = serde_json::json!({
        "project": project,
        "git_ref": "abc1234",
        "built_at": "1000",
        "artifacts": [{
            "src": src_bin.to_string_lossy(),
            "dest": format!("bin/{bin_name}")
        }]
    });
    let path = home.path().join("manifest.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path.to_string_lossy().to_string()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
fn add_creates_generation_directory() {
    let home = tempfile::tempdir().unwrap();
    let manifest = make_manifest(&home, "myprog", "myprog");

    let out = run_with_home(&home, &["add", "myprog", &manifest]);
    assert!(
        out.status.success(),
        "add should succeed\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Generation directory should exist under BF_HOME/generations/myprog/<id>/
    let gen_root = home.path().join("generations/myprog");
    let entries: Vec<_> = fs::read_dir(&gen_root)
        .expect("generations/myprog should exist")
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    assert_eq!(entries.len(), 1, "exactly one generation should be created");

    let gen_dir = &entries[0].path();
    assert!(gen_dir.join("generation.json").exists(), "generation.json must be written");
    assert!(gen_dir.join("bin/myprog").exists(), "artifact should be copied");
}

#[test]
fn activate_creates_bin_symlink() {
    let home = tempfile::tempdir().unwrap();
    let manifest = make_manifest(&home, "linked", "linked");

    run_with_home(&home, &["add", "linked", &manifest]);
    let out = run_with_home(&home, &["activate", "linked", "latest"]);
    assert!(
        out.status.success(),
        "activate should succeed\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let bin_link = home.path().join("bin/linked");
    assert!(
        bin_link.exists() || bin_link.is_symlink(),
        "~/.butterfork/bin/linked symlink should exist"
    );
}

#[test]
fn list_shows_added_generation() {
    let home = tempfile::tempdir().unwrap();
    let manifest = make_manifest(&home, "listed", "listed");
    run_with_home(&home, &["add", "listed", &manifest]);

    let out = run_with_home(&home, &["list", "listed"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"project\":\"listed\"") || stdout.contains(r#""project": "listed""#),
        "list should emit the generation JSON"
    );
}

#[test]
fn rollback_with_one_generation_fails() {
    let home = tempfile::tempdir().unwrap();
    let manifest = make_manifest(&home, "solo", "solo");
    run_with_home(&home, &["add", "solo", &manifest]);
    run_with_home(&home, &["activate", "solo", "latest"]);

    let out = run_with_home(&home, &["rollback", "solo"]);
    assert!(
        !out.status.success(),
        "rollback should fail when there is only one generation"
    );
}

#[test]
fn rollback_with_two_generations_succeeds() {
    let home = tempfile::tempdir().unwrap();

    // Add first generation.
    let manifest1 = make_manifest(&home, "duprog", "duprog");
    run_with_home(&home, &["add", "duprog", &manifest1]);
    run_with_home(&home, &["activate", "duprog", "latest"]);

    // Sleep 2ms so the second ID is strictly greater.
    std::thread::sleep(std::time::Duration::from_millis(2));

    // Add second generation.
    let manifest2 = make_manifest(&home, "duprog", "duprog");
    run_with_home(&home, &["add", "duprog", &manifest2]);
    run_with_home(&home, &["activate", "duprog", "latest"]);

    let out = run_with_home(&home, &["rollback", "duprog"]);
    assert!(
        out.status.success(),
        "rollback should succeed with two generations\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn gc_removes_old_generation() {
    let home = tempfile::tempdir().unwrap();

    // Create a fake "old" generation by manually planting a directory with an
    // ancient millisecond timestamp (year 2000 = 946684800000 ms).
    let old_id = "946684800000";
    let gen_dir = home.path().join(format!("generations/gctest/{old_id}"));
    fs::create_dir_all(&gen_dir).unwrap();
    fs::write(
        gen_dir.join("generation.json"),
        serde_json::json!({"id": old_id, "project": "gctest"}).to_string(),
    )
    .unwrap();

    // Add and activate a current generation so there is an active symlink.
    let manifest = make_manifest(&home, "gctest", "gctest");
    run_with_home(&home, &["add", "gctest", &manifest]);
    run_with_home(&home, &["activate", "gctest", "latest"]);

    let out = run_with_home(&home, &["gc"]);
    assert!(
        out.status.success(),
        "gc should succeed\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!gen_dir.exists(), "old generation should have been removed by gc");
}
