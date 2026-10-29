#!/usr/bin/env bash
# Batch 07 ownership gate.
#
# Batch 07 starts from the integration-cleanup staged tree on top of
# BASE_COMMIT.txt. It wires the Batch 02/03/04 seams: the real server
# TradeCommandPort + Telegram trade console, Raydium CPMM/LaunchLab sniper
# routing, the launch-dataset recorder runtime, and SwapVenue cleanup.
#
# Pre-push hotfix (2026-10-29): crates/solana-kit/src/pump.rs — a zeroed
# v2 `quote_mint` parsed as the system-program pubkey and aborted every
# pump-curve entry (found by a live paper smoke run). Parser + accessor
# now treat it as SOL-quoted; regression tests cover the account shape.
#
# This gate proves the delivered delta stays inside the recorded target
# tree (batch07 TARGET-TREE): every path changed relative to the base
# commit must be either a cleanup-owned path (already adjudicated by
# scripts/check-cleanup-ownership.sh), a Batch 07 owned path, a generated
# artifact (OpenAPI export, release manifest), or a stat-marker-only
# document whose non-marker bytes are identical to the base. Anything
# else fails. It also proves the Batch 07 new files exist as complete
# files, not stubs created merely to satisfy the list.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

EXPECTED_BASE="da0d93789a488ed7cc859e33ac8fc9deab943978"
fail=0
error() { echo "check-batch07-ownership: FAIL — $*" >&2; fail=1; }

if [[ ! -f BASE_COMMIT.txt ]]; then
    error "BASE_COMMIT.txt is missing"
    exit 1
fi
BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
if [[ "$BASE" != "$EXPECTED_BASE" ]]; then
    error "BASE_COMMIT.txt must contain $EXPECTED_BASE, found $BASE"
    exit 1
fi
if ! git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1; then
    error "base commit is unavailable: $BASE"
    exit 1
fi

# The 65 integration-cleanup paths (staged tree
# 186232cd6b6ccd669dd15f1a8d87ac7e17015edc over the base). Batch 07 may
# carry them forward; their content categories belong to the cleanup gate.
CLEANUP_PATHS=(
    .cargo/bin/rustup
    .cargo/env
    .config/solana/install/config.yml
    .github/workflows/datasets.yml
    .github/workflows/telegram-trade.yml
    .github/workflows/venues.yml
    APPLY.sh
    AUDIT.md
    BASE_COMMIT.txt
    FILES-MANIFEST.txt
    README.md
    config.toml
    core-launch.out
    crates/server/src/saas/auth_flows.rs
    crates/server/src/saas/password_reset.rs
    crates/server/tests/batch6_transaction_readiness.rs
    crates/server/tests/identity_plane_sql_contract.rs
    crates/solana-kit/src/venues/raydium_cpmm.rs
    docs/API-COMPATIBILITY-MATRIX.md
    docs/ARCHITECTURE-OVERVIEW.md
    docs/BUSINESS-MATRIX-2026.md
    docs/CAPABILITY-MATRIX.md
    docs/DELIVERY-MANIFEST.md
    docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md
    docs/EVIDENCE-INDEX.md
    docs/FEATURE-TRACEABILITY.md
    docs/FORENSIC-SQL-RESEARCH-2026.md
    docs/IP-OWNERSHIP-REGISTER.md
    docs/OPERATIONS-RUNBOOK.md
    docs/PENETRATION-TEST-READINESS.md
    docs/PRODUCTION-READINESS-MATRIX.md
    docs/RELEASE-NOTES-0.1.0.md
    docs/SECURITY-CONTROLS-MATRIX.md
    docs/SECURITY-THREAT-MODEL.md
    docs/SELLER-FACT-SHEET.md
    docs/SELLING-LISTING-SOURCE.md
    docs/STATS.md
    docs/TECHNICAL-FACT-SHEET.md
    docs/TRANSACTION-READINESS-REPORT.md
    docs/gap-updates/cleanup-rows.md
    release-manifest.json
    run-core.sh
    run-p2b.sh
    run-part2.sh
    run-tests.out
    run-tests.sh
    run_baseline.sh
    run_baseline2.sh
    rustup-init.sh
    scripts/apply-batch01-deletes.sh
    scripts/bootstrap-config.sh
    scripts/check-batch02-ownership.sh
    scripts/check-batch03-ownership.sh
    scripts/check-batch04-ownership.sh
    scripts/check-cleanup-ownership.sh
    scripts/check-rls-coverage.sh
    scripts/cleanup-tracked-junk.sh
    scripts/forensic-sql-scan.sh
    scripts/run-dataset-capture.sh
    scripts/sync-stat-markers.sh
    scripts/telegram-trade-smoke.sh
    scripts/untrack-batch01-junk.sh
    scripts/verify-batch-complete.sh
    scripts/verify-raydium-idl.sh
    tests/release/evidence_truth.sh
)

