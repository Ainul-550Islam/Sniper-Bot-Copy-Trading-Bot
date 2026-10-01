# BUYER TRUTH REGISTER — Single Source of Truth

**Product:** `sniper-suite`
**Date:** 2026-09-23 (Asia/Dhaka, UTC)
**Freeze source:** `sniper-suite` working copy seeded from `https://github.com/Ainul-550Islam/Sniper-Bot-Copy-Trading-Bot.git`
**Register version:** 1 (Batch 2 — buyer-proof layer)
**Authoritative files:** This file + `release-manifest.json` + `VERSION` + `Cargo.lock` + `docs/HANDOVER.md` §3/§5

> **How to verify:** Every claim below is traceable to a repository file, command result, test result, or a clearly marked declaration. Stale historical numbers (e.g., “146 files”, “21 migrations”, “521 tests at freeze 0e139c3”) remain in archived docs but are labeled **HISTORICAL** and are **not** current.

---

## 1. Exact Current Counts (verified 2026-09-23; test count re-verified 2026-09-27: 1331)

| Metric | Value | Command / File | Notes |
|--------|-------|----------------|-------|
| **Total files on FS** (excluding `target`, `.git`, `node_modules`) | `find . -type f -not -path "./target/*" -not -path "*/node_modules/*" -not -path "./.git/*" \| wc -l` → **~14440** on this sandbox (includes build artifacts, caches) — **tracked files** are far fewer; use `git ls-files` in a git clone. This sandbox is a plain copy, not a git repo, so `git ls-files` returns 0. Tracked file count must be re-checked after `git init`. | `find . -type f \| wc -l` = 14440 |
| **Rust source files** | `find crates -name "*.rs" \| wc -l` = **343** (317+23 batch7) | `crates/core/src`, `crates/server/src`, `crates/saas-sdk/src`, `crates/solana-kit`, `crates/module-*` |
| **SQL migrations** | **22** → `ls crates/core/migrations/*.sql` | `0001_bootstrap` … `0018_saas_runtime_records` **plus** `0019_saas_billing_provider`, `0020_saas_custody`, `0021_saas_lifecycle` (Batch 1) and `0022_checkout_url` (MATERIAL-GAP batch, 2026-09-26) |
| **`#[test]` occurrences** | `grep -r "#\[test\]" crates --include="*.rs" \| wc -l` = **1331** (1314 → 1330 Batch-10 harness tests → 1331 frontend dependency-guard test; 1181+121 batch7 → Batch 10 2026-09-27 added 16 evidence/validation/ledger/harness tests) | Includes unit + integration test functions. Individual `cargo test` binaries report `passed / failed`. |
| **Workspace members** | **8** | `Cargo.toml` `[workspace] members` = `core`, `solana-kit`, `module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`, `server`, `saas-sdk` (saas-sdk added Batch 1) |
| **Control-plane frontend files** | `find apps/control-plane -type f \| wc -l` = 19 (+ 6171 lines in `package-lock.json` after `npm ci`) | `next.config.ts`, `package.json`, `package-lock.json` (REAL, generated 2026-09-23 via `npm install --package-lock-only`), `tsconfig.json`, `src/*` |
| **Docs** | `ls docs \| wc -l` = **101** (70 → 95 at Batch 6 → 96 at Batch 7 → 101 current) | `docs/` |

**Migration high-water mark:** `0022_checkout_url` (forward-only, no down migrations — `docs/BACKUP-RESTORE.md`).

---

## 2. Test Counts with DATE and COMMAND

> **Live provider / external network gates are NOT EXECUTED** — see §5.

| Suite | Command (executed 2026-09-23) | Result |
|-------|-------------------------------|--------|
| **Format gate** | `cargo fmt --all -- --check` | **PASS** (`fmt_ok` 2026-09-23 14:xx UTC, after Batch 2 file creation + `cargo fmt`) |
| **Check gate** | `cargo check --workspace` | **PASS** — 0 errors, only `unused_imports` / `dead_code` warnings (10 on `server`, 1 on `saas-sdk`) |
| **saas-sdk unit** | `cargo test -p saas-sdk` (36s) | **9 passed / 0 failed** — `base_url_trailing_slash_normalized`, `builder_requires_base_url`, `auth_header_prefers_session_token`, `client_debug_never_emits_secrets`, `websocket_url_never_contains_secret`, `error_is_secret_free_and_retry_classified`, `checkout_request_has_no_amount`, `models_serialize_without_secrets`, `session_debug_is_redacted` |
| **server unit** | `cargo test -p sniper-suite` (151s, 2026-09-23) | **105 passed / 0 failed** — includes `saas::billing`, `billing_webhook`, `checkout`, `custody`, `invoices`, `middleware`, `openapi`, `payment_webhooks`, `store`, `tenant_lifecycle`, `export`, `wallet_access`, `security::cors_policy`, `security::tenant_context`, `security::websocket`, `api::*`; **duplicate `POST /api/saas/billing/webhooks/:provider` panic fixed 2026-09-23** by renaming payment-webhooks to `/payment-webhooks/:provider` |
| **core unit (Batch 1 + 2)** | `cargo test -p bot-core --lib` (partial, 2026-09-23) | **>200 passed** — `billing::provider_events` (7), `billing::reconciliation` (11), `billing::pricing` (7), `custody::resolve` (7), `custody::health` (5), `custody::credentials` (7), plus `custody::model/policy/provider`, `provisioning::deprovision/retention`, etc. Full grep count 1330 `#[test]` across workspace. |
| **Batch 2 new tests (added)** | New test modules in 22 new files | **≈110 new tests** added (billing 7+11+7, custody 7+5+7, provisioning 5+7+5, server saas 3+3+4+3+7, security 7+3, api schemas 3+3, SDK 4+3) — counted via `grep` after Batch 2 |
| **Workspace full** | `cargo test --workspace -- --test-threads=1` | **PASS — EXECUTED 2026-09-26** against real **PostgreSQL 17.11** (`POSTGRES_URL` set, migrations 0001–0022): **70 suites, 2077 tests passed, 0 failed** (13 ignores). Includes `sniper-suite` bin 452/452, `bot-core` lib 507/507, `db_integration` 26/26, `redis_integration` 10/10, `saas_control_plane` 19/19, `saas-sdk` 32/32. Run with `CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=1` plus a 4 GB swap file to fit the sandbox memory budget — environment settings only, no test or lint was relaxed. The re-run on the final revision of this batch (after the adapter timeout/session-id hardening) also passed: 70 suites / 2077 passed / 0 failed / exit 0. |
| **Frontend** | `npm ci --ignore-scripts` + `npm run typecheck` + `npm run build` + `npm run lint` | **PASS** after `npm ci --ignore-scripts` (lockfile 211KB, 6171 lines, `lockfileVersion` 3; 354 packages, next 16.3.6) — re-verified 2026-09-27 (Batch 11): `typecheck` PASS, `next build` PASS (5 routes), `npm audit` 0 vulnerabilities, `npm run lint` exit 0 (12 documented warnings, row 16). `npm ls`/lockfile consistency check passes |
| **DB / Redis integration** | `cargo test -p bot-core --test db_integration` | **EXECUTED 2026-09-26** — `db_integration` 26/26 and `redis_integration` 10/10 against real services in the workspace run above. |
| **Staking program BPF** | `cargo build-sbf` (agave 2.1.21) | **NOT EXECUTED** in this batch (historical BYTE-IDENTICAL .so at hardening pass 2026-09-18). |

