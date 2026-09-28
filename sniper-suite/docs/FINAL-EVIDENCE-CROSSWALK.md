# Final Evidence Crosswalk — sniper-suite 0.1.0

> Every buyer-facing claim must have an evidence reference. Orphaned claims or orphaned implementation are flagged here.

| Buyer Requirement | Source File | Implementation | Test | Document | Current Status |
|---|---|---|---|---|---|
| **Tenant isolation** | `crates/server/src/saas/middleware.rs`, `store.rs` | org→membership→permission→lifecycle at app+query | `crates/core/tests/saas_control_plane.rs` 19, `postgres_saas` | `docs/SECURITY-THREAT-MODEL.md` §1, `ARCHITECTURE-OVERVIEW.md` §5 | PASS (hermetic), VERIFIED (PG 17.11, 2026-09-26) |
| **Auth/session** | `saas/users.rs`, `core/session.rs` `hash_token` | hash, revocation | `saas_control_plane` | `SECURITY-CONTROLS-MATRIX.md` | PASS |
| **API keys tenant-scoped** | `saas/api_keys.rs` show once + hash | `hash_token` | 3 tests + `saas-sdk` 7 | `SECRETS-MANAGEMENT-MATRIX.md` | PASS |
| **WebSocket auth** | `security/websocket.rs`, `saas/websocket_auth.rs` | tenant-scoped, legacy guard disabled | 7 tests | `API-COMPATIBILITY-MATRIX.md` | PASS |
| **Billing price authority** | `core/billing/pricing.rs` immutable | server owns price | 7 + `saas-sdk` 2 | `SECURITY-THREAT-MODEL.md` §5 | PASS |
| **Billing webhook HMAC** | `saas/billing_webhook.rs` | HMAC-SHA256 verify, idempotency | 7 + 3 | `WEBHOOK-COMPATIBILITY-MATRIX.md` | PASS (fixture), EXTERNAL_REQUIRED (live) |
| **Billing reconciliation** | `core/billing/reconciliation.rs` | never invent success | 11 | `SECURITY-THREAT-MODEL.md` | PASS |
| **Billing live** | `billing/provider_config.rs` refs only | `StripeRef` no secrets | 8 (contains check) | `FINAL-BUYER-GAP-LEDGER.md` GAP-001 | EXTERNAL_REQUIRED |
| **Custody indirect refs** | `core/custody/credentials.rs` | VaultRef/KmsRef | 7 | `SECRETS-MANAGEMENT-MATRIX.md` | PASS |
| **Custody fail-closed** | `core/custody/provider_config.rs` `local_fallback_allowed=false` | `build_signer_registry` fails startup | 7 | `SECURITY-THREAT-MODEL.md` §6 | PASS |
| **Custody rotation** | `core/custody/rotation.rs` | safe old-not-revoked | 7 | `PRODUCTION-READINESS-MATRIX.md` | PASS |
| **Custody live** | `custody/provider_config.rs` | refs only | — | `GAP-002` | EXTERNAL_REQUIRED |
| **Lifecycle / retention** | `saas/data_lifecycle.rs`, `provisioning/retention_worker.rs` | purge after retention | 7 | `OPERATIONS-RUNBOOK.md` §10 | PASS |
| **Job leasing** | `provisioning/job_claim.rs` SKIP LOCKED | atomic claim | 5 | `SECURITY-THREAT-MODEL.md` §7 | PASS (PG) |
| **OpenAPI** | `saas/openapi.rs`, `api/openapi_*.rs` | `GET /api/saas/openapi.json` | 3+3 | `API-COMPATIBILITY-MATRIX.md` | PASS |
| **SDK typed** | `crates/saas-sdk/src/` | secret-free Debug, no secrets in URLs | 32 | `ARCHITECTURE-OVERVIEW.md` §7 | PASS |
| **Frontend lockfile** | `apps/control-plane/package-lock.json` 6171 lines v3 | real, deterministic | `npm ci` | `PRODUCTION-READINESS-MATRIX.md` | PASS |
| **Frontend build** | `apps/control-plane/src/app` | Next.js 16, 5 routes | `npm run typecheck/build` | `DEPLOYMENT-ENVIRONMENT-MATRIX.md` | PASS |
| **Observability** | `ops/observability_config.rs`, `metrics_snapshot.rs`, `trace_context.rs` | real counters only, redacted | 6+4+5 | `SECURITY-CONTROLS-MATRIX.md` | PASS |
| **Rate limits** | `bot_core::auth::RateLimiter` + `ops/rate_limit_report.rs` | thresholds only | 4 | `SECURITY-CONTROLS-MATRIX.md` | PASS |
| **Backup/restore** | `backup/{export,restore}_manifest.rs`, `preflight.rs`, `commands.rs` | DOCUMENTED→VERIFIED, safe pg_dump | 3+3+3+4 | `OPERATIONS-RUNBOOK.md` §11 | PASS (hermetic), EXTERNAL_REQUIRED (real dump) |
| **Migrations** | `crates/core/migrations/0001–0022` | forward-only 22 | `verify-delivery` 22 | `PRODUCTION-READINESS-MATRIX.md` | PASS |
| **Health/ready** | `obs.rs`, `ops/health_report.rs`, `ops/container_metadata.rs` | redacted | 4 | `OPERATIONS-RUNBOOK.md` §4 | PASS |
| **CI matrices** | `.github/workflows/ci.yml` + `frontend-ci.yml` | PG/Redis/frontend/Rust/release/external-gated jobs | `cargo check` | `DEPLOYMENT-ENVIRONMENT-MATRIX.md` | PASS |
| **SBOM/license** | `sbom.json` 200 comps, `licenses.json` 707 | per-artifact sha | `sbom_report` 4, `license_report` 5 | `THIRD-PARTY-SOFTWARE-INVENTORY.md` | PASS |
| **IP ownership** | `LICENSE` MIT, `docs/IP-OWNERSHIP-REGISTER.md` | generic holder → LEGAL_REVIEW | — | `OPEN-SOURCE-COMPLIANCE.md` | PARTIAL (LEGAL_REVIEW_REQUIRED) |
| **Trademark/domain** | — | NOT INCLUDED | — | `TRADEMARK-DOMAIN-REGISTER.md` | NOT INCLUDED |
| **Production deploy** | `Dockerfile`, `docker-compose.yml` | smoke `curl /api/health` | `docker` CI | `GAP-003` | EXTERNAL_REQUIRED |
| **Funded trading** | `EXECUTION_MODE=dry_run` | paper default | — | `GAP-004` | EXTERNAL_REQUIRED |
| **Staking E2E** | `programs/staking-suite/tests/validator_e2e.rs` | host 71/71, E2E gated | 3 gated | `GAP-005` | EXTERNAL_REQUIRED |
| **External audit** | — | `cargo audit`/`deny` internal only | — | `GAP-006` | NOT_EXECUTED |
| **Orphaned claim?** | Check `docs/FINAL-BUYER-GAP-LEDGER.md` (6 gaps) vs `external_validation.rs` 11 entries | Both map 1:1 (stripe/paddle→GAP-001 etc.) | — | `DATA-ROOM-INDEX.md` | No orphaned claim |
| **Orphaned impl?** | `ops/*` 41 files + `backup/*` 5 all referenced in `release-manifest.json` `saas_modules_batch5_25` + this crosswalk | — | — | `FINAL-BUYER-DATA-ROOM.md` | No orphaned impl |

> **Counts:** 343 rs, 22 migrations, 70→101 docs, 8 members, 1331 tests, version 0.1.0 — verified via `find`, `grep`, `release-manifest.json`. Every major claim has a file+test+doc reference; live/provider claims are correctly `EXTERNAL_REQUIRED`/`NOT_EXECUTED`.

*Stale check:* `bash scripts/verify-delivery.sh` (7/7 after hygiene fix), `bash scripts/final-release-check.sh` stale `manifest counts ok`.