# Batch 07 owned paths (target tree, including the forced fallouts).
BATCH07_PATHS=(
    APPLY-BATCH07.sh
    apps/control-plane/src/__tests__/telegram-consent.test.tsx
    apps/control-plane/src/app/trading/telegram/page.tsx
    apps/control-plane/src/lib/customer-trading-api.ts
    config.toml.example
    crates/core/src/config.rs
    crates/core/src/models.rs
    crates/core/src/tenant/rls_context.rs
    crates/module-copy/src/feeds.rs
    crates/module-copy/src/policy.rs
    crates/module-sniper/src/detect.rs
    crates/module-sniper/src/entry.rs
    crates/module-sniper/src/event.rs
    crates/module-sniper/src/exit.rs
    crates/module-sniper/src/market.rs
    crates/module-sniper/src/pipeline.rs
    crates/module-sniper/tests/geyser_detect.rs
    crates/module-sniper/tests/property.rs
    crates/module-telegram/src/lib.rs
    crates/server/src/lib.rs
    crates/server/src/main.rs
    crates/server/src/openapi_control_plane_surface.rs
    crates/server/src/telegram_trade_port.rs
    crates/server/src/trading_data_plane/mod.rs
    crates/server/src/trading_data_plane/telegram.rs
    crates/solana-kit/src/decode.rs
    crates/solana-kit/src/pump.rs
    crates/solana-kit/src/venues/raydium_launchlab/decode.rs
    crates/solana-kit/tests/raydium_launchlab_decode.rs
    docs/BACKTEST-DATASETS.md
    docs/gap-updates/batch-07-rows.md
    openapi/openapi.json
    openapi/openapi.yaml
    scripts/check-batch07-ownership.sh
    scripts/verify-batch07-complete.sh
)

is_listed() {
    local needle="$1"
    shift
    local item
    for item in "$@"; do
        [[ "$item" == "$needle" ]] && return 0
    done
    return 1
}

# Replay fixtures embed the full SniperConfig JSON; the new config fields
# forced additive changes there and nowhere else in the tree.
is_replay_fixture() {
    case "$1" in
        crates/module-sniper/fixtures/replay/*.json) return 0 ;;
        *) return 1 ;;
    esac
}

# A document outside both lists may only differ from the base inside
# generated <!-- stat:... --> marker values (scripts/generate-stats.sh +
# scripts/sync-stat-markers.sh). docs/STATS.md itself is wholly generated
# and is a cleanup path already.
is_marker_only_vs_base() {
    local path="$1"
    case "$path" in
        README.md|docs/*.md|docs/**/*.md) ;;
        *) return 1 ;;
    esac
    [[ -f "$path" ]] || return 1
    git show "$BASE:$path" >/dev/null 2>&1 || return 1
    python3 - "$BASE" "$path" <<'PY'
import re
import subprocess
import sys

base, path = sys.argv[1], sys.argv[2]
before = subprocess.run(
    ["git", "show", f"{base}:{path}"], capture_output=True, text=True, check=True
).stdout
with open(path, encoding="utf-8") as handle:
    after = handle.read()
strip = lambda text: re.sub(
    r"(<!-- stat:[A-Za-z0-9_]+ -->).*?(<!-- /stat -->)",
    r"\1\2",
    text,
    flags=re.S,
)
sys.exit(0 if strip(before) == strip(after) else 1)
PY
}

# Changed paths: index + worktree + untracked, all relative to the base.
mapfile -t CHANGED < <(
    { git diff --cached --name-only "$BASE" --;
       git diff --name-only "$BASE" --;
       git ls-files --others --exclude-standard; } | sort -u
)

for path in "${CHANGED[@]}"; do
    if is_listed "$path" "${CLEANUP_PATHS[@]}"; then
        continue
    fi
    if is_listed "$path" "${BATCH07_PATHS[@]}"; then
        continue
    fi
    if is_replay_fixture "$path"; then
        continue
    fi
    if [[ "$path" == "FILES-MANIFEST.txt" ]]; then
        # The manifest describes the combined delivered tree and is
        # verified by scripts/verify-batch07-complete.sh.
        continue
    fi
    if is_marker_only_vs_base "$path"; then
        continue
    fi
    error "path changed outside the Batch 07 target tree: $path"
done

# New Batch 07 files must be real, complete files.
require_file() {
    local path="$1" min_lines="$2"
    if [[ ! -s "$path" ]]; then
        error "required Batch 07 file missing or empty: $path"
        return
    fi
    local lines
    lines="$(grep -c '' "$path")"
    if [[ "$lines" -lt "$min_lines" ]]; then
        error "Batch 07 file suspiciously short ($lines < $min_lines lines): $path"
    fi
}

require_file crates/server/src/telegram_trade_port.rs 800
require_file apps/control-plane/src/__tests__/telegram-consent.test.tsx 100
require_file docs/gap-updates/batch-07-rows.md 40
require_file scripts/check-batch07-ownership.sh 100
require_file scripts/verify-batch07-complete.sh 100
require_file APPLY-BATCH07.sh 100

# The wiring points must actually be present (a path list alone proves
# nothing about behaviour).
grep -q "spawn_with_console" crates/server/src/main.rs \
    || error "main.rs does not attach the telegram console"
grep -q "DeploymentTradePort" crates/server/src/main.rs \
    || error "main.rs does not construct the deployment trade port"
grep -q "with_recorder" crates/server/src/main.rs \
    || error "main.rs does not attach the dataset recorder"
grep -q "RaydiumCpmmDirect" crates/module-sniper/src/pipeline.rs \
    || error "sniper pipeline has no CPMM direct route"
grep -q "RaydiumLaunchLabDirect" crates/module-sniper/src/pipeline.rs \
    || error "sniper pipeline has no LaunchLab direct route"
grep -q "RaydiumLaunchLab" crates/solana-kit/src/decode.rs \
    || error "solana-kit decode has no LaunchLab swap venue"

if [[ "$fail" -ne 0 ]]; then
    exit 1
fi
echo "check-batch07-ownership: PASS (${#CHANGED[@]} changed paths inside the target tree)"
