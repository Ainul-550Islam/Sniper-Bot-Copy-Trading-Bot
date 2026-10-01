# BUYER EVIDENCE PACK — Sniper Suite 0.1.0

**Date:** 2026-09-23 (Asia/Dhaka, UTC)  
**Version:** 0.1.0  
**Source:** `https://github.com/Ainul-550Islam/Sniper-Bot-Copy-Trading-Bot` seeded copy  
**Scope:** Commercial SaaS/IP package — *not* a claim of production deployment, funded trading, or external audit.

> **How to verify every claim:** This document cites its evidence source: a file path, a command output, or an explicit `NOT EXECUTED` marker. Historical numbers are labeled `HISTORICAL` with date.

---

## 1. Current Verified Architecture — VERIFIED

- **Workspace members (8):** `Cargo.toml:8` → `cargo metadata` shows `core, solana-kit, module-sniper, module-copy, module-polymarket, module-telegram, server, saas-sdk` — `find crates -name "*.rs" | wc -l` = **343** (317+23 batch7)
- **Migrations (22):** `ls crates/core/migrations/*.sql` → `0001_bootstrap` … `0022_checkout_url` (see `release-manifest.json: database_migrations.count=22, high_water=0022`)
- **Binary:** `crates/server` (Axum control plane + 5 modules + staking program `programs/staking-suite` with own lockfile)
- **Frontend:** `apps/control-plane` (Next.js 16 App Router, `package-lock.json` 211 KB, `lockfileVersion:3`, deterministic `npm ci`)

Evidence: `BUYER-TRUTH-REGISTER.md §1` (commands + outputs), `release-manifest.json` §components, `docs/REPOSITORY-MAP.md`.

---

## 2. Commercial Capabilities

### 2.1 Billing State — VERIFIED
- **Domain:** `crates/core/src/billing/{plan,subscription,entitlement,usage,checkout,payment,invoice,pricing,provider_events,reconciliation,billing_state,dunning,usage_policy,provider_config}.rs`
- **Server:** `crates/server/src/saas/{billing_status,usage_limits,commercial_state,billing_reconciliation,checkout,invoices,payment_webhooks}.rs` — tenant-scoped, `Permission::BillingRead` required, `organization_id` at query boundary.
- **Provider boundary:** `billing/provider_config.rs` + `provider_events.rs` — Stripe/Paddle references only (env var / ARN), never raw secrets; `validate()` distinguishes `not_configured / configured_but_unreachable / configured_and_ready / invalid`. Live validation is `NOT EXECUTED` (no credentials/network in sandbox).
- **Dunning:** `dunning.rs` state machine `Current → PaymentFailed → RetryPending → GracePeriod → BillingSuspended → Recovered/ManuallyResolved`. `Recovered` requires `provider: payment_succeeded evt_` evidence — never invents success. 7+ tests.
- **Usage policy:** `usage_policy.rs` maps measured usage to plan allowance / soft (80%) / hard (100%) / overage / suspension. Client cannot alter limits — server owns plan. Tests for server-authoritative enforcement.
- **SDK:** `crates/saas-sdk/src/commercial.rs` typed `billing_status / usage_limits / commercial_state` — deserialization tests, no secrets in URLs.

**Status:** `VERIFIED` hermetically; `PARTIAL` live provider (fixture only, see §9).

### 2.2 Custody — VERIFIED boundary, PARTIAL live
- **Domain:** `custody/{model,policy,provider,credentials,health,resolve,provider_config,rotation}.rs`
- **Provider config:** `provider_config.rs` — Vault/KMS/HSM references, `NotConfigured / Invalid / ConfiguredButUnreachable / Ready`, no local fallback when remote selected (explicit `allow_local_fallback=false`).
- **Rotation:** `rotation.rs` — `Pending → Active → Draining → Revoked` with `force` escape for emergency; non-force requires `Draining` before `Revoked`; rollback `Draining → Active`; audit emits.
- **Server:** `saas/custody_rotation.rs` — tenant-scoped, `WalletManage` required, audit events for create/activate/revoke; `saas/custody_health.rs`, `custody.rs` — health never exposes private keys.
- **SDK:** `custody.rs` — typed, safe.
- **Note:** the Vault-transit and AWS-KMS adapters are REAL code with unit-tested wire protocols (sign/verify envelope, SigV4 vs the AWS-documented test vector, fail-closed paths); only `Local` completes a sign in this environment (no live backend) — **NOT EXECUTED** against real Vault/KMS (see §9). The domain-layer `Vault/Kms/Hsm` stubs in `crates/core/src/custody/provider.rs` remain as fail-closed refusals; HSM is unimplemented end-to-end.