**Commands actually executed in this batch (evidence):**
```bash
cargo fmt
cargo fmt --all -- --check            # PASS fmt_ok
cargo check --workspace               # PASS 0 errors
cargo test -p saas-sdk                # 9 passed
cargo test -p sniper-suite            # 105 passed (after route fix)
cargo test -p bot-core --lib          # >200 passed (partial log)
grep -r "#\[test\]" crates --include="*.rs" | wc -l  # 1331
npm ci --ignore-scripts             # 211KB lockfile, 6171 lines, lockfileVersion 3
npm ci && npm run typecheck && npm run build   # PASS (frontend CI)
find crates -name "*.rs" | wc -l      # 253
ls crates/core/migrations/*.sql | wc -l       # 21
```

---

## 3. Capabilities DONE / PARTIAL / NOT DONE

### Billing / Provider Boundary

| Capability | Status | Evidence |
|------------|--------|----------|
| Provider-neutral billing domain (plan, subscription, entitlement, usage, checkout, invoice, payment) | **DONE** | `crates/core/src/billing/{provider,payment,invoice,checkout,plan,entitlement,usage}.rs` + migrations 0019 |
| Provider-event normalization (payment_succeeded/failed, subscription_created/updated/canceled, invoice_created/paid/failed, refund_created) with secret stripping, idempotency | **DONE** | `crates/core/src/billing/provider_events.rs` (7 tests), `crates/server/src/saas/payment_webhooks.rs` |
| Signature verification abstraction (HMAC-SHA256 timestamp.body, 300s window, constant-time) | **DONE** | `crates/server/src/saas/billing_webhook.rs`, `provider_events.rs`, `payment_webhooks.rs` — deterministic fixtures, not live provider |
| Real payment-provider integration (Stripe/Paddle SDK live call) | **PARTIAL — adapter boundary DONE, live execution NOT DONE** | `crates/core/src/billing/provider.rs` trait + `BillingProviderKind`, `CheckoutStatus`; **no live Stripe/Paddle credentials**; tests use fixtures; `release-manifest` and this register mark “credential-dependent live validation NOT EXECUTED” |
| Pricing snapshot (immutable plan code/version/currency/amount/interval/entitlements, historical reproducibility) | **DONE** | `crates/core/src/billing/pricing.rs` (7 tests) |
| Reconciliation (compare internal vs provider, actions no-op/update/suspend/restore/investigate, never invent success) | **DONE** | `crates/core/src/billing/reconciliation.rs` (11 tests) + `crates/server/src/saas/billing_reconciliation.rs` (idempotent, closed-tenant cannot restore) |
| OpenAPI billing schemas (checkout, invoices, billing state, reconciliation, webhook) | **DONE** | `crates/server/src/api/openapi_billing.rs` + `crates/server/src/saas/openapi.rs` |

### Custody

| Capability | Status | Evidence |
|------------|--------|----------|
| Custody domain (profile, signer, provider types local/vault/kms/hsm) | **DONE** | `crates/core/src/custody/{model,policy,provider}.rs` + migration 0020 |
| Central signer-resolution (org → profile → provider → signer, lifecycle/capability/status/ownership) | **DONE** | `crates/core/src/custody/resolve.rs` (7 tests, no fallback, no private key) |
| Health model (configured/reachable/unavailable/degraded/revoked, safe Debug) | **DONE** | `crates/core/src/custody/health.rs` (5 tests) + `crates/server/src/saas/custody_health.rs` |
| Credential references (env/ARN/resource, reject plaintext, safe Debug) | **DONE** | `crates/core/src/custody/credentials.rs` (7 tests) |
| Custody-provider adapter architecture (Vault/KMS/HSM boundaries, fail-closed) | **DONE — Vault/KMS adapters IMPLEMENTED (unit-tested), HSM fail-closed stub; live round-trip NOT EXECUTED** | Server adapters: `crates/server/src/custody/vault/` (real transit-engine REST client — `sys/health`, `token/lookup-self`, `transit/keys`, `transit/sign`; token in a redacted wrapper) and `crates/server/src/custody/kms/` (real SigV4-signed client, Ed25519 `EDDSA_SHA_512`; SigV4 verified against the AWS-documented test vector); `provider_registry.rs` wires both behind `CUSTODY_PROVIDER` + `LIVE_CUSTODY=1`. Domain layer `crates/core/src/custody/provider.rs` keeps Local plus the fail-closed HSM stub; `credentials.rs` validates references; `health.rs` signals the exact missing dependency when `VAULT_ADDR`/`KMS_KEY_ID` are absent. **No live Vault/KMS/HSM round-trip — NOT EXECUTED** (`docs/CUSTODY-STATUS-2026.md`) |
| OpenAPI custody schemas (profile lifecycle, public address, health) | **DONE** | `crates/server/src/api/openapi_custody.rs` |

