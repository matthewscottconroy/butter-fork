#!/usr/bin/env bash
# scripts/rollback.sh — shell-script equivalent of `bf rescue rollback <slug>`
#
# This script and `bf rescue activate` MUST produce identical side effects.
# CI verifies this file is executable and passes shellcheck.
#
# Usage:
#   scripts/rollback.sh <slug>
#   scripts/rollback.sh <slug> <generation-id>   # roll back to a specific generation

set -euo pipefail

usage() {
    echo "Usage: rollback.sh <slug> [<generation-id>]" >&2
    echo "" >&2
    echo "  <slug>            Project slug (e.g. ripgrep)" >&2
    echo "  <generation-id>   Specific generation to activate (default: previous)" >&2
    exit 64  # EX_USAGE
}

SLUG="${1:-}"
GEN_ID="${2:-}"

[[ -z "${SLUG}" ]] && usage

echo "rollback.sh: rolling back '${SLUG}'" >&2

if [[ -n "${GEN_ID}" ]]; then
    echo "rollback.sh: activating generation ${GEN_ID}" >&2
    bf-install activate "${SLUG}" "${GEN_ID}"
else
    bf-install rollback "${SLUG}"
fi

echo "rollback.sh: done — run 'bf rescue list ${SLUG}' to verify" >&2
