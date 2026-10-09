# Final Buyer Handover — sniper-suite 0.1.0

> 2026-09-26 · `0.1.0` · `343 rs` · `101 docs` · `22 migrations` · `1331 tests` · `8 members` · `1.98.1` · `CODE COMPLETE FOR CURRENT SCOPE` / `RELEASE VERIFIED` / `BUYER PACKAGE VERIFIED` / `EXTERNAL NOT YET VERIFIED` / `LEGAL REVIEW WHERE DOCUMENTED`
> Distinguishes **SELLER DELIVERS** / **BUYER CONFIGURES** / **BUYER VERIFIES** / **EXTERNAL AUDITOR VERIFIES**.

## 0. Package identity (frozen handover)

| Item | Value / where to verify |
|---|---|
| Release version | `0.1.0` — `cat VERSION` (equals `Cargo.toml`, `release-manifest.json`, `apps/control-plane/package.json`) |
| Package | `buyer-release/` — version `0.1.0`, file count **679** (`find buyer-release -type f | wc -l`) |
| Source tree | 561 files under `buyer-release/source/` (`# files:` line in `checksums/SOURCE-TREE.sha256`) |
| Source digest | the exact SHA-256 of this delivered build is written in `buyer-release/checksums/SOURCE-TREE.sha256` and reproduced by the freeze record supplied with this handover; recompute with `cd buyer-release/source && find . -type f | sort | xargs sha256sum | sha256sum` |
| Integrity layers | `checksums/SHA256SUMS` (`sha256sum -c`), `checksums/all-files.sha256` (every package file except itself), `checksums/SOURCE-TREE.sha256` |

## A. What is being delivered
* **SELLER DELIVERS:** Source (`crates/` 343 rs + `programs/staking-suite` + `apps/control-plane` Next.js control plane), docs (101 md), migrations (22), scripts (10 sh), config (`config.toml.example`, `docker-compose.yml`, `Dockerfile`, `deny.toml`, `VERSION`, `LICENSE`, `CHANGELOG.md`), SBOM/license artifacts, `release-manifest.json`, `buyer-release/` package. **No hosted deployment, no funded trading, no external audit** — explicitly NOT delivered.

## B. Source code
* **SELLER DELIVERS:** `crates/core`, `crates/solana-kit`, `crates/module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`, `crates/server` (Axum control plane — endpoint inventory in `docs/API.md`), `crates/saas-sdk` (typed billing/custody/commercial), `programs/staking-suite` (host-verified, separate lockfile)
* **BUYER VERIFIES:** `find crates -name "*.rs" | wc -l` → 343, `diff -rq crates buyer-release/source/crates` → 0

## C. Documentation
* **SELLER DELIVERS:** 101 docs — architecture, security (threat model, controls matrix), deployment, operations, backup/restore, billing/custody SAAS, data room. `docs/FINAL-BUYER-DATA-ROOM.md` is the one-page buyer navigation (14 sections with actual paths).
* **BUYER VERIFIES:** `ls docs | wc -l` → 101, `bash scripts/verify-delivery.sh` 7/7 (required files, document links, hygiene)

## D. Build/toolchain
* **SELLER DELIVERS:** `rust-toolchain.toml` `1.98.1` (+ clippy + rustfmt), `Cargo.lock` `5a053d8f…`, `apps/control-plane/package-lock.json` 6171 lines v3, `npm 10.8.2` pinned
* **BUYER CONFIGURES:** `rustup toolchain install 1.98.1`, `cd apps/control-plane && npm ci --ignore-scripts`
* **BUYER VERIFIES:** `cargo fmt --all -- --check` PASS, `cargo check --workspace` PASS, `npm run typecheck` PASS, `npm run build` PASS

