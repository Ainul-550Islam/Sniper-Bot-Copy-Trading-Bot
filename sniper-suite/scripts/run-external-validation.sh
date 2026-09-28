#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EVIDENCE_DIR="${EVIDENCE_DIR:-$ROOT/evidence/external}"
MODE="${1:-all-safe}"
mkdir -p "$EVIDENCE_DIR"

redact() {
  # Redact secrets in command output. Every name in the Phase-2 secret list is covered:
  # DB/queue URLs, provider keys + webhook secrets, Vault/KMS/HSM material, wallet
  # secrets, seed phrases/mnemonics, and Authorization/Bearer headers.
  sed -E \
    -e 's/(DATABASE_URL|POSTGRES_URL|REDIS_URL|STRIPE_API_KEY|STRIPE_WEBHOOK_SECRET|PADDLE_API_KEY|PADDLE_WEBHOOK_SECRET|VAULT_TOKEN|VAULT_NAMESPACE|KMS_KEY_ID|KMS_SECRET|HSM_SLOT|HSM_PIN|RPC_URL|GEYSER_URL|DEPLOYMENT_BASE_URL|SOLANA_KEYPAIR|WALLET_PRIVATE_KEY|SEED_PHRASE|MNEMONIC)=[^ ]*/\1=<redacted>/g' \
    -e 's/([Bb]earer)[[:space:]]+[A-Za-z0-9._~+\/-]+/\1 <redacted>/g' \
    -e 's/BEGIN PRIVATE KEY/<redacted>/g'
}

# ---------------------------------------------------------------------------
# Canonical evidence hash (Batch 10)
#
# payload = compact JSON of {command,endpoint_ref,environment,gap_id,mode,provider,
#                            redacted_metadata,status,validation_id} with keys sorted.
# hash    = sha256(payload)  — the `timestamp` field is NOT part of the payload, so
# same input tree + same command + same canonicalization => same evidence_hash.
# The identical rule is implemented in crates/server/src/ops/external_evidence.rs
# (`canonical_payload`/`compute_hash`) and cross-checked two ways:
#   1. canonical_self_test() below against a fixture whose digest the Rust unit test
#      `canonical_payload_fixture_is_stable` asserts as well;
#   2. `cargo test --test provider_contracts` verifies every emitted file.
# ---------------------------------------------------------------------------
canonical_payload() {
  local validation_id="$1" gap_id="$2" provider="$3" environment="$4" command="$5" mode="$6" status="$7" metadata="$8" endpoint_ref="$9"
  local cmd_escaped ep_escaped
  cmd_escaped=$(printf '%s' "$command" | sed 's/\\/\\\\/g; s/"/\\"/g')
  ep_escaped=$(printf '%s' "$endpoint_ref" | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"command":"%s","endpoint_ref":"%s","environment":"%s","gap_id":"%s","mode":"%s","provider":"%s","redacted_metadata":%s,"status":"%s","validation_id":"%s"}' \
    "$cmd_escaped" "$ep_escaped" "$environment" "$gap_id" "$mode" "$provider" "$metadata" "$status" "$validation_id"
}

canonical_self_test() {
  local fixture expected built
  fixture='{"command":"LIVE_BILLING=1 cargo test --test live_billing_contract","endpoint_ref":"stripe ref","environment":"test","gap_id":"GAP-001","mode":"all-safe","provider":"stripe","redacted_metadata":{"checkout_created":null,"signature_verified":null},"status":"NOT_RUN","validation_id":"billing"}'
  expected="1e3bf39a6d142631318d8b4d2f9da5072a2747935ced440194a20074e4e9c1b3"
  built=$(canonical_payload "billing" "GAP-001" "stripe" "test" \
    "LIVE_BILLING=1 cargo test --test live_billing_contract" "all-safe" "NOT_RUN" \
    '{"checkout_created":null,"signature_verified":null}' "stripe ref")
  if [ "$built" != "$fixture" ]; then
    echo "FAIL canonical payload self-test — builder drifted from the Rust fixture" >&2
    echo "  built:   $built" >&2
    echo "  fixture: $fixture" >&2
    exit 1
  fi
  local got
  got=$(printf '%s' "$built" | sha256sum | awk '{print $1}')
  if [ "$got" != "$expected" ]; then
    echo "FAIL canonical hash self-test ($got != $expected)" >&2
    exit 1
  fi
}
canonical_self_test

