# Release Notes — 0.1.0 (2026-09-24)

## Summary

- **Version:** 0.1.0 — first transaction-readiness cut (no prior tag; `CHANGELOG.md` Batch1–Batch6 chronicle)
- **Scope:** Source + docs + migrations + scripts + staking program (host-verified), no hosted deployment, no funded trading, no external audit

## Batch 7 (2026-09-24) — External Validation Harness

- **Added** new Rust sources (ops / saas / provisioning server modules, new server tests, lib.rs wiring), `EXTERNAL-VALIDATION-RUNBOOK.md` + wiring, new tests, `scripts/run-external-validation.sh` 9.3K (7 modes), `docs/EXTERNAL-VALIDATION-RUNBOOK.md` 13K, harness READY / live NOT_RUN, `cargo clippy --workspace --all-targets -- -D warnings` 0 with targeted `allow(dead_code)` only (no blanket)
- **Counts at Batch 7:** historical counts removed — live inventory in `docs/STATS.md` (generated); version 0.1.0 everywhere.

## Batch 6 (2026-09-24) — Transaction-Readiness / Due-Diligence

- **Fixed** `scripts/verify-delivery.sh` hygiene gate brought to full pass: `target/` (and `**/target`, `build/`, `programs/staking-suite/target/`) no longer counted as dirty contamination; build output is generated, `.gitignore`'d (`/target`, `**/target`), explicitly excluded from `buyer-release` via `scripts/build-release-package.sh --exclude target/ --exclude **/target/`, secret/forbidden checks (`BEGIN PRIVATE KEY`, `*keypair*.json`/`.pem`) unchanged and still hard FAIL; verified with a dummy `target/debug/` present and clean without, `buyer-release/source/target` absent.
- **Added** new docs: `DATA-ROOM-INDEX.md`, `ARCHITECTURE-OVERVIEW.md`, `SECURITY-THREAT-MODEL.md`, `SECURITY-CONTROLS-MATRIX.md`, `PENETRATION-TEST-READINESS.md`, `PRODUCTION-READINESS-MATRIX.md`, `OPERATIONS-RUNBOOK.md`, `INCIDENT-RESPONSE-RUNBOOK.md`, `ROLLBACK-RUNBOOK.md`, `DEPLOYMENT-ENVIRONMENT-MATRIX.md`, `SECRETS-MANAGEMENT-MATRIX.md`, `THIRD-PARTY-SOFTWARE-INVENTORY.md`, `OPEN-SOURCE-COMPLIANCE.md`, `IP-HANDOVER-CHECKLIST.md`, `IP-OWNERSHIP-REGISTER.md`, `TRADEMARK-DOMAIN-REGISTER.md`, `ENVIRONMENT-VARIABLE-REGISTER.md`, `API-COMPATIBILITY-MATRIX.md`, `WEBHOOK-COMPATIBILITY-MATRIX.md`, `BUYER-VERIFICATION-SCRIPT.md`, `KNOWN-LIMITATIONS.md`, `TRANSACTION-READINESS-REPORT.md`, `FINAL-EVIDENCE-CROSSWALK.md`, `RELEASE-NOTES-CURRENT.md` (+ one hygiene resolution doc)
- **Counts at Batch 7:** historical counts removed — live inventory in `docs/STATS.md` (generated); version 0.1.0 everywhere.
- **No new business logic** (billing/custody/tenant/SDK/WS/lifecycle) — documentation/wiring/cross-check only

## Batch 5 (2026-09-24) — Hardening & Observability Closeout