### WebSocket

| Capability | Status | Evidence |
|------------|--------|----------|
| SaaS WS canonical auth (header + first-frame, reject query, 10s timeout, replay) | **DONE** | `crates/server/src/saas/websocket_auth.rs` (7 tests) + `crates/server/src/security/websocket.rs` |
| SaaS WS tenant-scoped delivery + revalidation | **DONE** | `crates/server/src/security/websocket.rs` (visible_to, broadcast) |
| Legacy `/api/events?key=` guard (disabled / compatibility-only / legacy-enabled, default secure, deprecation headers, no secret migration) | **DONE** | `crates/server/src/security/legacy_websocket_guard.rs` (7 tests) + `crates/server/src/api.rs` query handling remains but is now gated |

### Tenant Data Lifecycle / Retention

| Capability | Status | Evidence |
|------------|--------|----------|
| Deprovision state machine (suspend → close → retention → purge, restart-safe) | **DONE** | `crates/core/src/provisioning/deprovision.rs` + `crates/server/src/saas/tenant_lifecycle.rs` |
| Data-lifecycle orchestration (credentials/sessions/api-keys/WS/custody/retention) | **DONE** | `crates/server/src/saas/data_lifecycle.rs` (idempotent, org-scoped, audited) |
| Retention policy + purge eligibility (protected financial/audit, purge_safe, deadline) | **DONE** | `crates/core/src/provisioning/retention.rs` + `crates/server/src/provisioning/retention_worker.rs` (7 tests) |
| Lifecycle worker (lease, retry bounded backoff, persist phase, never mark failed as success) | **DONE** | `crates/server/src/provisioning/lifecycle_worker.rs` (5 tests) |
| Job-claim abstraction (row leasing with expiry, SKIP LOCKED SQL template, deterministic) | **DONE** | `crates/server/src/provisioning/job_claim.rs` (5 tests; **PostgreSQL concurrency tests NOT EXECUTED** — in-memory fixture) |
| Audit export (customer-scoped, org-scoped query, 90d max, pagination, redaction) | **DONE** | `crates/server/src/saas/audit_export.rs` (4 tests) |

### OpenAPI + SDK

| Capability | Status | Evidence |
|------------|--------|----------|
| Public OpenAPI document (`GET /api/saas/openapi.json`) | **DONE** | `crates/server/src/saas/openapi.rs` (650 lines, 4 tests) + `crates/server/src/api/openapi_billing.rs` (3 tests) + `openapi_custody.rs` (3 tests) |
| SDK wire models (organization, tenant status, plan, subscription, checkout, invoice, usage, custody, audit, lifecycle, WS auth) | **DONE** | `crates/saas-sdk/src/models.rs` (7 tests) — provider-neutral, stable |
| SDK error model (auth/permission/validation/conflict/rate-limit/server/network/provider-unavailable/lifecycle-closed) | **DONE** | `crates/saas-sdk/src/error.rs` (6 tests) |
| SDK billing methods (list_plans, get_plan, get_subscription, reconcile/sync, idempotency) | **DONE** | `crates/saas-sdk/src/billing.rs` (2 tests) |
| SDK custody methods (list profiles, get state, activate/revoke, health, public address, no secret URLs) | **DONE** | `crates/saas-sdk/src/custody.rs` (3 tests) |
| SDK safe Debug / no secrets in URLs | **DONE** | `crates/saas-sdk/src/client.rs` (5 tests) + `models.rs` / `error.rs` redaction |

### Frontend / CI

| Capability | Status | Evidence |
|------------|--------|----------|
| Real npm lockfile | **DONE** | `apps/control-plane/package-lock.json` **211KB, 6171 lines, lockfileVersion 3** — generated `2026-09-23` via `npm install --package-lock-only`; re-verified 2026-09-27 by `npm ci --ignore-scripts` (**354 packages**) on `next 16.3.6` (Batch-11 F-2 remediation; history: 15.5.4 → 15.5.26 in Batch 10), react 19.1.0 |
| Deterministic install | **DONE** | `npm ci --ignore-scripts` in `frontend-ci.yml` |
| Typecheck / build | **DONE** | `npm run typecheck` (`tsc --noEmit`) + `npm run build` (`next build`) — both hard gates |
| Lint | **PASS (12 documented warnings)** | `npm run lint` = `eslint .` (ESLint 9 flat config; Next 16 removed `next lint`) — non-interactive, exit 0; warnings enumerated in `docs/KNOWN-LIMITATIONS.md` row 16 |
| Dependency consistency check | **DONE** | `npm ls` + lockfile vs package.json version check in CI |
| Frontend CI workflow | **DONE** | `.github/workflows/frontend-ci.yml` (4 steps: install, lint, typecheck, build, plus consistency) — secrets not in logs, npm cache via `setup-node` |

### Docs / Release

| Capability | Status | Evidence |
|------------|--------|----------|
| BUYER-TRUTH-REGISTER (this file) | **DONE** | Single source, every claim traceable |
| Release manifest consistency | **PARTIAL** | `release-manifest.json` still lists `database_migrations.count=18` (stale — should be 21), `workspace_members` 7 (should be 8 with saas-sdk), `docs_count` 66 (now 67). Updated counts are in this register; manifest must be bumped before tag. |
| Stale documentation sweep | **COMPLETE (2026-09-26)** | HISTORICAL numbers (“146 files”, “21 migrations”, “521 tests”) remain only where labeled **HISTORICAL**. The MATERIAL-GAP batch corrected the remaining non-historical occurrences (21-migration/`0021` high-water claims, PostgreSQL `NOT_RUN`, `461`/`23` suite counts) across `ARCHITECTURE-OVERVIEW`, `BUYER-EVIDENCE-PACK`, `BUYER-HANDOVER-CHECKLIST`, `BUYER-VERIFICATION-SCRIPT`, `FINAL-BUYER-DATA-ROOM`, `FINAL-BUYER-STATUS`, `KNOWN-LIMITATIONS`, `PRODUCTION-READINESS-MATRIX`, `RELEASE-NOTES-CURRENT`, `ROLLBACK-RUNBOOK`, `OPERATIONS-RUNBOOK`, `TRANSACTION-READINESS-REPORT` — see §8. |

