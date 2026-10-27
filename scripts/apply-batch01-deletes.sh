#!/usr/bin/env bash
# Applies Batch 01's duplicate-file deletions.
#
# Deletions cannot travel inside batch01-code.zip, so they are applied here,
# by git, after the archive is unzipped at the repository root. Every file
# below is a byte-identical or shadowed duplicate whose canonical copy is
# the one the tree actually imports (verified by the greps this script runs
# before touching anything):
#
#   apps/api/src/modules/exchanges/venues/*.provider.ts
#       canonical: apps/api/src/modules/exchanges/providers/*.provider.ts
#       (imported by exchange-provider.factory.ts; the venues/ spec moved
#       to providers/venue-providers.spec.ts in the same batch)
#   services/execution-engine/app/submission.py
#       canonical: services/execution-engine/app/orders/submission.py
#   services/execution-engine/app/placement.py
#       canonical: services/execution-engine/app/orders/placement.py
#   services/execution-engine/app/security.py
#       shadowed by the app/security/ package; canonical:
#       services/execution-engine/app/security/internal_auth.py
#   services/execution-engine/app/secret_fetcher.py
#       canonical: services/execution-engine/app/security/secret_fetcher.py
#   services/execution-engine/app/credentials.py
#       canonical: services/execution-engine/app/exchanges/credentials.py
#   apps/api/src/modules/auth/step-up/{step-up.controller,step-up.service,step-up.module}.ts
#   + dto/verify-step-up.dto.ts and the two specs of that stack
#       canonical: apps/api/src/modules/auth/step-up-auth.{controller,service}.ts
#       (the AuthModule-registered stack the guard consumes and both clients
#       call; the step-up/ stack is imported nowhere outside its own specs,
#       registers nothing, and stores proofs under a different Redis key -
#       importing its module would double-register POST /v1/auth/step-up/verify
#       against an incompatible proof store). The valuable coverage from its
#       specs was rescued into step-up.guard.spec.ts and step-up-auth.spec.ts;
#       the guard, decorator and guard spec in the same directory stay.
#   .github/workflows/*.yml (the copies nested under whitelabel-copytrade/)
#       canonical: the repository-root .github/workflows (the only ones
#       GitHub reads; root ci.yml carries the nested file's unique steps and
#       says so). The nested copies never run, have diverged from the root
#       workflows, and ops/production-validation-50-checks.js requires that
#       they not exist.
#
# The script aborts before deleting anything if a deletion target is still
# imported anywhere outside the deletion set itself.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
WL="whitelabel-copytrade"

fail() {
  echo "apply-batch01-deletes: $1" >&2
  exit 1
}

DELETES=(
  "$WL/apps/api/src/modules/exchanges/venues/bybit.provider.ts"
  "$WL/apps/api/src/modules/exchanges/venues/okx.provider.ts"
  "$WL/apps/api/src/modules/exchanges/venues/kraken.provider.ts"
  "$WL/apps/api/src/modules/exchanges/venues/coinbase.provider.ts"
  "$WL/apps/api/src/modules/exchanges/venues/venue-providers.spec.ts"
  "$WL/apps/api/src/modules/auth/step-up/step-up.controller.ts"
  "$WL/apps/api/src/modules/auth/step-up/step-up.service.ts"
  "$WL/apps/api/src/modules/auth/step-up/step-up.module.ts"
  "$WL/apps/api/src/modules/auth/step-up/step-up.controller.spec.ts"
  "$WL/apps/api/src/modules/auth/step-up/step-up.service.spec.ts"
  "$WL/apps/api/src/modules/auth/step-up/dto/verify-step-up.dto.ts"
  "$WL/services/execution-engine/app/submission.py"
  "$WL/services/execution-engine/app/placement.py"
  "$WL/services/execution-engine/app/security.py"
  "$WL/services/execution-engine/app/secret_fetcher.py"
  "$WL/services/execution-engine/app/credentials.py"
  "$WL/.github/workflows/ci.yml"
  "$WL/.github/workflows/infra.yml"
  "$WL/.github/workflows/production-release.yml"
  "$WL/.github/workflows/release.yml"
  "$WL/.github/workflows/security.yml"
)

# --- import guards -----------------------------------------------------------
# TypeScript: nothing outside venues/ may import the venues/ copies. The
# moved spec (providers/venue-providers.spec.ts) imports './<venue>.provider'
# and '../providers/<venue>.provider', neither of which names venues/.
if grep -rnE "from ['\"][^'\"]*venues/(bybit|okx|kraken|coinbase)\.provider" \
    "$WL/apps/api/src" --include='*.ts' \
    | grep -v "/exchanges/venues/" ; then
  fail "a venues/ provider is still imported; refusing to delete it"
fi

# TypeScript: nothing outside the dead stack's own directory may import it.
if grep -rnE "from ['\"][^'\"]*step-up/(step-up\.(controller|service|module)|dto/verify-step-up\.dto)" \
    "$WL/apps/api/src" --include='*.ts' \
    | grep -v "/auth/step-up/" ; then
  fail "the dead step-up stack is still imported; refusing to delete it"
fi
if grep -rnE "from ['\"]\./(step-up\.(controller|service|module)|dto/verify-step-up\.dto)" \
    "$WL/apps/api/src/modules/auth" --include='*.ts' \
    | grep -v "/auth/step-up/" ; then
  fail "the dead step-up stack is still imported; refusing to delete it"
fi

# Python (execution-engine only; sibling services have their own app.*
# packages): nothing outside the deletion set may import the top-level
# duplicates. Canonical imports are app.orders.*, app.security.* and
# app.exchanges.credentials, which these patterns do not match.
EE="$WL/services/execution-engine"
check_python() {
  local pattern="$1" label="$2"
  local hits
  hits="$(grep -rnE "$pattern" "$EE/app" "$EE/tests" --include='*.py' \
    | grep -vE "/app/(submission|placement|security|secret_fetcher|credentials)\.py:" || true)"
  if [ -n "$hits" ]; then
    echo "$hits" >&2
    fail "$label is still imported; refusing to delete it"
  fi
}
check_python 'from app\.submission import|from app import submission\b|import app\.submission\b' 'app/submission.py'
check_python 'from app\.placement import|from app import placement\b|import app\.placement\b' 'app/placement.py'
check_python 'from app\.secret_fetcher import|from app import secret_fetcher\b|import app\.secret_fetcher\b' 'app/secret_fetcher.py'
check_python 'from app\.credentials import|from app import credentials\b|import app\.credentials\b' 'app/credentials.py'
check_python 'from app\.security import|from app import security\b' 'app/security.py'

# Workflows: the live (repository-root) workflows must exist before their
# nested copies are removed.
for root_wf in ci.yml release.yml security.yml codeql.yml production-release.yml; do
  if [ ! -f ".github/workflows/$root_wf" ]; then
    fail "repository-root workflow $root_wf is missing; refusing to delete its nested copy"
  fi
done

# --- deletions ----------------------------------------------------------------
for path in "${DELETES[@]}"; do
  if git ls-files --error-unmatch "$path" >/dev/null 2>&1; then
    git rm -q "$path"
    echo "deleted $path"
  elif [ -e "$path" ]; then
    fail "$path exists but is not tracked by git; remove it manually so the deletion is reviewed"
  else
    echo "already absent $path"
  fi
done

echo "apply-batch01-deletes: done (deletions are staged, not committed)"

