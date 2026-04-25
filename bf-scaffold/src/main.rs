use anyhow::{Context, Result};
use bf_common::{emit, Event};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::Path;

#[derive(Parser)]
#[command(
    name = "bf-scaffold",
    about = "Turn an idea into a buildable project seed",
    long_about = "Three output modes:\n\
                  hello-world — smallest thing that runs (program, tests, CI, README, license)\n\
                  poc          — enough structure to demonstrate the core idea, with TODO markers\n\
                  design-doc   — a repo containing docs/DESIGN.md and a minimal build manifest",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: ScaffoldCommand,
}

#[derive(Subcommand)]
enum ScaffoldCommand {
    /// Create a new project scaffold
    New {
        /// Target directory for the new project
        path: String,
        /// Natural-language description of the project idea
        #[arg(long, short = 'd')]
        description: String,
        /// Existing design document to use as a seed
        #[arg(long)]
        spec: Option<String>,
        /// Named template (from `bf-scaffold template list`)
        #[arg(long)]
        template: Option<String>,
        /// Output mode
        #[arg(long, default_value = "design-doc")]
        mode: ScaffoldMode,
        /// Primary programming language
        #[arg(long, default_value = "rust")]
        language: String,
    },
    /// Template management
    Template {
        #[command(subcommand)]
        cmd: TemplateCommand,
    },
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// List available templates
    List,
    /// Add a template from a git URL
    Add {
        /// Git URL of the template repository
        git_url: String,
    },
    /// Show details about a template
    Show {
        /// Template name
        name: String,
    },
}

#[derive(Clone, Debug, ValueEnum)]
enum ScaffoldMode {
    HelloWorld,
    Poc,
    DesignDoc,
}

impl std::fmt::Display for ScaffoldMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldMode::HelloWorld => write!(f, "hello-world"),
            ScaffoldMode::Poc => write!(f, "poc"),
            ScaffoldMode::DesignDoc => write!(f, "design-doc"),
        }
    }
}

fn scaffold_design_doc(path: &str, description: &str, spec: Option<&str>) -> Result<()> {
    let root = Path::new(path);
    std::fs::create_dir_all(root.join("docs"))?;

    let design_content = if let Some(spec_path) = spec {
        std::fs::read_to_string(spec_path)
            .with_context(|| format!("reading spec file: {spec_path}"))?
    } else {
        format!(
            "# Design Document\n\n\
             **Status:** Draft\n\n\
             ## Overview\n\n\
             {description}\n\n\
             ## Goals\n\n\
             - TODO\n\n\
             ## Non-Goals\n\n\
             - TODO\n\n\
             ## Architecture\n\n\
             TODO\n\n\
             ## Open Questions\n\n\
             - TODO\n"
        )
    };
    std::fs::write(root.join("docs/DESIGN.md"), &design_content)?;

    // Minimal Cargo.toml so the repo is immediately buildable.
    let project_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "new-project".to_owned());
    let cargo_toml = format!(
        "[package]\nname = \"{project_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         license = \"Apache-2.0 OR MIT\"\n\n[dependencies]\n"
    );
    std::fs::write(root.join("Cargo.toml"), cargo_toml)?;
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n")?;

    write_common_files(root, &project_name, description)?;
    Ok(())
}

fn scaffold_hello_world(path: &str, description: &str, language: &str) -> Result<()> {
    let root = Path::new(path);
    std::fs::create_dir_all(root.join("src"))?;

    let project_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "new-project".to_owned());

    match language {
        "rust" => {
            let cargo_toml = format!(
                "[package]\nname = \"{project_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 license = \"Apache-2.0 OR MIT\"\n\n[dependencies]\n"
            );
            std::fs::write(root.join("Cargo.toml"), cargo_toml)?;
            std::fs::write(
                root.join("src/main.rs"),
                "fn main() {\n    println!(\"Hello from {project_name}!\");\n}\n",
            )?;
            std::fs::create_dir_all(root.join("tests"))?;
            std::fs::write(
                root.join("tests/integration.rs"),
                "#[test]\nfn it_works() {\n    assert!(true);\n}\n",
            )?;
        }
        lang => {
            eprintln!("bf-scaffold: language '{lang}' not yet supported; creating Rust skeleton");
            return scaffold_hello_world(path, description, "rust");
        }
    }

    write_common_files(root, &project_name, description)?;
    write_ci_workflow(root, &project_name)?;
    Ok(())
}