## E. SaaS/control plane
* **SELLER DELIVERS:** Axum control plane (`crates/server/src/main.rs` 1440 lines, `api.rs` 34 routes, `saas/` 9+ services, `security/` 6 files, `ops/` 41 files), tenant isolation 5-tuple (auth+membership+permission+org+lifecycle), WebSocket canonical `header/first-frame` + legacy `Disabled` default
* **BUYER CONFIGURES:** `DATABASE_URL` (Postgres 16+), `REDIS_URL` (Redis 7), `RPC_URL`/`WS_URL` if trading, `TELEGRAM_BOT_TOKEN` if needed
* **BUYER VERIFIES:** `cargo test -p sniper-suite --lib` 263/263 (3 ignored), `cargo test -p saas-sdk` 32/32

## F. Billing
* **SELLER DELIVERS:** `core/billing/` (plan, subscription, entitlement, usage, checkout, payment, invoice, dunning 7-state, usage_policy, provider_config, reconciliation) + `server/saas/{billing,billing_status,checkout,invoices,payment_webhooks,commercial_state,usage_limits}` — server-authoritative price (no client amount), webhook HMAC + idempotency, reconciliation never invents success
* **BUYER CONFIGURES:** `STRIPE_API_KEY` / `PADDLE_API_KEY` + `SAAS_WEBHOOK_SECRET_STRIPE/PADDLE` for live
* **BUYER VERIFIES:** `LIVE_BILLING=1 cargo test --test live_billing_contract -- --ignored` (without → NOT_RUN hermetic)
* **EXTERNAL AUDITOR VERIFIES:** live payment flow with funded provider account (not claimed)

## G. Custody
* **SELLER DELIVERS:** `core/custody/` (model, provider, credentials, health, rotation) + `server/saas/custody*` — Vault/KMS/HSM boundary, fail-closed (no local fallback when remote selected), `HealthState` safe metadata only, rotation `Pending→Active→Draining→Revoked` (force explicit)
* **BUYER CONFIGURES:** `VAULT_ADDR` + `VAULT_TOKEN` / `KMS_KEY_ID` / HSM, `CREDENTIAL_REF=env_var:...`, `LIVE_CUSTODY=1`
* **BUYER VERIFIES:** `LIVE_CUSTODY=1 cargo test --test live_custody_contract -- --ignored` (without → NOT_RUN)
* **EXTERNAL AUDITOR VERIFIES:** remote signing liveness (not claimed)

## H. Database/Redis
* **SELLER DELIVERS:** 22 migrations forward-only (`sqlx migrate run`), `docker-compose.yml` templates, harnesses `db_integration` (26) / `redis_integration` (10)
* **BUYER CONFIGURES:** Postgres 16+ (`DATABASE_URL=postgres://...`) + Redis 7 (`REDIS_URL=redis://...`)
* **BUYER VERIFIES:** `POSTGRES_URL=... cargo test --test db_integration` + `REDIS_URL=... cargo test --test redis_integration` — PostgreSQL **executed 2026-09-26** against real PostgreSQL 17.11 (26/26 + full workspace 2077 passed); Redis remains **NOT_RUN** in this sandbox, harness READY
* **EXTERNAL AUDITOR VERIFIES:** production load/HA (not in this delivery)

## I. Backup/restore
* **SELLER DELIVERS:** `server/backup/{export_manifest,restore_manifest,preflight,commands}.rs` (5 files) + `docs/BACKUP-RESTORE.md` + `docs/OPERATIONS-RUNBOOK.md` — manifests strict, `pg_dump/pg_restore` via `DATABASE_URL` env var (never inline URL), preflight SHA check
* **BUYER CONFIGURES:** `pg_dump`/`pg_restore` path, `DATABASE_URL` for dump/restore
* **BUYER VERIFIES:** `cargo test --test backup_restore_integration` hermetic 3+3+4 PASS; live `DATABASE_URL=... pg_dump ...` → NOT_RUN without DB

