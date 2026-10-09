#!/usr/bin/env bash
# ============================================================================
# scripts/rollback-release.sh — return an environment to a previously
# VERIFIED image (P0 §7).
#
#   ./scripts/rollback-release.sh production
#   ./scripts/rollback-release.sh production --to repo@sha256:…
#   ./scripts/rollback-release.sh production --list
#
# WHAT A ROLLBACK IS AND IS NOT HERE
# ----------------------------------
# It is: re-running a specific, immutable image digest that this ledger
# records as having passed verification in THIS environment before.
#
# It is NOT a database rollback. Migrations in this codebase are
# forward-only and additive by policy (see crates/core/migrations/), so an
# older application binary runs against a newer schema. That is safe for
# every migration up to and including 0036 with ONE stated exception,
# which this script checks for and warns about: rolling back past the
# 0036 application code loses the durable tenant kill-switch, because the
# older binary reads the process-local map instead of
# `tenant_module_controls`. A tenant pause recorded durably would not be
# honoured. Confirm explicitly before doing that.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

LEDGER="deploy/release/deployments.jsonl"
ENVIRONMENT="${1:-}"
shift || true
TARGET=""
LIST=0
ASSUME_YES=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --to)   TARGET="${2:?--to needs an image reference}"; shift 2 ;;
        --list) LIST=1; shift ;;
        --yes)  ASSUME_YES=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

case "$ENVIRONMENT" in
    staging|production) ;;
    *) echo "usage: $0 {staging|production} [--to repo@sha256:…] [--list] [--yes]" >&2; exit 2 ;;
esac

log()  { printf '\n==> %s\n' "$*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

[[ -f "$LEDGER" ]] || fail "$LEDGER not found — there is no recorded deployment to roll back to"

# Every verified deployment of this environment, oldest first.
history_for_env() {
    grep "\"environment\": *\"$ENVIRONMENT\"" "$LEDGER" | grep '"status": *"verified"'
}

if [[ "$LIST" -eq 1 ]]; then
    echo "verified deployments of $ENVIRONMENT (oldest first):"
    history_for_env | while IFS= read -r line; do
        ts="$(sed -E 's/.*"ts": *"([^"]+)".*/\1/' <<<"$line")"
        img="$(sed -E 's/.*"image": *"([^"]+)".*/\1/' <<<"$line")"
        rev="$(sed -E 's/.*"revision": *"([^"]+)".*/\1/' <<<"$line")"
        who="$(sed -E 's/.*"operator": *"([^"]+)".*/\1/' <<<"$line")"
        printf '  %s  %s  rev=%s  by=%s\n' "$ts" "$img" "$rev" "$who"
    done
    exit 0
fi

CURRENT="$(history_for_env | tail -n1 | sed -E 's/.*"image": *"([^"]+)".*/\1/')"
[[ -n "$CURRENT" ]] || fail "no verified deployment recorded for $ENVIRONMENT"

if [[ -z "$TARGET" ]]; then
    # The most recent verified image that is NOT the current one.
    TARGET="$(history_for_env | sed -E 's/.*"image": *"([^"]+)".*/\1/' \
              | grep -vx "$CURRENT" | tail -n1)"
    [[ -n "$TARGET" ]] || fail "only one verified image ($CURRENT) is recorded for $ENVIRONMENT — there is nothing to roll back to"
else
    # An explicit target must still be something this environment verified.
    # Rolling into an image that never passed here is not a rollback.
    history_for_env | grep -q "\"image\": *\"$TARGET\"" \
        || fail "$TARGET was never recorded as verified in $ENVIRONMENT; refusing to 'roll back' to an unproven image"
fi

[[ "$TARGET" == *"@sha256:"* || "$TARGET" == local:* ]] \
    || fail "rollback target is not an immutable reference: $TARGET"

ENV_FILE="deploy/environments/${ENVIRONMENT}.env"
[[ -f "$ENV_FILE" ]] || fail "$ENV_FILE not found"

cat <<EOF

ROLLBACK PLAN
  environment : $ENVIRONMENT
  from        : $CURRENT
  to          : $TARGET

Schema note: migrations are forward-only. The database stays at its
current version; the older binary runs against the newer schema.
EOF

# The one known-unsafe direction, called out by name.
if [[ -f crates/core/migrations/0036_durable_tenant_controls_and_rotations.sql ]]; then
    cat <<'EOF'

  WARNING — migration 0036 is present in this tree. If $TARGET predates
  the 0036 APPLICATION code, that binary reads module controls, custody
  rotation state and WebSocket replay state from PROCESS MEMORY, not from
  the database. Consequences while rolled back:
    * a tenant pause recorded in tenant_module_controls is NOT honoured;
    * rotations in custody_rotations are invisible to the API;
    * WebSocket replay protection stops being shared across replicas.
  If the target predates 0036, scale to ONE replica before rolling back.
EOF
fi

if [[ "$ASSUME_YES" -ne 1 ]]; then
    read -r -p $'\nProceed? type the environment name to confirm: ' answer
    [[ "$answer" == "$ENVIRONMENT" ]] || fail "not confirmed"
fi

set -a
# shellcheck disable=SC1091
. .env.template
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

COMPOSE=(docker compose --env-file "$ENV_FILE" -f docker-compose.yml -f deploy/compose/docker-compose.tls.yml)

log "pulling the target image"
if [[ "$TARGET" != local:* ]]; then
    docker pull "$TARGET" || fail "could not pull $TARGET — the rollback target is not retrievable"
else
    docker image inspect "${TARGET#local:}" >/dev/null 2>&1 \
        || fail "local image ${TARGET#local:} is no longer on this host; the rollback target is gone"
fi

log "rolling back"
SNIPER_IMAGE="$TARGET" "${COMPOSE[@]}" up -d --no-build bot nginx

log "verifying"
healthy=0
for _ in $(seq 1 60); do
    if "${COMPOSE[@]}" exec -T bot curl -fsS http://127.0.0.1:8080/api/health >/dev/null 2>&1; then
        healthy=1; break
    fi
    sleep 2
done
[[ "$healthy" -eq 1 ]] || fail "the rolled-back image did not become healthy within 120s — this environment needs hands-on recovery NOW (see docs/ROLLBACK-RUNBOOK.md)"
echo "  ok   /api/health"

printf '{"ts": "%s", "environment": "%s", "image": "%s", "previous": "%s", "revision": "rollback", "status": "verified", "detail": "rollback", "operator": "%s"}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$ENVIRONMENT" "$TARGET" "$CURRENT" "${USER:-unknown}" \
    >> "$LEDGER"

cat <<EOF

ROLLBACK OK
  environment : $ENVIRONMENT
  now running : $TARGET
  rolled from : $CURRENT

Recorded in $LEDGER. Re-deploy forward with:
  ./scripts/deploy-release.sh $ENVIRONMENT --image <fixed-image@sha256:…>
EOF
