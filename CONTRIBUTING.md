# Contributing to Butterfork

Thank you for considering a contribution. Butterfork is structured as a family of
independent components — you can contribute meaningfully to just one without touching
any other.

## Quick start

```sh
git clone https://github.com/matthewscottconroy/butter-fork
cd butter-fork
cargo build --all
cargo test --all
```

## Architecture overview

Each `bf-*` directory is a standalone Cargo crate that produces a standalone binary.
Start by reading [butterfork-design.md](butterfork-design.md) for the full design,
and [docs/schema.md](docs/schema.md) for the state database schema.

## Developer Certificate of Origin

Butterfork uses the [DCO](https://developercertificate.org/) instead of a CLA.
Sign off every commit with `git commit --signoff` (or `-s`). This certifies that
you wrote the code or have the right to submit it under the project's license.

## Code style

- `cargo fmt` is enforced in CI.
- `cargo clippy -- -D warnings` is enforced in CI.
- Comments only when the *why* is non-obvious; well-named identifiers carry the what.
- All public-facing CLI output on stdout is NDJSON (one JSON object per line).
  Human-readable progress goes to stderr.
- Exit codes follow `sysexits.h` conventions (see `bf-common/src/lib.rs`).

## Adding a build adapter

Create a new crate `bf-build-<name>` in the workspace root that implements:

```
bf-build-<name> detect <repo>   # print BuildDetection JSON and exit 0, or exit 1
bf-build-<name> plan <repo>     # print BuildPlan JSON
bf-build-<name> run <repo> [--plan plan.json] [--release]
```

Add the crate to the `[workspace]` members list in `Cargo.toml`. No changes to
`bf-build` are required — it discovers adapters on PATH by name.

## Adding a forge backend

Create a new crate `bf-forge-<name>` that implements the same CLI surface as
`bf-forge`: `fork`, `clone`, `issue open`, `pr open`, `pr status`, `pr watch`.

## Adding a scaffold template

Templates are ordinary git repositories containing a `scaffold.toml` manifest and
a `template/` directory. No changes to Butterfork core are required. Publish your
template as a public git repo and install it with:

```sh
bf-scaffold template add <git-url>
```

## Non-trivial design changes

Use the [RFC process](rfcs/README.md) for changes to public CLI contracts, the
on-disk state layout, or the agent event protocol.

## Release cadence

Each component has its own release cadence. A breaking change to one component does
not force a release of any other. CLI schema compatibility ranges are tracked via
`bf doctor`.

## License

By contributing, you agree that your contributions will be dual-licensed under
Apache-2.0 OR MIT, the same as the rest of the project.
