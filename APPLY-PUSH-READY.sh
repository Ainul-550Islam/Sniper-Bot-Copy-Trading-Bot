#!/usr/bin/env bash
# APPLY-PUSH-READY.sh — apply the combined push-ready overlay
# (integration cleanup + Batch 07 + pump quote_mint hotfix, 2026-10-29)
# to a clean clone at the base commit, then stage everything and PROVE
# the resulting tree is byte-identical to the delivered tree.
#
# Usage:
#   unzip push-ready-2026-10-29.zip to a folder OUTSIDE your clone, then:
#     bash /path/to/unzipped/APPLY-PUSH-READY.sh /path/to/your/clone
#   (omit the argument to use the current directory as the clone)
#
# This script never commits and never pushes. It stops with instructions
# if anything is not exactly as expected.
set -euo pipefail

BASE_COMMIT="da0d93789a488ed7cc859e33ac8fc9deab943978"
EXPECTED_TREE="bf85b75df6341e73f6b7a7eb455580393bac1816"
CLEANUP_TREE="186232cd6b6ccd669dd15f1a8d87ac7e17015edc"
BATCH07_TREE="cd903315395326a159c65130c77832af4d74d1c1"

OVERLAY="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLONE="${1:-$(pwd)}"
CLONE="$(cd "$CLONE" && pwd)"

say()  { echo "[apply-push-ready] $*"; }
die()  { echo "[apply-push-ready] STOP — $*" >&2; exit 1; }

if [[ "$OVERLAY" == "$CLONE" ]]; then
    die "you unzipped the package INSIDE the clone. Unzip it to a different folder and re-run."
fi
git -C "$CLONE" rev-parse --git-dir >/dev/null 2>&1 \
    || die "$CLONE is not a git repository."
cd "$CLONE"

HEAD_COMMIT="$(git rev-parse HEAD)"
HEAD_TREE="$(git rev-parse 'HEAD^{tree}')"
INDEX_TREE="$(git write-tree)"

if [[ "$HEAD_TREE" == "$EXPECTED_TREE" ]]; then
    say "already committed: HEAD tree matches the delivered tree ($EXPECTED_TREE). Nothing to do."
    exit 0
fi
if [[ "$INDEX_TREE" == "$EXPECTED_TREE" ]]; then
    say "already applied: staged tree matches the delivered tree ($EXPECTED_TREE). You can commit now."
    exit 0
fi
if [[ "$HEAD_COMMIT" != "$BASE_COMMIT" ]]; then
    die "HEAD is $HEAD_COMMIT, expected base $BASE_COMMIT. If GitHub has newer commits, STOP and ask for a rebase — do not force anything."
fi
if [[ -n "$(git status --porcelain)" ]]; then
    if [[ "$INDEX_TREE" == "$CLEANUP_TREE" || "$INDEX_TREE" == "$BATCH07_TREE" ]]; then
        die "an earlier package (cleanup/Batch 07) is applied here but not committed. This combined package replaces it: run 'git reset --hard HEAD' (back up any of your own work first), then re-run this script."
    fi
    die "working tree is dirty. Commit or stash your own changes first, then re-run."
fi

FILES=(
    .github/workflows/datasets.yml
    .github/workflows/telegram-trade.yml
    .github/workflows/venues.yml
    APPLY-BATCH07.sh
    APPLY.sh
    BASE_COMMIT.txt
    FILES-MANIFEST.txt
    README.md
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
    crates/server/src/saas/auth_flows.rs
    crates/server/src/saas/password_reset.rs
    crates/server/src/telegram_trade_port.rs
    crates/server/src/trading_data_plane/mod.rs
    crates/server/src/trading_data_plane/telegram.rs
    crates/server/tests/batch6_transaction_readiness.rs
    crates/server/tests/identity_plane_sql_contract.rs
    crates/solana-kit/src/decode.rs
    crates/solana-kit/src/pump.rs
    crates/solana-kit/src/venues/raydium_launchlab/decode.rs
    crates/solana-kit/tests/raydium_launchlab_decode.rs
    docs/API-COMPATIBILITY-MATRIX.md
    docs/ARCHITECTURE-OVERVIEW.md
    docs/BACKTEST-DATASETS.md
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
    docs/gap-updates/batch-07-rows.md
    docs/gap-updates/cleanup-rows.md
    openapi/openapi.json
    openapi/openapi.yaml
    release-manifest.json
    scripts/apply-batch01-deletes.sh
    scripts/bootstrap-config.sh
    scripts/check-batch02-ownership.sh
    scripts/check-batch03-ownership.sh
    scripts/check-batch04-ownership.sh
    scripts/check-batch07-ownership.sh
    scripts/check-cleanup-ownership.sh
    scripts/check-rls-coverage.sh
    scripts/cleanup-tracked-junk.sh
    scripts/forensic-sql-scan.sh
    scripts/run-dataset-capture.sh
    scripts/sync-stat-markers.sh
    scripts/telegram-trade-smoke.sh
    scripts/untrack-batch01-junk.sh
    scripts/verify-batch-complete.sh
    scripts/verify-batch07-complete.sh
    scripts/verify-raydium-idl.sh
    tests/release/evidence_truth.sh
)
DELETES=(
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

missing=0
for f in "${FILES[@]}"; do
    case "$f" in /*|*..*) die "unsafe path in package list: $f" ;; esac
    if [[ ! -f "$OVERLAY/$f" ]]; then
        echo "[apply-push-ready] missing from package: $f" >&2
        missing=$((missing + 1))
    fi
done
[[ "$missing" -eq 0 ]] || die "$missing package file(s) missing — re-download/re-unzip the package."

copied=0
for f in "${FILES[@]}"; do
    mkdir -p "$(dirname "$CLONE/$f")"
    cp -a "$OVERLAY/$f" "$CLONE/$f"
    copied=$((copied + 1))
done
removed=0
for f in "${DELETES[@]}"; do
    case "$f" in /*|*..*) die "unsafe delete path in package list: $f" ;; esac
    if [[ -e "$CLONE/$f" ]]; then
        rm -f "$CLONE/$f"
        removed=$((removed + 1))
    fi
done
say "copied $copied files, removed $removed deleted files."

git add -A
ACTUAL_TREE="$(git write-tree)"
if [[ "$ACTUAL_TREE" != "$EXPECTED_TREE" ]]; then
    echo "[apply-push-ready] STOP — tree mismatch!" >&2
    echo "  expected: $EXPECTED_TREE" >&2
    echo "  actual:   $ACTUAL_TREE" >&2
    echo "Changed paths vs base:" >&2
    git diff --cached --stat HEAD >&2 | tail -n 5
    echo "Run 'git reset --hard HEAD' to undo, and share this output." >&2
    exit 1
fi

say "TREE MATCH: $ACTUAL_TREE"
say "The staged tree is byte-identical to the delivered, tested tree."
echo
echo "Next steps (review, then run):"
echo "  git status"
echo "  git commit -F - <<'MSG'"
echo "feat: integration cleanup + Batch 07 wiring + pump quote_mint hotfix"
echo
echo "- Integration cleanup of batch seams on top of da0d9378"
echo "- Batch 07: server TradeCommandPort + Telegram trade console, sniper"
echo "  Raydium CPMM/LaunchLab routing, launch-dataset recorder runtime"
echo "- Hotfix: pump.fun bonding-curve parser treated a zeroed v2 quote_mint"
echo "  as the system-program pubkey, aborting every pump-curve entry; parse"
echo "  it as SOL-quoted (found by live paper smoke; regression tests added)"
echo "MSG"
echo "  git push origin main"
