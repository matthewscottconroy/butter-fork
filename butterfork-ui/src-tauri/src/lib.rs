use serde::{Deserialize, Serialize};
use std::process::Command;
use tauri::Manager;

// ── shared types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CatalogEntry {
    pub slug: String,
    pub upstream_url: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DaemonTask {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub label: String,
    pub started_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DoctorResult {
    pub ok: bool,
    pub output: String,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Run `bf doctor` and return the stderr output.
#[tauri::command]
fn run_doctor() -> DoctorResult {
    let out = Command::new("bf")
        .arg("doctor")
        .output();

    match out {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            DoctorResult {
                ok: o.status.success(),
                output: stderr,
            }
        }
        Err(e) => DoctorResult {
            ok: false,
            output: format!("bf not found on PATH: {e}"),
        },
    }
}

/// Search the catalog.
#[tauri::command]
fn catalog_search(query: String) -> Vec<CatalogEntry> {
    let out = Command::new("bf-catalog")
        .args(["search", &query])
        .output();

    let Ok(o) = out else { return vec![] };

    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| {
            Some(CatalogEntry {
                slug: v["slug"].as_str()?.to_owned(),
                upstream_url: v["upstream_url"].as_str()?.to_owned(),
                description: v["description"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

/// Install a project (non-blocking — returns immediately; tail progress via daemon log).
#[tauri::command]
fn install_project(slug: String, no_fork: bool, window: tauri::Window) -> String {
    let mut args = vec!["install".to_owned(), slug.clone()];
    if no_fork {
        args.push("--no-fork".to_owned());
    }

    // Spawn bf install in the background and stream stderr to the frontend via events.
    std::thread::spawn(move || {
        let child = Command::new("bf")
            .args(&args)
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn();

        let Ok(mut child) = child else {
            let _ = window.emit("install-progress", "bf not found on PATH");
            return;
        };

        // Stream stderr lines to frontend.
        if let Some(stderr) = child.stderr.take() {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stderr).lines().flatten() {
                let _ = window.emit("install-progress", &line);
            }
        }

        let status = child.wait().unwrap_or_else(|_| {
            std::process::exit(1);
        });
        let _ = window.emit(
            "install-done",
            if status.success() { "ok" } else { "failed" },
        );
    });

    format!("installing {slug}")
}

/// Query the daemon for active tasks.
#[tauri::command]
fn daemon_status() -> Vec<DaemonTask> {
    // Connect to the Unix socket and send a status command.
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    let sock_path = format!("{runtime_dir}/butterfork.sock");

    let Ok(mut stream) = UnixStream::connect(&sock_path) else {
        return vec![];
    };

    let _ = stream.write_all(b"{\"cmd\":\"status\"}\n");
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    let _ = reader.read_line(&mut line);

    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
        return vec![];
    };

    v["tasks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| serde_json::from_value(t.clone()).ok())
        .collect()
}

/// Open the daemon log in the system's default text viewer.
#[tauri::command]
fn open_daemon_log(app: tauri::AppHandle) {
    let home = std::env::var("HOME").unwrap_or_default();
    let log = format!("{home}/.butterfork/logs/daemon.log");
    let _ = tauri::api::shell::open(&app.shell_scope(), log, None);
}

// ── app bootstrap ─────────────────────────────────────────────────────────────

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            run_doctor,
            catalog_search,
            install_project,
            daemon_status,
            open_daemon_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Butterfork UI");
}
