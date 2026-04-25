use anyhow::Result;
use bf_common::{emit, CatalogEntry, Event};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "bf-catalog",
    about = "OSS project catalog: search, discover, and manage project entries",
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
        /// Free-text search query
        query: String,
    },
    /// Show detailed information about a single catalog entry
    Show {
        /// Project slug (e.g. "ripgrep") or upstream URL
        slug: String,
    },
    /// Add a user-defined project URL to the local catalog
    Add {
        /// Repository URL (GitHub, GitLab, …)
        url: String,
    },
    /// Refresh the local catalog cache from the upstream signed index
    Update,
}

// ── Phase 0 hardcoded catalog ────────────────────────────────────────────────
//
// The production path (signed remote index + GitHub search) ships in Phase 2.
// For now, these three entries are enough to drive the fork→build→install loop.
//
fn builtin_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            slug: "ripgrep".to_owned(),
            name: "ripgrep".to_owned(),
            description: "A line-oriented search tool that recursively searches the current \
                           directory for a regex pattern while respecting gitignore rules."
                .to_owned(),
            upstream_url: "https://github.com/BurntSushi/ripgrep".to_owned(),
            license: "MIT OR Unlicense".to_owned(),
            stars: 49_000,
            has_contributing: true,
            has_code_of_conduct: true,
            pr_response_latency_days: Some(3.0),
        },
        CatalogEntry {
            slug: "fd".to_owned(),
            name: "fd".to_owned(),
            description: "A simple, fast and user-friendly alternative to find.".to_owned(),
            upstream_url: "https://github.com/sharkdp/fd".to_owned(),
            license: "Apache-2.0 OR MIT".to_owned(),
            stars: 35_000,
            has_contributing: true,
            has_code_of_conduct: true,
            pr_response_latency_days: Some(5.0),
        },
        CatalogEntry {
            slug: "bat".to_owned(),
            name: "bat".to_owned(),
            description: "A cat(1) clone with wings: syntax highlighting, git integration, \
                           and automatic paging."
                .to_owned(),
            upstream_url: "https://github.com/sharkdp/bat".to_owned(),
            license: "Apache-2.0 OR MIT".to_owned(),
            stars: 50_000,
            has_contributing: true,
            has_code_of_conduct: true,
            pr_response_latency_days: Some(5.0),
        },
        CatalogEntry {
            slug: "butterfork".to_owned(),
            name: "Butterfork".to_owned(),
            description: "Collapse the gap between using and contributing to open source \
                           software into a single workflow backed by an LLM coding agent."
                .to_owned(),
            upstream_url: "https://github.com/matthewscottconroy/butter-fork".to_owned(),
            license: "Apache-2.0 OR MIT".to_owned(),
            stars: 0,
            has_contributing: true,
            has_code_of_conduct: false,
            pr_response_latency_days: None,
        },
    ]
}

// ── User-added entries ────────────────────────────────────────────────────────

fn user_catalog_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.butterfork/catalog-user.json"))
}

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

fn all_entries() -> Vec<CatalogEntry> {
    let mut entries = builtin_catalog();
    entries.extend(load_user_catalog());
    entries
}

fn find_entry(slug_or_url: &str) -> Option<CatalogEntry> {
    all_entries().into_iter().find(|e| {
        e.slug == slug_or_url
            || e.upstream_url == slug_or_url
            || e.upstream_url.trim_end_matches('/') == slug_or_url.trim_end_matches('/')
    })
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
        license: "Unknown".to_owned(),
        stars: 0,
        has_contributing: false,
        has_code_of_conduct: false,
        pr_response_latency_days: None,
    }
}

// ── main ─────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Search { query } => {
            eprintln!("bf-catalog: searching for '{query}'");
            let q = query.to_lowercase();
            let matches: Vec<_> = all_entries()
                .into_iter()
                .filter(|e| {
                    e.slug.contains(&q)
                        || e.name.to_lowercase().contains(&q)
                        || e.description.to_lowercase().contains(&q)
                })
                .collect();

            if matches.is_empty() {
                emit(&Event::Message {
                    text: format!("No results for: {query}"),
                });
            } else {
                for entry in &matches {
                    println!("{}", serde_json::to_string(entry)?);
                }
            }
            emit(&Event::Done { exit_code: 0 });
        }

        Command::Show { slug } => {
            eprintln!("bf-catalog: looking up '{slug}'");
            match find_entry(&slug) {
                Some(entry) => {
                    println!("{}", serde_json::to_string(&entry)?);
                    emit(&Event::Done { exit_code: 0 });
                }
                None => {
                    eprintln!("bf-catalog: '{slug}' not found in catalog");
                    eprintln!(
                        "bf-catalog: if you have the URL, run `bf-catalog add <url>` first"
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
            eprintln!("bf-catalog: refreshing (no remote index configured in Phase 0)");
            eprintln!("bf-catalog: built-in entries are always current; user entries are local");
            emit(&Event::Message {
                text: "Catalog is up to date (Phase 0: built-in only)".to_owned(),
            });
            emit(&Event::Done { exit_code: 0 });
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