## J. Release verification
* **SELLER DELIVERS:** `scripts/{verify-delivery,verify-buyer-package,final-release-check,build-release-package,run-external-validation,release-evidence,generate-sbom,generate-license-report}.sh` + `docs/BUYER-VERIFICATION-SCRIPT.md`
* **BUYER VERIFIES:** `bash scripts/verify-delivery.sh` 7/7 → `bash scripts/verify-buyer-package.sh` PASS → `bash scripts/final-release-check.sh` 8/8 ALL PASS → `bash scripts/build-release-package.sh` 0 diff + `SHA256SUMS`
* **EXTERNAL AUDITOR VERIFIES:** `bash scripts/run-external-validation.sh all-safe` 6/6 NOT_RUN (redacted evidence)

## K. SBOM/licenses
* **SELLER DELIVERS:** `sbom.json` 34K 200 comps `fd837e42…`, `sbom.cyclonedx.json` `fd837e42…`, `licenses.json` 107K 707 entries `c1c051ca…`, `crates/server/src/ops/{sbom_report,license_report}.rs`
* **BUYER VERIFIES:** `sha256sum sbom.json licenses.json` match `buyer-release/checksums/SHA256SUMS` (run `cd buyer-release && sha256sum -c checksums/SHA256SUMS`; the per-file values are also inside `checksums/all-files.sha256`)
* **LEGAL-REVIEW:** UNKNOWN (3) stays UNKNOWN — not guessed, `docs/OPEN-SOURCE-COMPLIANCE.md` → LEGAL REVIEW

## L. IP/license checklist
* **SELLER DELIVERS:** `LICENSE` MIT generic `sniper-suite authors`, `docs/IP-OWNERSHIP-REGISTER.md` per-component, `docs/IP-HANDOVER-CHECKLIST.md` seller vs buyer, `docs/FINAL-IP-AND-THIRD-PARTY-INVENTORY.md`
* **BUYER CONFIGURES:** Set real legal entity in `LICENSE`, set `Cargo.toml` `repository` URL (currently removed placeholder)
* **EXTERNAL AUDITOR VERIFIES:** `LEGAL-REVIEW` for copyright holder, trademark/domain NOT INCLUDED per `docs/TRADEMARK-DOMAIN-REGISTER.md`

## M. External validations
* **SELLER DELIVERS:** HARNESS READY (24 files: 8 `ops` + 2 solana +2 staking +2 billing +2 custody +6 integration tests + script + runbook) — **every result** has `validation_id/gap_id/provider/environment/timestamp/command/mode/status/evidence_hash/redacted_metadata/endpoint_ref` (Batch 10: canonical timestamp-excluded hash, verifiable with `cargo test --test provider_contracts`)
* **BUYER VERIFIES:** `bash scripts/run-external-validation.sh all-safe` → 6/6 NOT_RUN in hermetic (no live creds) — evidence `evidence/external/*.json` SHA256
* **EXTERNAL AUDITOR VERIFIES:** live execution with real credentials/URLs/validator/funded wallet/audit report — not claimed


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

## N. Known limitations
* **SELLER DELIVERS:** `docs/KNOWN-LIMITATIONS.md` 16 rows + `docs/FINAL-KNOWN-LIMITATIONS.md` + `docs/FINAL-BUYER-GAP-LEDGER.md` 6 gaps — billing live, custody live, prod deploy, funded trading, staking E2E, external audit, Redis-gated NOT_RUN (PostgreSQL executed 2026-09-26), UNKNOWN licenses, trademark NOT INCLUDED, placeholder program_id, generic LICENSE, missing repository URL, RPC/Geyser live, `target/.next` hygiene (now INFO not limitation)
* **BUYER ACTION:** provision services, set env, set-id with buyer keypair, commission audit

