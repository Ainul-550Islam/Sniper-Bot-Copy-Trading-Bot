# BUYER HANDOVER CHECKLIST — Sniper Suite 0.1.0

**Date:** 2026-09-23 — **Version:** 0.1.0 — **Migrations:** 22 (0022) — **Members:** 8

> Historical values with date labels only. No live Stripe/Paddle/Vault/KMS/HSM/production/deployment claims without execution.

## 1. SELLER PROVIDES

- [ ] Source delivery: git archive `sniper-suite.tar.gz` + `release-manifest.json` + `Cargo.lock` + `programs/staking-suite/Cargo.lock`
- [ ] Build environment: `rust-toolchain.toml` 1.98.1, `Dockerfile` 1.98.1-bookworm, `apps/control-plane` npm 10.8.2 + `package-lock.json` 211KB lockfileVersion 3
- [ ] Database: migrations `0001` → `0022` forward-only (22 files), `DATABASE_URL` required for production, `db_integration` 26/26 historical PG 17.11, now `postgres_saas_integration` harness
- [ ] Redis: optional, `REDIS_URL`, `redis_saas_integration` dedup/lease/rate-limit, restart ephemeral documented
- [ ] Env vars: see `deployment_preflight.rs` — `DATABASE_URL`, `REDIS_URL`, `CORS_ORIGINS`, secret refs (`*_REF`), `signer_mode`, `billing_provider`, `EXECUTION_MODE`
- [ ] Billing provider: Stripe/Paddle boundary `provider_config`, fixture deterministic, live **NOT EXECUTED**
- [ ] Custody provider: Vault/KMS/HSM refs, health fail-closed, rotation `Pending→Active→Draining→Revoked`, live **NOT EXECUTED**
- [ ] Wallets/public addresses: `crates/server/src/saas/wallet_access.rs` + `crates/core/src/custody` — public only, never private keys
- [ ] SBOM/license: `sbom_report.rs` + `license_report.rs`, `cargo metadata` source, unknown not invented, `needs_review` flagged
- [ ] Security evidence: `security_evidence.rs` 10 checks (`secret_scan` … `external_audit NOT_RUN`), never redacts to PASS
- [ ] Backup/restore: `backup_verification.rs` + `restore_verification.rs` (`NOT_EXECUTED` default) + `BACKUP-RESTORE.md` documented vs executed
- [ ] DR plan: `dr_recovery_plan.rs` RPO 3600 RTO 14400, restore_order, secrets reattachment redacted, `documented=true demonstrated=false` until real drill
- [ ] Incident/audit: `incident_evidence.rs` + `operator_actions.rs` (sensitive require reason, redact), append-only
- [ ] Release artifact: `release_artifact.rs` + `release_artifact_verify.rs` sha256 independent compute, `release-evidence.sh` → `release-evidence/` with `PASS/WARN/BLOCK/NOT_RUN`

## 2. BUYER CONFIGURES

- [ ] Insert legal copyright holder into `LICENSE` (currently `sniper-suite authors`)
- [ ] Publish real security contact in `SECURITY.md`
- [ ] Set real `repository` URL in `Cargo.toml`
- [ ] Provide production infra: PostgreSQL ≥16, Redis 7, RPC/WS, funded keys if live trading
- [ ] Configure `DATABASE_URL` + `REDIS_URL` + `CORS_ORIGINS` (no `*` in prod) + `*_REF` secret refs (Vault ARN/env)
- [ ] Choose `signer_mode` (`local` paper, `vault`/`kms`/`hsm` live) — fail closed if misconfigured, no local fallback
- [ ] Choose `billing_provider` (`manual` vs `stripe`/`paddle`) — provide webhook secrets only via refs, test fixture before live
- [ ] Generate staking keypair: `solana-keygen new` → `scripts/staking-identity.sh set-id <kp>` → `cargo build-sbf`
- [ ] Set `telemetry` / `RUST_LOG` per `observability` config

## 3. BUYER MUST VERIFY

