#!/usr/bin/env bash
# Integration-cleanup ownership gate.
#
# This batch starts at the merged Batch 03 + Batch 04 tree recorded in
# BASE_COMMIT.txt. It is allowed to remove tracked host state, restore three
# missing workflows, correct executable modes, regenerate measured release
# documents, and make the narrow identity-plane SQL adjudication described in
# docs/FORENSIC-SQL-RESEARCH-2026.md. It is not Batch 07: no executor,
# route, frontend, migration, trading algorithm, or live-evidence change is
# permitted.
#
# The gate checks more than path membership:
#   * deleted/untracked host-state paths must be absent from the git index;
#   * mode-only scripts must be byte-identical to the base and mode 100755;
#   * README/docs changed by stat synchronization may differ only inside
#     generated <!-- stat:... --> marker values;
#   * the substantive new/changed files must exist as complete files, not
#     stubs created merely to satisfy a path list.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

EXPECTED_BASE="da0d93789a488ed7cc859e33ac8fc9deab943978"
if [[ ! -f BASE_COMMIT.txt ]]; then
    echo "check-cleanup-ownership: BASE_COMMIT.txt is missing" >&2
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if [[ "$BASE" != "$EXPECTED_BASE" ]]; then
    echo "check-cleanup-ownership: BASE_COMMIT.txt must contain $EXPECTED_BASE, found $BASE" >&2
    exit 1
fi
if ! git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1; then
    echo "check-cleanup-ownership: base commit is unavailable: $BASE" >&2
    exit 1
fi

# Paths whose only permitted change is removing them from version control.
# The six run-*.sh launchers and config.toml may remain as ignored local
# working copies after APPLY.sh; index absence is the repository contract.
DELETED_PATHS=(
    .cargo/bin/rustup
    .cargo/env
    .config/solana/install/config.yml
    AUDIT.md
    config.toml
    core-launch.out
    crates/solana-kit/src/venues/raydium_cpmm.rs
    run-core.sh
    run-p2b.sh
    run-part2.sh
    run-tests.out
    run-tests.sh
    run_baseline.sh
    run_baseline2.sh
    rustup-init.sh
)