fn write_common_files(root: &Path, project_name: &str, description: &str) -> Result<()> {
    // .gitignore
    std::fs::write(
        root.join(".gitignore"),
        "/target\n*.rs.bk\n*.pdb\n.bf/index/\n",
    )?;

    // README.md
    let readme = format!(
        "# {project_name}\n\n{description}\n\n## License\n\nApache-2.0 OR MIT\n"
    );
    std::fs::write(root.join("README.md"), readme)?;

    // LICENSE files — reference only; full text pulled from SPDX in production.
    std::fs::write(
        root.join("LICENSE-APACHE"),
        "Apache License 2.0 — see https://www.apache.org/licenses/LICENSE-2.0\n",
    )?;
    std::fs::write(
        root.join("LICENSE-MIT"),
        "MIT License — see https://opensource.org/licenses/MIT\n",
    )?;

    Ok(())
}

fn write_ci_workflow(root: &Path, _project_name: &str) -> Result<()> {
    std::fs::create_dir_all(root.join(".github/workflows"))?;
    let workflow = "\
name: CI
on: [push, pull_request]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --check
      - run: cargo clippy -- -D warnings
      - run: cargo test
";
    std::fs::write(root.join(".github/workflows/ci.yml"), workflow)?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        ScaffoldCommand::New {
            path,
            description,
            spec,
            template,
            mode,
            language,
        } => {
            if template.is_some() {
                eprintln!("bf-scaffold: named templates not yet implemented (Phase 2)");
                std::process::exit(bf_common::exit::UNAVAILABLE);
            }

            eprintln!("bf-scaffold: creating '{path}' in {mode} mode");
            std::fs::create_dir_all(&path)?;

            match mode {
                ScaffoldMode::DesignDoc => {
                    scaffold_design_doc(&path, &description, spec.as_deref())?;
                }
                ScaffoldMode::HelloWorld => {
                    scaffold_hello_world(&path, &description, &language)?;
                }
                ScaffoldMode::Poc => {
                    // POC is hello-world with an explicit TODO comment block.
                    scaffold_hello_world(&path, &description, &language)?;
                    eprintln!("bf-scaffold: POC mode: review src/main.rs for TODO markers");
                    let main_path = Path::new(&path).join("src/main.rs");
                    let existing = std::fs::read_to_string(&main_path)?;
                    let with_todos = format!(
                        "// TODO: implement core idea — {description}\n// TODO: add error handling\n// TODO: add tests\n\n{existing}"
                    );
                    std::fs::write(&main_path, with_todos)?;
                }
            }

            emit(&Event::Message {
                text: format!("Scaffold created at: {path}"),
            });
            eprintln!("bf-scaffold: done — next steps:");
            eprintln!("  cd {path}");
            eprintln!("  git init && git add . && git commit -m 'initial scaffold'");
            eprintln!("  bf-forge fork <upstream-or-new-repo>");
        }

        ScaffoldCommand::Template { cmd } => match cmd {
            TemplateCommand::List => {
                eprintln!("bf-scaffold: listing templates");
                // TODO: scan ~/.butterfork/templates/ and emit template metadata.
                eprintln!("  (no templates installed — use `bf-scaffold template add <url>`)");
            }
            TemplateCommand::Add { git_url } => {
                eprintln!("bf-scaffold: adding template from {git_url}");
                // TODO: clone git_url into ~/.butterfork/templates/<name>,
                // read scaffold.toml, and register the template.
                eprintln!("bf-scaffold: template add not yet implemented");
                std::process::exit(bf_common::exit::UNAVAILABLE);
            }
            TemplateCommand::Show { name } => {
                eprintln!("bf-scaffold: showing template '{name}'");
                // TODO: print scaffold.toml for the named template.
                eprintln!("bf-scaffold: template show not yet implemented");
                std::process::exit(bf_common::exit::UNAVAILABLE);
            }
        },
    }

    Ok(())
}