- [ ] Run `scripts/release-evidence.sh` → inspect `release-evidence/summary.json` (`format:check:clippy:test:postgres:redis:frontend:secret:stale:manifest:package` each `PASS/WARN/BLOCK/NOT_RUN`)
- [ ] Run `scripts/verify-buyer-package.sh` → `PASS` (VERSION + Cargo.toml + manifest 0.1.0 consistent, 22 migrations, `BUYER-TRUTH-REGISTER` + `BUYER-EVIDENCE-PACK` + `BUYER-HANDOVER-CHECKLIST` present)
- [ ] Run `scripts/verify-delivery.sh` + `scripts/release-check.sh` → `PASS`
- [ ] Execute Postgres integration: `POSTGRES_URL=postgres://... cargo test --test postgres_saas_integration -- --nocapture` → expect `PASS` when PG up, else `NOT_RUN` (not `FAIL`)
- [ ] Execute Redis integration: `REDIS_URL=redis://... cargo test --test redis_saas_integration` → `PASS` or `NOT_RUN`
- [ ] Execute lifecycle/billing integration with PG: `cargo test --test tenant_lifecycle_integration --test billing_integration`
- [ ] Run frontend: `cd apps/control-plane && npm ci --ignore-scripts && npm run typecheck && npm run build` → `PASS` (Next.js 16, 5 routes prerendered static)
- [ ] Perform real `pg_dump` → `pg_restore` to new DB, verify `migration 0022`, row counts, `/health` `/ready` smoke — record in `restore_verification.rs` as `Succeeded` (not `NOT_EXECUTED`)
- [ ] Perform DR drill per `dr_recovery_plan.rs` → record `demonstrated=true` and `last_drilled_at`
- [ ] Review `sbom_report` + `license_report` → legal review for `copyleft/unknown/unavailable`
- [ ] Commission **external security audit** before mainnet (currently `NOT DONE`; deliverable slot: `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006)
- [ ] Do **not** claim `production deployment` / `funded live trading` / `staking validator E2E` without execution — keep `NOT EXECUTED`

### Six tracked gaps — single source of truth (never inferred from hermetic tests)

Source: `crates/server/src/ops/final_gap_ledger.rs` + `docs/FINAL-BUYER-GAP-LEDGER.md`; live evidence records:
`evidence/external/*.json` (`NOT_RUN` in hermetic). None of the six is `VERIFIED`; the registry promotes only via
`mark_verified(id, evidence_ref, verified_at, detail)` after the real command ran in the required environment.
Documented commands are asserted to match the executable harnesses by the test `ledger_commands_match_documented_harnesses`.

| Gap | Area | Ledger status (hermetic) | Buyer/operator command or deliverable |
|---|---|---|---|
| GAP-001 | Live billing (Stripe / Paddle) | `EXTERNAL_REQUIRED` / `NOT_RUN` | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test -p sniper-suite --test live_billing_contract -- --ignored --nocapture` (Paddle: `PADDLE_API_KEY=...`) |
| GAP-002 | Remote custody (Vault / KMS / HSM) | `EXTERNAL_REQUIRED` / `NOT_RUN` | `LIVE_CUSTODY=1 VAULT_ADDR=... VAULT_TOKEN=... cargo test -p sniper-suite --test live_custody_contract -- --ignored --nocapture` |
| GAP-003 | Deployment smoke (staging / production) | `EXTERNAL_REQUIRED` / `NOT_RUN` | `DEPLOYMENT_BASE_URL=https://<real> cargo test -p sniper-suite --test deployment_smoke -- --nocapture` (missing URL ⇒ fail safe, no `localhost` substitution) |
| GAP-004 | Funded live trading transition | `EXTERNAL_REQUIRED` / operator-only | Guard evidence: `cargo test -p sniper-suite --lib funded_mode_guard`; funded result only from a supervised operator run |
| GAP-005 | Staking validator E2E | `EXTERNAL_REQUIRED` / `NOT_RUN` | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` |
| GAP-006 | External security audit | `EXTERNAL_REQUIRED` / `BUYER_ACTION` — no report exists | Handover slot: `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006 (findings/severity/remediation/retest/sign-off) |

All live modes: `bash scripts/run-external-validation.sh all-safe` → `6/6 NOT_RUN` in a credential-free sandbox (correct, not a failure).

## 4. Known Limitations (require buyer action, not gaps claimed done)

- Live Stripe/Paddle, Vault/KMS/HSM, staking E2E, external pentest, production docker, funded trading — **NOT EXECUTED** (see `BUYER-EVIDENCE-PACK §8` and `BUYER-TRUTH-REGISTER`)
- `external security audit` **NOT DONE** until report exists
- `last_verified_restore` is `null` until real restore — `backup_status` reports `retention_configured=true` but `last_verified_restore=None` = configured not verified

## 5. Checksums & Consistency

- `VERSION` 0.1.0 = `Cargo.toml` workspace.package.version = `release-manifest.json` version + `BUYER-EVIDENCE-PACK` + `BUYER-TRUTH-REGISTER`
- Migrations `0022` = `release-manifest.json` `database_migrations.count=22` = `ls crates/core/migrations/*.sql` 22
- Members 8 = `Cargo.toml` members 8 = `release-manifest.json` 8
- `cargo check --workspace` + `cargo test --workspace` (hermetic 507+27) + `npm ci/typecheck/build` must PASS before `release-evidence.sh`

## 6. How to Reproduce

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p bot-core --lib       # 507
cargo test -p saas-sdk             # 27
cargo test -p sniper-suite --no-run
POSTGRES_URL=postgres://postgres:pass@127.0.0.1:5432/postgres cargo test --test postgres_saas_integration -- --nocapture
REDIS_URL=redis://127.0.0.1:6379 cargo test --test redis_saas_integration -- --nocapture
cd apps/control-plane && npm ci --ignore-scripts && npm run typecheck && npm run build
./scripts/release-evidence.sh
./scripts/verify-buyer-package.sh
./scripts/verify-delivery.sh
```

*Documentation is not evidence of a restore. Code is not evidence of live provider success.*