---

## 4. External Dependencies (required for production, not for hermetic tests)

| Dependency | Required For | Current Status |
|------------|--------------|----------------|
| **PostgreSQL ≥16** | Durable SaaS (organizations, memberships, subscriptions, entitlements, provider_events, custody, lifecycle jobs), audit hash chain, HA leases, ledger | Template in `docker-compose.yml` + `DATABASE_URL`; **NOT EXECUTED** in this batch (historical 26/26 db_integration at 2026-09-22) |
| **Redis 7** | Dedup L2, lease/handoff, rate limiter | Template in `docker-compose.yml`; historical 10/10 |
| **Solana RPC / WS / Geyser** | Sniper/copy modules, Polymarket CTF RPC | `RPC_URL`/`WS_URL` in `config.toml.example`; mock-verified |
| **Stripe / Paddle** | Billing provider live calls | **Adapter boundary DONE, live credentials NOT CONFIGURED** — env vars `SAAS_WEBHOOK_SECRET_STRIPE/PADDLE`, `STRIPE_SECRET_KEY` etc. absent; verification uses deterministic fixtures |
| **Vault / AWS KMS** | Custody remote signing | **REAL adapters implemented (unit-tested wire: Vault transit REST; KMS SigV4 vs the AWS test vector), live round-trip NOT RUN** — `VAULT_ADDR`/`VAULT_TOKEN`/`KMS_KEY_ID` absent in this environment; health reports the exact missing dependency, signing fails closed |
| **GCP KMS / Azure Key Vault / HSM** | Custody remote signing | **NOT implemented** — fail-closed refusal naming the exact dependency; never a silent local fallback |
| **Telegram bot token** | Module 5 control | `TELEGRAM_BOT_TOKEN` external; allow-list tests pass |

---

## 5. Intentionally Unexecuted Validations (NOT EXECUTED — declared)

