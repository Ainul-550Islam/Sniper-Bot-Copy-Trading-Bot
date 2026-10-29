#!/usr/bin/env bash
# APPLY-BATCH07.sh — apply the 2026-10-29 Batch 07 integration-wiring batch.
#
# Precondition: run from the repository root of a checkout where the
# integration-cleanup batch has already been applied and staged (its
# APPLY.sh), after extracting this package over the tree. The overlay only
# adds/replaces files (no deletions), so this script stages exactly the
# Batch 07 paths, forces the executable index modes of the two new gate
# scripts, and runs the batch completeness gate (manifest sha256 per
# file). It never runs a blanket `git add -A` and it creates no commit.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

say() { echo "[batch07] $*"; }
fail() { echo "[batch07] FAIL — $*" >&2; exit 1; }

EXPECTED_BASE="da0d93789a488ed7cc859e33ac8fc9deab943978"
CLEANUP_TREE="186232cd6b6ccd669dd15f1a8d87ac7e17015edc"
# No expected final-tree constant lives here: this script is itself part of
# the Batch 07 tree, so a self-referential tree hash could never match.
# Content identity is proved by FILES-MANIFEST.txt (per-file sha256,
# including this script) via scripts/verify-batch07-complete.sh below;
# the staged tree hash is reported for the record.

git rev-parse --is-inside-work-tree >/dev/null 2>&1 || fail "not inside a git work tree"
[[ -f BASE_COMMIT.txt ]] || fail "BASE_COMMIT.txt is missing (apply the cleanup batch first)"
[[ -f FILES-MANIFEST.txt ]] || fail "FILES-MANIFEST.txt is missing"
[[ -f scripts/check-cleanup-ownership.sh ]] || fail "cleanup batch is not present in this tree"
[[ -f scripts/check-batch07-ownership.sh ]] || fail "batch07 ownership gate is missing"
[[ -f scripts/verify-batch07-complete.sh ]] || fail "batch07 completeness gate is missing"
[[ -f crates/server/src/telegram_trade_port.rs ]] || fail "telegram trade port is missing"

BASE="$(tr -d '[:space:]' < BASE_COMMIT.txt)"
[[ "$BASE" == "$EXPECTED_BASE" ]] || fail "unexpected base commit: $BASE"
git rev-parse --verify "${BASE}^{commit}" >/dev/null 2>&1 || fail "base commit unavailable: $BASE"

# The package may have been extracted without Unix modes.
chmod +x scripts/check-batch07-ownership.sh scripts/verify-batch07-complete.sh

# Precondition + idempotency: the staged tree must be either the cleanup
# tree (first apply) or a tree that already contains this batch staged
# (re-run). Staging the same paths again is a no-op, so both cases simply
# flow into the staging + verification below; anything else is refused.
CURRENT_TREE="$(git write-tree)"
if [[ "$CURRENT_TREE" == "$CLEANUP_TREE" ]]; then
    say "cleanup staged tree verified: $CLEANUP_TREE"
elif git ls-files --error-unmatch APPLY-BATCH07.sh >/dev/null 2>&1; then
    say "Batch 07 appears staged already (tree $CURRENT_TREE); re-staging is a no-op"
else
    fail "staged tree is $CURRENT_TREE, expected the cleanup tree $CLEANUP_TREE (apply the cleanup batch first; unstage anything else)"
fi

BATCH07_PATHS=(
    APPLY-BATCH07.sh
    FILES-MANIFEST.txt
    apps/control-plane/src/__tests__/telegram-consent.test.tsx
    apps/control-plane/src/app/trading/telegram/page.tsx
    apps/control-plane/src/lib/customer-trading-api.ts
    config.toml.example
    crates/core/src/config.rs
    crates/core/src/models.rs
    crates/core/src/tenant/rls_context.rs
    crates/module-copy/src/feeds.rs
    crates/module-copy/src/policy.rs
    crates/module-sniper/fixtures/replay/01_valid_launch.json
    crates/module-sniper/fixtures/replay/02_duplicate_event.json
    crates/module-sniper/fixtures/replay/03_stale_event.json
    crates/module-sniper/fixtures/replay/04_malformed_events.json
    crates/module-sniper/fixtures/replay/05_insufficient_liquidity.json
    crates/module-sniper/fixtures/replay/06_excessive_slippage.json
    crates/module-sniper/fixtures/replay/06b_price_impact_limit.json
    crates/module-sniper/fixtures/replay/07_risk_rejection.json
    crates/module-sniper/fixtures/replay/08_successful_intent.json
    crates/module-sniper/fixtures/replay/09_failed_execution.json
    crates/module-sniper/fixtures/replay/10_reconnect_gap.json
    crates/module-sniper/fixtures/replay/11_pumpswap_launch.json
    crates/module-sniper/fixtures/replay/12_raydium_launch.json
    crates/module-sniper/fixtures/replay/13_token_state_and_concentration.json
    crates/module-sniper/fixtures/replay/13b_strict_gates.json
    crates/module-sniper/fixtures/replay/14_invalid_route.json
    crates/module-sniper/fixtures/replay/15_strategy_disabled_and_exposure.json
    crates/module-sniper/fixtures/replay/16_fee_budget.json
    crates/module-sniper/fixtures/replay/16b_fee_budget_off.json
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
    crates/solana-kit/src/venues/raydium_launchlab/decode.rs
    crates/solana-kit/tests/raydium_launchlab_decode.rs
    docs/ARCHITECTURE-OVERVIEW.md
    docs/BACKTEST-DATASETS.md
    docs/CAPABILITY-MATRIX.md
    docs/DELIVERY-MANIFEST.md
    docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md
    docs/EVIDENCE-INDEX.md
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
    docs/gap-updates/batch-07-rows.md
    openapi/openapi.json
    openapi/openapi.yaml
    release-manifest.json
    scripts/check-batch07-ownership.sh
    scripts/verify-batch07-complete.sh
)

for path in "${BATCH07_PATHS[@]}"; do
    [[ -f "$path" ]] || fail "package file missing after extract: $path"
    git add -- "$path"
done
git update-index --chmod=+x scripts/check-batch07-ownership.sh
git update-index --chmod=+x scripts/verify-batch07-complete.sh

STAGED_TREE="$(git write-tree)"
say "staged Batch 07 tree: $STAGED_TREE (content identity is proved by the manifest gate next)"

bash scripts/verify-batch07-complete.sh

say "PASS — Batch 07 staged. Review, run the full acceptance commands from the batch report, then commit deliberately. Nothing was committed."
