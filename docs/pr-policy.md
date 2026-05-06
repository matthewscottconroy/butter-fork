# PR Policy Configuration

Butterfork runs a set of pre-flight checks before opening a pull request.
Per-project policy overrides live in `~/.butterfork/pr-policy/`.

## File location

```
~/.butterfork/pr-policy/<repo-name>.toml
```

The file is named after the **repository name** only, not the full `owner/repo` slug.
For example, to configure policy for `BurntSushi/ripgrep`, create:

```
~/.butterfork/pr-policy/ripgrep.toml
```

## Format

The file is **flat TOML** — no section headers required. All fields are optional;
any omitted field falls back to its default value.

```toml
# Require DCO Signed-off-by on every commit in the PR.
# Default: true
require_dco = true

# Run cargo test (or equivalent) before opening the PR.
# Default: true
require_tests = true

# Run cargo fmt --check before opening the PR.
# Default: false
require_format_check = false

# Warn when the diff exceeds this many total line changes.
# Default: 1000
max_diff_lines = 800

# Warn when more than ~80% of the diff is whitespace changes.
# Default: true
warn_whitespace_churn = true

# Block PRs that introduce new dependencies not in the current lockfile.
# Default: false
block_new_dependencies = false

# How to handle the AI-assistance footer in the PR body.
# Options: "include" | "exclude" | "ask"
# Default: "include"
ai_footer = "include"
```

## Policy fields reference

| Field | Type | Default | Description |
|---|---|---|---|
| `require_dco` | bool | `true` | Require `Signed-off-by:` on every commit |
| `require_tests` | bool | `true` | Run `cargo test` (or equivalent) before PR |
| `require_format_check` | bool | `false` | Run `cargo fmt --check` before PR |
| `max_diff_lines` | integer | `1000` | Warn when diff exceeds this many total changes |
| `warn_whitespace_churn` | bool | `true` | Warn when diff is mostly whitespace |
| `block_new_dependencies` | bool | `false` | Block PRs adding undeclared dependencies |
| `ai_footer` | string | `"include"` | AI-assistance footer: `"include"`, `"exclude"`, or `"ask"` |

## AI footer values

- **`"include"`** — Always append the AI-assistance footer to the PR body. This is
  the default and is appropriate for projects that do not have a policy against
  AI assistance.
- **`"exclude"`** — Never append the footer. Use this for projects that explicitly
  prohibit or discourage AI-assisted contributions.
- **`"ask"`** — Not yet interactive; currently falls back to `"include"`.

You can also suppress the footer globally by setting the environment variable
`BF_NO_AI_FOOTER=1`.

## Example: restrictive project

For a project with strict DCO requirements, small-PR preferences, and a policy
against AI assistance:

```toml
# ~/.butterfork/pr-policy/linux.toml
require_dco = true
require_format_check = true
max_diff_lines = 300
warn_whitespace_churn = true
ai_footer = "exclude"
```

## Example: permissive project

For an experiment or a project you maintain yourself:

```toml
# ~/.butterfork/pr-policy/my-experiment.toml
require_dco = false
require_tests = false
max_diff_lines = 5000
ai_footer = "include"
```

## How checks are run

Checks run automatically when you execute `bf forge pr open` (or
`bf-forge-github pr open`). Results are printed to stderr:

```
bf-forge-github: pre-flight checks for BurntSushi/ripgrep@feat/my-change
bf-forge-github: [ok] CONTRIBUTING.md found
bf-forge-github: [ok] license: MIT (permissive)
bf-forge-github: [ok] all commits have DCO Signed-off-by
bf-forge-github: [ok] diff size within limit (47)
bf-forge-github: [warn] cargo fmt check failed — run `cargo fmt --all` before opening a PR
```

Warnings do not block the PR. Errors (e.g., failing tests when `require_tests = true`)
block the PR and exit with a non-zero status.
