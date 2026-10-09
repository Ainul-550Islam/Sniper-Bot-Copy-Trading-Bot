#!/usr/bin/env bash
# run-live-validation.sh — live evidence orchestrator (GAP MAP v2, Part 5).
#
# Writes evidence/live/<validation_id>.json for every live-validation category.
# The honesty contract is absolute:
#
#   - Default run (no live env): every category is recorded as NOT_RUN with
#     the exact command that WOULD prove it. Nothing is fabricated.
#   - Live run (LIVE_<CATEGORY>=1 + credentials): the matching cargo harness
#     runs; PASSED is written ONLY when the harness exits 0 AND its captured
#     output contains the required attestation (a transaction signature,
#     provider reference id, or equivalent). A harness that passes WITHOUT the
#     attestation is recorded as FAILED with reason=missing_attestation — an
#     unattested "success" is not evidence.
#   - Secrets are redacted in every written file and every printed line.
#
# Usage:
#   scripts/run-live-validation.sh                 # all categories, honest NOT_RUN
#   scripts/run-live-validation.sh <id>            # one category
#   LIVE_SOLANA_FUNDED=1 RPC_URL=… scripts/run-live-validation.sh solana_funded_preflight
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EVIDENCE_DIR="${EVIDENCE_DIR:-$ROOT/evidence/live}"
mkdir -p "$EVIDENCE_DIR"

redact() {
  sed -E \
    -e 's/(DATABASE_URL|POSTGRES_URL|REDIS_URL|STRIPE_API_KEY|STRIPE_WEBHOOK_SECRET|PADDLE_API_KEY|PADDLE_WEBHOOK_SECRET|VAULT_TOKEN|VAULT_NAMESPACE|KMS_KEY_ID|KMS_SECRET|HSM_SLOT|HSM_PIN|RPC_URL|GEYSER_URL|DEPLOYMENT_BASE_URL|SOLANA_KEYPAIR|WALLET_PRIVATE_KEY|SEED_PHRASE|MNEMONIC|POLYGON_RPC_URL)=[^ ]*/\1=<redacted>/g' \
    -e 's/([Bb]earer)[[:space:]]+[A-Za-z0-9._~+\/-]+/\1 <redacted>/g' \
    -e 's/BEGIN PRIVATE KEY/<redacted>/g'
}

NOW() { date -u +%Y-%m-%dT%H:%M:%SZ; }

# write_evidence <id> <provider> <status> <command> <metadata-json> <attestation_ref>
write_evidence() {
  local id="$1" provider="$2" status="$3" command="$4" metadata="$5" attestation="${6:-}"
  local file="$EVIDENCE_DIR/${id}.json"
  cat <<EOF | redact > "$file"
{
  "validation_id": "$id",
  "provider": "$provider",
  "environment": "live",
  "timestamp": "$(NOW)",
  "status": "$status",
  "command": "$command",
  "attestation_ref": "$attestation",
  "redacted_metadata": $metadata,
  "rule": "status PASSED requires attestation_ref to be non-empty (a real transaction signature or provider reference id). Never fabricate."
}
EOF
  echo "[live-validation] $id -> $status ($file)" | redact
}

# attempt <id> <provider> <gate> <command> -- <attestation-regex>
# Runs the harness live; PASSED only with exit 0 + attestation captured.
attempt() {
  local id="$1" provider="$2" gate="$3" command="$4"
  shift 4
  if [ "${1:-}" = "--" ]; then shift; fi
  local attest_re="${1:-[A-Za-z0-9]{16,}}"
  local log
  if ! command -v cargo >/dev/null 2>&1; then
    write_evidence "$id" "$provider" "NOT_RUN" "$command" \
      '{"reason":"cargo toolchain not available in this environment"}' ""
    return
  fi
  log=$(mktemp)
  if eval "$command" >"$log" 2>&1; then
    local attestation
    attestation=$(grep -Eo "$attest_re" "$log" | head -1 || true)
    if [ -n "$attestation" ]; then
      write_evidence "$id" "$provider" "PASSED" "$command" \
        "{\"harness_exit\":0}" "$(echo "$attestation" | redact)"
    else
      write_evidence "$id" "$provider" "FAILED" "$command" \
        '{"reason":"missing_attestation","harness_exit":0,"note":"harness passed but emitted no transaction signature / provider id; success without attestation is not evidence"}' ""
    fi
  else
    write_evidence "$id" "$provider" "FAILED" "$command" \
      "{\"reason\":\"harness_failed\",\"harness_exit\":1}" ""
  fi
  rm -f "$log"
}