### 2.3 Lifecycle — VERIFIED
- **Deprovision:** `provisioning/deprovision.rs` — `Requested → TradingDisabled → CredentialsRevoked → SessionsInvalidated → CustodyRevoked → ResourcesCleaned → Retention → Completed` (idempotent, restart-safe).
- **Workers:** `server/provisioning/{lifecycle_worker,job_claim,retention_worker}.rs` — lease (`SELECT ... SKIP LOCKED` template), bounded backoff, never marks failed as success.
- **Customer controls:** `saas/data_lifecycle.rs` + `apps/control-plane/src/app/settings/data-lifecycle/page.tsx` — type `CLOSE` to confirm, explains irreversible actions, tenant-scoped.
- **Retention:** `retention.rs` — `Operational/Credentials/Sessions/ApiKeys` purgeable, `ControlPlaneAudit/FinancialAccounting/LegalCompliance` never automatically purged.

---

## 3. Tenant Isolation — VERIFIED

Every customer-facing operation enforces `organization_id + authenticated membership + permission + lifecycle status` at both application and query boundaries.

- **Middleware:** `crates/server/src/security/tenant_context.rs`, `crates/server/src/saas/middleware.rs` — `authorize_request` + `Permission::ALL (22)` + `TenantStatus` gates.
- **Negative tests:** `billing_status`, `usage_limits`, `custody_rotation`, `audit_export`, `data_lifecycle` each assert cross-tenant returns `404` (not `403` to avoid existence oracle) and `closed/suspended` denied.
- **SQL path:** `saas/postgres.rs` + `store.rs` — every query `WHERE organization_id=$1` (audited in `docs/SAAS-SECURITY.md`).

Run: `grep -rn "organization_id" crates/server/src/saas --include="*.rs" | head`.

---

## 4. Security Controls — VERIFIED (code), NOT EXECUTED (external audit)

- **Auth:** session token in `Authorization: Bearer …` + `x-organization`, never query string; `websocket_auth.rs` header/first-frame only, 10 s timeout, replay bounded.
- **Secrets:** `is_secret_like` / `validate_reference` / `CredentialRef` reject plaintext private keys; `Debug` redacts; `grep -R "BEGIN PRIVATE KEY" crates` → 0; `scripts/release-check.sh` secret scan.
- **CORS/Headers:** `security/{cors_policy,headers,security_headers,legacy_websocket_guard}.rs` — fail-closed CORS, CSP/HSTS/XCTO, legacy `?key=` guard `disabled/compatibility/legacy-enabled` (default secure).
- **RBAC:** 22 permissions, 8 roles, never grant on unknown strings.
- **External audit:** `NOT EXECUTED / NOT DONE` — no third-party pentest exists before mainnet (see `docs/SECURITY.md`).

---

## 5. Test Evidence — VERIFIED (hermetic), NOT_RUN (gated integrations)

**Current counts (2026-09-23, `cargo test`):**

| Suite | Command | Result |
|---|---|---|
| `bot-core --lib` | `cargo test -p bot-core --lib` | **507 passed / 0 failed** (re-run 2026-09-26) (incl. billing: provider_config 8, billing_state 6, dunning 8, usage_policy 8; custody: provider_config 7, rotation 7) |
| `saas-sdk` | `cargo test -p saas-sdk` | **32 passed / 0 failed** (incl. commercial 4; re-run 2026-09-26) |
| `sniper-suite --no-run` | `cargo test -p sniper-suite --no-run` | compiles (105+ tests; full run requires 2m) |
| `grep "#[test]"` | `grep -r "#[test]" crates --include="*.rs" | wc -l` | **1331** occurrences (crates scope; 2026-09-26 baseline 1330 + 1 Batch-10 frontend dependency-guard test) |
| `frontend` | `npm ci && npm run typecheck && npm run build` | `PASS` after `npm install --package-lock-only` |