- **Added** 25 files (`ops/observability_config.rs`, `metrics_snapshot.rs`, `trace_context.rs`, `rate_limit_report.rs`, `container_metadata.rs`, `repro_builder.rs`, `config_diff_report.rs`, `external_validation.rs`, `final_gap_ledger.rs`, `health_report.rs`, `backup/*` 5, `saas_sdk_reports.rs`, `sbom_report.rs`, `license_report.rs`, `release_manifest_integration.rs`, `buyer_package_integration.rs`, `backup_restore_integration.rs`, `validator_e2e.rs`, `archive/AUDIT.md`, `data-store-model` alias)
- **Tests:** 58+ new integration (observability 3, metrics 2, trace 3, rate_limit 2 (+2 ignored), container 2, repro 2, config_diff 2, external 2, gap 2, backup 3, health 2, sdks 4 (+1 ignored), SBOM 2 (+2 ignored), licenses 3 (+2 ignored), release 3 (+2 ignored), buyer 2 (+2 ignored), validator 3 ignored), `verify-delivery` partially passing at close (target/ WARNING — fixed in Batch6)
- **Release evidence:** `release-evidence/summary.json` added, `release-manifest.json` `docs_count 28→45→70`, `batch5_delivery`, `verify_delivery_awaiting_target_hygiene_patch` (resolved Batch6)
- **Round-4 file audit note (added 2026-09-27 by the file-completeness pass; the historical text above is preserved unchanged):** three filenames in the Batch-5 list are not present under those names in the current tree — the closest shipped counterparts are `crates/server/src/ops/reproducibility.rs` (`repro_builder.rs`) and `crates/server/src/ops/config_diff.rs` (`config_diff_report.rs`); `saas_sdk_reports.rs` ships as the `crates/saas-sdk` report surface (`src/ops.rs`, `src/commercial.rs`) plus `crates/server/src/saas/*` status modules. The fourth, `validator_e2e.rs`, does exist — at `programs/staking-suite/tests/validator_e2e.rs` (no `crates/server/tests` copy was ever created). Likewise the Batch-5 `ops/backup/*` group ships as `ops/backup_verification.rs` + `ops/restore_verification.rs` (+ `tests/backup_restore_integration.rs`), `archive/AUDIT.md` lives at the repository root, and no Cargo alias `data-store-model` exists (`crates/core` = package `bot-core`). No file was fabricated to satisfy the old list.

## Batch 4 (prior) — Frontend / Release Hygiene

- Lockfile `apps/control-plane/package-lock.json` 6171 lines v3 real (`npm install --package-lock-only`), `frontend-ci.yml` matrix, `openapi_drift`/`sdk_lint`.

## Batch 3 (prior) — Billing / Custody / Checkout

- `billing/provider_config.rs` refs without secrets, `billing/checkout.rs` + webhook role matrix.

## Batch 2-1 (prior) — SaaS Foundations

- SaaS control plane migrations (permissions + roles), lifecycle/retention, WebSocket tenant auth, OpenAPI + SDK.

## Upgrade Notes

- **Migrations:** Forward-only; after pulling 0.1.0 run `sqlx migrate run` (adds `0018–0022` if from earlier 17).
- **Env:** Copy new `config.toml.example` fields (`secrets.*` → env), keep `.env` uncommitted.
- **Frontend:** `cd apps/control-plane && npm ci --ignore-scripts && npm run typecheck && npm run build` after pull.
- **Package:** `bash scripts/build-release-package.sh` now correctly excludes `target/` everywhere (fix confirmed by re-run).

## Known Limitations (current only)

- 6 buyer-facing gaps `EXTERNAL_REQUIRED` / buyer action: billing live, custody live, prod deploy, funded trading, staking E2E, external audit (`docs/FINAL-BUYER-GAP-LEDGER.md`).
- Service-backed: PostgreSQL **executed 2026-09-26** against a real PostgreSQL 17.11 instance (see the MATERIAL-GAP BATCH block below); Redis `NOT_RUN` without `REDIS_URL`.
- Trademark/domain `NOT INCLUDED`, program `placeholder program_id` must `set-id`.

## Verification

- Hermetic: `cargo fmt --all --check` / `cargo check --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace -- --test-threads=1` / `npm ci/typecheck/build` / `bash scripts/verify-delivery.sh` / `bash scripts/verify-buyer-package.sh`
- Idempotent: `bash scripts/final-release-check.sh` ×2.

> Full chronicle: `CHANGELOG.md`. Fixes & crosswalks: `docs/FINAL-EVIDENCE-CROSSWALK.md`.