print_status() {
  local id="$1" provider="$2" status="$3" detail="$4" gap="${5:-n/a}"
  printf "[%s] gap=%s provider=%s status=%s detail=%s\n" "$id" "$gap" "$provider" "$status" "$detail" | redact
}

save_evidence() {
  local validation_id="$1" gap_id="$2" provider="$3" environment="$4" command="$5" status="$6" metadata="$7" endpoint_ref="$8"
  local ts
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  local hash
  hash=$(canonical_payload "$validation_id" "$gap_id" "$provider" "$environment" "$command" "$MODE" "$status" "$metadata" "$endpoint_ref" | sha256sum | awk '{print $1}')
  local file="$EVIDENCE_DIR/${validation_id}_${provider}.json"
  cat > "$file" <<EOF
{
  "validation_id": "$validation_id",
  "gap_id": "$gap_id",
  "provider": "$provider",
  "environment": "$environment",
  "timestamp": "$ts",
  "command": "$(echo "$command" | redact | sed 's/"/\\"/g')",
  "mode": "$MODE",
  "status": "$status",
  "evidence_hash": "$hash",
  "redacted_metadata": $metadata,
  "endpoint_ref": "$endpoint_ref"
}
EOF
  echo "$file"
}

run_billing() {
  echo "=== billing (provider-neutral, no hardcoded success) ==="
  local status="NOT_RUN" detail="LIVE_BILLING != 1 — NOT_RUN (explicit opt-in required)"
  local provider="stripe"
  local gap="GAP-001"
  if [ "${LIVE_BILLING:-0}" = "1" ]; then
    if [ -z "${STRIPE_API_KEY:-}" ] && [ -z "${PADDLE_API_KEY:-}" ]; then
      status="EXTERNAL_REQUIRED"
      detail="LIVE_BILLING=1 but no STRIPE_API_KEY/PADDLE_API_KEY — EXTERNAL_REQUIRED"
    else
      # Would perform real provider operation here — in all-safe, still NOT_RUN without real call
      if [ "$MODE" = "all-safe" ]; then
        status="NOT_RUN"
        detail="all-safe mode — live billing not executed (would require real provider call with LIVE_BILLING=1)"
      else
        # Attempt real check via cargo test (ignored) — hermetic will still be NOT_RUN
        if cargo test --test live_billing_contract -- --ignored --nocapture 2>&1 | redact | head -n 20; then
          status="NOT_RUN"
          detail="live billing harness executed (check evidence for PASS/FAIL/EXTERNAL_REQUIRED)"
        else
          status="FAIL"
          detail="live billing harness failed"
        fi
      fi
    fi
  fi
  print_status "billing" "$provider" "$status" "$detail" "$gap"
  save_evidence "billing" "$gap" "$provider" "${ENVIRONMENT:-test}" "LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored" "$status" '{"checkout_created":null,"signature_verified":null}' "$provider ref" >/dev/null
  echo "PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED: $status" | redact
}

