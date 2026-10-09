# Final Buyer Status — sniper-suite 0.1.0

> 2026-09-29 Asia/Dhaka · `VERSION 0.1.0` · `506 rs under crates/ (512 incl. programs/staking-suite)` · `101 docs` · `34 migrations` · `1553 #[test]` · `8 workspace crates + 1 standalone program` · `rustc/cargo 1.98.1 (rust-toolchain.toml pin)`
> Categories: **VERIFIED** / **CODE-READY** / **SERVICE-BACKED-NOT-RUN** / **EXTERNAL-REQUIRED** / **BUYER-ACTION** / **LEGAL-REVIEW**
> No `100% production ready`, `fully audited`, `live verified`, `mainnet ready` without evidence.

## VERIFIED (hermetic, current execution — 2026-09-29, on a fresh install of the pinned toolchain)

* Toolchain reproducibility: rustup install of the `rust-toolchain.toml` pin (`1.98.1` + rustfmt + clippy, minimal profile) from scratch on a clean sandbox, followed by full re-verification below → **VERIFIED** everything green on the fresh install.
* `cargo check --workspace` → **VERIFIED** PASS (clean compile of the full dependency graph, 5m01s cold).
* `cargo clippy --workspace --all-targets -- -D warnings` → **VERIFIED** PASS 0 warnings (all 8 crates, lib + bins + tests).
* `cargo fmt --all -- --check` → **VERIFIED** PASS (0 diff).
* `cargo test -p bot-core --lib --tests -- --test-threads=1` (real PostgreSQL 17.11, `POSTGRES_URL` set) → **VERIFIED** 18 test binaries / **727 passed / 0 failed / 0 ignored / exit 0** — includes 599 lib unit tests, `db_integration` 26/26 (live PG, first full live execution of the legacy ownership suite), the 18 new cross-tenant isolation tests (`{orders,execution,intents,positions,copy,polymarket,workers,reporting}_cross_tenant_pg.rs`), and `tenant_idempotency_isolation` 3/3 (live PG, updated to the 0027 tenant-local contract)
* `cargo test -p sniper-suite trading_data_plane -- --test-threads=1` (real PG + real axum router) → **VERIFIED** 2/2 — the tenant data-plane route tests (cross-tenant 404-no-leak, cursor 400, unauthenticated 401, 503 plane-unavailable)
* `cargo test -p saas-sdk -- --test-threads=1` → **VERIFIED** 32/32 PASS (2026-09-29 re-run on the fresh toolchain)
* `cargo test -p sniper-suite --test {observability_config,release_manifest_integration,buyer_package_integration,backup_restore_integration} -- --test-threads=1` → **VERIFIED** 14/14 (3 + 4 + 3 + 4), including `buyer_release_package_built` against the regenerated 2026-09-29 package
* `release_manifest_counts_and_version_are_current` (sniper-suite `batch6_transaction_readiness`) → **VERIFIED** PASS after the manifest refresh (`rust_files 506`, `migrations 34`, `docs_files 101`, `#[test]` 1553, migration high-water mark `0034`)
* `bash scripts/final-release-check.sh` → **VERIFIED** **ALL PASS** (8/8 gates: fmt, workspace check, clippy ×8 crates, saas-sdk tests, secret scan, manifest counts, SBOM/license artifacts, buyer package; the only non-executed item is the script's own designed WARN path for the heavy per-harness 90s-timeout builds — those four harnesses were then executed explicitly, 14/14 above)
* Verification found and fixed 4 real defects that static checks had missed (two SQL binding bugs that only manifest at runtime against a live database): positions-upsert column/placeholder mismatch, legacy claim-store release `$4`/`$5` bind bug, data-plane `write_scope` error type, and the stale pre-0027 idempotency test contract — full account in `PROMPT-3-RESULT.md` §5

## VERIFIED (hermetic, previous execution — dated, on the then-current tree)

* `cargo test --workspace -- --test-threads=1` (real PostgreSQL 17.11) → 70 suites / 2077 passed / 0 failed / 13 ignored / exit 0 — **2026-09-26 tree** (pre-PROMPT-3 final state; supersede with a fresh workspace run at handover)
* `cargo audit` (app + staking lockfiles) → 0 vulnerabilities, 9 allowed warnings — 2026-09-26
* `cargo test -p sniper-suite --lib` → 263/263 PASS (3 ignored) — 2026-09-27 tree
* `npm ci --ignore-scripts` → 354 packages (next 16.3.6; 2026-09-27 Batch 11)
* `npm run typecheck` → PASS · `npm run build` → PASS (Next.js 16, 5 routes prerendered static, exit 0)
* `npm run lint` → PASS (ESLint 9 flat config, exit 0, 12 pre-existing warnings documented in `docs/KNOWN-LIMITATIONS.md` row 16)
* `sbom.json` 200 comps `fd837e42…` + `licenses.json` 707 entries `c1c051ca…`

## VERIFIED (buyer package — regenerated 2026-09-29)

* `bash scripts/build-release-package.sh` → **VERIFIED** PASS (package rebuilt from the canonical current tree, including the 2026-09-29 re-audit `AUDIT.md`, `PROMPT-3-RESULT.md`, all PROMPT-3 sources and tests)
* `bash scripts/verify-buyer-package.sh` → **VERIFIED** PASS (byte-identical source mirror, checksums, docs parity 101, evidence records, source-tree digest `1e42ca12…`; the single `WARN manifest migration count 34 != 24` is the verifier's own hardcoded pre-0025 baseline, informational only)
* `bash scripts/verify-delivery.sh` → **VERIFIED** 7/7 PASS
* The 2026-09-29 external re-audit (`AUDIT.md` §4) had correctly flagged the previously shipped package as STALE (manifest DIFFERS, 23 tenant files missing, 15 files differing, `PROMPT-2-RESULT.md` DIFFERS); the 2026-09-29 regeneration closes that P0-1 finding

## CODE-READY (implemented, hermetic tests, no live env needed)

* **Tenant trading data plane** — 21 authenticated `/api/tenant/*` routes over the tenant-scoped `bot-core` `trading_repository` (orders, executions/transactions, positions/trades/balances, copy, polymarket, recovery, reporting); every SQL predicate carries `organization_id`; 503 when unattached — `CODE-READY` (2 route-level + 18 repository-level cross-tenant tests against live PostgreSQL)
* **Tenant DB isolation (STEP 10 swaps)** — 17 trading-truth PK/arbiter surfaces tenant-composite (migrations 0026–0034), tenant-local idempotency, tenant worker lanes with fencing generations, tenant-scoped reconciliation queue — `CODE-READY` (live-PG cross-tenant suites)
* **Billing** — server-authoritative price, dunning, usage_policy, reconciliation, idempotent provider_events — `CODE-READY` (tests 7+7+11+8, no live keys)
* **Custody boundary** — provider registry, health, rotation (real-profile resolution), guard-ordered sign executor, full audit; REAL Vault-transit + AWS-KMS adapters (unit-tested wire; KMS SigV4 vs the AWS test vector), fail-closed, never silent fallback; HSM fail-closed unimplemented — `CODE-READY` (no live round-trip — see limitations)
* **Tenant control-plane isolation** — 5-tuple (auth+membership+permission+org+lifecycle) per handler, WS `reject_query_credentials` — `CODE-READY` (31 handlers)
* **Lifecycle/retention** — 8-phase deprovision, 7-state retention, job_claim SKIP LOCKED — `CODE-READY`
* **Backup/restore manifests** — `export_manifest/restore_manifest/preflight/commands` — `CODE-READY` (hermetic 3+3+4)
* **Observability/SBOM/license/gap ledger** — `ops/*` — `CODE-READY`

## SERVICE-BACKED-NOT-RUN (harness exists, no real service in this env)

* **PostgreSQL** `SERVICE-BACKED VERIFIED (2026-09-29)` — executed against a real PostgreSQL 17.11 instance (migrations `0001`–`0034`): `db_integration` 26/26, all 8 cross-tenant isolation suites (18 tests), the tenant data-plane route tests, and `tenant_idempotency_isolation` 3/3. Buyer infra re-runs with `POSTGRES_URL=...` (template in `docker-compose.yml`)
* **Redis** `SERVICE-BACKED-NOT-RUN` — `redis_integration` 10 tests — requires `REDIS_URL=...`

## EXTERNAL-REQUIRED (ADAPTER/BOUNDARY READY, live NOT_RUN)

* **GAP-001 Billing LIVE** `EXTERNAL-REQUIRED` — Stripe/Paddle adapter READY, live → `NOT_RUN` (needs `LIVE_BILLING=1 + provider keys`)
* **GAP-002 Custody LIVE** `EXTERNAL-REQUIRED` — Vault/KMS adapters READY (real, unit-tested), live → `NOT_RUN` (needs `LIVE_CUSTODY=1 + VAULT_ADDR/TOKEN` or `KMS_KEY_ID` + AWS credentials); HSM still unimplemented (fail-closed refusal)
* **GAP-003 Production deployment** `EXTERNAL-REQUIRED` — SMOKE HARNESS READY, `NOT_RUN` (needs `DEPLOYMENT_BASE_URL` + host/TLS)
* **GAP-004 Funded trading** `EXTERNAL-REQUIRED` — GUARD READY, `NOT_RUN` (needs funded wallet + `execution_mode=live`)
* **GAP-005 Staking E2E** `EXTERNAL-REQUIRED / BLOCKED` — HARNESS READY, `NOT_RUN` (needs `STAKING_E2E=1 + solana-test-validator` + `set-id`)
* **GAP-006 External security audit** `NOT DONE` — internal `cargo audit/deny` only — `EXTERNAL-REQUIRED`
* **GAP-007 Tenant runtime wiring into trading modules** `EXTERNAL-REQUIRED (design ready, code not started)` — the tenant data plane and repositories are tenant-scoped, but the five module crates (`module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`, `solana-kit`) are not yet tenant-context-wired; module startup remains process-level (see `AUDIT.md` §11). This is the largest remaining engineering gap for a true multi-tenant SaaS claim.

## BUYER-ACTION (must be done post-transfer)

* Provision Postgres 16+ + Redis 7 + RPC/Geyser + OTLP endpoint
* Set `.env` secrets (never commit) per `docs/ENVIRONMENT-VARIABLE-REGISTER.md`
* Run `bash scripts/verify-delivery.sh` + `final-release-check.sh` on buyer infra
* Optionally run `POSTGRES_URL=... REDIS_URL=... cargo test --test db_integration/redis_integration` and the 8 `*_cross_tenant_pg` suites (`--test-threads=1`)
* Configure Stripe/Paddle + Vault/KMS/HSM + Telegram token per runbooks, then `run-external-validation.sh` live modes
* Execute production smoke `docker build -t sniper-suite:prod . && curl /api/health`

## LEGAL-REVIEW (not seller-owned claim)

* `LICENSE` generic `sniper-suite authors` → **LEGAL-REVIEW** (needs real entity)
* `Cargo.toml` `repository` placeholder removed (no URL) → **LEGAL-REVIEW**
* `licenses.json` 3 UNKNOWN entries → **LEGAL-REVIEW** (stays UNKNOWN, not guessed)
* `programs/staking-suite` placeholder `program_id 3vEEMM…` → **LEGAL-REVIEW** (requires `set-id`)
* `docs/TRADEMARK-DOMAIN-REGISTER.md` NOT INCLUDED / NOT VERIFIED → **LEGAL-REVIEW**
* `docs/IP-OWNERSHIP-REGISTER.md` per-component origin, not `100% seller-owned` — **LEGAL-REVIEW** where unclear

> Overall: **CODE COMPLETE FOR CURRENT SCOPE**, **TENANT DATA PLANE VERIFIED AGAINST LIVE POSTGRESQL**, **FULL TOOLCHAIN RE-VERIFICATION GREEN 2026-09-29**, **RELEASE PACKAGE REGENERATED AND VERIFIED 2026-09-29**, **EXTERNAL NOT YET VERIFIED**, **LEGAL REVIEW WHERE DOCUMENTED**.
> Standing buyer-audit reference: `AUDIT.md` (2026-09-29 re-audit) — its §26 scorecard remains the honest gap map; its §12 tenant-DB finding is closed by the PROMPT-3 work recorded in `PROMPT-3-RESULT.md`, its §4 package finding is closed by the 2026-09-29 regeneration, its §11 tenant-runtime-wiring finding remains OPEN.
