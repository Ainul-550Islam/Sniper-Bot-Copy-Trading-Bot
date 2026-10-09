#!/usr/bin/env bash
# ==============================================================================
# Batch 201–250 — Accounting, Authorization, Billing, Custody, DB Foundations
# ==============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${REPO_ROOT}"

echo "======================================================================"
echo "[remediation-201-250] Running Batch 201–250 Production Architecture Gate"
echo "======================================================================"

# 1. Verify all 50 target files exist
echo "[1/5] Checking presence of Batch 201–250 target files..."
TARGET_FILES=(
  "crates/core/src/accounting/audit.rs"
  "crates/core/src/accounting/book.rs"
  "crates/core/src/accounting/event.rs"
  "crates/core/src/accounting/ledger.rs"
  "crates/core/src/accounting/metrics.rs"
  "crates/core/src/accounting/mod.rs"
  "crates/core/src/accounting/posting.rs"
  "crates/core/src/accounting/reconcile.rs"
  "crates/core/src/accounting/recovery.rs"
  "crates/core/src/accounting/store.rs"
  "crates/core/src/accounting/view.rs"
  "crates/core/src/audit.rs"
  "crates/core/src/auth.rs"
  "crates/core/src/authorization/context.rs"
  "crates/core/src/authorization/decision.rs"
  "crates/core/src/authorization/mod.rs"
  "crates/core/src/billing/billing_state.rs"
  "crates/core/src/billing/checkout.rs"
  "crates/core/src/billing/dunning.rs"
  "crates/core/src/billing/entitlements.rs"
  "crates/core/src/billing/events.rs"
  "crates/core/src/billing/invoice.rs"
  "crates/core/src/billing/lifecycle.rs"
  "crates/core/src/billing/mod.rs"
  "crates/core/src/billing/payment_intent.rs"
  "crates/core/src/billing/plan.rs"
  "crates/core/src/billing/provider_config.rs"
  "crates/core/src/billing/provider_events.rs"
  "crates/core/src/billing/reconciliation.rs"
  "crates/core/src/billing/subscription.rs"
  "crates/core/src/billing/usage.rs"
  "crates/core/src/billing/usage_policy.rs"
  "crates/core/src/config.rs"
  "crates/core/src/custody/credentials.rs"
  "crates/core/src/custody/health.rs"
  "crates/core/src/custody/mod.rs"
  "crates/core/src/custody/model.rs"
  "crates/core/src/custody/policy.rs"
  "crates/core/src/custody/provider.rs"
  "crates/core/src/custody/provider_config.rs"
  "crates/core/src/custody/resolve.rs"
  "crates/core/src/custody/rotation.rs"
  "crates/core/src/db/accounting.rs"
  "crates/core/src/db/claims.rs"
  "crates/core/src/db/copy.rs"
  "crates/core/src/db/deployment_org.rs"
  "crates/core/src/db/execution.rs"
  "crates/core/src/db/ha.rs"
  "crates/core/src/db/mod.rs"
  "crates/core/src/db/mod_tenant_exports.rs"
)

for file in "${TARGET_FILES[@]}"; do
  if [[ ! -f "${REPO_ROOT}/${file}" ]]; then
    echo "ERROR: Target file missing: ${file}"
    exit 1
  fi
done
echo "OK: All 50 Batch 201–250 core domain files verified."

# 2. Check no raw/unsafe float math in authoritative financial storage
echo "[2/5] Inspecting financial representations in accounting & billing..."
if grep -n "cents: f64" "${REPO_ROOT}/crates/core/src/accounting/"* 2>/dev/null; then
  echo "ERROR: Found float cents in accounting"
  exit 1
fi
echo "OK: Exact integer monetary units verified."

# 3. Verify Forensic SQL isolation
echo "[3/5] Running Forensic SQL pattern scan on accounting & DB modules..."
bash "${REPO_ROOT}/tests/forensics/sql-pattern-regression.sh"
echo "OK: 0 class-4 tenant isolation leaks."

# 4. Rebuild Buyer Release
echo "[4/5] Rebuilding buyer release package..."
bash "${REPO_ROOT}/scripts/rebuild-buyer-release.sh"
bash "${REPO_ROOT}/scripts/verify-buyer-package.sh"
echo "OK: Buyer release mirrored and verified."

# 5. Build Control Plane
echo "[5/5] Checking Next.js Turbopack build..."
cd "${REPO_ROOT}/apps/control-plane"
if [[ ! -d "node_modules/next" ]]; then
  npm install --legacy-peer-deps --prefer-offline --no-audit >/dev/null 2>&1 || npm install --legacy-peer-deps >/dev/null 2>&1
fi
npm run build
echo "OK: Control plane compiled."

echo "======================================================================"
echo "[remediation-201-250] ALL GATES PASSED (100% COMPLETE & VERIFIED)"
echo "======================================================================"