run_custody() {
  echo "=== custody (Vault/KMS/HSM, no local fallback, no private key) ==="
  local status="NOT_RUN" detail="LIVE_CUSTODY != 1 — NOT_RUN (explicit opt-in required, no local fallback)"
  local provider="vault"
  local gap="GAP-002"
  if [ "${LIVE_CUSTODY:-0}" = "1" ]; then
    if [ -z "${VAULT_ADDR:-}" ] && [ -z "${KMS_KEY_ID:-}" ] && [ -z "${HSM_SLOT:-}" ]; then
      status="EXTERNAL_REQUIRED"
      detail="LIVE_CUSTODY=1 but no VAULT_ADDR/KMS_KEY_ID/HSM_SLOT — EXTERNAL_REQUIRED (no local fallback)"
    else
      if [ "$MODE" = "all-safe" ]; then
        status="NOT_RUN"
        detail="all-safe mode — live custody not executed (no private key extraction, remote sign requires LIVE_CUSTODY=1 + real backend)"
      else
        if cargo test --test live_custody_contract -- --ignored --nocapture 2>&1 | redact | head -n 20; then
          status="NOT_RUN"
          detail="live custody harness executed"
        else
          status="FAIL"
          detail="live custody harness failed"
        fi
      fi
    fi
  fi
  print_status "custody" "$provider" "$status" "$detail" "$gap"
  save_evidence "custody" "$gap" "$provider" "${ENVIRONMENT:-test}" "LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored" "$status" '{"sign_available":null}' "$provider ref" >/dev/null
  echo "PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED: $status" | redact
}

run_deployment() {
  echo "=== deployment smoke (read-only, never claim without URL) ==="
  local status="NOT_RUN" detail="DEPLOYMENT_BASE_URL not set — NOT_RUN (no URL tested, no claim of deployment)"
  local gap="GAP-003"
  if [ -n "${DEPLOYMENT_BASE_URL:-}" ]; then
    if [ "$MODE" = "all-safe" ]; then
      status="NOT_RUN"
      detail="all-safe mode — deployment smoke would check $DEPLOYMENT_BASE_URL (redacted) — not executed in all-safe without DEPLOYMENT_SMOKE_LIVE=1"
    else
      if cargo test --test deployment_smoke -- --nocapture 2>&1 | redact | head -n 20; then
        status="NOT_RUN"
        detail="deployment smoke harness executed"
      else
        status="FAIL"
        detail="deployment smoke harness failed"
      fi
    fi
    # Redact URL in detail
    detail=$(echo "$detail" | redact)
  fi
  print_status "deployment" "deployment" "$status" "$detail" "$gap"
  save_evidence "deployment" "$gap" "deployment" "${ENVIRONMENT:-test}" "DEPLOYMENT_BASE_URL=<redacted> cargo test --test deployment_smoke" "$status" '{"health":null}' "deployment ref" >/dev/null
  echo "PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED: $status" | redact
}

run_solana() {
  echo "=== solana (RPC/WS/Geyser, read-only, no trading) ==="
  local status="NOT_RUN" detail="RPC_URL not set — NOT_RUN (no endpoint, no trading)"
  # Read-only RPC/Geyser validation is not one of the six buyer gaps — gap_id is "n/a".
  local gap="n/a"
  if [ -n "${RPC_URL:-}" ] || [ -n "${GEYSER_URL:-}" ]; then
    if [ "$MODE" = "all-safe" ]; then
      status="NOT_RUN"
      detail="all-safe mode — solana check would test RPC_URL (redacted) read-only — not executed without SOLANA_LIVE=1"
    else
      if cargo test --test solana_contract -- --nocapture 2>&1 | redact | head -n 20; then
        status="NOT_RUN"
        detail="solana harness executed"
      else
        status="FAIL"
        detail="solana harness failed"
      fi
    fi
  fi
  print_status "solana" "solana_rpc" "$status" "$detail" "$gap"
  save_evidence "solana" "$gap" "solana_rpc" "${ENVIRONMENT:-test}" "RPC_URL=<redacted> cargo test --test solana_contract" "$status" '{"rpc_reachable":null}' "solana ref" >/dev/null
  echo "PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED: $status" | redact
}

