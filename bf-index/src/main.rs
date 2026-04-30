use anyhow::{Context, Result};
use bf_common::{emit, exit, Event};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "bf-index",
    about = "Incremental codebase index: symbol, grep, and semantic queries",
    long_about = "Index data lives under .bf/index/ inside the repo (gitignored).\n\
                  Symbol queries use a regex-based scan; grep delegates to rg/grep.\n\
                  Semantic search is a Phase 2 feature (requires embeddings).\n\
                  Use --no-embeddings to explicitly skip embedding generation.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: IndexCommand,
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Build or refresh the symbol index for a repository
    Update {
        repo: String,
        #[arg(long)]
        no_embeddings: bool,
    },
    /// Query the index
    Query {
        repo: String,
        #[command(flatten)]
        mode: QueryMode,
    },
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct QueryMode {
    #[arg(long)]
    symbol: Option<String>,
    #[arg(long)]
    grep: Option<String>,
    #[arg(long)]
    semantic: Option<String>,
}

// ── index data model ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SymbolRecord {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexData {
    pub indexed_at: String,
    pub repo: String,
    pub symbols: Vec<SymbolRecord>,
    pub files: Vec<String>,
}

fn index_path(repo: &str) -> PathBuf {
    Path::new(repo).join(".bf/index/symbols.json")
}

// ── file walker ───────────────────────────────────────────────────────────────

fn collect_source_files(repo: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_dir(Path::new(repo), &mut files);
    files
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            walk_dir(&path, out);
        } else if is_source_file(&name) {
            out.push(path);
        }
    }
}

fn is_source_file(name: &str) -> bool {
    matches!(
        name.rsplit('.').next().unwrap_or(""),
        "rs" | "go"
            | "py"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "cs"
            | "zig"
    )
}

// ── symbol extraction ─────────────────────────────────────────────────────────

fn extract_symbols(file: &Path, repo: &str) -> Vec<SymbolRecord> {
    let Ok(f) = std::fs::File::open(file) else {
        return Vec::new();
    };
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    let rel = file
        .strip_prefix(repo)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();

    BufReader::new(f)
        .lines()
        .enumerate()
        .filter_map(|(idx, line_result)| {
            let line = line_result.ok()?;
            let (name, kind) = parse_symbol(line.trim(), ext)?;
            Some(SymbolRecord {
                name,
                kind,
                file: rel.clone(),
                line: idx + 1,
            })
        })
        .collect()
}

fn parse_symbol(line: &str, ext: &str) -> Option<(String, String)> {
    match ext {
        "rs" => parse_rust_symbol(line),
        "go" => parse_go_symbol(line),
        "py" => parse_python_symbol(line),
        "ts" | "tsx" | "js" | "jsx" => parse_js_symbol(line),
        "c" | "cpp" | "h" | "hpp" => parse_c_symbol(line),
        _ => None,
    }
}

fn parse_rust_symbol(line: &str) -> Option<(String, String)> {
    for (prefix, kind) in &[
        ("pub async fn ", "async_function"),
        ("async fn ", "async_function"),
        ("pub fn ", "function"),
        ("fn ", "function"),
        ("pub struct ", "struct"),
        ("struct ", "struct"),
        ("pub enum ", "enum"),
        ("enum ", "enum"),
        ("pub trait ", "trait"),
        ("trait ", "trait"),
        ("impl ", "impl"),
        ("pub type ", "type_alias"),
        ("type ", "type_alias"),
        ("pub const ", "const"),
        ("const ", "const"),
        ("pub mod ", "module"),
        ("mod ", "module"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some((name, kind.to_string()));
            }
        }
    }
    None
}

fn parse_go_symbol(line: &str) -> Option<(String, String)> {
    if let Some(rest) = line.strip_prefix("func ") {
        let name: String = rest
            .chars()
            .skip_while(|c| *c == '(') // skip receiver
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some((name, "function".to_owned()));
        }
    }
    if let Some(rest) = line.strip_prefix("type ") {
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some((name, "type".to_owned()));
        }
    }
    None
}

fn parse_python_symbol(line: &str) -> Option<(String, String)> {
    if let Some(rest) = line.strip_prefix("def ") {
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        return Some((name, "function".to_owned()));
    }
    if let Some(rest) = line.strip_prefix("class ") {
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        return Some((name, "class".to_owned()));
    }
    None
}

fn parse_js_symbol(line: &str) -> Option<(String, String)> {
    for prefix in &[
        "export async function ",
        "export function ",
        "async function ",
        "function ",
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !name.is_empty() {
                return Some((name, "function".to_owned()));
            }
        }
    }
    for prefix in &["export class ", "class "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some((name, "class".to_owned()));
            }
        }
    }
    None
}

fn parse_c_symbol(line: &str) -> Option<(String, String)> {
    // Heuristic: `type name(args)` ending in `)` or `{`
    if !line.contains('(') {
        return None;
    }
    let before_paren = line.split('(').next()?.trim();
    let name = before_paren
        .split_whitespace()
        .last()?
        .trim_start_matches('*');
    if name.is_empty()
        || !name.chars().all(|c| c.is_alphanumeric() || c == '_')
        || matches!(name, "if" | "for" | "while" | "switch" | "return")
    {
        return None;
    }
    Some((name.to_owned(), "function".to_owned()))
}

