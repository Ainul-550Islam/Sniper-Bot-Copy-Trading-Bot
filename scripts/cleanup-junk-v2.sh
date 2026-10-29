#!/usr/bin/env bash
# cleanup-junk-v2.sh — remove tracked host junk, binaries, and obsolete scripts.
#
# Modes:
#   ./scripts/cleanup-junk-v2.sh          # dry-run (prints actions, modifies nothing)
#   ./scripts/cleanup-junk-v2.sh --apply  # execute git rm --cached and delete files
#
# Refuses to run outside the repository root.
set -euo pipefail

MODE="dry-run"
if [[ "${1:-}" == "--apply" ]]; then
  MODE="apply"
elif [[ -n "${1:-}" && "${1:-}" != "--dry-run" ]]; then
  echo "Usage: $0 [--dry-run|--apply]" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CURRENT_DIR="$(pwd -P)"
if [[ "$CURRENT_DIR" != "$REPO_ROOT" ]]; then
  echo "cleanup-junk-v2: FAIL — must be run from the repository root ($REPO_ROOT)" >&2
  exit 1
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "cleanup-junk-v2: FAIL — not inside a git work tree" >&2
  exit 1
fi

JUNK_PATHS=(
  "apps/api/src/modules/auth/auth.module.ts"
  ".cargo/bin/rustup"
  "rustup-init.sh"
  "core-launch.out"
  "run-tests.sh"
  "run-p2b.sh"
  "run_baseline.sh"
  "run_baseline2.sh"
  "run-core.sh"
  "APPLY.sh"
  "APPLY-BATCH07.sh"
  "APPLY-PUSH-READY.sh"
  "AUDIT.md"
)

echo "cleanup-junk-v2: mode=$MODE root=$REPO_ROOT"

for p in "${JUNK_PATHS[@]}"; do
  is_tracked=0
  if git ls-files --error-unmatch "$p" >/dev/null 2>&1; then
    is_tracked=1
  fi

  exists_on_disk=0
  if [[ -e "$p" || -L "$p" ]]; then
    exists_on_disk=1
  fi

  if [[ $is_tracked -eq 0 && $exists_on_disk -eq 0 ]]; then
    echo "  [clean] $p already absent"
    continue
  fi

  if [[ "$MODE" == "dry-run" ]]; then
    if [[ $is_tracked -eq 1 ]]; then
      echo "  [would untrack] git rm --cached $p"
    fi
    if [[ $exists_on_disk -eq 1 ]]; then
      echo "  [would delete]  rm -f $p"
    fi
  else
    if [[ $is_tracked -eq 1 ]]; then
      git rm --cached -f -- "$p" >/dev/null 2>&1 || true
      echo "  [untracked] git rm --cached $p"
    fi
    if [[ $exists_on_disk -eq 1 ]]; then
      rm -f "$p"
      echo "  [deleted]   rm -f $p"
    fi
  fi
done

# Clean up empty parent directories if any were left behind
if [[ "$MODE" == "apply" ]]; then
  rmdir apps/api/src/modules/auth 2>/dev/null || true
  rmdir apps/api/src/modules 2>/dev/null || true
  rmdir apps/api/src 2>/dev/null || true
  rmdir apps/api 2>/dev/null || true
  rmdir .cargo/bin 2>/dev/null || true
fi

echo "cleanup-junk-v2: finished ($MODE)"
