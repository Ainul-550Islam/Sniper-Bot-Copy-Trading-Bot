#!/usr/bin/env bash
# APPLY.sh — apply the 2026-10-28 integration-cleanup batch.
#
# Run this from the repository root after copying the batch package over a
# checkout that contains the base commit in BASE_COMMIT.txt. A zip overlay
# can add and replace files, but it cannot by itself express tracked-file
# deletions or git index-mode changes. This script performs those git
# operations explicitly and idempotently:
#
#   1. untrack/delete the host state listed by the repository's own
#      scripts/cleanup-tracked-junk.sh;
#   2. delete the placeholder Raydium CPMM stub superseded by Batch 03;
#   3. restore executable modes for tracked shell scripts;
#   4. stage exactly the files listed in FILES-MANIFEST.txt plus batch
#      bookkeeping — never a blanket `git add -A`;
#   5. run the batch completeness and ownership gates.
#
# The local launcher scripts and config.toml are untracked, not deleted:
# cleanup-tracked-junk.sh preserves their working copies. Downloaded host
# state, captured output, and the duplicate root AUDIT.md are removed.
#
# This script does not create a commit, regenerate statistics, or rerun the
# release-manifest generator. Generated files in the package were produced
# from a clean tree by their repository generators; regenerating them after
# untracking local launchers could contaminate a filesystem-derived tree
# digest with ignored local files. Review the staged result, run the full
# acceptance commands printed at the end, and commit deliberately.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

say() {
    echo "[integration-cleanup] $*"
}

fail() {
    echo "[integration-cleanup] FAIL — $*" >&2
    exit 1
}

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    fail "not inside a git work tree"
fi

[[ -f BASE_COMMIT.txt ]] || fail "BASE_COMMIT.txt is missing"
[[ -f FILES-MANIFEST.txt ]] || fail "FILES-MANIFEST.txt is missing"
[[ -f scripts/cleanup-tracked-junk.sh ]] || fail "cleanup script is missing"
[[ -f scripts/check-cleanup-ownership.sh ]] || fail "ownership gate is missing"
[[ -f scripts/verify-batch-complete.sh ]] || fail "completeness gate is missing"

BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
EXPECTED_BASE="da0d93789a488ed7cc859e33ac8fc9deab943978"
[[ "$BASE" == "$EXPECTED_BASE" ]] || fail "unexpected base commit: $BASE"
git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1 \
    || fail "base commit is not available in this checkout: $BASE"
if ! git merge-base --is-ancestor "$BASE" HEAD; then
    fail "HEAD does not contain the batch base; apply from the merged Batch 03 + Batch 04 tree or a descendant"
fi

say "base commit verified: $BASE"

# The package may have been extracted by a tool that does not preserve Unix
# modes. Make the batch's own entry points runnable before invoking them;
# their index modes are also forced explicitly after staging below.
chmod +x \
    APPLY.sh \
    scripts/check-cleanup-ownership.sh \
    scripts/verify-batch-complete.sh \
    scripts/forensic-sql-scan.sh \
    scripts/sync-stat-markers.sh

# ---------------------------------------------------------------------------
# 1. Tracked host state
# ---------------------------------------------------------------------------
# Use the repository's reviewed cleanup mechanism rather than duplicating
# its path list here. It is idempotent: a second APPLY run reports that the
# tree is already clean and changes nothing.
say "removing tracked host state with scripts/cleanup-tracked-junk.sh --apply"
bash scripts/cleanup-tracked-junk.sh --apply

# ---------------------------------------------------------------------------
# 2. Batch 03 placeholder deletion
# ---------------------------------------------------------------------------
# The real CPMM adapter lives in venues/raydium_cpmm/; the single-file stub
# at venues/raydium_cpmm.rs was supposed to be deleted by Batch 03's APPLY
# step. --ignore-unmatch keeps this idempotent after the index removal.
say "deleting the placeholder Raydium CPMM stub"
git rm -f --ignore-unmatch --quiet -- \
    crates/solana-kit/src/venues/raydium_cpmm.rs
