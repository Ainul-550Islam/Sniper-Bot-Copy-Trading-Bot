# Transaction Readiness Report — sniper-suite 0.1.0

> Evidence, not sales guarantee. No valuation claim, no “guaranteed $20k-$60k” language.

**Snapshot (live):** Version 0.1.0 · <!-- stat:migrations -->54<!-- /stat --> migrations · <!-- stat:rust_files -->675<!-- /stat --> Rust sources under `crates/` · <!-- stat:crates -->8<!-- /stat --> workspace members · <!-- stat:docs_canonical -->101<!-- /stat --> docs · <!-- stat:test_attrs_plain -->2061<!-- /stat --> `#[test]` attributes (a static count, not a run result) · `rust-toolchain.toml` pinned — historical snapshots quoted dated counts that no longer matched the tree and were removed; `docs/STATS.md` is now the single source

## 1. Code Completeness

| Area | Files | Tests | Status |
|---|---|---|---|
| Core | `crates/core/src/{config,state,billing/*,custody/*,db,ownership}` + `crates/solana-kit` | `pricing 7`, `provider_events 7`, `reconciliation 11`, `billing_state 6`, `dunning 8`, `usage_policy 8`, `provider_config 8`, `custody/*` 7+5+7 | **COMPLETE** (hermetic) |
| Control plane | `crates/server/src/{main.rs,api.rs,ops (41),backup (5),saas (9+),security (6)}` | `observability 6`, `metrics 4`, `trace 5`, `rate_limit 4`, `container 3`, `repro 4`, `config_diff 5`, `external_validation 5`, `gap_ledger 5`, `backup 3+3+3+4` | **COMPLETE** |
| Trading modules | `module-sniper,module-copy,module-polymarket,module-telegram` | `module-sniper` host, `module-polymarket` mock CLOB | **COMPLETE** (dry_run) |
| Staking program | `programs/staking-suite/src/lib.rs` (no built .so committed) | host suite exists (no run log ships), `validator_e2e` gated `STAKING_E2E=1` NOT_EXECUTED | **COMPLETE** (host) / EXTERNAL (E2E) |
| Frontend | `apps/control-plane` Next.js 16, 5 routes | `npm ci` 354 pkgs, `typecheck` PASS, `build` PASS | **COMPLETE** |
| Migrations | `crates/core/migrations/0001–0022` forward-only | 22 contiguous | **COMPLETE** |

*Evidence:* `cargo check --workspace` and `cargo test -p saas-sdk` harnesses exist in-tree (no run logs ship — run them yourself), `cargo fmt --check` runnable, `find crates -name "*.rs" | wc -l` → live value in `docs/STATS.md` (`rust_files`); historical pass counts were removed.

## 2. Security Evidence

- **Threat model:** `docs/SECURITY-THREAT-MODEL.md` (11 areas, controls + remaining exposure)
- **Controls matrix:** `docs/SECURITY-CONTROLS-MATRIX.md` (PASS/PARTIAL/NOT_EXECUTED/EXTERNAL_REQUIRED per control)
- **Pentest readiness:** `docs/PENETRATION-TEST-READINESS.md` (attack surface, test accounts, roles, endpoints) — **no pentest has occurred**, internal `cargo audit`/`cargo deny` only.
- **Secret hygiene:** `.gitignore` + `is_secret_like` redaction + `secret_scan` (BEGIN PRIVATE KEY 0), `saas-sdk` secret-free Debug, `verify-buyer-package` PASS.

**Untested:** live Stripe/Paddle, live Vault/KMS/HSM, production deployment, funded trading, staking E2E, external audit — all `EXTERNAL_REQUIRED`.

## 3. Commercial SaaS

- **Plans/tenants:** 8 roles, 22+1 permissions, 7 provisioning states, 4 plan tiers, 12-table `0001_saas_control_plane` (Batch1), 3-table `0018`, `0019_billing_provider`, `0020_custody`, `0021_lifecycle`, `0022_checkout_url` (MATERIAL-GAP batch) — all forward-only.
- **Billing:** Server-authoritative price, webhook HMAC + idempotency, reconciliation never invents success, dunning 7-state, usage 80/100%.
- **Custody:** Vault/KMS/hardware-security-module indirect refs, fail-closed (no local fallback), rotation safe.
- **Lifecycle:** `tenant_lifecycle`, `data_lifecycle`, `retention_worker` (purge after retention), `job_claim` SKIP LOCKED.

## 4. Deployment Readiness

- **Production matrix:** `docs/PRODUCTION-READINESS-MATRIX.md` — app READY, DB/Redis EXTERNAL_REQUIRED (Postgres 16+ / Redis 7), billing/custody EXTERNAL_REQUIRED, observability READY, backups READY (manifests), restore EXTERNAL_REQUIRED, frontend READY, CI READY, incident response READY.
- **Environments:** `docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md` — local hermetic & CI actually tested; staging/prod NOT_EXECUTED (buyer to provision).
- **Runbooks:** `docs/OPERATIONS-RUNBOOK.md` (startup/shutdown/migration/health/queue/billing/custody/WS/backup/restore), `docs/INCIDENT-RESPONSE-RUNBOOK.md`, `docs/ROLLBACK-RUNBOOK.md` (forward-only migrations flagged).