**Gated:** `db_integration` (26) **executed 2026-09-26 against real PostgreSQL 17.11** (26/26); `redis_integration` (10) and `distributed_integration` (4) remain `NOT_RUN` in this sandbox without `REDIS_URL`. See `crates/server/src/ops/integration_matrix.rs` `default_for_current`.

**Never fabricated:** All live/provider/validator/production rows are `EXTERNAL_REQUIRED` / `NOT_RUN` (see `ops/integration_matrix.rs`).

---

## 6. Frontend Evidence — VERIFIED

- **Lockfile:** `apps/control-plane/package-lock.json` 211 KB, `lockfileVersion:3`, deterministic `npm ci --ignore-scripts` in CI.
- **CI:** `.github/workflows/frontend-ci.yml` — install, `npm ls` consistency check (`node -e` compares `package.json` vs `lockfile`), `lint` (where configured), `typecheck` (hard gate), `build` (`NEXT_TELEMETRY_DISABLED=1`).
- **Pages:**
  - `apps/control-plane/src/app/billing/page.tsx` — plan, subscription, payment/invoice summary, usage/limits, dunning/grace; loading/error/empty; no secrets.
  - `apps/control-plane/src/app/custody/page.tsx` — provider, signer status, public address, capabilities, health, rotation; safe revoke/rotation; never private keys.
  - `apps/control-plane/src/app/settings/data-lifecycle/page.tsx` — active/suspended/closure/retention/closed; `CLOSE` confirmation; irreversible warning.
- **Libs:** `lib/commercial.ts` (typed billing/commercial/usage/custody), `lib/release-status.ts` (public vs operator readiness, never claims production verified).

---

## 7. Known Limitations — REQUIRES BUYER/OPERATOR ACTION

1. **Live Stripe/Paddle** — adapter boundary **+ end-to-end wiring** done (`create_checkout` invokes the adapter, persists provider session id/URL, propagates `Idempotency-Key`), deterministic fixtures, but **no live credentials/network**; `provider live` row is `EXTERNAL_REQUIRED` and execution is `LIVE NOT_RUN`.
2. **Live Vault/KMS/HSM** — references + health + fail-closed done; no real Vault/KMS/HSM endpoint/creds; signing `PARTIAL` / `NOT EXECUTED`.
3. **External security audit** — code reviews + secret scans done; formal audit `NOT DONE`.
4. **Production deployment** — `docker-compose.yml` template + `health/ready` endpoints done; no `docker compose up` / `/health` / funded wallet proof in this repo — `NOT EXECUTED`.
5. **Funded live trading** — paper/simulate only in tests; `EXECUTION_MODE=live` is fail-closed and requires operator explicit action.
6. **Release-manifest history preserved** — current values are `migrations=21`, `members=8`, `docs ~67`; historical `18`/ `7`/ `146 files` remain in docs with `HISTORICAL` label (see `ops/stale_claims.rs`).

---

## 8. Intentionally-Unexecuted Live Validations — NOT EXECUTED

