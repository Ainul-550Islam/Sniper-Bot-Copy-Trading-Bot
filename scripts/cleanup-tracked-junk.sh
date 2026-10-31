#!/usr/bin/env bash
# cleanup-tracked-junk.sh — untrack the host state and local run scaffolding
# that was committed into the repository root (PART 03, K3).
#
# What this is for
#   The tracked tree carries files that are not source: a rustup shim and its
#   shell env file, a Solana CLI install config, a bootstrap downloader, and a
#   pile of one-shot launcher scripts and captured console output from somebody's
#   machine. They break other people's checkouts (`.cargo/config.toml` is read by
#   every cargo invocation in the workspace, `.cargo/env` is a shell script for one
#   specific HOME path), they lie about what the project is, and `core-launch.out`
#   is the kind of file that quietly accumulates a keypair in its first line.
#   The AUDIT report at the root is also out of place: the archive under
#   docs/archive/ is where the repo keeps dated reports.
#
# Modes
#   --dry-run (default)  print every action, change nothing. Always safe.
#   --apply              perform them (git rm / git mv / .gitignore edits).
#
# Idempotent: a second run reports "no tracked junk" and exits 0, so it is safe
# in a pipeline and after a partial run.
#
# Note on `git rm` vs deleting: a delivery that only adds files cannot remove a
# tracked path, so the removals here are the mechanism — running this script (or
# applying the printed commands) is what takes the files out of version control.
# Working-tree copies of the two report/output files are deleted with them; the
# launcher scripts are untracked only, so nothing on disk is lost if they were
# still in use.
set -euo pipefail

MODE="dry-run"
case "${1:-}" in
"") ;;
--dry-run) MODE="dry-run" ;;
--apply) MODE="apply" ;;
-h | --help)
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
  ;;
*)
  echo "[cleanup-tracked-junk] FAIL — unknown argument '$1' (expected --dry-run or --apply)" >&2
  exit 2
  ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "[cleanup-tracked-junk] FAIL — not inside a git work tree" >&2
  exit 1
fi

# Files that are host/tooling state, never source. Each is untracked (and its
# working copy removed) only if it is actually tracked right now.
JUNK=(
  # Explicit round-one cleanup: these four paths must not ship.
  ".cargo/env"
  ".config/solana/install/config.yml"
  "run-part2.sh"
  "run-tests.out"
  ".cargo/bin/rustup"
  "rustup-init.sh"
  "core-launch.out"
  "run-core.sh"
  "run-p2b.sh"
  "run-tests.sh"
  "run_baseline.sh"
  "run_baseline2.sh"
)

# Patterns that keep them out again. Anchored deliberately: `run-*.sh` unanchored
# would swallow scripts/run-external-validation.sh, which IS source.
IGNORE=(
  "/run-*.sh"
  "/run_baseline*.sh"
  "/core-launch.out"
  "/run-tests.out"
  "*.out"
  "rustup-init.sh"
  ".cargo/env"
  ".config/"
  ".config/solana/"
)

# Files that reference the audit report by path. They must point at the archive
# copy; the script rewrites any bare `AUDIT.md` reference it finds.
AUDIT_REFS=(
  "README.md"
  "GAP-MISSING-FILES.md"
  "CHANGELOG.md"
  "scripts/release-check.sh"
  "scripts/staking-identity.sh"
  "scripts/verify-marketing-claims.sh"
  "scripts/update-release-manifest.sh"
)

say() { echo "[cleanup-tracked-junk] $*"; }
run() {
  if [ "$MODE" = "apply" ]; then
    "$@"
  else
    echo "  would run: $*"
  fi
}

tracked() { git ls-files --error-unmatch -- "$1" >/dev/null 2>&1; }

changed=0

say "mode: $MODE"
say "repo: $ROOT"