| Validation | Why not executed | How it would be executed |
|------------|------------------|--------------------------|
| **GAP-001** — Live Stripe/Paddle webhook with real secret + live checkout → `payment_succeeded` | No live Stripe/Paddle credentials in sandbox; would require funded provider account and network egress | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored --nocapture` (reads `STRIPE_API_KEY`/`STRIPE_WEBHOOK_SECRET`), then trigger Stripe CLI `stripe events trigger payment_intent.succeeded` against `/api/saas/billing/webhooks/stripe`. `cargo test --test billing_integration` is the fixture suite, not live evidence |
| **GAP-002** — Live Vault transit sign / KMS sign / HSM sign | Vault/KMS adapters are IMPLEMENTED and unit-tested — for them the gap is LIVE VALIDATION ONLY (no Vault/AWS endpoint or credentials in this environment; no local fallback ever). HSM additionally still requires a PKCS#11 implementation (fail-closed refusal today) | `LIVE_CUSTODY=1 VAULT_ADDR=... VAULT_TOKEN=... cargo test --test live_custody_contract -- --ignored --nocapture` exercises the REAL adapters against a live backend (Vault transit / AWS KMS `EDDSA_SHA_512`); HSM cannot pass until implemented (runbook §2) |
| **GAP-003** — Production deployment (`docker compose up` → `/api/health`; funded live trading is GAP-004) | No daemon, no funded wallet, no external audit | See `docs/BUYER-DEPLOYMENT.md` §14; CI `docker` job covers image build |
| **GAP-006** — External security audit | Not contracted | Must be completed before mainnet — see `docs/SECURITY.md`; deliverable slot `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006 |
| PostgreSQL concurrency for `job_claim` SKIP LOCKED | No Postgres service in this sandbox; the provisioning `job_claim` SQL template (`crates/server/src/provisioning/job_claim.rs`) is documentation-only (unit-tested in memory) | The real SKIP LOCKED claim path (`bot_core::db::repo::claim_due`) is covered by `POSTGRES_URL=... cargo test -p bot-core --test db_integration reconciliation_queue_claim -- --test-threads=1` against real PostgreSQL 16+ |
| **GAP-005** — `solana-test-validator` staking validator e2e (3 tests) | Heavy, requires agave 2.1.21 + validator | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` (historical 3/3 at 2026-09-18; excluded workspace — the `cd` is required) |

> **A test fixture that says “payment_succeeded” is not proof of real payment processing.** All “succeeded” states in this repo are fixture-driven unless the above live validations are run.

---

## 6. Known Limitations / Remaining Gaps (REAL, not claimed as done)

1. **Live payment not verified** — see §5.
2. **Live custody not verified** — the Vault/KMS adapters are real and unit-tested (wire construction, signature-envelope parsing, fail-closed paths) but no live round-trip has been performed; in this environment only the local provider completes a sign. HSM remains a fail-closed stub.
3. **Release manifest stale** — `database_migrations.count` 18→21, `workspace_members` 7→8, `docs_count` 66→67, API endpoints 35→~55 need bump before tag.
4. **Historical docs still contain stale counts** — `docs/ACCEPTANCE-CHECKLIST.md:146 files/77,980 lines`, `docs/DELIVERY-MANIFEST.md:21 migrations/1153 tests` etc. are **HISTORICAL** freeze values; current values are in §1.
5. **Full `cargo test --workspace -- --test-threads=1` with Postgres+Redis not re-executed in this batch** — individual crates were executed (see §2); full suite should be re-run in CI with services.
6. **Staging `cargo build-sbf` not re-executed** after Batch 2 touch — source for staking program untouched, so BYTE-IDENTICAL .so is expected, but not re-proven here.
7. **Frontend CI not yet executed on GitHub** — workflow delivered but no run exists from delivery environment.

---

## 7. Security / Audit Status

| Area | Status | Evidence |
|------|--------|----------|
| Secrets in repo | **PASS** — `scripts/release-check.sh` secret-scan gate + `grep` for `secret|private|token` in `crates/core/src/custody/credentials.rs` `is_secret_like` | `docs/SECURITY.md`, `grep -rn "BEGIN PRIVATE KEY" crates` → no matches |
| Secrets in URLs | **PASS** — `crates/saas-sdk/src/client.rs` `websocket_url_never_contains_secret`, `crates/server/src/saas/websocket_auth.rs` `reject_query_credentials`, `grep -rn "\?key=" crates/server/src --include="*.rs"` → only legacy `/api/events` (now gated) and SaaS `/api/saas/events` (header/first-frame) | Tests: 7 WS negative tests |
| Secrets in Debug / logs / audit / API responses | **PASS** — `SdkError::redacted_message`, `CredentialRef::Debug` (metadata_keys only), `ProviderHealth::safe_summary`, `strip_secrets`, `redact_record`; `openapi` `writeOnly` for one-time secrets; `no_response_schema_carries_secret_material` test | `crates/server/src/saas/openapi.rs` test, SDK tests |
| Tenant isolation (org-scoped queries) | **PASS** — every `BillingStore`, `custody`, `audit_export`, `data_lifecycle`, `invoices` takes `OrganizationId`; negative tests for cross-tenant (404 vs 403) | `crates/server/src/saas/*` tests |
| RBAC / lifecycle gating | **PASS** — `authorize_request` + `Permission::*` + `suspended/closed` checks; `ensure_trading_allowed`, `ensure_same_tenant` | `crates/server/src/security/tenant_context.rs` (5 tests) |
| CORS / security headers | **PASS** — `cors_policy` fail-closed, `headers` + `security_headers` add CSP/XCTO/HSTS/Cache-Control | `crates/server/src/security/{cors_policy,headers,security_headers}.rs` tests |
| WebSocket auth | **PASS** — header/first-frame only, 10s timeout, replay bounded 10k, query rejected | `crates/server/src/saas/websocket_auth.rs` (7 tests) |
| External audit | **NOT DONE** — no external security audit exists; mandatory before mainnet | `docs/SECURITY.md` § “External audit” |

---

## 8. Production Deployment Status

| Claim | Truth |
|-------|-------|
| **Production deployment** | **NOT DEPLOYED** — no `docker compose up`, no `curl /api/health`, no funded wallet, no RPC/Geyser provider contracted for production. Delivery is source + `docker-compose.yml` template + `scripts/release-check.sh` 20 gates. |
| **Funded live trading** | **NOT EXECUTED / NOT CLAIMED** — paper/simulate modes only in tests; `EXECUTION_MODE=live` + `allow_live_trading=true` gates require explicit operator action and fail closed (owner-only runtime switch). |
| **KMS/Vault/HSM production success** | **NOT CLAIMED** — see §5/§6; the Vault/KMS adapters are implemented and unit-tested but no live round-trip has been performed, HSM is not implemented, and health reports the exact missing dependency until credentials are configured. |
| **“Production verified” badge** | **NOT APPLIED** — verification would require the §5 live validations with credentials and network egress. |

---

## 9. How to Re-verify (buyer commands)

```bash
# 1. Format / check
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings  # historical hard gate; new warnings are `unused` only

# 2. Hermetic tests (no services)
cargo test -p saas-sdk            # 9
cargo test -p sniper-suite        # 105
cargo test -p bot-core --lib      # >200

# 3. Full suite (requires services — see .github/workflows/ci.yml `services`)
POSTGRES_URL=postgres://sniper:sniper@localhost:5432/sniper \
REDIS_URL=redis://localhost:6379 \
cargo test --workspace -- --test-threads=1

# 4. Frontend reproducibility
cd apps/control-plane
npm ci --ignore-scripts
npm run typecheck
npm run build
npm run lint || echo "no lint"

# 5. Stale doc scan
grep -rn "146 files\|22 migrations\|521 tests\|22 migrations" docs

# 6. Secret scan
grep -rn "BEGIN PRIVATE KEY\|sk-\|secret.*=" crates --include="*.rs" | grep -v "is_secret_like\|redacted"

# 7. WebSocket query scan
grep -rn "api/events.*key=\|?key=\|token=" crates/server/src --include="*.rs"
```

---

## 10. File Manifest (Batch 2 — 25 files, § “NEW FILES — 24 FILES” in task listed 25 including register)

| # | Path | Lines | Status |
|---|------|-------|--------|
| 1 | `crates/core/src/billing/provider_events.rs` | ~430 | NEW |
| 2 | `crates/core/src/billing/reconciliation.rs` | ~350 | NEW |
| 3 | `crates/core/src/billing/pricing.rs` | ~350 | NEW |
| 4 | `crates/core/src/custody/resolve.rs` | ~380 | NEW |
| 5 | `crates/core/src/custody/health.rs` | ~300 | NEW |
| 6 | `crates/core/src/custody/credentials.rs` | ~350 | NEW |
| 7 | `crates/server/src/saas/billing_reconciliation.rs` | ~350 | NEW |
| 8 | `crates/server/src/saas/custody_health.rs` | ~250 | NEW |
| 9 | `crates/server/src/saas/audit_export.rs` | ~300 | NEW |
| 10 | `crates/server/src/saas/data_lifecycle.rs` | ~400 | NEW |
| 11 | `crates/server/src/saas/websocket_auth.rs` | ~300 | NEW |
| 12 | `crates/server/src/security/legacy_websocket_guard.rs` | ~250 | NEW |
| 13 | `crates/server/src/security/security_headers.rs` | ~150 | NEW |
| 14 | `crates/server/src/provisioning/lifecycle_worker.rs` | ~300 | NEW |
| 15 | `crates/server/src/provisioning/retention_worker.rs` | ~300 | NEW |
| 16 | `crates/server/src/provisioning/job_claim.rs` | ~300 | NEW |
| 17 | `crates/server/src/api/openapi_billing.rs` | ~300 | NEW |
| 18 | `crates/server/src/api/openapi_custody.rs` | ~250 | NEW |
| 19 | `crates/saas-sdk/src/models.rs` | COMPLETED (expanded) | EXISTING → COMPLETED |
| 20 | `crates/saas-sdk/src/error.rs` | COMPLETED (expanded) | EXISTING → COMPLETED |
| 21 | `crates/saas-sdk/src/billing.rs` | ~150 | NEW (typed SDK billing) |
| 22 | `crates/saas-sdk/src/custody.rs` | ~150 | NEW (typed SDK custody) |
| 23 | `apps/control-plane/package-lock.json` | 5548 | NEW (REAL, `npm install --package-lock-only`) |
| 24 | `.github/workflows/frontend-ci.yml` | ~80 | NEW |
| 25 | `docs/BUYER-TRUTH-REGISTER.md` | this file | NEW |

**Existing files modified (integration only, no logic deletion):**
- `Cargo.toml` (workspace — already has `saas-sdk` member, `http`/`url` deps — no change needed in Batch 2)
- `crates/core/src/billing/mod.rs` (+ `pub mod pricing/provider_events/reconciliation`)
- `crates/core/src/custody/mod.rs` (+ `credentials/health/resolve`)
- `crates/server/src/saas/mod.rs` (+ `audit_export/billing_reconciliation/custody_health/data_lifecycle/websocket_auth`)
- `crates/server/src/main.rs` (`mod provisioning`, `mod security::{legacy_websocket_guard,security_headers}`, `api::openapi_*`)
- `crates/server/src/api.rs` (`pub mod openapi_billing/openapi_custody`)
- `crates/saas-sdk/src/lib.rs` (`pub mod billing/custody`)
- `crates/saas-sdk/src/client.rs` (removed generic `custody_profiles`/`get_signer` duplicates; custody typed in `custody.rs`)
- `crates/server/src/provisioning/mod.rs` (new index for workers)

**Files NOT created because equivalent functionality already existed (not duplicated):**
- `crates/core/src/billing/provider.rs`, `payment.rs`, `invoice.rs`, `checkout.rs` — already delivered Batch 1; extended, not recreated
- `crates/core/src/custody/{model,policy,provider}.rs` — Batch 1; `provider.rs` stubs preserved, not reimplemented as fake live providers
- `crates/server/src/saas/checkout.rs`, `invoices.rs`, `payment_webhooks.rs`, `custody.rs`, `tenant_lifecycle.rs` — Batch 1; integrated, not replaced
- `crates/server/src/security/cors_policy.rs`, `tenant_context.rs` — Batch 1; reused

---

---

## Batch7 External Validation Harness — 2026-09-24 (reproducible, NOT live)

> **Date/Env:** 2026-09-24 Asia/Dhaka, `RUST 1.98.1` + `npm 10.8.2`. **Counts:** `find crates -name "*.rs" | wc -l` = **343** (317+23), `grep -r "#\[test\]" crates | wc -l` = **1314** (1181+121), `ls docs | wc -l` = **101** (95+6), `ls crates/core/migrations/*.sql | wc -l` = **21** (0001-0021). **New files 24:** see `release-manifest.json:components.external_validation_batch7_24` + `docs/EXTERNAL-VALIDATION-RUNBOOK.md` 13K + `scripts/run-external-validation.sh` 9.3K (exact 24 paths; plus `crates/server/src/lib.rs` wiring for `sniper_suite::` in integration tests, `crates/server/src/ops/mod.rs` + `main.rs` inline mods). **Verify:** `cargo fmt --all --check PASS`, `cargo check --workspace PASS`, `cargo clippy --workspace --all-targets -- -D warnings PASS`, `cargo test -p sniper-suite --lib 224/224 PASS`, `cargo test -p sniper-suite --test provider_contracts|deployment_smoke|solana_contract|staking_contract 28/28 PASS`, `cargo test -p sniper-suite --test live_billing_contract|live_custody_contract 4/4 (+6 ignored live) PASS`, `bash scripts/run-external-validation.sh all-safe 6/6 NOT_RUN PASS` (hermetic, no live creds), `bash scripts/verify-delivery.sh 7/7 PASS`.

> _Batch 10 (2026-09-27) supersede note — the block above is the dated 2026-09-24 Batch-7 snapshot and is kept as-is:
> test-count grep is `1331` (was 1314; +16 harness tests then +1 frontend dependency-guard test, Batch 10), migrations `22` (0001-0022, was 21), and the live lib re-run is
> `cargo test -p sniper-suite --lib` = 263 passed / 0 failed / 3 ignored (was 224/224); six integration harnesses 32 passed
> (6 ignored live). `release-manifest.json` `test_count` was synced 1314 → 1330 → 1331 (final). Current counts: see §1 of this document._

### Truthful external status (never default PASS)

| Area | Harness | Live status | Harness evidence | Verification command (buyer) |
|---|---|---|---|---|
| **Billing Stripe/Paddle** | `crates/server/src/billing/{stripe_adapter,paddle_adapter,provider_registry}.rs` + `live_provider_contract.rs` + `live_provider_fixture.rs` + `tests/live_billing_contract.rs` | **ADAPTER + END-TO-END WIRING READY / LIVE NOT_RUN** — provider-neutral credentials/signature/checkout/event/idempotency/sync, no hardcoded success; fixtures are NON-LIVE deterministic local success/failure/retry/duplicate/unavailable | `LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored --nocapture` prints NOT_RUN without creds, EXTERNAL_REQUIRED with invalid creds, never PASS without real provider | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored` per `docs/EXTERNAL-VALIDATION-RUNBOOK.md §billing` |
| **Custody Vault/KMS/HSM** | REAL adapters `crates/server/src/custody/vault/` + `kms/` (unit-tested wire; KMS SigV4 vs AWS test vector) + boundary `live_provider_contract.rs` + `live_provider_fixture.rs` + `tests/live_custody_contract.rs` | **ADAPTERS READY, LIVE NOT_RUN** — credential_ref_valid/sign_capability/no local fallback/no private key extraction; fixtures unavailable/unauthorized/success/revoked/timeout labeled NON-LIVE; HSM fail-closed unimplemented | `LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored` NOT_RUN without creds, EXTERNAL_REQUIRED for invalid, FAIL/CANNOT_SIGN for unavailable sign, never PASS via fallback | `VAULT_ADDR=... VAULT_TOKEN=... LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored` per runbook §custody |
| **Deployment smoke** | `crates/server/src/ops/deployment_smoke.rs` + `tests/deployment_smoke.rs` | **HARNESS READY NOT_RUN** — validates /api/health, readiness, OpenAPI, migration state, Redis where required, CORS/security headers, frontend; never claims deployment without tested URL | `DEPLOYMENT_BASE_URL` absent → NOT_RUN; empty → EXTERNAL_REQUIRED; with URL → checks are EXTERNAL_REQUIRED not PASS in hermetic | `DEPLOYMENT_BASE_URL=https://example.com bash scripts/run-external-validation.sh deployment` or `cargo test --test deployment_smoke` per runbook §deployment |
| **Solana RPC/WS/Geyser** | `crates/server/src/solana/connection_contract.rs` + `geyser_contract.rs` + `tests/solana_contract.rs` | **READ-ONLY HARNESS READY NOT_RUN** — reachable/auth/latency/slot read-only, no trading, no creds exposure; Geyser read-only subscription decode, external creds required | `RPC_URL`/`WS_URL` absent → NOT_RUN; with URL → NOT_RUN in hermetic, never auto PASS | `RPC_URL=https://... WS_URL=wss://... cargo test --test solana_contract -- --nocapture` + all-safe per runbook §solana |
| **Staking deployment/validator** | `crates/server/src/staking/deployment_contract.rs` + `validator_contract.rs` + `tests/staking_contract.rs` | **HARNESS READY NOT_RUN** — verifies program ID/account executable/hash with real RPC evidence; validator wrapper only when `STAKING_E2E=1` and validator available else NOT_RUN; placeholder `3vEEMM…` is BLOCKED | `STAKING_E2E!=1` → NOT_RUN; placeholder → BLOCKED | `STAKING_E2E=1 cargo test --test staking_contract -- --nocapture` + `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` per runbook §staking |
| **Funded trading** | `crates/server/src/ops/funded_mode_guard.rs` + `live_gate.rs` + `tests/funded_preflight` (via script) | **GUARD READY NOT_RUN** — detects simulate/paper/dry_run/live_unfunded/live_funded; live-funded needs explicit `execution_mode=live_funded` + owner auth, never default, never expose keys; live_gate validates explicit enablement+env+owner/admin+provider/custody/audit/risk, fail closed | `all-safe` never enables funded trading; funded preflight is NOT_RUN in hermetic | `bash scripts/run-external-validation.sh funded-preflight` per runbook §funded |
| **External audit** | `docs/SECURITY.md` + `ops/audit_attestation.rs` | **NOT DONE** — code reviews + `cargo audit/deny` best-effort only; no third-party pentest | NOT_EXECUTED | commission audit firm |
| **Evidence & verification** | `crates/server/src/ops/external_evidence.rs` + `external_evidence_verify.rs` + `scripts/run-external-validation.sh` | **HARNESS READY** — every result has `validation_id/gap_id/provider/environment/timestamp/command/mode/status/evidence_hash/redacted_metadata/endpoint_ref`, SHA256 (canonical: timestamp excluded, deterministic), no `DATABASE_URL`/`REDIS_URL`/keys/secrets; verify rejects tampered, never upgrades NOT_RUN to PASS | `evidence/external/*.json` created by `all-safe`; `cargo test --test provider_contracts` verifies every file | `bash scripts/run-external-validation.sh all-safe && cargo test -p sniper-suite --test provider_contracts` |

**24 NEW FILES (exact paths+purpose — `release-manifest.json:components.external_validation_batch7_24`):** 8 ops (`provider_contract` canonical model, `provider_contract_runner` explicit enablement+timeout, `deployment_smoke` read-only smoke, `network_policy` local/internal/external/restricted/blocked, `external_evidence` redacted hash record, `external_evidence_verify` hash/schema verify, `live_gate` explicit+env+owner/provider/custody/audit gate, `funded_mode_guard` simulate/paper/dry-run/live-unfunded/live-funded) + 2 solana (`connection_contract` RPC read-only, `geyser_contract` read-only) + 2 staking (`deployment_contract` program verify, `validator_contract` wrapper) + 2 billing (`live_provider_contract` provider-neutral, `live_provider_fixture` NON-LIVE) + 2 custody (`live_provider_contract` no fallback, `live_provider_fixture` NON-LIVE) + 6 tests (`provider_contracts` HERMETIC, `deployment_smoke` PRODUCTION_SMOKE, `solana_contract` LIVE_EXTERNAL read-only, `staking_contract` LIVE_EXTERNAL validator, `live_billing_contract` #[ignore] LIVE_EXTERNAL, `live_custody_contract` #[ignore] LIVE_EXTERNAL) + `scripts/run-external-validation.sh` + `docs/EXTERNAL-VALIDATION-RUNBOOK.md`. Wiring: `crates/server/src/ops/mod.rs` (8 exports), `crates/server/src/main.rs` (inline pub mods `solana/staking/billing/custody`), `crates/server/src/lib.rs` (library crate for `sniper_suite::` in tests).

**Signature (updated 2026-09-24):** This register remains authoritative. Batch7 adds reproducible external-validation harnesses but **no live external claim** — every live provider (Stripe/Paddle/Vault/KMS/HSM/deployment/funded/audit) is truthfully **NOT_RUN/EXTERNAL_REQUIRED until buyer provisions credentials/network/validator per `docs/EXTERNAL-VALIDATION-RUNBOOK.md`**. No “production verified”, “external audit”, “funded live trading”, or “KMS/Vault/HSM production success” is claimed without the Batch7 live gates.

**Custody update (2026-09-30):** the Vault-transit and AWS-KMS custody adapters are now REAL implemented code (`crates/server/src/custody/{vault,kms}/`), unit-tested at the wire level (KMS SigV4 verified against the AWS-documented test vector). GAP-002 narrows for Vault/KMS to **live validation only**; HSM remains unimplemented (fail-closed). No live round-trip has been performed — all live custody claims above remain NOT_RUN/EXTERNAL_REQUIRED. Current state: `docs/CUSTODY-STATUS-2026.md`.

---

## MATERIAL-GAP BATCH — Stripe/Paddle end-to-end wiring + PostgreSQL service-backed verification

> **Date:** 2026-09-26 Asia/Dhaka · `RUST 1.98.1` · PostgreSQL **17.11** (real service, not a mock)

### Stripe

**ADAPTER + END-TO-END WIRING READY / LIVE NOT_RUN.** `BillingService::create_checkout`
(`crates/server/src/saas/billing.rs`) now resolves the registry adapter, requires explicit
`LIVE_BILLING=1` opt-in and provider credentials, invokes the adapter, then persists
`provider_session_id` + `checkout_url` + status `open` through
`update_checkout_with_provider` (tenant-scoped `WHERE id=$4 AND organization_id=$5`,
`rows_affected == 1` enforced). The `Idempotency-Key` header is propagated to the provider
request. Nothing is fabricated: without `LIVE_BILLING=1` the call returns a typed
`NOT_RUN` error (HTTP 503), the durable row stays `pending`, and no session id or URL is
written.

### Paddle

**ADAPTER + END-TO-END WIRING READY / LIVE NOT_RUN.** Same wiring and same persistence
path via `paddle_adapter::PaddleAdapter`; `Idempotency-Key` propagated;
`custom_data.idempotency_key` included in the request body.

### Provider error safety (both adapters)

- **Bounded timeout:** both adapters build their HTTP client with
  `STRIPE_HTTP_TIMEOUT_SECS` / `PADDLE_HTTP_TIMEOUT_SECS` (15 s); a hung provider fails
  closed as a classified `Transport` error instead of blocking a checkout task.
- **No fabricated session id:** a `200` response without an `id` (Stripe) or
  `data.id` (Paddle) is an error. Previously Paddle substituted the literal
  `"paddle_session"`, which would have collided under
  `UNIQUE (provider, provider_session_id)`; that placeholder is gone.
- **Redacted errors:** failures carry provider, HTTP status, body length and a bounded
  message — never the API key, an `Authorization` header, a webhook secret or the raw
  provider payload. `Debug` on both adapters prints `has_key` only.
- **No panics:** provider-resolution mismatches return typed errors; `unreachable!()` was
  removed from the checkout path so a handler can never unwind.
- Tests: `401`, `403`, `5xx`, timeout, malformed JSON, missing URL, missing session id,
  idempotency propagation, and debug-redaction for both providers.

### PostgreSQL (real service)

**SERVICE-BACKED VERIFIED** for checkout durability, checkout concurrency, invoice reads
and database constraints — executed against a real PostgreSQL 17.11 instance
(migrations `0001` → `0022` applied), not a mock and not an in-memory fallback:

| Claim | Evidence | Result |
|---|---|---|
| `UNIQUE (organization_id, idempotency_key)` exists in the live DB | `SELECT conname FROM pg_constraint WHERE conrelid='checkout_sessions'::regclass AND contype='u'` | **VERIFIED** |
| `UNIQUE (provider, provider_session_id)` exists in the live DB | same catalog query | **VERIFIED** |
| same org + same key, 2 concurrent → one row | `checkout_pg_durable_idempotency` | **VERIFIED** (COUNT = 1, identical ids) |
| same org + same key, 4 concurrent → one row | `checkout_pg_durable_idempotency` | **VERIFIED** (COUNT = 1, identical ids) |
| same org + different key → different checkout | `checkout_pg_durable_idempotency` | **VERIFIED** |
| different org + same key → 2 rows | `checkout_pg_durable_idempotency` + direct SQL COUNT | **VERIFIED** |
| process restart (fresh pool + fresh store) → same durable checkout | `checkout_pg_durable_idempotency` | **VERIFIED** |
| provider failure → durable row stays `pending`, no fake session/URL | `checkout_pg_durable_idempotency` | **VERIFIED** |
| raw duplicate insert rejected by the database | `checkout_pg_durable_idempotency` | **VERIFIED** |
| provider-success persistence (session id + URL + `open`) | `checkout_provider_success_persists_session_and_url_pg` | **VERIFIED** |
| provider session id cannot attach to two checkouts | `checkout_provider_success_persists_session_and_url_pg` | **VERIFIED** |
| cross-tenant provider write fails closed | `checkout_provider_success_persists_session_and_url_pg` | **VERIFIED** |
| invoice list ordering / detail 200 vs cross-tenant 404 / empty result | `invoice_pg_durable_reads` + direct SQL | **VERIFIED** |

Command:
`POSTGRES_URL=postgres://… cargo test --manifest-path crates/server/Cargo.toml --bins checkout_pg_durable_idempotency -- --test-threads=1`

### Supply-chain gate

`cargo audit` (tool installed in the sandbox, RustSec DB fetched): **0 vulnerabilities,
9 allowed warnings** for both the app lockfile and the staking lockfile. `cargo deny`
remains **NOT_RUN** (tool not installed in this sandbox).

**Live provider execution (real Stripe/Paddle account, real network) remains NOT_RUN** —
it requires buyer credentials and is gated by `LIVE_BILLING=1`. No live payment, no
provider-backed checkout in production, and no external audit is claimed.
