# Architecture Overview — sniper-suite 0.1.0 (2026-09-24)

> **Current values:** Version 0.1.0 · 22 migrations · 343 Rust sources · 8 workspace members · 70→101 docs · 1331 tests (grep)

## 1. Workspace & Crates

```
sniper-suite/
├─ Cargo.toml (workspace, 8 members)
├─ crates/core (bot-core) — shared types, config, state, billing, custody, DB, ownership, HA
├─ crates/solana-kit — RPC/WS, instruction builders, Jito, tokens
├─ crates/module-sniper — Module 1, sniper engine
├─ crates/module-copy — Module 2, copy trading
├─ crates/module-polymarket — Module 3, Polymarket CLOB
├─ crates/module-telegram — Module 5, Telegram control
├─ crates/server (sniper-suite binary) — Axum control plane
├─ crates/saas-sdk — typed SaaS client (billing/custody/audit/lifecycle)
└─ programs/staking-suite — standalone Solana program (own Cargo.lock, cargo build-sbf)
```

**Toolchain:** `rust-toolchain.toml` 1.98.1, `Dockerfile` `rust:1.98.1-bookworm`, `programs/staking-suite` built with `cargo build-sbf` (agave 2.1.21).

## 2. Control Plane (crates/server)

- **Entry:** `src/main.rs` — loads `AppConfig`, init tracing, builds `AppState`, `Rpc`, `Wallet`, `SignerRegistry`, optional `Database` (Postgres) + `Redis`, `DedupStore`, `OrderManager`, accounting (Task5), HA (Task6), `AuditTrail`, `OwnershipRegistry`, `RateLimiter`, `Authenticator`, then spawns modules and serves Axum.
- **REST:** `src/api.rs` — `GET /health`, `/api/*` (status, orders, positions, modules, risk, HA), `src/saas/*` under `/api/saas/*`
- **Ops evidence:** `src/ops/` (41 files — `ls crates/server/src/ops/*.rs | wc -l`) — `health_report.rs`, `release_readiness.rs`, `observability_config.rs`, `metrics_snapshot.rs`, `trace_context.rs`, `rate_limit_report.rs`, `container_metadata.rs`, `reproducibility.rs`, `config_diff.rs`, `external_validation.rs`, `final_gap_ledger.rs`, `release_lock.rs`, etc.
- **Backup:** `src/backup/` (5 files) — strict manifests for export/restore.
- **Security:** `src/security/{headers.rs,websocket.rs,cors_policy.rs,tenant_context.rs,legacy_websocket_guard.rs}`

**Boundaries:** Control plane decides *who* may ask (`saas/middleware.rs` checks org→membership→permission→entitlement) but never approves trades alone — Task5 global risk + Task6 lease fencing still run.

## 3. Core (crates/core)

- `config` — `AppConfig` with `ObservabilityConfig`, `DatabaseConfig`, `RedisConfig`, secrets via env
- `state` — `AppState` with replica id, balances, positions
- `billing` — `pricing.rs` (immutable snapshot), `provider_events.rs`, `reconciliation.rs`, `billing_state.rs`, `dunning.rs` (7-state), `usage_policy.rs`, `provider_config.rs` (Stripe/Paddle refs, no secrets)
- `custody` — `credentials.rs`/`health.rs`/`resolve.rs` + `provider_config.rs`/`rotation.rs` (Vault/KMS/HSM refs, fail-closed)
- `db` — `repo::*` with `Database` (sqlx, postgres), migrations `0001–0022` in `crates/core/migrations/` (forward-only)
- `ownership` — `ClaimStore` (Postgres > Redis > Memory), fencing, `RuntimeFlags`
- `obs` — `HealthRegistry`, metrics

## 4. Trading Modules

| Module | Crate | Truth Source | Durable Sink |
|---|---|---|---|
| 1 Sniper | `module-sniper` | Solana RPC/WS, pump.fun / Raydium | `recon::DbIntentSink` |
| 2 Copy | `module-copy` | leader wallets poll | `recon::DbCopyStore` |
| 3 Polymarket | `module-polymarket` | CLOB Gamma + CTF | `recon::DbPolyStore` |
| 4 Staking | `programs/staking-suite` (on-chain) | `solana-test-validator` | program state |
| 5 Telegram | `module-telegram` | Telegram Bot API | — |