---

## MATERIAL-GAP BATCH — provider end-to-end wiring + PostgreSQL service-backed verification (2026-09-26)

**Scope:** two buyer-proof items only — Stripe/Paddle end-to-end checkout wiring, and real
PostgreSQL execution of the checkout-concurrency/invoice claims. No new billing
architecture, no fabricated provider results, no mocked database results.

**Code**

- `BillingService::create_checkout` (`crates/server/src/saas/billing.rs`) now performs the
  full sequence: authorize tenant → validate plan (server authority) → validate provider →
  validate idempotency → durable `pending` row → provider call → durable update on success
  (`provider_session_id`, `checkout_url`, status `open`, tenant-scoped, `rows_affected == 1`)
  → typed failure on provider error with the row left `pending`. Provider success followed by
  a failed durable write returns `reconciliation required` (HTTP 503) and an audit event —
  never a success claim.
- `Idempotency-Key` is propagated to both providers; Paddle also carries
  `custom_data.idempotency_key`.
- `provider_registry` mismatch and configuration errors are typed; the former
  `unreachable!()` panics in the checkout path were removed.
- Both adapters now use a bounded 15 s HTTP timeout and **require a real provider session id**
  (Paddle's fabricated `"paddle_session"` placeholder was removed — it would have collided
  under `UNIQUE (provider, provider_session_id)`).
- `migrations/0022_checkout_url.sql` adds the durable `checkout_url` column.
- `SaasStore::plans()` fails closed instead of degrading to an empty catalogue;
  `PostgresSaasRepo` replica test cleans up its probe rows so a shared database cannot
  pollute the plan namespace; `ProviderContractRunner` resolves credentials only from the
  supplied map so hermetic classification cannot be influenced by the ambient environment.
- `scripts/build-release-package.sh` no longer exits 141 on a successful build (`head`
  SIGPIPE under `pipefail`).

**Verification (all executed)**

- `cargo test --workspace -- --test-threads=1` with real PostgreSQL 17.11 → **70 suites,
  2077 passed, 0 failed, 13 ignored, exit 0**
- `cargo clippy --workspace --all-targets -- -D warnings` → **PASS (0 warnings)**
- `bash scripts/release-check.sh` → **17 PASS / 1 FAIL / 2 SKIP** (FAIL: `cargo deny` not
  installed in the sandbox; SKIPs: Redis-gated)
- `bash scripts/verify-delivery.sh` PASS · `verify-buyer-package.sh` PASS ·
  `final-release-check.sh` **ALL PASS** · `build-release-package.sh` exit 0
- `cargo audit` (app + staking) → **0 vulnerabilities**, 9 allowed warnings
- Frontend `npm ci --ignore-scripts` + `typecheck` + `build` → **PASS**

**Counts recomputed (2026-09-26, superseded below):** historical counts removed — live inventory in `docs/STATS.md`
(grep `#[test]`, crates scope; +516 `#[tokio::test]`), 8 workspace members, version 0.1.0 everywhere
(`VERSION`, `Cargo.toml`, `release-manifest.json`, `sbom.json`). The `buyer-release` package ships both
license formats (`licenses.json` + `licenses.csv`) and both SBOM formats, verified by
`scripts/build-release-package.sh` + `scripts/verify-buyer-package.sh`.

**Batch 10 count re-check (2026-09-27):** `#[test]` grep now **1331** (1330 after the Batch-10 harness/guard tests + 1 frontend dependency-guard test added at the end of the batch; lib `cargo test -p sniper-suite --lib` = 263 passed / 0 failed / 3 ignored);
`release-manifest.json` `test_count` synced 1314 → 1331; the 2026-09-26 figure above is retained as the historical recompute.

**Still external (unchanged):** live Stripe/Paddle execution (`LIVE_BILLING=1` + real keys)
is **NOT_RUN**; `cargo deny` and Redis-gated suites are **NOT_RUN** in this sandbox;
no production deployment, funded trading or external audit is claimed.