not_run() {
  local id="$1" provider="$2" command="$3" note="$4"
  write_evidence "$id" "$provider" "NOT_RUN" "$command" "{\"reason\":\"$note\"}" ""
}

run_category() {
  local id="$1"
  case "$id" in
    solana_funded_preflight)
      if [ "${LIVE_SOLANA_FUNDED:-0}" = "1" ] && [ -n "${RPC_URL:-}" ] && [ -n "${SOLANA_KEYPAIR:-}" ]; then
        attempt "$id" "solana_rpc" "1" \
          'LIVE_SOLANA_FUNDED=1 cargo test -p sniper-suite --test live_funded_preflight -- --ignored --nocapture' \
          -- '[1-9A-HJ-NP-Za-km-z]{32,}'
      else
        not_run "$id" "solana_rpc" \
          "LIVE_SOLANA_FUNDED=1 RPC_URL=<redacted> SOLANA_KEYPAIR=<redacted> cargo test -p sniper-suite --test live_funded_preflight -- --ignored" \
          "requires LIVE_SOLANA_FUNDED=1 + RPC_URL + funded keypair; read-only balance + rent probe, never trades"
      fi ;;
    pumpfun_buy_sell_roundtrip)
      if [ "${LIVE_PUMPFUN:-0}" = "1" ] && [ -n "${RPC_URL:-}" ] && [ -n "${SOLANA_KEYPAIR:-}" ]; then
        attempt "$id" "pump_fun" "1" \
          'LIVE_PUMPFUN=1 cargo test -p sniper-suite --test live_pumpfun_roundtrip -- --ignored --nocapture' \
          -- '[1-9A-HJ-NP-Za-km-z]{64,}'
      else
        not_run "$id" "pump_fun" \
          "LIVE_PUMPFUN=1 RPC_URL=<redacted> SOLANA_KEYPAIR=<redacted> cargo test -p sniper-suite --test live_pumpfun_roundtrip -- --ignored" \
          "dust-sized bonding-curve buy+sell on mainnet; requires funded keypair; attestation = both tx signatures"
      fi ;;
    pumpswap_buy_sell_roundtrip)
      if [ "${LIVE_PUMPSWAP:-0}" = "1" ] && [ -n "${RPC_URL:-}" ] && [ -n "${SOLANA_KEYPAIR:-}" ]; then
        attempt "$id" "pump_swap" "1" \
          'LIVE_PUMPSWAP=1 cargo test -p sniper-suite --test live_pumpswap_roundtrip -- --ignored --nocapture' \
          -- '[1-9A-HJ-NP-Za-km-z]{64,}'
      else
        not_run "$id" "pump_swap" \
          "LIVE_PUMPSWAP=1 RPC_URL=<redacted> SOLANA_KEYPAIR=<redacted> cargo test -p sniper-suite --test live_pumpswap_roundtrip -- --ignored" \
          "dust-sized AMM buy+sell post-graduation; requires funded keypair; attestation = both tx signatures"
      fi ;;
    polymarket_order_roundtrip)
      if [ "${LIVE_POLYMARKET:-0}" = "1" ] && [ -n "${POLYMARKET_API_KEY:-}" ] && [ -n "${POLYMARKET_SECRET:-}" ]; then
        attempt "$id" "polymarket_clob" "1" \
          'LIVE_POLYMARKET=1 cargo test -p sniper-suite --test live_polymarket_roundtrip -- --ignored --nocapture' \
          -- '0x[0-9a-fA-F]{8,}'
      else
        not_run "$id" "polymarket_clob" \
          "LIVE_POLYMARKET=1 POLYMARKET_API_KEY=<redacted> cargo test -p sniper-suite --test live_polymarket_roundtrip -- --ignored" \
          "1-cent limit order place+cancel on CLOB; attestation = order id; requires funded Polygon wallet"
      fi ;;
    stripe_checkout_roundtrip)
      if [ "${LIVE_STRIPE:-0}" = "1" ] && [ -n "${STRIPE_API_KEY:-}" ]; then
        attempt "$id" "stripe" "1" \
          'LIVE_STRIPE=1 cargo test -p sniper-suite --test live_stripe_contract -- --ignored --nocapture' \
          -- 'cs_(test|live)_[A-Za-z0-9]+'
      else
        not_run "$id" "stripe" \
          "LIVE_STRIPE=1 STRIPE_API_KEY=<redacted> cargo test -p sniper-suite --test live_stripe_contract -- --ignored" \
          "creates+expires a test-mode checkout session; attestation = checkout session id"
      fi ;;
    kms_sign_transit)
      if [ "${LIVE_KMS:-0}" = "1" ] && [ -n "${KMS_KEY_ID:-}" ]; then
        attempt "$id" "aws_kms" "1" \
          'LIVE_KMS=1 cargo test -p sniper-suite --test live_kms_sign -- --ignored --nocapture' \
          -- '[A-Za-z0-9+/=]{40,}'
      else
        not_run "$id" "aws_kms" \
          "LIVE_KMS=1 KMS_KEY_ID=<redacted> cargo test -p sniper-suite --test live_kms_sign -- --ignored" \
          "remote sign round-trip; key material never leaves KMS; attestation = signature blob reference"
      fi ;;
    vault_transit)
      if [ "${LIVE_VAULT:-0}" = "1" ] && [ -n "${VAULT_ADDR:-}" ] && [ -n "${VAULT_TOKEN:-}" ]; then
        attempt "$id" "vault" "1" \
          'LIVE_VAULT=1 cargo test -p sniper-suite --test live_vault_transit -- --ignored --nocapture' \
          -- 'vault:v[0-9]+:[A-Za-z0-9+/=]+'
      else
        not_run "$id" "vault" \
          "LIVE_VAULT=1 VAULT_ADDR=<redacted> VAULT_TOKEN=<redacted> cargo test -p sniper-suite --test live_vault_transit -- --ignored" \
          "transit encrypt+decrypt round-trip; attestation = ciphertext version prefix"
      fi ;;
    deployment_smoke)
      if [ "${LIVE_DEPLOYMENT:-0}" = "1" ] && [ -n "${DEPLOYMENT_BASE_URL:-}" ]; then
        attempt "$id" "deployment" "1" \
          'LIVE_DEPLOYMENT=1 cargo test -p sniper-suite --test deployment_smoke -- --nocapture' \
          -- 'health=(ok|degraded)'
      else
        not_run "$id" "deployment" \
          "LIVE_DEPLOYMENT=1 DEPLOYMENT_BASE_URL=<redacted> cargo test -p sniper-suite --test deployment_smoke" \
          "read-only /healthz + /api/openapi.json probe of the deployed environment"
      fi ;;
    staking_devnet_e2e)
      if [ "${LIVE_STAKING:-0}" = "1" ]; then
        attempt "$id" "staking_validator" "1" \
          'cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1 --nocapture' \
          -- 'e2e_tx=[1-9A-HJ-NP-Za-km-z]{32,}'
      else
        not_run "$id" "staking_validator" \
          "cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1" \
          "devnet validator boot + stake/claim/withdraw; attestation = e2e tx signature; requires solana-test-validator"
      fi ;;
    latency_report)
      if [ "${LIVE_LATENCY:-0}" = "1" ] && [ -n "${RPC_URL:-}" ]; then
        attempt "$id" "solana_rpc" "1" \
          'LIVE_LATENCY=1 cargo test -p sniper-suite --test live_latency_probe -- --ignored --nocapture' \
          -- 'p50_ms=[0-9]+'
      else
        not_run "$id" "solana_rpc" \
          "LIVE_LATENCY=1 RPC_URL=<redacted> cargo test -p sniper-suite --test live_latency_probe -- --ignored" \
          "getSlot/getLatestBlockhash round-trip percentiles; no latency claim may be published without a PASSED run"
      fi ;;
    *)
      echo "unknown category: $id" >&2
      echo "categories: solana_funded_preflight pumpfun_buy_sell_roundtrip pumpswap_buy_sell_roundtrip polymarket_order_roundtrip stripe_checkout_roundtrip kms_sign_transit vault_transit deployment_smoke staking_devnet_e2e latency_report" >&2
      exit 1 ;;
  esac
}

ALL="solana_funded_preflight pumpfun_buy_sell_roundtrip pumpswap_buy_sell_roundtrip polymarket_order_roundtrip stripe_checkout_roundtrip kms_sign_transit vault_transit deployment_smoke staking_devnet_e2e latency_report"

echo "[run-live-validation] evidence dir: $EVIDENCE_DIR"
echo "[run-live-validation] rule: PASSED requires a captured attestation (tx signature / provider id); otherwise NOT_RUN or FAILED"
if [ "${1:-all}" = "all" ]; then
  for c in $ALL; do run_category "$c"; done
else
  run_category "$1"
fi

echo "[run-live-validation] summary:"
for f in "$EVIDENCE_DIR"/*.json; do
  [ -e "$f" ] || continue
  grep -o '"status": "[A-Z_]*"' "$f" | head -1 | sed "s|^|  $(basename "$f"): |"
done
echo "[run-live-validation] done — no result was invented; unrun categories are NOT_RUN"
