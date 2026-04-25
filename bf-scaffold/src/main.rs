use anyhow::{Context, Result};
use bf_common::{emit, Event};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

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

// ── scaffold.toml format ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ScaffoldToml {
    template: TemplateMetadata,
}

#[derive(Debug, Deserialize)]
struct TemplateMetadata {
    name: String,
    description: String,
    language: Option<String>,
    version: Option<String>,
}

// ── template storage ──────────────────────────────────────────────────────────

fn templates_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    let bf_home = std::env::var("BF_HOME").unwrap_or_else(|_| format!("{home}/.butterfork"));
    format!("{bf_home}/templates")
}

fn template_path(name: &str) -> String {
    format!("{}/{name}", templates_dir())
}

fn load_scaffold_toml(template_dir: &str) -> Result<ScaffoldToml> {
    let toml_path = format!("{template_dir}/scaffold.toml");
    let s = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("reading {toml_path}"))?;
    toml::from_str(&s).with_context(|| format!("parsing {toml_path}"))
}

fn repo_name_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

// ── template: add ─────────────────────────────────────────────────────────────

fn template_add(git_url: &str) -> Result<()> {
    let name = repo_name_from_url(git_url);
    let dest = template_path(&name);

    if Path::new(&dest).exists() {
        eprintln!("bf-scaffold: template '{name}' already exists at {dest}");
        eprintln!("bf-scaffold: remove it first or choose a different URL");
        std::process::exit(bf_common::exit::CANTCREAT);
    }

    eprintln!("bf-scaffold: cloning template '{name}' from {git_url}");
    let tdir = templates_dir();
    std::fs::create_dir_all(&tdir)?;

    let status = Command::new("git")
        .args(["clone", "--", git_url, &dest])
        .status()
        .context("running `git clone`")?;
    if !status.success() {
        anyhow::bail!("`git clone` failed for template {git_url}");
    }

    // Verify scaffold.toml exists and is parseable.
    let meta = load_scaffold_toml(&dest)
        .with_context(|| format!("template '{name}' must contain a valid scaffold.toml"))?;

    eprintln!(
        "bf-scaffold: template '{}' added — {}",
        meta.template.name, meta.template.description
    );
    emit(&Event::Message {
        text: format!("Template '{}' installed at {dest}", meta.template.name),
    });
    Ok(())
}

// ── template: list ────────────────────────────────────────────────────────────

fn template_list() -> Result<()> {
    let tdir = templates_dir();
    let dir = std::path::Path::new(&tdir);

    if !dir.exists() {
        eprintln!("bf-scaffold: no templates installed — use `bf-scaffold template add <url>`");
        return Ok(());
    }

    let entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    if entries.is_empty() {
        eprintln!("bf-scaffold: no templates installed — use `bf-scaffold template add <url>`");
        return Ok(());
    }

    let mut found = false;
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(meta) = load_scaffold_toml(&path.to_string_lossy()) {
            println!(
                "{}",
                serde_json::json!({
                    "name": meta.template.name,
                    "description": meta.template.description,
                    "language": meta.template.language,
                    "version": meta.template.version,
                    "dir": path.display().to_string(),
                })
            );
            eprintln!(
                "  {} — {} [{}]",
                meta.template.name,
                meta.template.description,
                meta.template.language.as_deref().unwrap_or("any")
            );
            found = true;
        }
    }

    if !found {
        eprintln!("bf-scaffold: no valid templates found (each must contain scaffold.toml)");
    }
    Ok(())
}

// ── template: show ────────────────────────────────────────────────────────────

fn template_show(name: &str) -> Result<()> {
    let dest = template_path(name);
    if !Path::new(&dest).exists() {
        eprintln!("bf-scaffold: template '{name}' not found");
        eprintln!("bf-scaffold: use `bf-scaffold template list` to see available templates");
        std::process::exit(bf_common::exit::NOINPUT);
    }

    let meta = load_scaffold_toml(&dest)?;
    println!(
        "{}",
        serde_json::json!({
            "name": meta.template.name,
            "description": meta.template.description,
            "language": meta.template.language,
            "version": meta.template.version,
            "dir": dest,
        })
    );
    eprintln!("bf-scaffold: template '{}':", meta.template.name);
    eprintln!("  description: {}", meta.template.description);
    if let Some(lang) = &meta.template.language {
        eprintln!("  language:    {lang}");
    }
    if let Some(ver) = &meta.template.version {
        eprintln!("  version:     {ver}");
    }
    eprintln!("  dir:         {dest}");
    Ok(())
}

