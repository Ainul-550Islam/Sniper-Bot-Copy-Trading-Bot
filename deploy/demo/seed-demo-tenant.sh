#!/usr/bin/env bash
# ============================================================================
# seed-demo-tenant.sh — provision the deterministic `demo` tenant on the demo
# stack (deploy/demo/docker-compose.demo.yml). GAP MAP v2, P1 (deploy).
#
# What it does (idempotent — safe to run repeatedly):
#   1. Waits for the API to answer.
#   2. Registers the demo user (or reuses it if it already exists).
#   3. Logs in and captures the session token.
#   4. Creates the `demo` organization (or reuses it if it already exists).
#   5. Flags the organization with organizations.is_demo = true (migration
#      0051) so operators can query/exclude demo tenants.
#
# Usage:
#   ./deploy/demo/seed-demo-tenant.sh            # defaults: localhost demo stack
#   API=http://127.0.0.1:8080 ./deploy/demo/seed-demo-tenant.sh
#
# This script drives the SAME public HTTP API a real signup uses — it never
# hand-crafts password hashes or inserts rows directly, except for the single
# is_demo flag in step 5. It prints exactly what it created; it never invents
# data.
# ============================================================================
set -euo pipefail

API="${API:-http://127.0.0.1:8080}"
DEMO_EMAIL="${DEMO_EMAIL:-demo@example.com}"
DEMO_PASSWORD="${DEMO_PASSWORD:-demo-password-not-for-production}"
DEMO_ORG_SLUG="${DEMO_ORG_SLUG:-demo}"
DEMO_ORG_NAME="${DEMO_ORG_NAME:-Demo Tenant}"

# Repo root (the script lives in deploy/), so compose paths work from any cwd.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# --- tiny helpers -----------------------------------------------------------
require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "ERROR: '$1' is required but not found on PATH." >&2
    exit 1
  }
}
require_cmd curl
require_cmd python3

jq_get() {  # jq_get '<python-expr on parsed json>'  (reads stdin)
  python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"
}

# --- 0. wait for the API ----------------------------------------------------
echo "[demo] waiting for API at $API ..."
for i in $(seq 1 60); do
  # `/health` is the liveness probe (api.rs::obs::health_live).
  if curl -sf -o /dev/null "$API/health" 2>/dev/null; then
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "ERROR: API at $API did not become healthy in time." >&2
    echo "       Is the demo stack running? Try:" >&2
    echo "       docker compose -f deploy/demo/docker-compose.demo.yml up -d" >&2
    exit 1
  fi
  sleep 1
done
echo "[demo] API is up."

# --- 1. register (or reuse) the demo user -----------------------------------
echo "[demo] registering user '$DEMO_EMAIL' ..."
reg_status=$(curl -s -o /tmp/demo_register.json -w "%{http_code}" \
  -X POST "$API/api/saas/users" \
  -H 'content-type: application/json' \
  -d "$(python3 -c "import json,sys; print(json.dumps({'email':'$DEMO_EMAIL','password':'$DEMO_PASSWORD','display_name':'Demo User'}))")")

case "$reg_status" in
  201) echo "[demo] user created." ;;
  409) echo "[demo] user already exists — reusing." ;;
  *)
    echo "ERROR: unexpected register status $reg_status:" >&2
    cat /tmp/demo_register.json >&2; echo >&2
    exit 1 ;;
esac

# --- 2. login and capture the session token ---------------------------------
echo "[demo] logging in ..."
login_body=$(curl -s -X POST "$API/api/saas/sessions" \
  -H 'content-type: application/json' \
  -d "$(python3 -c "import json; print(json.dumps({'email':'$DEMO_EMAIL','password':'$DEMO_PASSWORD'}))")")

TOKEN=$(printf '%s' "$login_body" | jq_get "d.get('token','')")
if [ -z "$TOKEN" ]; then
  echo "ERROR: login did not return a session token. Response:" >&2
  echo "$login_body" >&2
  exit 1
fi
echo "[demo] session token captured (prefix shown only): ${TOKEN:0:10}..."

AUTH=(-H "authorization: Bearer $TOKEN")

# --- 3. create (or reuse) the demo organization -----------------------------
echo "[demo] creating organization '$DEMO_ORG_SLUG' ..."
org_status=$(curl -s -o /tmp/demo_org.json -w "%{http_code}" \
  -X POST "$API/api/saas/organizations" \
  "${AUTH[@]}" \
  -H 'content-type: application/json' \
  -d "$(python3 -c "import json; print(json.dumps({'name':'$DEMO_ORG_NAME','slug':'$DEMO_ORG_SLUG'}))")")

case "$org_status" in
  201) echo "[demo] organization created." ;;
  409) echo "[demo] organization already exists — reusing." ;;
  *)
    echo "ERROR: unexpected create-organization status $org_status:" >&2
    cat /tmp/demo_org.json >&2; echo >&2
    exit 1 ;;
esac

# Resolve the org id authoritatively from the user's membership list (works
# for both the fresh-create and the already-exists paths).
org_id=$(curl -s "${AUTH[@]}" "$API/api/saas/users/me" \
  | jq_get "next((o['organization_id'] for o in d.get('organizations',[]) if o.get('slug')=='$DEMO_ORG_SLUG'),'')")

if [ -z "$org_id" ]; then
  echo "ERROR: could not resolve organization id for slug '$DEMO_ORG_SLUG'." >&2
  exit 1
fi
echo "[demo] organization id: $org_id"

# --- 4. flag it as a demo tenant (migration 0051) ---------------------------
# The is_demo column is an OPERATIONAL flag (see 0051_demo_tenant_flag.sql):
# it is intentionally not part of the SaaS wire model, so we set it directly.
# Prefer `docker compose exec` against the demo stack; fall back to psql if
# the caller has a direct DB connection in PSQL_DEMO.
flag_via_compose() {
  docker compose -f "$ROOT/deploy/demo/docker-compose.demo.yml" exec -T postgres \
    psql -U demo -d demo -c \
    "UPDATE organizations SET is_demo = true WHERE id = '$org_id';" >/dev/null 2>&1
}
flag_via_psql() {
  : "${PSQL_DEMO:?set PSQL_DEMO to a psql-usable connection string}"
  "$PSQL_DEMO" -c "UPDATE organizations SET is_demo = true WHERE id = '$org_id';" >/dev/null 2>&1
}

if command -v docker >/dev/null 2>&1 && flag_via_compose; then
  echo "[demo] is_demo flag set via docker compose exec."
elif [ -n "${PSQL_DEMO:-}" ] && flag_via_psql; then
  echo "[demo] is_demo flag set via PSQL_DEMO."
else
  echo "WARN: could not set the is_demo flag automatically." >&2
  echo "      The tenant still works; set the flag manually with:" >&2
  echo "      UPDATE organizations SET is_demo = true WHERE id = '$org_id';" >&2
fi

# --- 5. summary -------------------------------------------------------------
echo
echo "=============================================================="
echo " Demo tenant ready."
echo "   API:        $API"
echo "   Email:      $DEMO_EMAIL"
echo "   Password:   $DEMO_PASSWORD"
echo "   Org slug:   $DEMO_ORG_SLUG"
echo "   Org id:     $org_id"
echo "   is_demo:    flagged (operators can query/exclude it)"
echo
echo " Sign in through the control plane and select the '$DEMO_ORG_SLUG'"
echo " organization. It is a PAPER-mode tenant on devnet — no real funds."
echo "=============================================================="
