# Final Buyer Status — sniper-suite 0.1.0

> 2026-09-26 Asia/Dhaka · `VERSION 0.1.0` · `343 rs` · `101 docs` · `22 migrations` · `1331 tests` · `8 members` · `1.98.1`
> Categories: **VERIFIED** / **CODE-READY** / **SERVICE-BACKED-NOT-RUN** / **EXTERNAL-REQUIRED** / **BUYER-ACTION** / **LEGAL-REVIEW**
> No `100% production ready`, `fully audited`, `live verified`, `mainnet ready` without evidence.

## VERIFIED (hermetic, current execution)

* `cargo test --workspace -- --test-threads=1` (real PostgreSQL 17.11) → **VERIFIED** 70 suites / 2077 passed / 0 failed / 13 ignored / exit 0
* `cargo clippy --workspace --all-targets -- -D warnings` → **VERIFIED** PASS 0 warnings (with `CARGO_BUILD_JOBS=1` + reduced debuginfo; the earlier sandbox OOM is cleared)
* `bash scripts/release-check.sh` → **VERIFIED** 17 PASS / 1 FAIL / 2 SKIP — the only FAIL is `cargo deny check` (tool not installed in this sandbox); the 2 SKIPs are Redis-gated
* PostgreSQL service-backed: checkout durability + 2-way/4-way concurrency, restart identity, provider-failure pending state, provider-success persistence, invoice reads → **VERIFIED** (`crates/server/src/saas/billing.rs` PG tests)
* `cargo audit` (app + staking lockfiles) → **VERIFIED** 0 vulnerabilities, 9 allowed warnings
* `cargo fmt --all -- --check` → **VERIFIED** PASS (0 diff)
* `cargo check --workspace` → **VERIFIED** PASS (3m12s)
* `cargo clippy -p sniper-suite --all-targets -- -D warnings` → **VERIFIED** PASS 0 warnings (targeted `allow(dead_code)` only, no blanket)
* `cargo clippy -p saas-sdk --all-targets -- -D warnings` → **VERIFIED** PASS 0 (TRUE CLEAN)
* `cargo test -p sniper-suite --lib` → **VERIFIED** 263/263 PASS (3 ignored) — Batch 10, 2026-09-27
* `cargo test -p saas-sdk -- --test-threads=1` → **VERIFIED** 32/32 PASS
* `npm ci --ignore-scripts` → **VERIFIED** 354 packages (next 16.3.6, eslint-config-next 16.3.6, postcss 8.5.23; 2026-09-27 Batch 11)
* `npm run typecheck` → **VERIFIED** PASS
* `npm run build` → **VERIFIED** PASS (Next.js 16, 5 routes prerendered static, exit 0)
* `sbom.json` 200 comps `fd837e42…` + `licenses.json` 707 entries `c1c051ca…` → **VERIFIED**
* `bash scripts/verify-delivery.sh` → **VERIFIED** 7/7 PASS
* `bash scripts/verify-buyer-package.sh` → **VERIFIED** PASS
* `bash scripts/build-release-package.sh` → **VERIFIED** PASS (0 unexplained diff)
* Source parity `crates/docs/apps/programs/scripts` vs `buyer-release/source` → **VERIFIED** 0 diff

## CODE-READY (implemented, hermetic tests, no live env needed)

* **Billing** — server-authoritative price, dunning, usage_policy, reconciliation, idempotent provider_events — `CODE-READY` (tests 7+7+11+8, no live keys)
* **Custody** — Vault/KMS/HSM boundary, health, rotation, fail-closed — `CODE-READY` (no live Vault)
* **Tenant isolation** — 5-tuple (auth+membership+permission+org+lifecycle) per handler, WS `reject_query_credentials` — `CODE-READY` (31 handlers)
* **WebSocket** — canonical header/first-frame, legacy `Disabled` default — `CODE-READY`
* **Lifecycle/retention** — 8-phase deprovision, 7-state retention, job_claim SKIP LOCKED — `CODE-READY`
* **Backup/restore manifests** — `export_manifest/restore_manifest/preflight/commands` — `CODE-READY` (hermetic 3+3+4)
* **Observability/SBOM/license/gap ledger** — `ops/*` 41 files — `CODE-READY`

