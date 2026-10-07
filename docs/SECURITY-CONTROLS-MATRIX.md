# Security Controls Matrix — sniper-suite 0.1.0

> Status taxonomy: **PASS** (verified here) · **PARTIAL** (implemented, limited) · **NOT_EXECUTED** (needs service) · **EXTERNAL_REQUIRED** (needs buyer/external)

| Control | Implementation | Test | Verification Command | Status |
|---|---|---|---|---|
| Tenant isolation (app+query) | `crates/server/src/saas/middleware.rs` `authorize_request`, `store.rs` tenant-scoped queries | `crates/core/tests/saas_control_plane.rs` (19), `crates/server/tests/tenant_lifecycle_integration.rs` | `cargo test --workspace -- --test-threads=1` (hermetic), `POSTGRES_URL=... cargo test --test tenant_lifecycle_integration` | PASS (hermetic), NOT_EXECUTED (live PG) |
| Authentication + session | `saas/users.rs` `hash_token`, `store.rs` revocation, frontend tab-memory | `saas_control_plane` + `store.rs` restart test | `cargo test -p bot-core --test saas_control_plane -- --nocapture` | PASS |
| API key secret-free | `saas/api_keys.rs` show once + hash, `saas-sdk/src/client.rs` secret-free Debug | `api_keys` 3 tests, `saas-sdk` 7 tests | `cargo test -p saas-sdk` (32/32) | PASS |
| Rate limiting (IP+principal) | `bot_core::auth::RateLimiter`, `api.rs` `ip_rate_limit` + `require_role` | `ops/rate_limit_report.rs` 4 tests | `cargo test -p sniper-suite --test backup_restore_integration` (report) | PASS |
| WebSocket auth (tenant-scoped) | `security/websocket.rs`, `saas/websocket_auth.rs` (header/first-frame), `legacy_websocket_guard.rs` | `websocket_auth` 7 tests | `cargo test --workspace` (ws) | PASS |
| Billing price authority | `core/billing/pricing.rs` immutable | `pricing.rs` 7, `saas-sdk billing` 2 | `cargo test -p bot-core` | PASS |
| Billing webhook verify | `saas/billing_webhook.rs` HMAC | `provider_events` 7 | `cargo test -p bot-core` | PASS |
| Billing reconciliation | `core/billing/reconciliation.rs` never invent success | 11 tests | `cargo test -p bot-core` | PASS |
| Custody indirect refs | `core/custody/credentials.rs` VaultRef/KmsRef | 7 tests | `cargo test -p bot-core` | PASS |
| Custody fail-closed | `core/custody/provider_config.rs` `local_fallback_allowed=false` | 7 tests | `cargo test -p bot-core` | PASS |
| Custody rotation | `core/custody/rotation.rs` safe old-not-revoked | 7 tests | `cargo test -p bot-core` | PASS |
| Dedup / idempotency | `bot_core::dedup::DedupStore` (Redis/Postgres/Memory) | `redis_integration` dedup | `REDIS_URL=... cargo test --test redis_integration` | PASS (hermetic), NOT_EXECUTED (live Redis) |
| Lease fencing (SKIP LOCKED) | `provisioning/job_claim.rs` | 5 tests | `POSTGRES_URL=... cargo test --test tenant_lifecycle` | PASS/PARTIAL (needs PG) |
| Security headers | `security/headers.rs` (CSP,XCTO,HSTS…) | `security_headers` 3 tests | `cargo test -p sniper-suite` | PASS |
| CORS fail-closed | `security/cors_policy.rs` | `cors_policy` | `cargo test -p sniper-suite` | PASS |
| Audit trail hash chain | `bot_core::audit::AuditTrail`, `ops/audit_attestation.rs` HMAC | `db_integration` audit-chain | `POSTGRES_URL=... cargo test --test db_integration` | PASS (memory), NOT_EXECUTED (live PG chain) |
| Secret redaction | `is_secret_like`, `redacted`, `saas/ops` | `security_evidence` 5, `saas-sdk` 7 | `bash scripts/verify-buyer-package.sh` | PASS |
| Health redaction | `ops/health_report.rs` redacts postgres://, sk_live | 4 tests | `cargo test -p sniper-suite` | PASS |
| Observability safe | `ops/observability_config.rs` rejects trace+creds, `trace_context.rs` redacted | 6+5 tests | `cargo test --test observability_config` | PASS |
| Backup/restore strict | `backup/{export,restore}_manifest.rs` DOCUMENTED→VERIFIED | 3+3 tests | `cargo test --test backup_restore_integration` | PASS |
| SBOM / license | `sbom.json` + `licenses.json` generated, per-artifact sha | `sbom_report` 4, `license_report` 5 | `bash scripts/generate-sbom.sh && sha256sum sbom.json` | PASS |
| Live Stripe/Paddle | `billing/provider_config.rs` refs only, no secrets | — | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored` | EXTERNAL_REQUIRED (NOT_EXECUTED) |
| Live Vault/KMS | REAL adapters `crates/server/src/custody/{vault,kms}/` (transit REST wire; SigV4 vs AWS test vector) + `custody/provider_config.rs` refs | unit tests in `custody::vault` / `custody::kms` | `LIVE_CUSTODY=1 VAULT_ADDR=... cargo test --test live_custody_contract -- --ignored` | EXTERNAL_REQUIRED (boundary PASS; live round-trip NOT_EXECUTED) |
| HSM custody | fail-closed refusal naming the PKCS#11 dependency | refusal tests in `custody` suites | — | NOT IMPLEMENTED (by design, fail-closed) |
| Staking validator E2E | `programs/staking-suite/tests/validator_e2e.rs` | 3 tests gated | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | EXTERNAL_REQUIRED (unit 71/71 PASS) |
| Prod deployment | `Dockerfile`, `docker-compose.yml` | `docker build` smoke is LOCAL only | `DEPLOYMENT_BASE_URL=... cargo test --test deployment_smoke` (local `docker run` + `curl localhost` is never production verification) | EXTERNAL_REQUIRED |
| Funded trading | `EXECUTION_MODE=dry_run` default | `funded_mode_guard` | `cargo test -p sniper-suite --lib funded_mode_guard` | EXTERNAL_REQUIRED (guard PASS; funded step operator-only) |
| External audit | — | — | n/a — external auditor deliverable (`docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006) | NOT_EXECUTED (internal cargo audit/deny only) |

> **Counts (2026-10-06):** Current repo: 616 Rust files under `crates/`, 43 forward-only migrations through `0043`, 149 docs, 1783 `#[test]` attributes, 8 workspace members, version 0.1.0. See `release-manifest.json` and `docs/FINAL-BUYER-GAP-LEDGER.md`.
