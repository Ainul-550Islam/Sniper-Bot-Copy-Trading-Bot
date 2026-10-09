#!/usr/bin/env bash
# ============================================================================
# scripts/verify-environment-separation.sh — staging/prod separation gate
# (P0 §6).
#
# Answers one question mechanically: can staging reach anything that
# belongs to production? Separation that is only written in a runbook is
# not separation; this script is the enforcement.
#
# Modes:
#   (default)  TEMPLATE mode — validate the committed templates: staging
#              has every live gate off, both files define the same
#              variable set, no real secret is committed. Runs in CI with
#              no access to real values.
#
#   --filled   FILLED mode — compare the real, filled-in
#              deploy/environments/{staging,production}.env and fail on
#              ANY shared secret, database, key id or hostname. Run this
#              on the deploy host, never in CI.
#
# Exit non-zero on the first violation.
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ENV_DIR="deploy/environments"
STAGING_T="$ENV_DIR/staging.env.template"
PROD_T="$ENV_DIR/production.env.template"
MODE="template"
[[ "${1:-}" == "--filled" ]] && MODE="filled"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "  ok   $*"; }

# Read `KEY=value` from an env file, ignoring comments. Prints the value
# with surrounding quotes and trailing inline comments stripped.
value_of() {
    local key="$1" file="$2"
    sed -nE "s/^[[:space:]]*${key}=([^#]*).*/\1/p" "$file" \
        | tail -n1 \
        | sed -E 's/[[:space:]]+$//; s/^"(.*)"$/\1/; s/^'"'"'(.*)'"'"'$/\1/'
}

keys_of() {
    sed -nE 's/^[[:space:]]*([A-Z0-9_]+)=.*/\1/p' "$1" | sort -u
}

echo "== files exist =="
for f in "$STAGING_T" "$PROD_T" ".env.template"; do
    [[ -f "$f" ]] || fail "missing $f"
    pass "$f"
done

# ---------------------------------------------------------------------------
echo
echo "== staging: every live gate is OFF =="
# The complete list of switches that let a deployment touch real money,
# real keys or a real card. Each must hold the stated value in staging.
declare -A STAGING_REQUIRED=(
    [ALLOW_LIVE_TRADING]=false
    [SOLANA_LIVE]=0
    [LIVE_BILLING]=0
    [LIVE_CUSTODY]=0
    [DEPLOYMENT_SMOKE_LIVE]=0
    [SNIPER_ENVIRONMENT]=staging
)
for k in "${!STAGING_REQUIRED[@]}"; do
    want="${STAGING_REQUIRED[$k]}"
    got="$(value_of "$k" "$STAGING_T")"
    [[ "$got" == "$want" ]] || fail "staging $k must be '$want', found '${got:-<unset>}'"
    pass "staging $k=$want"
done

mode="$(value_of EXECUTION_MODE "$STAGING_T")"
[[ "$mode" == "paper" || "$mode" == "dry_run" ]] \
    || fail "staging EXECUTION_MODE must be paper|dry_run, found '$mode'"
pass "staging EXECUTION_MODE=$mode"

cluster="$(value_of SOLANA_CLUSTER "$STAGING_T")"
[[ "$cluster" == "devnet" || "$cluster" == "testnet" ]] \
    || fail "staging SOLANA_CLUSTER must be devnet|testnet, found '$cluster'"
pass "staging SOLANA_CLUSTER=$cluster"

# ---------------------------------------------------------------------------
echo
echo "== production: no unsafe default is pre-enabled =="
for k in ALLOW_LIVE_TRADING SOLANA_LIVE LIVE_BILLING LIVE_CUSTODY DEPLOYMENT_SMOKE_LIVE; do
    got="$(value_of "$k" "$PROD_T")"
    case "$got" in
        false|0) pass "production $k=$got (promoted deliberately, not by default)" ;;
        *) fail "production template ships $k='$got' — live gates must default OFF" ;;
    esac
done

[[ "$(value_of SNIPER_ENVIRONMENT "$PROD_T")" == "production" ]] \
    || fail "production SNIPER_ENVIRONMENT must be 'production'"
pass "production SNIPER_ENVIRONMENT=production"