# Existing scripts whose content must remain byte-identical to the base;
# only their git index mode changes from 100644 to 100755.
MODE_ONLY_PATHS=(
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

# Documents whose only permitted cleanup change is a regenerated value
# inside an existing stat marker. docs/STATS.md itself is wholly generated;
# BUSINESS-MATRIX and FORENSIC-SQL-RESEARCH have separately reviewed prose.
MARKER_ONLY_PATHS=(
    README.md
    docs/API-COMPATIBILITY-MATRIX.md
    docs/ARCHITECTURE-OVERVIEW.md
    docs/CAPABILITY-MATRIX.md
    docs/DELIVERY-MANIFEST.md
    docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md
    docs/EVIDENCE-INDEX.md
    docs/FEATURE-TRACEABILITY.md
    docs/IP-OWNERSHIP-REGISTER.md
    docs/OPERATIONS-RUNBOOK.md
    docs/PENETRATION-TEST-READINESS.md
    docs/PRODUCTION-READINESS-MATRIX.md
    docs/RELEASE-NOTES-0.1.0.md
    docs/SECURITY-CONTROLS-MATRIX.md
    docs/SECURITY-THREAT-MODEL.md
    docs/SELLER-FACT-SHEET.md
    docs/SELLING-LISTING-SOURCE.md
    docs/TECHNICAL-FACT-SHEET.md
    docs/TRANSACTION-READINESS-REPORT.md
)

contains_path() {
    local wanted="$1"
    shift
    local candidate
    for candidate in "$@"; do
        [[ "$candidate" == "$wanted" ]] && return 0
    done
    return 1
}

is_allowed() {
    local path="$1"

    contains_path "$path" "${DELETED_PATHS[@]}" && return 0
    contains_path "$path" "${MODE_ONLY_PATHS[@]}" && return 0
    contains_path "$path" "${MARKER_ONLY_PATHS[@]}" && return 0

    case "$path" in
        # Batch bookkeeping and application machinery.
        APPLY.sh|BASE_COMMIT.txt|FILES-MANIFEST.txt|SHA256SUMS) return 0 ;;
        scripts/check-cleanup-ownership.sh) return 0 ;;
        scripts/verify-batch-complete.sh) return 0 ;;

        # The three restored feature workflows.
        .github/workflows/datasets.yml) return 0 ;;
        .github/workflows/telegram-trade.yml) return 0 ;;
        .github/workflows/venues.yml) return 0 ;;

        # Identity-plane adjudication: two narrow source edits, one scanner
        # change, one stale count pin, and the new source-level contract.
        crates/server/src/saas/auth_flows.rs) return 0 ;;
        crates/server/src/saas/password_reset.rs) return 0 ;;
        crates/server/tests/batch6_transaction_readiness.rs) return 0 ;;
        crates/server/tests/identity_plane_sql_contract.rs) return 0 ;;
        scripts/forensic-sql-scan.sh) return 0 ;;

        # Generated statistics and release measurements. The sync script is
        # included because its writer omitted README.md while the checker
        # required README markers; generated outputs themselves are never
        # hand-edited by this batch.
        scripts/sync-stat-markers.sh) return 0 ;;
        docs/STATS.md) return 0 ;;
        release-manifest.json) return 0 ;;
        docs/archive/manifest-history-2026-10.json) return 0 ;;

        # Reviewed prose for the business and forensic decisions, plus the
        # integration rows that keep Batch 07 explicitly open.
        docs/BUSINESS-MATRIX-2026.md) return 0 ;;
        docs/FORENSIC-SQL-RESEARCH-2026.md) return 0 ;;
        docs/gap-updates/cleanup-rows.md) return 0 ;;
        *) return 1 ;;
    esac
}

mapfile -t changed < <(
    {
        git diff --name-only "$BASE" --
        git ls-files --others --exclude-standard
    } | sed '/^[[:space:]]*$/d' | sort -u
)

bad=0
for path in "${changed[@]}"; do
    if ! is_allowed "$path"; then
        echo "check-cleanup-ownership: FORBIDDEN changed path: $path" >&2
        bad=1
    fi
done

# Deletions must be real index removals. Checking only the worktree would
# incorrectly fail the intentionally preserved local launchers/config and
# would miss a path that was deleted on disk but remained staged/tracked.
for path in "${DELETED_PATHS[@]}"; do
    if git ls-files --error-unmatch -- "$path" >/dev/null 2>&1; then
        echo "check-cleanup-ownership: deleted path remains tracked: $path" >&2
        bad=1
    fi
done
if [[ -e crates/solana-kit/src/venues/raydium_cpmm.rs ]]; then
    echo "check-cleanup-ownership: placeholder Raydium stub still exists" >&2
    bad=1
fi

# Mode-only means byte-identical content and an executable index entry.
for path in "${MODE_ONLY_PATHS[@]}"; do
    if [[ ! -f "$path" ]]; then
        echo "check-cleanup-ownership: mode-only script is missing: $path" >&2
        bad=1
        continue
    fi
    if ! git show "${BASE}:${path}" | cmp -s - "$path"; then
        echo "check-cleanup-ownership: content changed in mode-only script: $path" >&2
        bad=1
    fi
    mode="$(git ls-files -s -- "$path" | awk 'NR == 1 { print $1 }')"
    if [[ "$mode" != "100755" ]]; then
        echo "check-cleanup-ownership: $path has index mode ${mode:-missing}, expected 100755" >&2
        bad=1
    fi
done