// ── timestamp helper ──────────────────────────────────────────────────────────

fn now_unix() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

// ── operations ────────────────────────────────────────────────────────────────

fn update(repo: &str, no_embeddings: bool) -> Result<()> {
    eprintln!("bf-index: scanning '{repo}'");
    if no_embeddings {
        eprintln!("bf-index: embeddings skipped (--no-embeddings)");
    }

    let files = collect_source_files(repo);
    eprintln!("bf-index: found {} source file(s)", files.len());

    let mut symbols: Vec<SymbolRecord> = Vec::new();
    let mut file_paths: Vec<String> = Vec::new();

    for file in &files {
        let rel = file
            .strip_prefix(repo)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();
        file_paths.push(rel);
        symbols.extend(extract_symbols(file, repo));
    }

    eprintln!("bf-index: extracted {} symbol(s)", symbols.len());

    let index = IndexData {
        indexed_at: now_unix(),
        repo: repo.to_owned(),
        symbols,
        files: file_paths,
    };

    let dest = index_path(repo);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &dest,
        serde_json::to_string_pretty(&index).context("serializing index")?,
    )
    .with_context(|| format!("writing index to {}", dest.display()))?;

    // Ensure .bf/ is gitignored inside the repo.
    let gitignore = Path::new(repo).join(".bf/.gitignore");
    if !gitignore.exists() {
        std::fs::create_dir_all(gitignore.parent().unwrap())?;
        std::fs::write(&gitignore, "index/\n")?;
    }

    eprintln!("bf-index: index written → {}", dest.display());
    emit(&Event::Message {
        text: format!(
            "Indexed {} files, {} symbols",
            index.files.len(),
            index.symbols.len()
        ),
    });
    Ok(())
}

fn load_index(repo: &str) -> Result<IndexData> {
    let path = index_path(repo);
    let s = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no index at {} — run `bf-index update {repo}` first",
            path.display()
        )
    })?;
    serde_json::from_str(&s).context("parsing index JSON")
}

fn query_symbol(repo: &str, name: &str) -> Result<()> {
    let index = load_index(repo)?;
    let matches: Vec<_> = index
        .symbols
        .iter()
        .filter(|s| s.name.contains(name))
        .collect();

    if matches.is_empty() {
        eprintln!("bf-index: no symbols matching '{name}'");
        std::process::exit(exit::NOINPUT);
    }
    for sym in &matches {
        println!(
            "{}",
            serde_json::json!({
                "name": sym.name,
                "kind": sym.kind,
                "file": sym.file,
                "line": sym.line,
            })
        );
    }
    eprintln!("bf-index: {} match(es)", matches.len());
    Ok(())
}

fn query_grep(repo: &str, pattern: &str) -> Result<()> {
    let has_rg = Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let status = if has_rg {
        Command::new("rg")
            .args(["--line-number", "--no-heading", pattern, repo])
            .status()?
    } else {
        Command::new("grep").args(["-rn", pattern, repo]).status()?
    };
    std::process::exit(status.code().unwrap_or(1));
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        IndexCommand::Update {
            repo,
            no_embeddings,
        } => update(&repo, no_embeddings)?,
        IndexCommand::Query { repo, mode } => {
            if let Some(sym) = mode.symbol {
                query_symbol(&repo, &sym)?;
            } else if let Some(pat) = mode.grep {
                query_grep(&repo, &pat)?;
            } else if let Some(text) = mode.semantic {
                eprintln!("bf-index: semantic search not yet available (Phase 2)");
                eprintln!("bf-index: falling back to grep for: {text}");
                query_grep(&repo, &text)?;
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_pub_fn() {
        assert_eq!(
            parse_rust_symbol("pub fn hello_world() {"),
            Some(("hello_world".to_owned(), "function".to_owned()))
        );
    }

    #[test]
    fn rust_struct() {
        assert_eq!(
            parse_rust_symbol("pub struct Foo {"),
            Some(("Foo".to_owned(), "struct".to_owned()))
        );
    }

    #[test]
    fn rust_enum() {
        assert_eq!(
            parse_rust_symbol("enum Direction {"),
            Some(("Direction".to_owned(), "enum".to_owned()))
        );
    }

    #[test]
    fn python_def() {
        assert_eq!(
            parse_python_symbol("def compute(x, y):"),
            Some(("compute".to_owned(), "function".to_owned()))
        );
    }

    #[test]
    fn python_class() {
        assert_eq!(
            parse_python_symbol("class MyWidget(QWidget):"),
            Some(("MyWidget".to_owned(), "class".to_owned()))
        );
    }

    #[test]
    fn non_symbol_lines() {
        assert!(parse_rust_symbol("let x = 42;").is_none());
        assert!(parse_rust_symbol("// comment").is_none());
        assert!(parse_python_symbol("    x = 1").is_none());
    }

    #[test]
    fn source_file_filter() {
        assert!(is_source_file("main.rs"));
        assert!(is_source_file("app.py"));
        assert!(is_source_file("index.ts"));
        assert!(!is_source_file("README.md"));
        assert!(!is_source_file("config.toml"));
    }
}