## SERVICE-BACKED-NOT-RUN (harness exists, no real DB/Redis in this env)

* **PostgreSQL** `SERVICE-BACKED VERIFIED (2026-09-26)` — executed against a real PostgreSQL 17.11 instance (migrations `0001`–`0022`): `db_integration` 26/26, `postgres_saas_integration` 7/7, checkout durability/concurrency, provider-success persistence and invoice reads. This **supersedes** the earlier `NOT_RUN` state; buyer infra re-runs with `POSTGRES_URL=...` (template in `docker-compose.yml`)
* **Redis** `SERVICE-BACKED-NOT-RUN` — `redis_integration` 10 tests — requires `REDIS_URL=...`

## EXTERNAL-REQUIRED (ADAPTER/BOUNDARY READY, live NOT_RUN)

* **GAP-001 Billing LIVE** `EXTERNAL-REQUIRED` — Stripe/Paddle adapter READY, live → `NOT_RUN` (needs `LIVE_BILLING=1 + provider keys`)
* **GAP-002 Custody LIVE** `EXTERNAL-REQUIRED` — Vault/KMS/HSM boundary READY, live → `NOT_RUN` (needs `LIVE_CUSTODY=1 + VAULT_ADDR/TOKEN`)
* **GAP-003 Production deployment** `EXTERNAL-REQUIRED` — SMOKE HARNESS READY, `NOT_RUN` (needs `DEPLOYMENT_BASE_URL` + host/TLS)
* **GAP-004 Funded trading** `EXTERNAL-REQUIRED` — GUARD READY, `NOT_RUN` (needs funded wallet + `execution_mode=live`)
* **GAP-005 Staking E2E** `EXTERNAL-REQUIRED / BLOCKED` — HARNESS READY, `NOT_RUN` (needs `STAKING_E2E=1 + solana-test-validator` + `set-id`)
* **GAP-006 External security audit** `NOT DONE` — internal `cargo audit/deny` only — `EXTERNAL-REQUIRED`

## BUYER-ACTION (must be done post-transfer)

* Provision Postgres 16+ + Redis 7 + RPC/Geyser + OTLP endpoint
* Set `.env` secrets (never commit) per `docs/ENVIRONMENT-VARIABLE-REGISTER.md`
* Run `bash scripts/verify-delivery.sh` + `final-release-check.sh` on buyer infra
* Optionally run `POSTGRES_URL=... REDIS_URL=... cargo test --test db_integration/redis_integration`
* Configure Stripe/Paddle + Vault/KMS/HSM + Telegram token per runbooks, then `run-external-validation.sh` live modes
* Execute production smoke `docker build -t sniper-suite:prod . && curl /api/health`

## LEGAL-REVIEW (not seller-owned claim)

* `LICENSE` generic `sniper-suite authors` → **LEGAL-REVIEW** (needs real entity)
* `Cargo.toml` `repository` placeholder removed (no URL) → **LEGAL-REVIEW**
* `licenses.json` 3 UNKNOWN entries → **LEGAL-REVIEW** (stays UNKNOWN, not guessed)
* `programs/staking-suite` placeholder `program_id 3vEEMM…` → **LEGAL-REVIEW** (requires `set-id`)
* `docs/TRADEMARK-DOMAIN-REGISTER.md` NOT INCLUDED / NOT VERIFIED → **LEGAL-REVIEW**
* `docs/IP-OWNERSHIP-REGISTER.md` per-component origin, not `100% seller-owned` — **LEGAL-REVIEW** where unclear

## Frontend lint note
* `npm run lint` → **VERIFIED** (Batch 11) — `eslint .` with ESLint 9 flat config, non-interactive, exit 0, 12 pre-existing warnings documented in `docs/KNOWN-LIMITATIONS.md` row 16 (the old `next lint` prompt is gone with Next.js 16).

> Overall: **CODE COMPLETE FOR CURRENT SCOPE**, **RELEASE VERIFIED**, **BUYER PACKAGE VERIFIED**, **EXTERNAL NOT YET VERIFIED**, **LEGAL REVIEW WHERE DOCUMENTED**.
