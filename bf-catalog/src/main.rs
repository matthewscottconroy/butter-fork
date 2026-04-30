use anyhow::Result;
use bf_common::{emit, CatalogEntry, Event};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "bf-catalog",
    about = "OSS project catalog: search, discover, and manage project entries",
    long_about = "Entry sources (merged in order):\n\
                  1. Built-in curated entries\n\
                  2. User-added entries (~/.butterfork/catalog-user.json)\n\
                  3. Cached remote signed index (~/.butterfork/catalog-index.json)\n\
                  4. GitHub Search API (live, via `gh search repos`)\n\n\
                  Set BF_CATALOG_INDEX_URL to override the remote index URL.\n\
                  Set BF_NO_GITHUB_SEARCH=1 to skip live GitHub search.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search the catalog for projects matching a query (emits NDJSON entries)
    Search {
        query: String,
        /// Include live GitHub Search results (requires `gh` on PATH)
        #[arg(long)]
        github: bool,
    },
    /// Show detailed information about a single catalog entry
    Show { slug: String },
    /// Add a user-defined project URL to the local catalog
    Add { url: String },
    /// Refresh the local catalog cache from the upstream signed index
    Update,
    /// Compute and display the contribution-friendliness score for an entry
    Score { slug: String },
}

// ── paths ─────────────────────────────────────────────────────────────────────

fn bf_home() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"))
}

fn user_catalog_path() -> PathBuf {
    PathBuf::from(format!("{}/catalog-user.json", bf_home()))
}

fn remote_index_path() -> PathBuf {
    PathBuf::from(format!("{}/catalog-index.json", bf_home()))
}

fn remote_index_url() -> String {
    std::env::var("BF_CATALOG_INDEX_URL").unwrap_or_else(|_| {
        "https://github.com/matthewscottconroy/butter-fork/releases/latest/download/catalog-index.json"
            .to_owned()
    })
}

// ── built-in catalog ──────────────────────────────────────────────────────────

fn builtin_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            slug: "ripgrep".into(),
            name: "ripgrep".into(),
            description: "A line-oriented search tool that recursively searches the current \
                           directory for a regex pattern while respecting gitignore rules."
                .into(),
            upstream_url: "https://github.com/BurntSushi/ripgrep".into(),
            license: "MIT OR Unlicense".into(),
            stars: 49_000,
            has_contributing: true,
            has_code_of_conduct: true,
            pr_response_latency_days: Some(3.0),
            contribution_score: Some(0.95),
            spdx_id: Some("MIT".into()),
            is_copyleft: false,
        },
        CatalogEntry {
            slug: "fd".into(),
            name: "fd".into(),
            description: "A simple, fast and user-friendly alternative to find.".into(),
            upstream_url: "https://github.com/sharkdp/fd".into(),
            license: "Apache-2.0 OR MIT".into(),
            stars: 35_000,
            has_contributing: true,
            has_code_of_conduct: true,
            pr_response_latency_days: Some(5.0),
            contribution_score: Some(0.90),
            spdx_id: Some("Apache-2.0".into()),
            is_copyleft: false,
        },
        CatalogEntry {
            slug: "bat".into(),
            name: "bat".into(),
            description: "A cat(1) clone with wings: syntax highlighting, git integration, \
                           and automatic paging."
                .into(),
            upstream_url: "https://github.com/sharkdp/bat".into(),
            license: "Apache-2.0 OR MIT".into(),
            stars: 50_000,
            has_contributing: true,
            has_code_of_conduct: true,
            pr_response_latency_days: Some(5.0),
            contribution_score: Some(0.90),
            spdx_id: Some("Apache-2.0".into()),
            is_copyleft: false,
        },
        CatalogEntry {
            slug: "butterfork".into(),
            name: "Butterfork".into(),
            description: "Collapse the gap between using and contributing to open source \
                           software into a single workflow backed by an LLM coding agent."
                .into(),
            upstream_url: "https://github.com/matthewscottconroy/butter-fork".into(),
            license: "Apache-2.0 OR MIT".into(),
            stars: 0,
            has_contributing: true,
            has_code_of_conduct: false,
            pr_response_latency_days: None,
            contribution_score: None,
            spdx_id: Some("Apache-2.0".into()),
            is_copyleft: false,
        },
    ]
}

