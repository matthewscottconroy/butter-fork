#!/usr/bin/env bash
# setup-path.sh — add ~/.butterfork/bin to the user's shell PATH
#
# Usage: bash scripts/setup-path.sh [--dry-run]
#   --dry-run   Print what would be written without modifying any file.
#
# Supports: bash, zsh, fish.  Falls back to a printed manual instruction.

set -euo pipefail

BF_BIN="${BF_HOME:-$HOME/.butterfork}/bin"
DRY_RUN=0

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    *) echo "Unknown argument: $arg" >&2; exit 64 ;;
  esac
done

info()  { echo "  [info]  $*" >&2; }
ok()    { echo "  [ok]    $*" >&2; }
warn()  { echo "  [warn]  $*" >&2; }

# Already on PATH — nothing to do.
if echo ":$PATH:" | grep -q ":$BF_BIN:"; then
  ok "$BF_BIN is already on PATH."
  exit 0
fi

detect_shell() {
  # Prefer $SHELL, fall back to the running process's parent.
  local shell_bin
  shell_bin="$(basename "${SHELL:-}")"
  case "$shell_bin" in
    fish|zsh|bash) echo "$shell_bin"; return ;;
  esac
  # Try to detect from /proc/self/status on Linux.
  if [[ -r /proc/self/status ]]; then
    local ppid
    ppid=$(awk '/^PPid:/{print $2}' /proc/self/status)
    local parent_cmd
    parent_cmd=$(cat "/proc/$ppid/comm" 2>/dev/null || true)
    case "$parent_cmd" in
      fish|zsh|bash) echo "$parent_cmd"; return ;;
    esac
  fi
  echo "unknown"
}

write_or_print() {
  local rc_file="$1"
  local line="$2"

  if [[ $DRY_RUN -eq 1 ]]; then
    info "Would append to $rc_file:"
    echo "    $line"
    return
  fi

  mkdir -p "$(dirname "$rc_file")"
  printf '\n%s\n' "$line" >> "$rc_file"
  ok "Appended to $rc_file"
}

SHELL_NAME=$(detect_shell)
info "Detected shell: $SHELL_NAME"
info "Adding $BF_BIN to PATH..."

case "$SHELL_NAME" in
  bash)
    RC="$HOME/.bashrc"
    # On macOS, login shells read .bash_profile, not .bashrc.
    if [[ "$(uname -s)" == "Darwin" && -f "$HOME/.bash_profile" ]]; then
      RC="$HOME/.bash_profile"
    fi
    write_or_print "$RC" "export PATH=\"$BF_BIN:\$PATH\"  # added by butterfork setup-path"
    ;;
  zsh)
    RC="${ZDOTDIR:-$HOME}/.zshrc"
    write_or_print "$RC" "export PATH=\"$BF_BIN:\$PATH\"  # added by butterfork setup-path"
    ;;
  fish)
    FISH_CONF="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/butterfork.fish"
    write_or_print "$FISH_CONF" "fish_add_path $BF_BIN"
    ;;
  *)
    warn "Could not detect shell."
    warn "Add the following line to your shell's RC file manually:"
    echo ""
    echo "    export PATH=\"$BF_BIN:\$PATH\""
    echo ""
    exit 0
    ;;
esac

if [[ $DRY_RUN -eq 0 ]]; then
  echo ""
  ok "Done. Open a new terminal (or run: source ~/<rc-file>) to pick up the change."
fi