[[ "$(value_of DATABASE_REQUIRED "$PROD_T")" == "true" ]] \
    || fail "production DATABASE_REQUIRED must be true — production must never run on the memory store"
pass "production DATABASE_REQUIRED=true"

[[ "$(value_of DATABASE_AUTO_MIGRATE "$PROD_T")" == "false" ]] \
    || fail "production DATABASE_AUTO_MIGRATE must be false — migrations run once in the deploy step, not per replica"
pass "production DATABASE_AUTO_MIGRATE=false"

# ---------------------------------------------------------------------------
echo
echo "== the two environments name different resources =="
# Compared at TEMPLATE level these are placeholders, but they must still
# be DIFFERENT placeholders: identical placeholders is how an operator
# ends up filling one value into both files.
for k in SNIPER_PUBLIC_HOST POSTGRES_USER POSTGRES_DB REDIS_URL SOLANA_KEYPAIR_HOST_PATH; do
    s="$(value_of "$k" "$STAGING_T")"
    p="$(value_of "$k" "$PROD_T")"
    [[ -n "$s" && -n "$p" ]] || fail "$k must be set in BOTH templates (staging='$s' production='$p')"
    [[ "$s" != "$p" ]] || fail "$k is identical in staging and production ('$s') — these must be distinct resources"
    pass "$k differs ($s vs $p)"
done

# ---------------------------------------------------------------------------
echo
echo "== templates declare the same variable set =="
# A variable present in one file and missing from the other is how an
# environment silently inherits a default it should have overridden.
missing_in_prod="$(comm -23 <(keys_of "$STAGING_T") <(keys_of "$PROD_T") | grep -v '^$' || true)"
missing_in_stg="$(comm -13 <(keys_of "$STAGING_T") <(keys_of "$PROD_T") | grep -v '^$' || true)"
# Variables that are legitimately environment-specific.
ALLOWED_ONLY_PROD='^(REDIS_ENABLED|REDIS_REQUIRED|GEYSER_WS_URL|HA_REPLICA_ID|HA_CLAIM_LEASE_SECS)$'
ALLOWED_ONLY_STG='^$'
unexpected_prod="$(grep -Ev "$ALLOWED_ONLY_STG" <<<"$missing_in_prod" | grep -v '^$' || true)"
unexpected_stg="$(grep -Ev "$ALLOWED_ONLY_PROD" <<<"$missing_in_stg" | grep -v '^$' || true)"
[[ -z "$unexpected_prod" ]] || fail "declared in staging but not production: $(tr '\n' ' ' <<<"$unexpected_prod")"
[[ -z "$unexpected_stg" ]] || fail "declared in production but not staging: $(tr '\n' ' ' <<<"$unexpected_stg")"
pass "variable sets reconcile"

# ---------------------------------------------------------------------------
echo
echo "== no real secret is committed =="
# Templates must ship EMPTY secret slots. A committed credential is a
# release blocker regardless of which environment it belongs to.
# Strip comment lines and inline comments first: these templates DOCUMENT
# the forbidden patterns ("a sk_live_ key here is a release blocker"), and
# a gate that cannot tell a warning from a credential is a gate nobody
# keeps.
assignments_only() {
    sed -E 's/#.*$//' "$1" | grep -E '^[[:space:]]*[A-Z0-9_]+=' || true
}

for f in "$STAGING_T" "$PROD_T"; do
    if assignments_only "$f" | grep -nE '^[[:space:]]*(STRIPE_API_KEY|PADDLE_API_KEY|VAULT_TOKEN|API_KEY|AUTH_KEY_[A-Z]+|SAAS_WEBHOOK_SECRET_[A-Z]+)=[^[:space:]]'; then
        fail "$f contains a non-empty secret value"
    fi
    if assignments_only "$f" | grep -nE 'sk_live_|whsec_[A-Za-z0-9]{10,}|hvs\.[A-Za-z0-9]{10,}|-----BEGIN [A-Z ]*PRIVATE KEY-----'; then
        fail "$f contains what looks like a real credential"
    fi
done
pass "secret slots are empty in both templates"

