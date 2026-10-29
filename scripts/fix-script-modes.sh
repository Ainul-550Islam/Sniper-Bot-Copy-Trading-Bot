#!/usr/bin/env bash
# fix-script-modes.sh — ensure every tracked shell script has executable permissions (100755).
# Idempotent.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "fix-script-modes: FAIL — not inside a git work tree" >&2
  exit 1
fi

echo "fix-script-modes: setting executable bits for tracked *.sh scripts..."
git ls-files '*.sh' | while IFS= read -r file; do
  if [[ -f "$file" ]]; then
    chmod +x "$file"
    git update-index --chmod=+x "$file"
  fi
done

echo "fix-script-modes: OK"
