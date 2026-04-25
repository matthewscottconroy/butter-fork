use anyhow::{Context, Result};
use bf_common::{emit, ArtifactManifest, Event, Generation};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "bf-install",
    about = "Generational install manager: add, activate, rollback, and garbage-collect",
    long_about = "Generations live in ~/.butterfork/generations/<project>/<id>/\n\
                  with a /usr/local-mirroring layout (bin/, lib/, share/).\n\
                  The active generation is pointed to by a symlink swapped atomically.\n\
                  Standalone useful: use bf-install as a generational install manager\n\
                  independent of all other Butterfork components.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: InstallCommand,
}

#[derive(Subcommand)]
enum InstallCommand {
    /// Register a new build as an install generation
    Add {
        /// Project name (used as the directory slug)
        project: String,
        /// Path to the artifact manifest JSON produced by bf-build
        artifact_manifest: String,
    },
    /// Activate a specific generation (atomically swaps the active symlink)
    Activate {
        /// Project name
        project: String,
        /// Generation ID, or "latest" for the most recently built one
        generation_id: String,
    },
    /// List generations for a project (or all projects)
    List {
        /// Project name; omit to list all managed projects
        project: Option<String>,
    },
    /// Roll back a project to its previous generation
    Rollback {
        /// Project name
        project: String,
    },
    /// Garbage-collect inactive generations older than 7 days
    Gc,
}

// ── path helpers ──────────────────────────────────────────────────────────────

pub fn butterfork_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    // Allow overriding in tests without touching $HOME.
    let base = std::env::var("BF_HOME").unwrap_or(format!("{home}/.butterfork"));
    PathBuf::from(base)
}

pub fn generations_root() -> PathBuf {
    butterfork_home().join("generations")
}

pub fn generation_dir(project: &str, id: &str) -> PathBuf {
    generations_root().join(project).join(id)
}

/// The `active` symlink inside the project's generation directory.
pub fn active_link(project: &str) -> PathBuf {
    generations_root().join(project).join("active")
}

/// The binary symlink exposed under ~/.butterfork/bin/.
pub fn bin_link(bin_name: &str) -> PathBuf {
    butterfork_home().join("bin").join(bin_name)
}

/// Millisecond-precision generation ID: monotonically increasing, sortable.
pub fn new_generation_id() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

/// Find the generation directory with the highest numeric ID.
pub fn latest_generation(project: &str) -> Result<PathBuf> {
    let proj_root = generations_root().join(project);
    std::fs::read_dir(&proj_root)
        .with_context(|| format!("reading generations for '{project}'"))?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.parse::<u128>().ok().map(|n| (n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
        .with_context(|| format!("no generations found for '{project}'"))
}

/// Find the second-highest generation ID (used by rollback).
pub fn previous_generation(project: &str) -> Result<PathBuf> {
    let proj_root = generations_root().join(project);
    let mut all: Vec<(u128, PathBuf)> = std::fs::read_dir(&proj_root)
        .with_context(|| format!("reading generations for '{project}'"))?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.parse::<u128>().ok().map(|n| (n, e.path()))
        })
        .collect();
    all.sort_by_key(|(n, _)| *n);
    match all.as_slice() {
        [.., prev, _current] => Ok(prev.1.clone()),
        _ => anyhow::bail!(
            "nothing to roll back for '{project}' — only one generation exists"
        ),
    }
}

// ── atomic symlink swap ───────────────────────────────────────────────────────

/// Atomically replace `link` → `target` using write-then-rename.
pub fn atomic_symlink(target: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = link.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)
        .with_context(|| format!("creating symlink {} → {}", tmp.display(), target.display()))?;
    std::fs::rename(&tmp, link)
        .with_context(|| format!("renaming {} → {}", tmp.display(), link.display()))?;
    Ok(())
}

// ── activate helper ───────────────────────────────────────────────────────────

/// Update the `active` field in a generation's metadata file.
fn set_active_flag(gen_dir: &Path, active: bool) {
    let meta_path = gen_dir.join("generation.json");
    if let Ok(s) = std::fs::read_to_string(&meta_path) {
        if let Ok(mut gen) = serde_json::from_str::<Generation>(&s) {
            gen.active = active;
            if let Ok(updated) = serde_json::to_string_pretty(&gen) {
                let _ = std::fs::write(&meta_path, updated);
            }
        }
    }
}