| Validation | How to execute |
|---|---|
| Live Stripe webhook + checkout (`payment_succeeded`) | Set `STRIPE_API_KEY_REF` + `STRIPE_WEBHOOK_SECRET_REF` + `stripe` provider; run `stripe trigger payment_intent.succeeded` against `/api/saas/billing/webhooks/stripe`; verify `billing_state` moves to `Recovered` with `provider: payment_succeeded evt_` |
| Live Vault transit sign | Deploy Vault dev, set `VAULT_ADDR` + `VAULT_TOKEN_REF`, configure `CustodyProviderConfig::Vault` with `vault/transit` path, call `resolve_active_signer` → `sign_message` |
| Live KMS / HSM sign | Configure `KMS_KEY_ID` ARN / HSM slot, probe `health_report` → `Ready` |
| `solana-test-validator` staking e2e (3 tests) | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` (historical 3/3 at 2026-09-18; excluded workspace — the `cd` is required) |
| `docker compose up` → `/api/health` | `docker compose up -d && curl localhost:3000/api/health` (no daemon in sandbox) |
| Funded live trading | Fund wallet, set `execution.allow_live_trading=true` explicitly, run canary |

**A fixture that says `payment_succeeded` is not live proof.**

---

## 9. External Dependencies

| Dependency | Need | Evidence |
|---|---|---|
| PostgreSQL ≥16 | durable SaaS (orgs, memberships, subscriptions, entitlements, provider_events, custody, jobs, audit hash-chain, HA leases) | `docker-compose.yml` + `DATABASE_URL`; `db_integration 26/26` historical with PG 17.11 |
| Redis 7 | dedup L2, lease/handoff, rate limiter | `REDIS_URL`; 10/10 historical |
| Solana RPC/WS/Geyser | sniper/copy, Polymarket CTF | `RPC_URL`/`WS_URL`; mock-verified |
| Stripe/Paddle | billing live | adapter + end-to-end wiring done (bounded 15 s timeout, session id required, errors redacted, no panics); creds `NOT CONFIGURED`; live execution `NOT_RUN` |
| PostgreSQL (service-backed) | checkout durability/concurrency, provider-success persistence, invoice reads | real PostgreSQL 17.11 with migrations `0001`–`0022`: catalog-confirmed unique constraints + all concurrency scenarios pass |
| Vault/KMS/HSM | custody remote signing | refs done, backends `NOT CONFIGURED` |
| Telegram bot token | module 5 | `TELEGRAM_BOT_TOKEN` external |

---

## 10. IP / License Handover Checklist — REQUIRES BUYER ACTION

- [ ] Insert legal copyright holder into `LICENSE` (currently `sniper-suite authors`)
- [ ] Publish real security contact in `SECURITY.md`
- [ ] Set real repository URL in `Cargo.toml` (`repository` field)
- [ ] Generate final staking program keypair: `solana-keygen new` → `scripts/staking-identity.sh set-id <kp>` → `deploy` (placeholder `3vEEMMF...` must not ship)
- [ ] Commission independent external audit before mainnet
- [ ] Provide production infra: PG ≥16, Redis 7, funded keys, RPC/WS providers
- [ ] Execute `docker` build + GitHub Actions on real runners (local-equivalent evidence recorded)
- [ ] Fund live-trading validation under operator supervision (paper is default; `docs/LIVE-VALIDATION.md`)

---

## 11. Production-Deployment Checklist — NOT EXECUTED

See `docs/BUYER-DEPLOYMENT.md §14` + `ops/release_readiness.rs` + `ops/health_report.rs`:

1. `cargo fmt --all -- --check` → PASS
2. `cargo check --workspace` → 0 errors
3. `cargo test --workspace -- --test-threads=1` with PostgreSQL 17.11 → PASS — **executed 2026-09-26: 70 suites / 2077 passed / 0 failed / 13 ignored**; hermetic subset `bot-core` 507 + `saas-sdk` 32 + `sniper-suite` 105; Redis-gated 10 remain `NOT_RUN` without `REDIS_URL`
4. `npm ci && npm run typecheck && npm run build` → PASS
5. `release-manifest.json` consistent with `BUYER-TRUTH-REGISTER` (8 members, 22 migrations, 101 docs)
6. `stale_claims` scan → 0 non-historical
7. `secrets_scan` → PASS
8. `health_report` overall `Healthy` (or `Degraded` with reason)
9. Operator attests via `ops/audit_attestation.rs` (`evidence_snapshot` hash → `Attestation` → `verify`)
10. Deploy + smoke: `curl /api/health` + `curl /api/saas/readiness` (public safe, operator redacted)

---

## 12. How to Re-verify

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test -p bot-core --lib           # 507
cargo test -p saas-sdk                 # 32
cargo test -p sniper-suite --no-run    # compiles
cd apps/control-plane && npm ci --ignore-scripts && npm run typecheck && npm run build
grep -R "21 migrations\|21 migrations\|146 files\|production verified" docs | grep -v HISTORICAL
cargo test --workspace -- --test-threads=1  # full, needs PG/Redis
```

