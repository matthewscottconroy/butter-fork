use anyhow::Result;
use bf_common::{emit, Event};
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "bf-index",
    about = "Incremental codebase index: symbol, grep, and semantic queries",
    long_about = "Index data lives under .bf/index/ inside the repo (gitignored).\n\
                  tree-sitter drives structural queries; an optional local embedding\n\
                  store drives semantic queries. Use --no-embeddings to skip ML deps.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: IndexCommand,
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Build or refresh the index for a repository
    Update {
        /// Path to the repository root
        repo: String,
        /// Skip embedding generation (faster; disables --semantic queries)
        #[arg(long)]
        no_embeddings: bool,
    },
    /// Query the index
    Query {
        /// Path to the repository root
        repo: String,
        #[command(flatten)]
        mode: QueryMode,
    },
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct QueryMode {
    /// Look up a symbol by name
    #[arg(long)]
    symbol: Option<String>,
    /// Search for a pattern (regex)
    #[arg(long)]
    grep: Option<String>,
    /// Semantic (embedding) search
    #[arg(long)]
    semantic: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        IndexCommand::Update { repo, no_embeddings } => {
            eprintln!("bf-index: indexing '{repo}'");
            if no_embeddings {
                eprintln!("bf-index: skipping embeddings (--no-embeddings)");
            }
            // TODO: run tree-sitter over the repo and write index data to .bf/index/.
            // Optionally generate embeddings with a local model.
            emit(&Event::Message {
                text: format!("Index update not yet implemented for: {repo}"),
            });
        }

        IndexCommand::Query { repo, mode } => {
            if let Some(symbol) = mode.symbol {
                eprintln!("bf-index: symbol query '{symbol}' in '{repo}'");
                // TODO: look up symbol in tree-sitter index
                emit(&Event::Message {
                    text: format!("Symbol lookup not yet implemented: {symbol}"),
                });
            } else if let Some(pattern) = mode.grep {
                eprintln!("bf-index: grep '{pattern}' in '{repo}'");
                // Delegate to ripgrep/grep as a thin wrapper until the index is built.
                let status = std::process::Command::new("rg")
                    .args(["--json", &pattern, &repo])
                    .status();
                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                    Err(_) => {
                        eprintln!("bf-index: rg not found; grep fallback not yet implemented");
                        std::process::exit(bf_common::exit::UNAVAILABLE);
                    }
                }
            } else if let Some(text) = mode.semantic {
                eprintln!("bf-index: semantic query '{text}' in '{repo}'");
                // TODO: encode query with local embedding model, search index.
                emit(&Event::Message {
                    text: format!("Semantic search not yet implemented: {text}"),
                });
            }
        }
    }

    Ok(())
}