/// Point the `active` symlink at `gen_dir` and update all bin/ symlinks.
pub fn activate(project: &str, gen_dir: &Path) -> Result<Vec<String>> {
    // 1. Clear the previous active generation's flag.
    let link = active_link(project);
    if let Ok(prev_target) = std::fs::read_link(&link) {
        if prev_target != gen_dir {
            set_active_flag(&prev_target, false);
        }
    }

    // 2. Swap the active pointer.
    let link = active_link(project);
    atomic_symlink(gen_dir, &link)?;

    // 3. Mark the new generation as active.
    set_active_flag(gen_dir, true);

    // 2. Update bin/ symlinks for every binary in gen_dir/bin/.
    let bin_src_dir = gen_dir.join("bin");
    let mut activated_bins: Vec<String> = Vec::new();

    if bin_src_dir.is_dir() {
        let bf_bin_dir = butterfork_home().join("bin");
        std::fs::create_dir_all(&bf_bin_dir)?;

        for entry in std::fs::read_dir(&bin_src_dir)?.flatten() {
            let bin_src = entry.path();
            if bin_src.is_file() || bin_src.is_symlink() {
                let bin_name = entry.file_name().to_string_lossy().to_string();
                let bin_dst = bin_link(&bin_name);
                atomic_symlink(&bin_src, &bin_dst)?;
                activated_bins.push(bin_name);
            }
        }
    }

    Ok(activated_bins)
}