// ── template: apply ───────────────────────────────────────────────────────────

fn apply_template(template_name: &str, target: &str, project_name: &str, description: &str) -> Result<()> {
    let src = template_path(template_name);
    if !Path::new(&src).exists() {
        eprintln!("bf-scaffold: template '{template_name}' not found");
        eprintln!("bf-scaffold: install with `bf-scaffold template add <url>`");
        std::process::exit(bf_common::exit::NOINPUT);
    }

    let meta = load_scaffold_toml(&src)?;
    eprintln!(
        "bf-scaffold: applying template '{}' — {}",
        meta.template.name, meta.template.description
    );

    copy_template_dir(Path::new(&src), Path::new(target), project_name, description)?;
    Ok(())
}

fn copy_template_dir(src: &Path, dest: &Path, project_name: &str, description: &str) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip .git directory and scaffold.toml — they're template metadata.
        if name == ".git" || name == "scaffold.toml" {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dest.join(&name);
        if src_path.is_dir() {
            copy_template_dir(&src_path, &dst_path, project_name, description)?;
        } else {
            let content = std::fs::read_to_string(&src_path).unwrap_or_default();
            let rendered = content
                .replace("{{project_name}}", project_name)
                .replace("{{description}}", description);
            std::fs::write(&dst_path, rendered)?;
        }
    }
    Ok(())
}

// ── built-in scaffold modes ───────────────────────────────────────────────────

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
                format!("fn main() {{\n    println!(\"Hello from {project_name}!\");\n}}\n"),
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
    write_ci_workflow(root)?;
    Ok(())
}

fn write_common_files(root: &Path, project_name: &str, description: &str) -> Result<()> {
    std::fs::write(
        root.join(".gitignore"),
        "/target\n*.rs.bk\n*.pdb\n.bf/index/\n",
    )?;
    let readme = format!(
        "# {project_name}\n\n{description}\n\n## License\n\nApache-2.0 OR MIT\n"
    );
    std::fs::write(root.join("README.md"), readme)?;
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

fn write_ci_workflow(root: &Path) -> Result<()> {
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

// ── entry points ──────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
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
            if let Some(tmpl) = template {
                eprintln!("bf-scaffold: creating '{path}' from template '{tmpl}'");
                std::fs::create_dir_all(&path)?;
                let project_name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "new-project".to_owned());
                apply_template(&tmpl, &path, &project_name, &description)?;
                write_common_files(Path::new(&path), &project_name, &description)?;
            } else {
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
                        scaffold_hello_world(&path, &description, &language)?;
                        eprintln!("bf-scaffold: POC mode: review src/main.rs for TODO markers");
                        let main_path = Path::new(&path).join("src/main.rs");
                        let existing = std::fs::read_to_string(&main_path)?;
                        let with_todos = format!(
                            "// TODO: implement core idea — {description}\n\
                             // TODO: add error handling\n\
                             // TODO: add tests\n\n{existing}"
                        );
                        std::fs::write(&main_path, with_todos)?;
                    }
                }
            }

            emit(&Event::Message {
                text: format!("Scaffold created at: {path}"),
            });
            eprintln!("bf-scaffold: done — next steps:");
            eprintln!("  cd {path}");
            eprintln!("  git init && git add . && git commit -s -m 'initial scaffold'");
            eprintln!("  bf-forge fork <upstream-or-new-repo>");
        }

        ScaffoldCommand::Template { cmd } => match cmd {
            TemplateCommand::List => template_list()?,
            TemplateCommand::Add { git_url } => template_add(&git_url)?,
            TemplateCommand::Show { name } => template_show(&name)?,
        },
    }

    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run()
}
