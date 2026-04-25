#!/usr/bin/env bash
# scripts/install.sh — shell-script equivalent of `bf install <slug>`
#
# This script and `bf install` MUST produce identical side effects.
# CI runs shellcheck on this file and verifies it is executable.
# If you change the pipeline here, update bf/src/main.rs accordingly.
#
# Usage:
#   scripts/install.sh <slug>              # fork + clone + build + install
#   BF_NO_FORK=1 scripts/install.sh <url> # skip forking (test / own-repo mode)
#   scripts/install.sh --help

set -euo pipefail

usage() {
    echo "Usage: install.sh <slug-or-url> [--no-fork] [--debug] [--dest <path>]" >&2
    echo "" >&2
    echo "  <slug>        Project slug from the catalog (e.g. ripgrep) or a full URL" >&2
    echo "  --no-fork     Skip GitHub fork; clone upstream directly" >&2
    echo "  --debug       Build in debug mode instead of release" >&2
    echo "  --dest PATH   Override the local clone destination" >&2
    exit 64  # EX_USAGE
}

SLUG=""
NO_FORK="${BF_NO_FORK:-}"
RELEASE_FLAG="--release"
DEST=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-fork)   NO_FORK=1; shift ;;
        --debug)     RELEASE_FLAG=""; shift ;;
        --dest)      DEST="$2"; shift 2 ;;
        --help|-h)   usage ;;
        -*)          echo "Unknown flag: $1" >&2; usage ;;
        *)           SLUG="$1"; shift ;;
    esac
done

[[ -z "${SLUG}" ]] && usage

BF_HOME="${BF_HOME:-$HOME/.butterfork}"

echo "install.sh: installing '${SLUG}'" >&2

# ── step 1: resolve upstream URL ────────────────────────────────────────────
echo "install.sh: step 1/5 — catalog lookup" >&2
if CATALOG_JSON="$(bf-catalog show "${SLUG}" 2>/dev/null)"; then
    UPSTREAM_URL="$(echo "${CATALOG_JSON}" | \
        grep -o '"upstream_url":"[^"]*"' | head -1 | cut -d'"' -f4)"
else
    # Treat the slug as a raw URL.
    if [[ "${SLUG}" == http* ]]; then
        echo "install.sh: '${SLUG}' not in catalog; treating as URL" >&2
        UPSTREAM_URL="${SLUG}"
    else
        echo "install.sh: '${SLUG}' not found — run \`bf-catalog add <url>\` first" >&2
        exit 66  # EX_NOINPUT
    fi
fi

echo "install.sh: upstream: ${UPSTREAM_URL}" >&2

# ── step 2: fork (or skip) ───────────────────────────────────────────────────
if [[ -n "${NO_FORK}" ]]; then
    echo "install.sh: step 2/5 — skipping fork (BF_NO_FORK)" >&2
    FORK_URL="${UPSTREAM_URL}"
else
    echo "install.sh: step 2/5 — forking on GitHub" >&2
    FORK_OUTPUT="$(bf-forge fork "${UPSTREAM_URL}")"
    FORK_URL="$(echo "${FORK_OUTPUT}" | \
        python3 -c "
import sys, json
for line in sys.stdin:
    try:
        obj = json.loads(line)
        if obj.get('type') == 'fork-created':
            print(obj['fork_url'])
            break
    except Exception:
        pass
" 2>/dev/null || true)"

    if [[ -z "${FORK_URL}" ]]; then
        echo "install.sh: could not parse fork URL from bf-forge output" >&2
        echo "install.sh: is \`gh\` installed and authenticated?" >&2
        exit 69  # EX_UNAVAILABLE
    fi
fi
echo "install.sh: fork: ${FORK_URL}" >&2

# ── step 3: clone ────────────────────────────────────────────────────────────
PROJECT_SLUG="${FORK_URL%/}"
PROJECT_SLUG="${PROJECT_SLUG%.git}"
PROJECT_SLUG="${PROJECT_SLUG##*/}"

DEST="${DEST:-${BF_HOME}/repos/${PROJECT_SLUG}}"

if [[ -d "${DEST}" ]]; then
    echo "install.sh: step 3/5 — destination exists, pulling" >&2
    git -C "${DEST}" pull --ff-only || true
else
    echo "install.sh: step 3/5 — cloning ${FORK_URL} → ${DEST}" >&2
    bf-forge clone "${FORK_URL}" "${DEST}"
fi

# ── step 4: build ────────────────────────────────────────────────────────────
echo "install.sh: step 4/5 — building" >&2
# shellcheck disable=SC2086
BUILD_OUTPUT="$(bf-build run "${DEST}" ${RELEASE_FLAG})"
MANIFEST_PATH="$(echo "${BUILD_OUTPUT}" | \
    python3 -c "
import sys, json
for line in sys.stdin:
    try:
        obj = json.loads(line)
        if obj.get('type') == 'build-complete':
            print(obj['manifest_path'])
            break
    except Exception:
        pass
" 2>/dev/null || true)"

MANIFEST_PATH="${MANIFEST_PATH:-${DEST}/target/bf-artifact-manifest.json}"
echo "install.sh: manifest: ${MANIFEST_PATH}" >&2

# ── step 5: install ──────────────────────────────────────────────────────────
echo "install.sh: step 5/5 — installing generation" >&2
bf-install add "${PROJECT_SLUG}" "${MANIFEST_PATH}"
bf-install activate "${PROJECT_SLUG}" latest

echo "install.sh: '${PROJECT_SLUG}' installed — binaries under ${BF_HOME}/bin/" >&2
echo "install.sh: add ${BF_HOME}/bin to your PATH if not already there" >&2
