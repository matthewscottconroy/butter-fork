#!/usr/bin/env bash
# Build bf-fat and install component symlinks under $BF_HOME/bin/.
# Usage: ./scripts/fat-install.sh [--prefix <dir>]
set -euo pipefail

BF_HOME="${BF_HOME:-$HOME/.butterfork}"
PREFIX="$BF_HOME"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; exit 64 ;;
  esac
done

BIN_DIR="$PREFIX/bin"
mkdir -p "$BIN_DIR"

echo "fat-install: building bf-fat (release)..."
cargo build --release -p bf-fat

FAT="$(cargo metadata --format-version 1 --no-deps | \
  python3 -c "import sys,json; d=json.load(sys.stdin); print(d['target_directory'])")/release/bf-fat"

echo "fat-install: installing fat binary → $BIN_DIR/bf-fat"
install -m755 "$FAT" "$BIN_DIR/bf-fat"

COMPONENTS=(
  bf
  bf-agent
  bf-agent-ollama
  bf-bootstrap
  bf-build
  bf-build-cargo
  bf-build-cmake
  bf-build-meson
  bf-build-npm
  bf-catalog
  bf-forge
  bf-forge-github
  bf-forge-gitlab
  bf-index
  bf-install
  bf-sandbox
  bf-scaffold
)

echo "fat-install: creating symlinks..."
for comp in "${COMPONENTS[@]}"; do
  target="$BIN_DIR/$comp"
  ln -sf bf-fat "$target"
  echo "  $target -> bf-fat"
done

echo "fat-install: done — add $BIN_DIR to your PATH if it isn't already"