# --- 1. untrack committed host state and local run scaffolding -------------
say "tracked junk:"
for path in "${JUNK[@]}"; do
  if ! tracked "$path"; then
    echo "  - $path: not tracked (ok)"
    continue
  fi
  changed=$((changed + 1))
  # Captured output and downloaded binaries have no local value worth keeping;
  # the launcher scripts do, so they are only untracked.
  case "$path" in
  *.out | rustup-init.sh | .cargo/* | .config/*)
    say "  - $path: untrack and delete (generated/downloaded)"
    run git rm -f --quiet -- "$path"
    ;;
  *)
    say "  - $path: untrack, keep the working copy"
    run git rm --cached --quiet -- "$path"
    ;;
  esac
done

# --- 2. move the root audit report into the archive ------------------------
if tracked "AUDIT.md"; then
  changed=$((changed + 1))
  if [ -f "docs/archive/AUDIT.md" ]; then
    say "  - AUDIT.md: already archived at docs/archive/AUDIT.md, removing the root copy"
    run git rm -f --quiet -- "AUDIT.md"
  else
    say "  - AUDIT.md: moving to docs/archive/AUDIT.md"
    run mkdir -p docs/archive
    run git mv -- "AUDIT.md" "docs/archive/AUDIT.md"
  fi
else
  say "  - AUDIT.md: not tracked at the root (ok)"
fi

# --- 3. config.toml: tracked local instance, keep the file ----------------
# The operator's real config (with endpoints and, in practice, keys) must not
# be in the repository. .gitignore already lists /config.toml, and
# scripts/bootstrap-config.sh creates it from the example.
if tracked "config.toml"; then
  changed=$((changed + 1))
  say "  - config.toml: untracking, the local file stays on disk"
  run git rm --cached --quiet -- "config.toml"
else
  say "  - config.toml: not tracked (ok)"
fi

# --- 4. references to the report's old path -------------------------------
say "audit report references:"
for file in "${AUDIT_REFS[@]}"; do
  [ -f "$file" ] || continue
  # A line that already says `docs/archive/` is describing the archive (or
  # already points at it); only a bare root path is a dangling reference.
  hits="$(grep -nE '(^|[^/[:alnum:]._-])AUDIT\.md' "$file" 2>/dev/null | grep -v 'docs/archive' || true)"
  if [ -z "$hits" ]; then
    echo "  - $file: no bare AUDIT.md reference (ok)"
    continue
  fi
  changed=$((changed + 1))
  echo "  - $file: references the root path, retargeting to docs/archive/AUDIT.md"
  echo "$hits" | sed 's/^/      /'
  if [ "$MODE" = "apply" ]; then
    # `sed -i.bak` then drop the backup: portable across GNU/BSD sed, and a
    # stray *.bak in the tree is exactly what this script is here to remove.
    sed -i.bak -E 's#(^|[^/[:alnum:]._-])AUDIT\.md#\1docs/archive/AUDIT.md#g' "$file"
    rm -f "${file}.bak"
  else
    echo "      would run: sed -i -E 's#(^|[^/[:alnum:]._-])AUDIT\\.md#\\1docs/archive/AUDIT.md#g' $file"
  fi
done

# --- 5. .gitignore: keep them out again ------------------------------------
missing=()
for pattern in "${IGNORE[@]}"; do
  grep -qxF "$pattern" .gitignore || missing+=("$pattern")
done
if [ "${#missing[@]}" -gt 0 ]; then
  changed=$((changed + 1))
  say "  .gitignore: ${#missing[@]} pattern(s) to add"
  if [ "$MODE" = "apply" ]; then
    {
      echo ""
      echo "# Local run scaffolding and captured console output (PART 03 K3): one-shot"
      echo "# launchers and their transcripts are host state, not source. Anchored to the"
      echo "# root so scripts/run-*.sh — which ARE source — stay tracked."
      printf '%s\n' "${missing[@]}"
    } >>.gitignore
  else
    for pattern in "${missing[@]}"; do echo "      would append: $pattern"; done
  fi
else
  say "  .gitignore: every pattern already ignored (ok)"
fi

# --- summary ----------------------------------------------------------------
if [ "$changed" -eq 0 ]; then
  say "no tracked junk — the repository is already clean"
  exit 0
fi
say "$changed change(s) identified"
if [ "$MODE" = "dry-run" ]; then
  say "dry run only: nothing was modified. Re-run with --apply."
else
  say "applied. Review with: git status --short && git diff --cached --stat"
  say "then: ./scripts/verify-script-modes.sh && cargo test -p sniper-suite --lib"
fi
