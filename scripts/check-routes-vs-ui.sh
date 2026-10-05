#!/usr/bin/env bash
# Contract gate for the customer-facing routes that are easy to orphan when
# a page or handler is renamed. It checks both sides of each declared pair;
# OpenAPI remains the authoritative full route contract.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER="$ROOT/crates/server/src"
UI="$ROOT/apps/control-plane/src"

pairs=(
  "/api/saas/activity|activity"
  "/api/saas/alerts|alerts"
  "/api/saas/portfolio|portfolio"
  "/api/saas/support/tickets|support"
  "/api/saas/pricing|pricing"
  "/api/saas/security/status|security"
  "/api/saas/team/invites|team"
  "/api/saas/risk-dashboard|risk"
  "/api/tenant/analytics|analytics"
  "/api/tenant/integrations|integrations"
  "/api/tenant/onboarding|onboarding"
  "/api/tenant/sniper/config|sniper/config"
  "/api/tenant/copy/config|copy/config"
  "/api/tenant/polymarket/config|polymarket/config"
)

failed=0
for pair in "${pairs[@]}"; do
  route="${pair%%|*}"
  label="${pair##*|}"
  if ! grep -RFlq -- "$route" "$SERVER"; then
    echo "missing server route: $route ($label)" >&2
    failed=1
  fi
  if ! grep -RFlq -- "$route" "$UI"; then
    echo "missing frontend client reference: $route ($label)" >&2
    failed=1
  fi
done

if (( failed )); then
  exit 1
fi
echo "check-routes-vs-ui: declared server/UI route pairs are present"