---

---

## 13. Batch7 External Validation Harness — HARNESS READY, LIVE NOT_RUN (2026-09-24)

> **New counts:** `343 rs` (317+23), `1314 tests` (Batch-7 snapshot, superseded — current 1331), `101 docs` (96 at Batch 7 → 101 now), `22 migrations` (unchanged). **24 new files** (exact): see `release-manifest.json:components.external_validation_batch7_24` + wiring `crates/server/src/lib.rs` (library crate for `sniper_suite::` in integration tests). **Harness harness:** `scripts/run-external-validation.sh` (9.3K, modes 7) + `docs/EXTERNAL-VALIDATION-RUNBOOK.md` (13K, 7 sections). **Verify harness:** `bash scripts/run-external-validation.sh all-safe` → `6/6 NOT_RUN` in hermetic (redacted `evidence/external/*.json` with `validation_id/provider/env/timestamp/command/status/evidence_hash/redacted metadata`, no `DATABASE_URL`/`REDIS_URL`/keys/secrets, SHA256); `cargo test -p sniper-suite --lib 224/224` + `cargo test -p sniper-suite --test provider_contracts|deployment_smoke|solana_contract|staking_contract 28/28` + `live_billing_contract|live_custody_contract 4/4 (+6 ignored live)`; `cargo clippy --workspace --all-targets -- -D warnings` clean.

> _Batch 10 (2026-09-27) update: evidence records now also carry `gap_id`, and `evidence_hash` is canonical (timestamp excluded, deterministic). The `9.3K`/`13K` sizes above are the Batch-7 snapshot; both files have grown since. The Batch-7 `224/224` lib and
`28/28` harness figures are likewise the 2026-09-24 snapshot; the live re-run is `cargo test -p sniper-suite --lib` =
263 passed / 0 failed / 3 ignored and the six integration harnesses 32 passed (6 ignored live). Schema + hash rule: `docs/EXTERNAL-VALIDATION-RUNBOOK.md` §0/§7._