# Marker-only documents: erase every generated marker value from both the
# base blob and the current file, then require byte equality. This permits
# exactly the generator's number updates and nothing else, including no
# accidental prose rewrapping by an editor.
for path in "${MARKER_ONLY_PATHS[@]}"; do
    if ! git cat-file -e "${BASE}:${path}" 2>/dev/null; then
        echo "check-cleanup-ownership: marker document absent at base: $path" >&2
        bad=1
        continue
    fi
    if [[ ! -f "$path" ]]; then
        echo "check-cleanup-ownership: marker document is missing: $path" >&2
        bad=1
        continue
    fi
    if ! BASE="$BASE" MARKER_PATH="$path" python3 - <<'PY'
import os
import re
import subprocess
import sys
from pathlib import Path

base = os.environ["BASE"]
relative = os.environ["MARKER_PATH"]
base_text = subprocess.check_output(
    ["git", "show", f"{base}:{relative}"], text=True
)
current_text = Path(relative).read_text(encoding="utf-8")
marker = re.compile(
    r"(<!-- stat:[A-Za-z0-9_]+ -->).*?(<!-- /stat -->)", re.DOTALL
)
normalized_base = marker.sub(r"\1<generated>\2", base_text)
normalized_current = marker.sub(r"\1<generated>\2", current_text)
sys.exit(0 if normalized_base == normalized_current else 1)
PY
    then
        echo "check-cleanup-ownership: non-marker content changed in $path" >&2
        bad=1
    fi
done

# Substantive owned files must be present and complete. Generated files and
# one-line bookkeeping are handled by their own generators/gates, while the
# marker-only documents above already existed as complete documents.
REQUIRED_TREE=(
    .github/workflows/datasets.yml
    .github/workflows/telegram-trade.yml
    .github/workflows/venues.yml
    APPLY.sh
    crates/server/src/saas/auth_flows.rs
    crates/server/src/saas/password_reset.rs
    crates/server/tests/batch6_transaction_readiness.rs
    crates/server/tests/identity_plane_sql_contract.rs
    docs/BUSINESS-MATRIX-2026.md
    docs/FORENSIC-SQL-RESEARCH-2026.md
    docs/STATS.md
    docs/gap-updates/cleanup-rows.md
    release-manifest.json
    scripts/check-cleanup-ownership.sh
    scripts/forensic-sql-scan.sh
    scripts/sync-stat-markers.sh
    scripts/verify-batch-complete.sh
)

for path in "${REQUIRED_TREE[@]}"; do
    if [[ ! -f "$path" ]]; then
        echo "check-cleanup-ownership: MISSING owned file: $path" >&2
        bad=1
        continue
    fi
    lines="$(wc -l < "$path" | tr -d ' ')"
    minimum=40
    case "$path" in
        docs/STATS.md)
            # The generator's complete 16-key table is intentionally short.
            minimum=20
            ;;
        APPLY.sh|scripts/check-cleanup-ownership.sh|scripts/verify-batch-complete.sh)
            minimum=100
            ;;
        .github/workflows/*.yml)
            minimum=100
            ;;
        crates/server/tests/identity_plane_sql_contract.rs)
            minimum=200
            ;;
    esac
    if (( lines < minimum )); then
        echo "check-cleanup-ownership: $path has only $lines lines (need >= $minimum)" >&2
        bad=1
    fi
done

# The restored workflow names are themselves referenced by prior batch
# gates and operator documentation. Guard the exact filenames and the job
# names so an accidentally renamed workflow cannot look complete.
for pair in \
    ".github/workflows/datasets.yml:datasets:" \
    ".github/workflows/venues.yml:venues:" \
    ".github/workflows/telegram-trade.yml:telegram-trade:"
do
    workflow="${pair%%:*}"
    job="${pair#*:}"
    if [[ -f "$workflow" ]] && ! grep -qx "  ${job}" "$workflow"; then
        echo "check-cleanup-ownership: $workflow does not define expected job $job" >&2
        bad=1
    fi
done

if (( bad )); then
    echo "check-cleanup-ownership: FAIL" >&2
    exit 1
fi

echo "check-cleanup-ownership: PASS — ${#changed[@]} changed path(s), all inside integration-cleanup ownership"
echo "check-cleanup-ownership: PASS — deletions, mode-only scripts, marker-only docs, and required tree verified"
