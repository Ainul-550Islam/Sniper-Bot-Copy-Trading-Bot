#!/usr/bin/env bash
# Frontend contract gate: type safety plus the production truthfulness scan.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

(cd "$ROOT/apps/control-plane" && npm run typecheck)
bash "$ROOT/scripts/forbid-fake-data.sh"

echo "check-frontend-contract: typecheck and truthfulness checks passed"