rm -f crates/solana-kit/src/venues/raydium_cpmm.rs

# ---------------------------------------------------------------------------
# 3. Executable modes for pre-existing tracked scripts
# ---------------------------------------------------------------------------
# These files' contents are byte-identical to the base. Only their index
# mode changes, and scripts/check-cleanup-ownership.sh proves that claim by
# comparing each working file with its base blob.
MODE_ONLY_SCRIPTS=(
    scripts/apply-batch01-deletes.sh
    scripts/bootstrap-config.sh
    scripts/check-batch02-ownership.sh
    scripts/check-batch03-ownership.sh
    scripts/check-batch04-ownership.sh
    scripts/check-rls-coverage.sh
    scripts/cleanup-tracked-junk.sh
    scripts/run-dataset-capture.sh
    scripts/telegram-trade-smoke.sh
    scripts/untrack-batch01-junk.sh
    scripts/verify-raydium-idl.sh
    tests/release/evidence_truth.sh
)

say "restoring executable modes on tracked shell scripts"
for path in "${MODE_ONLY_SCRIPTS[@]}"; do
    [[ -f "$path" ]] || fail "mode-only script is missing: $path"
    chmod +x -- "$path"
    git update-index --chmod=+x -- "$path"
done

# ---------------------------------------------------------------------------
# 4. Stage exactly the manifest tree
# ---------------------------------------------------------------------------
# FILES-MANIFEST.txt is tab-separated: path, newline count, sha256. Reading
# the path column here makes the staged set and the verified set the same
# set by construction. Deletions were already staged by git rm above, and
# ignored preserved local files are intentionally not named in the manifest.
say "staging the files listed in FILES-MANIFEST.txt"
while IFS=$'\t' read -r path _lines _sha256; do
    [[ -n "$path" ]] || continue
    case "$path" in
        /*|../*|*/../*)
            fail "unsafe path in FILES-MANIFEST.txt: $path"
            ;;
    esac
    [[ -f "$path" ]] || fail "manifest file is missing: $path"
    git add -- "$path"
done < FILES-MANIFEST.txt

git add -- BASE_COMMIT.txt FILES-MANIFEST.txt

# Do not rely on core.fileMode or archive extraction preserving +x. Force
# the index mode for every shell script this batch stages.
while IFS=$'\t' read -r path _lines _sha256; do
    case "$path" in
        *.sh)
            chmod +x -- "$path"
            git update-index --chmod=+x -- "$path"
            ;;
    esac
done < FILES-MANIFEST.txt
chmod +x APPLY.sh
git update-index --chmod=+x -- APPLY.sh

# ---------------------------------------------------------------------------
# 5. Verify before handing the tree back
# ---------------------------------------------------------------------------
say "running script-mode gate"
bash scripts/verify-script-modes.sh

say "running integration-cleanup completeness and ownership gates"
bash scripts/verify-batch-complete.sh

say "applied and staged; no commit was created"
say "review with: git status --short && git diff --cached --stat"
say "full acceptance commands:"
cat <<'EOF'
  cargo fmt --all --check
  CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
    cargo check --workspace --all-targets --locked
  CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
    cargo clippy --workspace --all-targets --locked -- -D warnings
  # Start PostgreSQL/Redis scratch services, export POSTGRES_URL,
  # TEST_POSTGRES_URL, DATABASE_URL and REDIS_URL for them, then run:
  CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
    cargo test --workspace --locked
  bash scripts/forensic-sql-scan.sh
  bash tests/forensics/sql-pattern-regression.sh
  bash tests/release/stats_current.sh
  bash tests/release/manifest_current.sh
  bash tests/business/business-matrix-completeness.sh
  bash scripts/verify-delivery.sh
EOF
