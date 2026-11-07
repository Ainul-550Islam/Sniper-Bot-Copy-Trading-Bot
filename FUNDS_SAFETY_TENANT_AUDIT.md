# Funds-Safety and Multi-Tenant Audit — Engineering Summary

**Review date:** 2026-11-06  
**Scope:** the current working tree of Sniper-Bot-Copy-Trading-Bot. This is an engineering review and test record, not a formal security certification or a guarantee against financial loss.

## Summary

The reviewed changes strengthen accounting integrity and fail-closed risk decisions, coordinate in-flight entry exposure across replicas, and tighten tenant boundaries across backend and control-plane paths. The complete Rust workspace test suite passed against a disposable PostgreSQL 17 setup with separate app/system/migration roles. No live trading or external venue execution was performed.

## Funds-safety controls reviewed

- **Accounting identity and recovery:** ledger hydration rejects durable rows whose event identity/payload is inconsistent; repeated local facts remain idempotent. Recovery/reconciliation treats missing or gapped history as incomplete rather than synthesizing activity.
- **Position arithmetic:** buy/sell accounting validates values and overflow before mutating positions. Exposure conversion rejects unsupported quote units instead of silently treating them as zero.
- **Global risk admission:** accepted entries hold RAII reservations while module entry work is pending. PostgreSQL-backed stores coordinate independent replicas with an organization-scoped admission lease and durable reservation rows. These paths use the system pool where configured and constrain queries by deployment organization; tenant-runtime reservations also carry tenant identity.
- **Failure behavior:** reservation-store/admission failures reject entry. Durable reservations expire after five minutes to recover from process crashes; normal release is best-effort, with explicit awaited release at known completion points.
- **Tenant risk isolation:** tenant decisions do not consume or expose the deployment-wide portfolio view. When tenant-scoped aggregate inputs are unavailable but aggregate limits are enabled, the entry gate refuses rather than bypassing those limits.
- **Trade identity:** paper-fill correlation and deterministic exit intent IDs reduce duplicate accounting caused by regenerated IDs.
- **Control plane:** tenant-scoped API handling and response validation are covered by route/client tests; secret-handling, operator authentication, and fail-closed UI contracts have dedicated checks.

## Corrections made during the additional full-suite pass

- Made the PostgreSQL realized-PnL integration test tenant-scoped and self-contained, so unrelated test rows cannot make its baseline incomplete.
- Updated the Polymarket concurrency assertion to accept the existing `AlreadyInMarket` rejection as well as duplicate-intent/open-order rejections; it still requires exactly one submitted order and one venue POST.
- Normalized the SaaS/PostgreSQL timestamp fixture to PostgreSQL's microsecond precision before asserting exact round-trips.
- Regenerated the release manifest from the actual tree (789 Rust files, 168 top-level documentation files, 75 migrations; high-water mark 0075) and refreshed the OpenAPI JSON/YAML artifacts from the current server contract.

## Verification performed

### Full Rust workspace

- `cargo test --workspace -j 1 -- --test-threads=1`, with PostgreSQL 17 and the configured RLS app/system/DDL URLs — **4,710 passed, 0 failed, 35 ignored** across 176 test binaries, including database-backed tenant isolation, migrations, risk reservations, SaaS, and RLS pool-routing checks.
- `CARGO_INCREMENTAL=0 cargo check --workspace --tests` — passed.
- `CARGO_INCREMENTAL=0 cargo test --workspace --lib` — **2,732 passed, 0 failed, 3 ignored** at the earlier checkpoint.
- `cargo fmt --all -- --check` and `git diff --check` — passed.
- `./scripts/export-openapi.sh --check` — JSON and YAML contract artifacts current (194 paths).
- Release-manifest count contract — passed after regeneration.

### Targeted PostgreSQL verification

- PostgreSQL 17 cross-replica `global_risk_reservations_pg` — passed; the test uses a one-connection pool and verifies one of two over-capacity requests is admitted and represented by one durable reservation.
- PostgreSQL positions/risk tenant-isolation suite — **4 passed**.
- Fresh-database migration test — passed.
- PostgreSQL tenant risk-oracle count/PnL test — passed after isolation correction.

### Control plane

- `npm test` — 14 contract/security checks passed.
- `npm run test:unit` — **82 files, 361 tests passed**.
- `npm run typecheck` — passed.
- `npm run lint` — passed.
- `npm run build` — optimized production build passed.

## Remaining scope

- This does not constitute an independent penetration test, formal audit, or review of production infrastructure, credentials, deployment settings, or operational procedures.
- The 35 ignored workspace tests are explicitly gated/live-provider cases; no live wallet signing, mainnet/testnet order placement, external market/provider interaction, or load/fault-injection campaign was performed.
- PostgreSQL tests ran against a disposable local PostgreSQL 17 instance and test databases, not a production-like cluster. Fresh migration coverage does not by itself prove every historical production database is healthy; use the project's controlled migration/backup process before rollout.
- Durable entry reservations are bounded to five minutes. If an entry can legitimately remain unresolved longer, verify lifecycle and expiry policy before deployment.

## Rollout guidance

Back up the production database, apply migrations through the release process, verify the accounting recovery report is clean before enabling entries, and keep live trading disabled until staged-environment smoke tests confirm tenant routing, reservation expiry/release, kill switches, and venue execution behavior. Monitor reservation age, recovery readiness, rejects due to unavailable risk state, and reconciliation gaps after deployment.