| Area | Harness (24) | Truthful live status | Evidence that harness works (hermetic) | Buyer live command (per runbook) |
|---|---|---|---|---|
| Provider contract model | `ops/provider_contract.rs` canonical model (name/capability/required refs/command/expected evidence/status 5 tests) + `ops/provider_contract_runner.rs` explicit-enable+timeout+failure classification 4 tests + `ops/network_policy.rs` 4 tests | **NEVER default PASS** — status is PASS/FAIL/NOT_RUN/EXTERNAL_REQUIRED/BLOCKED per evidence, never invented; runner never silently enables live | `cargo test --test provider_contracts` → `missing_credentials_external_required` PASS, `provider_contract_never_default_pass` asserts no default PASS | `cargo test --test provider_contracts -- --nocapture` |
| Deployment smoke (GAP-003) | `ops/deployment_smoke.rs` 5 tests + `tests/deployment_smoke.rs` 5 tests (PRODUCTION_SMOKE) | **SMOKE HARNESS READY NOT_RUN** — never claim deployment without tested URL; validates /api/health, readiness, OpenAPI, migration state, Redis where required, CORS/security headers, frontend | absent `DEPLOYMENT_BASE_URL` → NOT_RUN, empty → EXTERNAL_REQUIRED, with URL → checks EXTERNAL_REQUIRED not PASS in hermetic | `DEPLOYMENT_BASE_URL=https://prod.example.com bash scripts/run-external-validation.sh deployment` |
| Solana | `solana/connection_contract.rs` 5 tests + `geyser_contract.rs` 4 tests + `tests/solana_contract.rs` 7 tests (LIVE_EXTERNAL read-only) | **READ-ONLY HARNESS READY NOT_RUN** — RPC reachable/auth/latency/slot read-only, Geyser subscription decode read-only, no trading, no creds exposure | absent RPC/WS → NOT_RUN/EXTERNAL_REQUIRED; with URL → NOT_RUN in hermetic, never PASS without real network | `RPC_URL=... WS_URL=... cargo test --test solana_contract` |
| Staking (GAP-005) | `staking/deployment_contract.rs` 4 tests + `validator_contract.rs` 4 tests + `tests/staking_contract.rs` 7 tests (LIVE_EXTERNAL) | **HARNESS READY NOT_RUN** — program ID/account executable/hash with real RPC evidence; validator wrapper only when `STAKING_E2E=1` else NOT_RUN; placeholder BLOCKED | `STAKING_E2E!=1` → NOT_RUN, placeholder `3vEEMM…` → BLOCKED | `STAKING_E2E=1 cargo test --test staking_contract` / `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e` |
| Billing live (GAP-001) | `billing/live_provider_contract.rs` 4 tests + `live_provider_fixture.rs` 3 tests + `tests/live_billing_contract.rs` 5 tests (3 ignored LIVE_EXTERNAL) | **ADAPTER READY LIVE NOT_RUN** — provider-neutral credentials/signature/checkout/event/idempotency/subscription sync, no hardcoded success; fixtures NON-LIVE | `LIVE_BILLING!=1` → NOT_RUN, invalid creds → EXTERNAL_REQUIRED, valid → NOT_RUN in hermetic | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored` |
| Custody live (GAP-002) | `custody/live_provider_contract.rs` 5 tests + `live_provider_fixture.rs` 5 tests + `tests/live_custody_contract.rs` 5 tests (3 ignored LIVE_EXTERNAL) | **BOUNDARY READY NOT_RUN** — Vault/KMS/HSM credential_ref_valid/sign_capability, no local fallback, no private key extraction; fixtures NON-LIVE | `LIVE_CUSTODY!=1` → NOT_RUN, invalid → EXTERNAL_REQUIRED, no sign → FAIL/CANNOT_SIGN | `VAULT_ADDR=... LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored` |
| Evidence & live/funded gates (GAP-004 guard) | `ops/external_evidence.rs` 4 tests + `external_evidence_verify.rs` 4 tests + `ops/live_gate.rs` 5 tests + `ops/funded_mode_guard.rs` 5 tests + `scripts/run-external-validation.sh` | **HARNESS READY** — every result has `validation_id/gap_id/provider/environment/timestamp/command/mode/status/evidence_hash/redacted_metadata/endpoint_ref` with SHA256 (canonical: timestamp excluded); verify rejects tampered never upgrades NOT_RUN to PASS; live_gate fail-closed; funded guard never default live-funded | `cargo test --test provider_contracts` verifies every saved evidence file (hash+schema), tamper fails; `bash scripts/run-external-validation.sh all-safe` 6/6 NOT_RUN, no funded trade | `bash scripts/run-external-validation.sh all-safe` + `cargo test --test provider_contracts` for evidence verification; runbook per validation for live |
| External audit boundary (GAP-006) | `ops/external_validation.rs` audit slot + `docs/FINAL-BUYER-GAP-LEDGER.md` GAP-006 row + `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006 handover slot | **EXTERNAL_REQUIRED / BUYER_ACTION** — no audit performed by the seller; no report exists to ship. Boundary is code-enforced: `audit NOT DONE` and the phrase scanner (`ops/stale_claims.rs`) block `externally audited / independently audited / penetration-tested / security-certified`. | Hermetic: ledger + runbook slot only (deliberately empty until a real auditor deliverable exists) | `n/a — external auditor deliverable (handover slot: docs/EXTERNAL-VALIDATION-RUNBOOK.md § GAP-006)` |

**Updated verification:** `release-manifest.json` §batch7_delivery + §external_validation_batch7_24 list exact 24 paths; current counts 343 Rust / 101 docs / 1331 tests / 22 migrations; harness invariants (§5) are now code-enforced. Historical `21 migrations / 146 files` remain labeled HISTORICAL. No live Stripe/Paddle/Vault/KMS/HSM/production/funded/audit claim without the §13 live gates.

**This evidence pack separates:** `VERIFIED` (hermetic code + tests) / `PARTIAL` (boundary done, live creds missing) / `NOT EXECUTED` (external / production) / `REQUIRES BUYER/OPERATOR ACTION`. No marketing claim is disguised as evidence. Batch7 makes external validation **reproducible** (harness + runbook + script) but live external systems remain **NOT_RUN until buyer provisions per runbook**.