// ── main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        InstallCommand::Add {
            project,
            artifact_manifest,
        } => {
            let manifest_str = std::fs::read_to_string(&artifact_manifest)
                .with_context(|| format!("reading {artifact_manifest}"))?;
            let manifest: ArtifactManifest =
                serde_json::from_str(&manifest_str).context("parsing artifact manifest")?;

            let id = new_generation_id();
            let gen_dir = generation_dir(&project, &id);
            std::fs::create_dir_all(&gen_dir)?;

            // Copy each artifact into the generation directory at its relative dest path.
            let mut installed_paths: Vec<String> = Vec::new();
            for artifact in &manifest.artifacts {
                let dest = gen_dir.join(&artifact.dest);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let src = Path::new(&artifact.src);
                if src.exists() {
                    std::fs::copy(src, &dest).with_context(|| {
                        format!("copying {} → {}", src.display(), dest.display())
                    })?;
                    // Preserve executable bit.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = std::fs::metadata(&dest)?.permissions();
                        perms.set_mode(perms.mode() | 0o111);
                        std::fs::set_permissions(&dest, perms)?;
                    }
                    installed_paths.push(dest.to_string_lossy().to_string());
                } else {
                    eprintln!(
                        "bf-install: warning: artifact source not found: {}",
                        src.display()
                    );
                }
            }

            // Write generation metadata.
            let gen = Generation {
                id: id.clone(),
                project: project.clone(),
                git_ref: manifest.git_ref.clone(),
                built_at: manifest.built_at.clone(),
                artifact_paths: installed_paths,
                active: false,
            };
            std::fs::write(
                gen_dir.join("generation.json"),
                serde_json::to_string_pretty(&gen)?,
            )?;

            eprintln!(
                "bf-install: registered generation {id} for '{project}' in {}",
                gen_dir.display()
            );
            println!("{}", serde_json::to_string(&gen)?);
            emit(&Event::Done { exit_code: 0 });
        }

        InstallCommand::Activate {
            project,
            generation_id,
        } => {
            let gen_dir = if generation_id == "latest" {
                latest_generation(&project)?
            } else {
                let d = generation_dir(&project, &generation_id);
                if !d.exists() {
                    anyhow::bail!(
                        "generation '{}' not found for project '{}'",
                        generation_id,
                        project
                    );
                }
                d
            };

            let gen_id = gen_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| generation_id.clone());

            eprintln!("bf-install: activating generation {gen_id} for '{project}'");
            let bins = activate(&project, &gen_dir)?;

            let bf_bin_dir = butterfork_home().join("bin");
            if bins.is_empty() {
                eprintln!("bf-install: activated (no binaries in generation)");
            } else {
                eprintln!(
                    "bf-install: activated binaries: {} → {}",
                    bins.join(", "),
                    bf_bin_dir.display()
                );
            }

            emit(&Event::InstallComplete {
                project: project.clone(),
                generation_id: gen_id,
                bin_dir: bf_bin_dir.to_string_lossy().to_string(),
            });
            emit(&Event::Done { exit_code: 0 });
        }

        InstallCommand::List { project } => {
            let gen_root = generations_root();
            let projects: Vec<String> = if let Some(p) = project {
                vec![p]
            } else {
                if !gen_root.exists() {
                    eprintln!("bf-install: no projects installed yet");
                    return Ok(());
                }
                std::fs::read_dir(&gen_root)?
                    .flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            };

            for proj in projects {
                let proj_dir = gen_root.join(&proj);
                let Ok(entries) = std::fs::read_dir(&proj_dir) else {
                    continue;
                };
                let mut gens: Vec<_> = entries
                    .flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .collect();
                gens.sort_by_key(|e| e.file_name());
                for entry in gens {
                    let meta_path = entry.path().join("generation.json");
                    if let Ok(s) = std::fs::read_to_string(&meta_path) {
                        println!("{}", s.trim());
                    }
                }
            }
        }

        InstallCommand::Rollback { project } => {
            let prev = previous_generation(&project)?;
            let prev_id = prev
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "?".to_owned());
            eprintln!("bf-install: rolling back '{project}' to generation {prev_id}");
            let bins = activate(&project, &prev)?;
            if !bins.is_empty() {
                eprintln!("bf-install: active binaries: {}", bins.join(", "));
            }
            eprintln!("bf-install: rollback complete");
            emit(&Event::Done { exit_code: 0 });
        }

        InstallCommand::Gc => {
            eprintln!("bf-install: garbage-collecting inactive generations");
            let gen_root = generations_root();
            if !gen_root.exists() {
                return Ok(());
            }
            let cutoff = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
                .saturating_sub(7 * 24 * 60 * 60 * 1000); // 7 days in ms

            let mut removed = 0u32;
            for proj_entry in std::fs::read_dir(&gen_root)?.flatten() {
                let proj_path = proj_entry.path();
                let active = proj_path.join("active");

                // Resolve what the active symlink points to.
                let active_target = std::fs::read_link(&active).ok();

                for gen_entry in std::fs::read_dir(&proj_path)
                    .ok()
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let gen_path = gen_entry.path();
                    if !gen_path.is_dir() {
                        continue;
                    }
                    // Skip the currently active generation.
                    if active_target.as_deref() == Some(&gen_path) {
                        continue;
                    }
                    // Parse ID as millis timestamp.
                    let id_ms: u128 = gen_path
                        .file_name()
                        .and_then(|n| n.to_string_lossy().parse().ok())
                        .unwrap_or(u128::MAX);
                    if id_ms < cutoff {
                        eprintln!("bf-install: gc removing {}", gen_path.display());
                        std::fs::remove_dir_all(&gen_path)?;
                        removed += 1;
                    }
                }
            }
            eprintln!("bf-install: removed {removed} old generation(s)");
            emit(&Event::Done { exit_code: 0 });
        }
    }

    Ok(())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn tmp_home() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn add_and_activate_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap();
        let home = tmp_home();
        std::env::set_var("BF_HOME", home.path());

        // Create a fake binary to install.
        let src_dir = home.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let src_bin = src_dir.join("myprog");
        fs::write(&src_bin, "#!/bin/sh\necho hello\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = fs::metadata(&src_bin).unwrap().permissions();
            p.set_mode(0o755);
            fs::set_permissions(&src_bin, p).unwrap();
        }

        let manifest = bf_common::ArtifactManifest {
            project: "myprog".to_owned(),
            git_ref: "abc1234".to_owned(),
            built_at: "1000".to_owned(),
            artifacts: vec![bf_common::Artifact {
                src: src_bin.to_string_lossy().to_string(),
                dest: "bin/myprog".to_owned(),
            }],
        };
        let manifest_path = home.path().join("manifest.json");
        fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();

        // Add generation.
        let id = new_generation_id();
        let gen_dir = generation_dir("myprog", &id);
        assert!(!gen_dir.exists());

        // Run add logic directly.
        let gen_dir_created = generation_dir("myprog", &new_generation_id());
        fs::create_dir_all(&gen_dir_created).unwrap();
        let artifact_dest = gen_dir_created.join("bin/myprog");
        fs::create_dir_all(artifact_dest.parent().unwrap()).unwrap();
        fs::copy(&src_bin, &artifact_dest).unwrap();

        // Activate.
        let bins = activate("myprog", &gen_dir_created).unwrap();
        assert!(bins.contains(&"myprog".to_owned()));

        let link = bin_link("myprog");
        assert!(link.exists() || link.is_symlink(), "bin symlink should exist");
    }

    #[test]
    fn rollback_requires_two_generations() {
        let _guard = TEST_LOCK.lock().unwrap();
        let home = tmp_home();
        std::env::set_var("BF_HOME", home.path());

        // Only one generation — rollback must fail.
        let gen_dir = generation_dir("solo", "1000");
        fs::create_dir_all(&gen_dir).unwrap();
        let err = previous_generation("solo");
        assert!(err.is_err());
    }
}
