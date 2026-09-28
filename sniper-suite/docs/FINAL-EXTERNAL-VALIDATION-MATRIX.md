# Final External Validation Matrix — sniper-suite 0.1.0

> 2026-09-26 · `all-safe` mode — never funded trade, never private keys. Every result `validation_id/gap_id/provider/environment/timestamp/command/mode/status/evidence_hash/redacted_metadata/endpoint_ref` (Batch 10: canonical hash, timestamp excluded — see §below).

| System | Harness | Live Status | Required Buyer Action |
|--------|---------|-------------|-----------------------|
| **Stripe/Paddle** | **READY** `crates/server/src/ops/provider_contract.rs`, `provider_contract_runner.rs`, `tests/live_billing_contract.rs` | **NOT_RUN** `EXTERNAL_REQUIRED` | Set `LIVE_BILLING=1` + `STRIPE_API_KEY=sk_live_...` / `PADDLE_API_KEY` + provider account → `LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored` + `stripe events trigger payment_intent.succeeded` against `/api/saas/billing/webhooks/stripe` |
| **Vault/KMS/HSM** | **READY (boundary only)** `crates/server/src/custody/live_provider_*.rs`, `tests/live_custody_contract.rs` | **NOT_RUN** `EXTERNAL_REQUIRED` | Deploy Vault dev / KMS / HSM, set `VAULT_ADDR=https://...` + `VAULT_TOKEN` / `KMS_KEY_ID` + `LIVE_CUSTODY=1` + `CREDENTIAL_REF=env_var:VAULT_TOKEN` → `LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored` (health `unavailable` without, `is_signing_allowed` false). **Remote signing backends are not implemented in this build** — selecting `vault`/`kms`/`hsm` fails startup with `SignerError::UnsupportedBackend` (no silent local fallback) |
| **Deployment** | **READY** `crates/server/src/ops/deployment_smoke.rs`, `tests/deployment_smoke.rs` | **NOT_RUN** | Provision host + TLS + Postgres16+Redis7 + env per `docs/BUYER-DEPLOYMENT.md` → `DEPLOYMENT_BASE_URL=https://your-host cargo test --test deployment_smoke -- --nocapture`. A local `docker run` + `curl localhost:8080/api/health` is LOCAL CONTAINER SMOKE only — never production verification (and `localhost` cannot be substituted, guarded by a test) |
| **Solana RPC/WS/Geyser** | **READY** `crates/server/src/solana/connection_contract.rs`, `geyser_contract.rs` | **NOT_RUN** | Set `RPC_URL=https://...` / `WS_URL=wss://...` / `GEYSER_URL` → `cargo test --test solana_contract -- --ignored` (hermetic `without_url is EXTERNAL_REQUIRED`) |
| **Staking** | **READY / BLOCKED** `programs/staking-suite/tests/validator_e2e.rs`, `crates/server/src/staking/deployment_contract.rs` | **NOT_RUN / BLOCKED** | Install `agave 2.1.21` + `solana-test-validator`, `cargo build-sbf`, `set-id` with buyer keypair (placeholder `3vEEMM…` BLOCKED), then `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` (excluded workspace — the `cd` is required) |
| **Funded trading** | **GUARD READY** `crates/core/src/execution.rs` `ExecutionMode::Live` + `allow_live_trading` gate; guard proven by `cargo test -p sniper-suite --lib funded_mode_guard` | **NOT_RUN** | Funded wallet + `EXECUTION_MODE=live` + `execution.allow_live_trading=true` + owner approval → **operator action**, default `dry_run` never auto-trades (`live_unfunded` denied by the guard; testnet/devnet success is never funded-live) |
| **External audit** | **N/A** | **NOT DONE** | Commission independent audit firm → handover slot `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006 (report/findings/remediation/retest/sign-off paths; no command). Internal `cargo audit`/`deny` best-effort only, `docs/SECURITY.md` states NOT DONE |

**No item may say VERIFIED without current real evidence.** `bash scripts/run-external-validation.sh all-safe`
→ **6/6 NOT_RUN** in a hermetic environment.

Evidence records live in `evidence/external/*.json` with
`validation_id/gap_id/provider/environment/timestamp/command/mode/status/evidence_hash/redacted_metadata/endpoint_ref`.
The `evidence_hash` is canonical and deterministic — SHA256 of the compact, sorted-key JSON of
`{command, endpoint_ref, environment, gap_id, mode, provider, redacted_metadata, status, validation_id}`
with the **timestamp excluded** — so re-running the same tree/command reproduces the same hash
(only the informational `timestamp` field changes). Canonical hashes at 2026-09-27 (regenerate +
verify yourself; `cargo test --test provider_contracts` fails if any file was edited by hand):

| evidence file | gap | canonical `evidence_hash` |
|---|---|---|
| `billing_stripe.json` | GAP-001 | `ca63580f2e065d67a4cfc3637b930be461506a679d48cf3b8c50268f98ddfbfd` |
| `custody_vault.json` | GAP-002 | `6ecc12b58e00bf745ba18f91d035ec0a86b1acee043a09187faa398ec7e9af54` |
| `deployment_deployment.json` | GAP-003 | `d85a36c3bcaa38afe47b19975adb3149a837cae9ac8dece938fd642c1e4e4557` |
| `solana_solana_rpc.json` | n/a | `d4d71554761e08304574e03319356ed3ce8f292bbc89ab2711304d688800c4cb` |
| `staking_staking_validator.json` | GAP-005 | `3d7747f535783351e6289d93f692f1111e487ad73d5e449ea9e8c85e403c978d` |
| `funded-preflight_funded.json` | GAP-004 | `f766aed547e9ece7e8174a73a822f43658ef7946740e3fa717c03e378fa112ed` |

All redacted, no `DATABASE_URL`/`REDIS_URL`/keys/secrets, no funded trade placed.
(The pre-Batch-10 files hashed the timestamp, so their quoted hashes changed every run — that is
why this table now pins the canonical rule instead.)

To verify live after provisioning, see `docs/EXTERNAL-VALIDATION-RUNBOOK.md` §7 sections and run the verification command in this table column.