# POSTGRES_PASSWORD is checked separately: the root .env.template ships a
# `change-me-…` placeholder, and these files must not reintroduce one.
for f in "$STAGING_T" "$PROD_T"; do
    pw="$(value_of POSTGRES_PASSWORD "$f")"
    [[ -z "$pw" ]] || fail "$f sets POSTGRES_PASSWORD ('$pw'); it must be empty in a committed template"
done
pass "POSTGRES_PASSWORD empty in both templates"

# staging must never even NAME a live credential or mainnet in a VALUE.
if assignments_only "$STAGING_T" | grep -qE 'sk_live_|mainnet-beta'; then
    fail "staging template assigns a live credential or mainnet endpoint"
fi
pass "staging assigns no live credential or mainnet endpoint"

if [[ "$MODE" == "template" ]]; then
    echo
    echo "ENVIRONMENT SEPARATION (templates): PASS"
    echo "(run with --filled on the deploy host to compare the real values)"
    exit 0
fi

# ---------------------------------------------------------------------------
# FILLED mode — the real values
# ---------------------------------------------------------------------------
STAGING_F="$ENV_DIR/staging.env"
PROD_F="$ENV_DIR/production.env"
for f in "$STAGING_F" "$PROD_F"; do
    [[ -f "$f" ]] || fail "--filled needs $f (copy it from the template and fill it in)"
done

echo
echo "== filled: no shared value on any sensitive variable =="
SENSITIVE=(
    POSTGRES_PASSWORD POSTGRES_URL DATABASE_URL REDIS_URL
    API_KEY AUTH_KEY_OWNER AUTH_KEY_OPERATOR
    STRIPE_API_KEY STRIPE_WEBHOOK_SECRET PADDLE_API_KEY PADDLE_WEBHOOK_SECRET
    SAAS_WEBHOOK_SECRET_MANUAL SAAS_WEBHOOK_SECRET_STRIPE SAAS_WEBHOOK_SECRET_PADDLE
    KMS_KEY_ID VAULT_ADDR VAULT_TOKEN
    SOLANA_KEYPAIR_HOST_PATH SOLANA_RPC_URL SNIPER_PUBLIC_HOST
    POSTGRES_DB POSTGRES_USER
)
violations=0
for k in "${SENSITIVE[@]}"; do
    s="$(value_of "$k" "$STAGING_F")"
    p="$(value_of "$k" "$PROD_F")"
    # Both empty = neither environment uses it. Not a violation.
    [[ -z "$s" && -z "$p" ]] && continue
    if [[ "$s" == "$p" ]]; then
        # Never print the value itself.
        echo "FAIL: $k has the SAME value in staging and production" >&2
        violations=$((violations + 1))
    else
        pass "$k differs"
    fi
done
[[ "$violations" -eq 0 ]] || fail "$violations shared value(s) between staging and production"

echo
echo "== filled: staging points at no production host =="
prod_host="$(value_of SNIPER_PUBLIC_HOST "$PROD_F")"
prod_db="$(value_of POSTGRES_URL "$PROD_F")"
if [[ -n "$prod_host" ]] && grep -q -- "$prod_host" "$STAGING_F"; then
    fail "staging.env mentions the production host '$prod_host'"
fi
pass "staging.env does not mention the production host"
if [[ -n "$prod_db" ]]; then
    # Compare the host portion only — never echo the credential.
    prod_db_host="$(sed -E 's#^[^@]*@([^/:]+).*#\1#' <<<"$prod_db")"
    if [[ -n "$prod_db_host" ]] && grep -q -- "$prod_db_host" "$STAGING_F"; then
        fail "staging.env points at the production database host"
    fi
fi
pass "staging.env does not point at the production database host"

echo
echo "== filled: staging live gates still OFF =="
for k in "${!STAGING_REQUIRED[@]}"; do
    want="${STAGING_REQUIRED[$k]}"
    got="$(value_of "$k" "$STAGING_F")"
    [[ "$got" == "$want" ]] || fail "staging.env $k must be '$want', found '${got:-<unset>}'"
    pass "staging.env $k=$want"
done

if grep -qE '^STRIPE_API_KEY=sk_live_' "$STAGING_F"; then
    fail "staging.env holds a LIVE Stripe key"
fi
pass "staging.env holds no live Stripe key"

echo
echo "ENVIRONMENT SEPARATION (filled): PASS"