## O. Buyer actions after transfer
1. **SELLER DELIVERS** already: code + docs + package + checksums
2. **BUYER CONFIGURES:** provision Postgres 16 + Redis 7 + RPC/Geyser + OTLP, set `.env` per `ENVIRONMENT-VARIABLE-REGISTER.md`, set `LICENSE` holder + `repository` URL + `set-id` for staking
3. **BUYER VERIFIES:** `bash scripts/verify-delivery.sh` (7/7) → `final-release-check.sh` (8/8) → optional `POSTGRES_URL/REDIS_URL` service tests → `run-external-validation.sh` live modes after credentials → `docker build -t sniper-suite:prod . && curl /api/health`
4. **EXTERNAL AUDITOR VERIFIES:** security audit report before mainnet staking — deliverable paths (report/findings/remediation/retest/sign-off) in `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006.

## P. Buyer first steps (in order)

1. **Verify checksums** — `cd buyer-release && sha256sum -c checksums/SHA256SUMS` (8/8) and `sha256sum -c checksums/all-files.sha256` (all entries OK)
2. **Verify package layout** — `bash source/scripts/verify-delivery.sh` → `7 PASS / 0 FAIL`, then `bash source/scripts/verify-buyer-package.sh` → `PASS`
3. **Verify version / manifest** — `cat VERSION` (`0.1.0`) equals `manifests/release-manifest.json` `version`; `python3 -c "import json;print(json.load(open('manifests/release-manifest.json'))['docs_files'])"` → 101
4. **Install frontend dependencies** — `cd source/apps/control-plane && npm ci --ignore-scripts` (354 packages) and `npm audit` (0 vulnerabilities)
5. **Run frontend gates** — `npm run typecheck` (PASS), `npm run lint` (exit 0, 12 documented warnings), `npm run build` (5 static routes)
6. **Run Rust verification** — `cd source && cargo fmt --all --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p sniper-suite --lib`; then the four harnesses (`release_manifest_integration`, `buyer_package_integration`, `provider_contracts`, `deployment_smoke`)
7. **Configure buyer infrastructure** — Postgres 16+/Redis 7/RPC/secrets per `docs/BUYER-DEPLOYMENT.md` §4–§8 and `docs/ENVIRONMENT-VARIABLE-REGISTER.md` (secrets outside the repository)
8. **Execute the external validation runbook** — `docs/EXTERNAL-VALIDATION-RUNBOOK.md` §0.1 (canonical table) → provision each prerequisite → run the exact command → save evidence; `bash source/scripts/run-external-validation.sh all-safe` stays the credential-free baseline (6/6 `NOT_RUN`)

## Q. Evidence retention (buyer)

Keep the following **outside the extracted package** (e.g. `buyer-evidence/<date>/`), together with a
hash list (`find buyer-evidence -type f -exec sha256sum {} + > buyer-evidence/SHA256SUMS`), so the
package itself stays immutable:

* **logs** — the full output of every script and `cargo`/`npm` command you ran (including failures)
* **provider responses** — Stripe/Paddle ids, webhook event ids and dashboard exports (no secrets)
* **deployment URLs** — the `DEPLOYMENT_BASE_URL` values tested, with timestamps
* **validator output** — staking E2E logs, program id, binary hash, slot
* **audit reports** — report, findings register, remediation/retest evidence and the auditor sign-off (GAP-006 slot)
* **evidence hashes** — the `evidence/external/*.json` files as generated locally, and their `evidence_hash` values

Never store secrets, private keys or seed phrases in the evidence set; the redaction rules in
`docs/EXTERNAL-VALIDATION-RUNBOOK.md` § Security Precautions apply to buyer-generated files too.

> **Overall:** **SELLER** delivered 343 rs + 101 docs + 22 migrations + 1331 tests + toolchain + SBOM + harness + package **VERIFIED**; **BUYER** configures live secrets/infra; **EXTERNAL** live/audit remains NOT YET VERIFIED; **LEGAL** review where documented. No `100% production ready`, `fully audited`, `live verified`, `mainnet ready` without evidence.