## 5. Operations

- **Health:** `GET /health` (version), `GET /ready` (state+leases+deps), `GET /metrics`, `GET /api/ha`
- **Runbook commands are real:** `sqlx migrate run`, `docker build -t sniper-suite:prod .`, `curl /health`, `bash scripts/verify-delivery.sh` (after hygiene fix), `bash scripts/build-release-package.sh` (excludes target/node_modules/.git/.env)
- **Backup/restore:** `backup/{export,restore}_manifest.rs` strict, `preflight.rs` sha check, `commands.rs` safe `pg_dump/pg_restore` (env var, never inline URL)

## 6. IP / Legal

- **License:** Proprietary (`LICENSE`; all rights reserved)
- **Ownership:** `docs/IP-OWNERSHIP-REGISTER.md` per-component origin/internal/external, `docs/IP-HANDOVER-CHECKLIST.md` seller vs buyer
- **Third-party:** `licenses.json` 707, `sbom.json` 200, `docs/THIRD-PARTY-SOFTWARE-INVENTORY.md` source-data only, `docs/OPEN-SOURCE-COMPLIANCE.md` permissive vs unknown (UNKNOWN stays unknown)
- **Trademark/domain:** `docs/TRADEMARK-DOMAIN-REGISTER.md` says **NOT INCLUDED / NOT VERIFIED** — never invented
- **Unresolved:** `LEGAL_REVIEW_REQUIRED` for copyright holder, repository URL placeholder, UNKNOWN licenses, program placeholder id.

## 7. Known Limitations (only genuine)

See `docs/KNOWN-LIMITATIONS.md` (16 rows): billing live, custody live, prod deployment, funded trading, staking E2E, external audit, Redis service-backed NOT_RUN without `REDIS_URL` (PostgreSQL executed 2026-09-26), UNKNOWN licenses, trademark/domain NOT INCLUDED, placeholder program id, etc. — **not** listing completed Batch1-5.

## 8. Buyer Actions (to go live)

1. Provision Postgres 16 + Redis 7 + RPC/Geyser + OTLP endpoint
2. Set env via `.env` (never commit) — see `docs/ENVIRONMENT-VARIABLE-REGISTER.md`
3. `bash scripts/verify-delivery.sh`, `cargo test --workspace -- --test-threads=1` (hermetic), `bash scripts/build-release-package.sh`
4. Optional service-backed: `POSTGRES_URL=... REDIS_URL=... cargo test --test db_integration` etc.
5. Provision Stripe/Paddle + Vault/KMS/hardware-security-module + Telegram token, then verify via `docs/BUYER-VERIFICATION-SCRIPT.md` EXTERNAL section (each requires buyer secret, never claimed here)

## 9. External Validations (NOT_EXECUTED)

| Validation | Status | Command |
|---|---|---|
| Stripe/Paddle LIVE | NOT_EXECUTED | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored` |
| Vault/KMS/HSM LIVE | NOT_EXECUTED | `LIVE_CUSTODY=1 VAULT_ADDR=... cargo test --test live_custody_contract -- --ignored` |
| Production deployment | NOT_EXECUTED | `DEPLOYMENT_BASE_URL=... cargo test --test deployment_smoke` (local `docker run` + `curl localhost` is not production verification) |
| Funded trading | NOT_EXECUTED | `cargo test -p sniper-suite --lib funded_mode_guard` (funded step operator-only) |
| Staking validator E2E | NOT_EXECUTED (host suite exists; no run log ships) | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` |
| External audit | NOT_EXECUTED | n/a — auditor deliverable (runbook § GAP-006 slot) |

*All default to `NOT_EXECUTED` in `crates/server/src/ops/external_validation.rs` — never auto-VERIFIED.*

> **Buyer status summary:** See `docs/DATA-ROOM-INDEX.md` for navigation and Section 12 below for `VERIFIED / PARTIAL / EXTERNAL REQUIRED / LEGAL REVIEW / BUYER ACTION` matrix.

## MATERIAL-GAP BATCH — billing transaction ordering (2026-09-26)

`POST /api/saas/checkout` now executes, in order:

- **A** authorize tenant (`organization()` lookup; closed org rejected)
- **B** validate plan against the server-side catalogue (client price never trusted)
- **C** validate provider (typed `PROVIDER_NOT_CONFIGURED`, never a silent Manual fallback)
- **D** validate idempotency key + redirect URLs
- **E** create the durable `pending` row (`INSERT … ON CONFLICT (organization_id,
  idempotency_key) DO NOTHING`; loser re-reads the winner)
- **F** call the provider adapter (Stripe/Paddle; gated by `LIVE_BILLING=1` + credentials)
- **G** on provider success: update the durable record (session id, checkout URL,
  status `open`) — tenant-scoped with `rows_affected == 1`
- **H** on provider failure: leave the durable row `pending`, record a failure audit event,
  return a typed error (503) — never a success claim
- **I** return the provider-backed record

**Partial-failure rule:** if the provider succeeds but the durable write fails, the call
returns a `reconciliation required` error (503) and an audit event is written. The system
never reports success without a durable record.