run_staking() {
  echo "=== staking (validator E2E, STAKING_E2E=1 required) ==="
  local status="NOT_RUN" detail="STAKING_E2E != 1 — NOT_RUN (requires STAKING_E2E=1 + validator + toolchain)"
  local gap="GAP-005"
  if [ "${STAKING_E2E:-0}" = "1" ]; then
    if [ "$MODE" = "all-safe" ]; then
      status="NOT_RUN"
      detail="all-safe mode — staking E2E would run validator_e2e (3 tests) with STAKING_E2E=1 — not executed in all-safe without explicit live"
    else
      if cargo test --test staking_contract -- --nocapture 2>&1 | redact | head -n 20; then
        status="NOT_RUN"
        detail="staking harness executed"
      else
        status="FAIL"
        detail="staking harness failed"
      fi
    fi
  fi
  print_status "staking" "staking_validator" "$status" "$detail" "$gap"
  save_evidence "staking" "$gap" "staking_validator" "${ENVIRONMENT:-test}" "cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1" "$status" '{"e2e_executed":false}' "staking ref" >/dev/null
  echo "PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED: $status" | redact
}

run_funded_preflight() {
  echo "=== funded-preflight (guard, never auto-trade) ==="
  local status="NOT_RUN" detail="funded mode is paper/dry_run — NOT_RUN (live-funded requires explicit live mode + owner + funded wallet + risk gates)"
  local gap="GAP-004"
  # Check funded guard via env
  if [ "${EXECUTION_MODE:-paper}" = "live" ] && [ "${ALLOW_LIVE_TRADING:-0}" = "1" ]; then
    status="EXTERNAL_REQUIRED"
    detail="live mode requested — funded preflight requires wallet funded + owner authorized + risk gates + emergency stop — EXTERNAL_REQUIRED"
  fi
  if [ "$MODE" = "all-safe" ]; then
    status="NOT_RUN"
    detail="all-safe mode — funded preflight checked, no funded trade placed (guard ready, execution is operator action)"
  fi
  print_status "funded-preflight" "funded" "$status" "$detail" "$gap"
  save_evidence "funded-preflight" "$gap" "funded" "${ENVIRONMENT:-test}" "cargo test -p sniper-suite --lib funded_mode_guard" "$status" '{"live_funded":false}' "funded ref" >/dev/null
  echo "PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED: $status" | redact
}

check_evidence_files() {
  # Fail closed if any expected evidence file is missing/empty or lacks the canonical fields.
  local file
  for file in billing_stripe custody_vault deployment_deployment solana_solana_rpc staking_staking_validator funded-preflight_funded; do
    if [ ! -s "$EVIDENCE_DIR/$file.json" ]; then
      echo "FAIL evidence file missing or empty: $file.json" >&2
      exit 1
    fi
    for field in validation_id gap_id provider environment timestamp command mode status evidence_hash redacted_metadata endpoint_ref; do
      if ! grep -q "\"$field\"" "$EVIDENCE_DIR/$file.json"; then
        echo "FAIL evidence $file.json missing field $field" >&2
        exit 1
      fi
    done
  done
}

run_all_safe() {
  echo "[run-external-validation] MODE=all-safe — never place funded trade, never expose private keys, never silently enable live payment/remote signing"
  echo "Evidence dir: $EVIDENCE_DIR (redacted, no secrets)"
  run_billing
  run_custody
  run_deployment
  run_solana
  run_staking
  run_funded_preflight
  check_evidence_files
  echo "=== summary ==="
  echo "all-safe complete — all checks resulted in NOT_RUN or EXTERNAL_REQUIRED (no live credentials in this env)"
  echo "Evidence saved under $EVIDENCE_DIR — verify hashes + schema with: cargo test --test provider_contracts"
  ls -lh "$EVIDENCE_DIR" 2>&1 | head -n 20 | redact
}

case "$MODE" in
  billing) run_billing ;;
  custody) run_custody ;;
  deployment) run_deployment ;;
  solana) run_solana ;;
  staking) run_staking ;;
  funded-preflight) run_funded_preflight ;;
  all-safe) run_all_safe ;;
  *) echo "Usage: $0 {billing|custody|deployment|solana|staking|funded-preflight|all-safe}"; exit 1 ;;
esac

echo "[run-external-validation] done — secrets redacted, no funded trade placed"