// ── user catalog ──────────────────────────────────────────────────────────────

fn load_user_catalog() -> Vec<CatalogEntry> {
    let path = user_catalog_path();
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<CatalogEntry>>(&s).unwrap_or_default()
}

fn save_user_catalog(entries: &[CatalogEntry]) -> Result<()> {
    let path = user_catalog_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(entries)?)?;
    Ok(())
}

// ── remote index ──────────────────────────────────────────────────────────────

fn load_remote_index() -> Vec<CatalogEntry> {
    let path = remote_index_path();
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<CatalogEntry>>(&s).unwrap_or_default()
}

/// Fetch the remote index with `curl`, verify SHA-256, and cache locally.
fn fetch_remote_index() -> Result<usize> {
    let url = remote_index_url();
    let dest = remote_index_path();
    let checksum_url = format!("{url}.sha256");

    eprintln!("bf-catalog: fetching index from {url}");
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Download index
    let status = std::process::Command::new("curl")
        .args(["-fsSL", "--output", &dest.to_string_lossy(), &url])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            anyhow::bail!("curl failed fetching {url} — check network connectivity");
        }
        Err(_) => {
            anyhow::bail!("curl not found — install curl to refresh the remote catalog index");
        }
    }

    // Download checksum and verify (best-effort; skip if checksum file absent)
    let tmp_sha = format!("{}.sha256", dest.display());
    if std::process::Command::new("curl")
        .args(["-fsSL", "--output", &tmp_sha, &checksum_url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        // `sha256sum --check <file>` expects format: `<hash>  <filename>`
        // Rewrite to match the local path.
        if let Ok(chk_content) = std::fs::read_to_string(&tmp_sha) {
            let hash = chk_content
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            let check_line = format!("{hash}  {}", dest.display());
            let check_file = format!("{}.check", dest.display());
            let _ = std::fs::write(&check_file, &check_line);
            let ok = std::process::Command::new("sha256sum")
                .args(["--check", &check_file, "--status"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            let _ = std::fs::remove_file(&check_file);
            let _ = std::fs::remove_file(&tmp_sha);
            if !ok {
                // Corrupt download — remove and bail.
                let _ = std::fs::remove_file(&dest);
                anyhow::bail!(
                    "catalog index checksum mismatch — refusing to use corrupted download"
                );
            }
            eprintln!("bf-catalog: checksum verified");
        }
    } else {
        eprintln!("bf-catalog: no checksum file available — skipping verification");
    }

    let entries = load_remote_index();
    eprintln!(
        "bf-catalog: cached {} entries from remote index",
        entries.len()
    );
    Ok(entries.len())
}

// ── GitHub Search via `gh` ────────────────────────────────────────────────────

fn github_search(query: &str) -> Vec<CatalogEntry> {
    if std::env::var("BF_NO_GITHUB_SEARCH").as_deref() == Ok("1") {
        return Vec::new();
    }

    let available = std::process::Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !available {
        return Vec::new();
    }

    eprintln!("bf-catalog: querying GitHub Search for '{query}'");
    let out = std::process::Command::new("gh")
        .args([
            "search",
            "repos",
            query,
            "--limit",
            "10",
            "--json",
            "name,description,url,license,stargazersCount,owner",
        ])
        .output();

    let Ok(o) = out else { return Vec::new() };
    if !o.status.success() {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(&o.stdout);
    let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(|v| {
            let name = v["name"].as_str()?.to_owned();
            let url = v["url"].as_str()?.to_owned();
            let owner = v["owner"]["login"].as_str().unwrap_or("").to_owned();
            let slug = if owner.is_empty() {
                name.clone()
            } else {
                format!("{owner}/{name}")
            };
            let spdx = v["license"]["spdxId"]
                .as_str()
                .filter(|s| *s != "NOASSERTION")
                .map(str::to_owned);
            let is_copyleft = spdx.as_deref().map(is_spdx_copyleft).unwrap_or(false);
            Some(CatalogEntry {
                slug,
                name,
                description: v["description"].as_str().unwrap_or("").to_owned(),
                upstream_url: url,
                license: v["license"]["name"]
                    .as_str()
                    .unwrap_or("Unknown")
                    .to_owned(),
                stars: v["stargazersCount"].as_u64().unwrap_or(0),
                has_contributing: false, // unknown at search time
                has_code_of_conduct: false,
                pr_response_latency_days: None,
                contribution_score: None,
                spdx_id: spdx,
                is_copyleft,
            })
        })
        .collect()
}

// ── contribution score ────────────────────────────────────────────────────────

/// Compute a 0.0–1.0 contribution-friendliness score from entry metadata.
fn contribution_score(e: &CatalogEntry) -> f64 {
    let mut score = 0.0_f64;

    // CONTRIBUTING.md presence is the strongest signal.
    if e.has_contributing {
        score += 0.35;
    }
    // Code of conduct indicates a welcoming community.
    if e.has_code_of_conduct {
        score += 0.15;
    }
    // Fast PR response latency is a strong positive signal.
    if let Some(days) = e.pr_response_latency_days {
        score += if days <= 3.0 {
            0.30
        } else if days <= 7.0 {
            0.20
        } else if days <= 14.0 {
            0.10
        } else {
            0.0
        };
    }
    // Copyleft discourages casual contribution due to relicensing complexity.
    if e.is_copyleft {
        score -= 0.10;
    }
    // Star count as a rough community-size proxy.
    score += (e.stars as f64 / 100_000.0).min(0.20);

    score.clamp(0.0, 1.0)
}

// ── SPDX helpers ──────────────────────────────────────────────────────────────

fn is_spdx_copyleft(spdx: &str) -> bool {
    let s = spdx.to_uppercase();
    s.contains("GPL")
        || s.contains("AGPL")
        || s.contains("LGPL")
        || s.contains("EUPL")
        || s.contains("OSL")
        || s.contains("MPL")
}

// ── URL → slug inference ──────────────────────────────────────────────────────

fn entry_from_url(url: &str) -> CatalogEntry {
    let slug = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_owned();
    CatalogEntry {
        name: slug.clone(),
        slug,
        description: String::new(),
        upstream_url: url.to_owned(),
        license: "Unknown".into(),
        stars: 0,
        has_contributing: false,
        has_code_of_conduct: false,
        pr_response_latency_days: None,
        contribution_score: None,
        spdx_id: None,
        is_copyleft: false,
    }
}

// ── merged entry set ──────────────────────────────────────────────────────────

fn all_local_entries() -> Vec<CatalogEntry> {
    let mut entries = builtin_catalog();
    entries.extend(load_user_catalog());
    entries.extend(load_remote_index());
    // Deduplicate by upstream_url, preferring entries seen first (built-in wins).
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(e.upstream_url.clone()));
    entries
}

fn find_entry(slug_or_url: &str) -> Option<CatalogEntry> {
    all_local_entries().into_iter().find(|e| {
        e.slug == slug_or_url
            || e.upstream_url == slug_or_url
            || e.upstream_url.trim_end_matches('/') == slug_or_url.trim_end_matches('/')
    })
}

// ── entry point ───────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Search { query, github } => {
            eprintln!("bf-catalog: searching for '{query}'");
            let q = query.to_lowercase();
            let mut matches: Vec<CatalogEntry> = all_local_entries()
                .into_iter()
                .filter(|e| {
                    e.slug.to_lowercase().contains(&q)
                        || e.name.to_lowercase().contains(&q)
                        || e.description.to_lowercase().contains(&q)
                })
                .collect();

            // Live GitHub search if explicitly requested or no local results.
            if github || matches.is_empty() {
                let gh_results = github_search(&query);
                // Merge, deduplicate by upstream_url.
                let existing_urls: std::collections::HashSet<_> =
                    matches.iter().map(|e| e.upstream_url.clone()).collect();
                for r in gh_results {
                    if !existing_urls.contains(&r.upstream_url) {
                        matches.push(r);
                    }
                }
            }

            if matches.is_empty() {
                emit(&Event::Message {
                    text: format!("No results for: {query}"),
                });
            } else {
                for mut entry in matches {
                    // Fill in score if missing.
                    if entry.contribution_score.is_none() {
                        entry.contribution_score = Some(contribution_score(&entry));
                    }
                    println!("{}", serde_json::to_string(&entry)?);
                }
            }
            emit(&Event::Done { exit_code: 0 });
        }

        Command::Show { slug } => {
            eprintln!("bf-catalog: looking up '{slug}'");
            match find_entry(&slug) {
                Some(mut entry) => {
                    if entry.contribution_score.is_none() {
                        entry.contribution_score = Some(contribution_score(&entry));
                    }
                    println!("{}", serde_json::to_string(&entry)?);
                    emit(&Event::Done { exit_code: 0 });
                }
                None => {
                    eprintln!("bf-catalog: '{slug}' not found in catalog");
                    eprintln!(
                        "bf-catalog: try `bf-catalog search {slug}` or `bf-catalog add <url>`"
                    );
                    std::process::exit(bf_common::exit::NOINPUT);
                }
            }
        }

        Command::Add { url } => {
            eprintln!("bf-catalog: adding '{url}'");
            let mut user = load_user_catalog();
            if user.iter().any(|e| e.upstream_url == url) {
                eprintln!("bf-catalog: '{url}' is already in the user catalog");
            } else {
                let entry = entry_from_url(&url);
                eprintln!("bf-catalog: added as slug '{}'", entry.slug);
                println!("{}", serde_json::to_string(&entry)?);
                user.push(entry);
                save_user_catalog(&user)?;
            }
            emit(&Event::Done { exit_code: 0 });
        }

        Command::Update => {
            eprintln!("bf-catalog: refreshing remote index");
            match fetch_remote_index() {
                Ok(n) => {
                    emit(&Event::Message {
                        text: format!("Remote catalog index refreshed: {n} entries cached"),
                    });
                }
                Err(e) => {
                    eprintln!("bf-catalog: remote index update failed: {e}");
                    eprintln!("bf-catalog: built-in and user entries are still available");
                    emit(&Event::Message {
                        text: format!("Catalog update failed: {e}"),
                    });
                    std::process::exit(bf_common::exit::TEMPFAIL);
                }
            }
            emit(&Event::Done { exit_code: 0 });
        }

        Command::Score { slug } => match find_entry(&slug) {
            Some(entry) => {
                let score = entry
                    .contribution_score
                    .unwrap_or_else(|| contribution_score(&entry));
                let breakdown = serde_json::json!({
                    "slug": entry.slug,
                    "contribution_score": score,
                    "signals": {
                        "has_contributing": entry.has_contributing,
                        "has_code_of_conduct": entry.has_code_of_conduct,
                        "pr_response_latency_days": entry.pr_response_latency_days,
                        "is_copyleft": entry.is_copyleft,
                        "stars": entry.stars,
                        "spdx_id": entry.spdx_id,
                    }
                });
                println!("{}", serde_json::to_string_pretty(&breakdown)?);
                emit(&Event::Done { exit_code: 0 });
            }
            None => {
                eprintln!("bf-catalog: '{slug}' not found");
                std::process::exit(bf_common::exit::NOINPUT);
            }
        },
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