All default to `EXECUTION_MODE=dry_run`; live requires `execution.mode=live` + `allow_live_trading=true`.

## 5. SaaS Layer (crates/server/src/saas)

- `organizations.rs` — create via provisioning state machine, members, suspension
- `users.rs`, `api_keys.rs` (secret shown once, hash stored), `middleware.rs` (8 decisions)
- `billing/*`, `custody/*`, `tenant_lifecycle.rs`, `data_lifecycle.rs`, `export.rs` (deterministic tenant-scoped)
- `openapi.rs` + `api/openapi_*.rs` — OpenAPI at `GET /api/saas/openapi.json`
- `saas-sdk` — typed client, `secret-free Debug`, no secrets in URLs (`crates/saas-sdk/src/billing.rs,custody.rs,commercial.rs`)

## 6. Database (Postgres) & Redis

- **Postgres 16+** — authoritative for `users`, `organizations`, `memberships`, `sessions`, `api_keys`, `plans`, `subscriptions`, `entitlements`, `usage`, `provisioning jobs`, `orders`, `positions`, `claims`, `flags`, `cursors`. Migrations 22, high-water `0022`, policy forward-only (`docs/BACKUP-RESTORE.md`). Tested via `crates/core/tests/db_integration.rs` (26) and `crates/server/tests/postgres_saas_integration.rs` when `POSTGRES_URL` set.
- **Redis 7** — optional, for dedup L2, leases, rate-limit buckets, cursor cache. Tested via `redis_integration.rs` + `redis_saas_integration.rs` when `REDIS_URL` set.
- **Degrade loudly:** if both off, `MemoryClaimStore` (single replica, paper only) with warning.

## 7. OpenAPI & SDK

- `GET /api/saas/openapi.json` — tenant-scoped, redacted
- `saas-sdk 0.1.0` — `billing_status`, `usage_limits`, `commercial_state`, `readiness`, `lifecycle`, `backup_status`, typed errors (`SdkErrorKind`), `cargo test -p saas-sdk` 32/32

## 8. Frontend

- `apps/control-plane` — Next.js 16 App Router (next 16.3.6), 5 routes (billing/custody/data-lifecycle…), `package-lock.json` 6171 lines v3, `npm ci`, `npm run typecheck` (strict), `npm run build` (5 static routes: `/`, `/_not-found`, `/billing`, `/custody`, `/settings/data-lifecycle`), `npm run lint`

## 9. Deployment Boundaries (Implemented vs External)

| Area | Implemented | External Required |
|---|---|---|
| App | binary + Docker `sniper-suite:ci` smoke (`/api/health`) | buyer cloud, TLS, secrets via env |
| DB/Redis | sqlx migrations, health samplers | buyer Postgres 16 + Redis 7 |
| Billing | provider-neutral boundary, webhook verify, idempotency, reconciliation | live Stripe/Paddle keys + funded account → `EXTERNAL_REQUIRED` |
| Custody | Vault/KMS/HSM refs, rotation, health, fail-closed fallback | live Vault/KMS/HSM cluster → `EXTERNAL_REQUIRED` |
| Observability | `ObservabilityConfig`, `MetricsSnapshot`, `TraceContext` (real counters only) | buyer OTLP endpoint (secret-free ref) |
| Trading | dry_run default, risk, HA | funded keys + live RPC/Geyser → `EXTERNAL_REQUIRED` |
| Staking | program `.so` 187KB, host tests 71/71 | `solana-test-validator` E2E → `EXTERNAL_REQUIRED` |

Every external row is `EXTERNAL_REQUIRED` / `NOT_EXECUTED` in `crates/server/src/ops/external_validation.rs` and `docs/FINAL-BUYER-GAP-LEDGER.md` — never claimed VERIFIED without execution.

> **Verification:** `cargo check --workspace`, `cargo test -p saas-sdk`, `ls crates/server/src/ops/*.rs | wc -l` (41), `find crates -name "*.rs" | wc -l` (343), `ls docs/*.md | wc -l` (70 → 101), `release-manifest.json` `docs_files`/`migrations`.
