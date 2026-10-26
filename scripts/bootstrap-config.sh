#!/usr/bin/env bash
# bootstrap-config.sh — create the local configuration files from the shipped
# examples, without ever overwriting what is already there (PART 03, K5).
#
# Why this exists
#   `docker compose up` mounts ./config.toml read-only and reads POSTGRES_*
#   from .env, so a fresh checkout cannot start until both exist. Copying them
#   by hand is where the two usual accidents happen: overwriting a configured
#   file, or copying an example into place and then committing it.
#
# Rules this script follows
#   * a file that already exists is never touched — no backup, no overwrite,
#     no merge; the operator's file is the source of truth;
#   * nothing from either file is ever printed: the examples are placeholders,
#     but a later run has to be safe against a populated one;
#   * the created files get mode 0600 (they are the ones that end up holding a
#     wallet path and a database password) and both are already gitignored,
#     which this script verifies rather than assumes.
#
# Usage
#   ./scripts/bootstrap-config.sh            create what is missing
#   ./scripts/bootstrap-config.sh --check    report only; exit 1 if anything is missing
set -euo pipefail

MODE="create"
case "${1:-}" in
"") ;;
--check) MODE="check" ;;
-h | --help)
  sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
  ;;
*)
  echo "[bootstrap-config] FAIL — unknown argument '$1' (expected --check or nothing)" >&2
  exit 2
  ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# destination:source, in the order an operator needs them.
PAIRS=(
  "config.toml:config.toml.example"
  ".env:.env.template"
)

say() { echo "[bootstrap-config] $*"; }

missing=0
for pair in "${PAIRS[@]}"; do
  dest="${pair%%:*}"
  src="${pair##*:}"
  if [ -e "$dest" ]; then
    say "$dest: already present — left untouched"
    continue
  fi
  if [ ! -f "$src" ]; then
    say "$dest: MISSING and $src is not in the tree — cannot create it here"
    missing=$((missing + 1))
    continue
  fi
  if [ "$MODE" = "check" ]; then
    say "$dest: missing (would be copied from $src)"
    missing=$((missing + 1))
    continue
  fi
  # `install` sets the mode atomically: a window where a config file is
  # world-readable is a window someone with a shell on this host can use.
  install -m 0600 "$src" "$dest"
  say "$dest: created from $src (mode 0600)"
  if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if git check-ignore -q -- "$dest"; then
      say "$dest: ignored by git (ok)"
    else
      say "$dest: WARNING — not covered by .gitignore; do not commit it"
    fi
  fi
done

if [ "$MODE" = "check" ]; then
  if [ "$missing" -gt 0 ]; then
    say "$missing file(s) missing — run ./scripts/bootstrap-config.sh"
    exit 1
  fi
  say "all local configuration present"
  exit 0
fi

say "next: edit config.toml (which modules, sizes, RPC endpoint) and .env"
say "      (POSTGRES_* for the compose stack). The wallet is configured through"
say "      the environment, not this file — see 'Going live' in README.md."
say "      Neither file's contents are ever printed by this script."
