# State Database Schema

Butterfork stores all durable state in SQLite at `~/.butterfork/state.db`.

This file is the authoritative schema reference. Read it with:

```
sqlite3 ~/.butterfork/state.db .schema
```

## Tables

### `projects`

One row per OSS project under Butterfork management.

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY | Auto-increment |
| `upstream_url` | TEXT NOT NULL | Original upstream repo URL |
| `fork_url` | TEXT | User's fork URL |
| `repo_path` | TEXT | Local checkout path (`~/.butterfork/repos/<slug>`) |
| `slug` | TEXT UNIQUE NOT NULL | Short identifier (e.g. `ripgrep`) |
| `default_build_adapter` | TEXT | Name of the bf-build-* adapter to use |
| `install_generation` | TEXT | Currently active generation ID |
| `self_hosting` | INTEGER | 1 if this is Butterfork itself |
| `added_at` | TEXT | ISO-8601 timestamp |

### `generations`

One row per build generation for each project.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | Generation ID (incrementing integer as text) |
| `project_id` | INTEGER REFERENCES projects | Parent project |
| `git_ref` | TEXT | Git commit SHA or branch ref at build time |
| `built_at` | TEXT | ISO-8601 timestamp |
| `artifact_paths` | TEXT | JSON array of installed file paths |
| `active` | INTEGER | 1 if this is the currently active generation |

### `intents`

A user's pending or completed change request.

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY | Auto-increment |
| `project_id` | INTEGER REFERENCES projects | Target project |
| `branch` | TEXT | Feature branch name (e.g. `bf/exclude-hidden-default`) |
| `issue_url` | TEXT | URL of the issue opened in the fork |
| `status` | TEXT | `open`, `in-progress`, `done`, `submitted`, `merged` |
| `title` | TEXT | Short description |
| `body` | TEXT | Full natural-language description |
| `created_at` | TEXT | ISO-8601 timestamp |

### `agent_runs`

One row per bf-agent invocation tied to an intent.

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY | Auto-increment |
| `intent_id` | INTEGER REFERENCES intents | Parent intent |
| `started_at` | TEXT | ISO-8601 timestamp |
| `finished_at` | TEXT | ISO-8601 timestamp or NULL if still running |
| `tokens_in` | INTEGER | Input tokens consumed |
| `tokens_out` | INTEGER | Output tokens generated |
| `cost_cents` | INTEGER | Estimated cost in US cents × 100 |
| `result` | TEXT | `success`, `failed`, `interrupted` |

### `prs`

Pull requests opened on behalf of an intent.

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PRIMARY KEY | Auto-increment |
| `intent_id` | INTEGER REFERENCES intents | Parent intent |
| `upstream_pr_url` | TEXT | URL of the PR on the upstream forge |
| `state` | TEXT | `open`, `closed`, `merged` |
| `last_polled_at` | TEXT | ISO-8601 timestamp |

## Schema migrations

Migrations live in `migrations/` as numbered SQL files (`0001_initial.sql`, etc.).
Applied automatically on startup; never applied destructively. The current schema
version is stored in `PRAGMA user_version`.

## Querying examples

```sh
# List all projects
sqlite3 ~/.butterfork/state.db 'SELECT slug, install_generation FROM projects'

# Show generations for ripgrep
sqlite3 ~/.butterfork/state.db \
  "SELECT id, git_ref, built_at, active FROM generations
   JOIN projects ON generations.project_id = projects.id
   WHERE projects.slug = 'ripgrep'"

# Show all open intents
sqlite3 ~/.butterfork/state.db \
  "SELECT p.slug, i.title, i.status FROM intents i
   JOIN projects p ON i.project_id = p.id
   WHERE i.status != 'merged'"
```
