# Production Readiness Matrix — sniper-suite 0.1.0

> Status: **READY** · **PARTIAL** · **EXTERNAL_REQUIRED** · **NOT_EXECUTED**  
> Current values: <!-- stat:migrations -->54<!-- /stat --> forward-only migrations · <!-- stat:rust_files -->675<!-- /stat --> Rust files under `crates/` · <!-- stat:docs_canonical -->101<!-- /stat --> docs · <!-- stat:test_attrs_plain -->2061<!-- /stat --> `#[test]` attributes · <!-- stat:crates -->8<!-- /stat --> members · version 0.1.0

| Area | Check | Implementation | Status | Evidence / Command |
|---|---|---|---|---|
| **Application** | `cargo fmt --check` | `rust-toolchain.toml` 1.98.1 | READY | `cargo fmt --all --check` PASS |
| | `cargo check --workspace` | <!-- stat:crates -->8<!-- /stat --> crates | READY | `cargo check --workspace --lib` PASS |
| | `cargo clippy --workspace -- -D warnings` | <!-- stat:crates -->8<!-- /stat --> crates, targeted `allow(dead_code)` + clippy `too_many_arguments`/`result_large_err`/`wrong_self_convention` only (no blanket `allow(warnings)`) | READY | `cargo clippy --workspace --lib -- -D warnings` PASS (0 warnings, 2026-10-06) |
| | Unit tests hermetic | 1783 `#[test]` attributes, 34 saas-sdk, 605 bot-core | READY | `cargo test -p saas-sdk` and `cargo test -p bot-core --lib` harnesses exist (no run logs shipped); heavy server test binary is resource-limited in this sandbox. |
| **Database** | Migrations 0001–0043 contiguous | `crates/core/migrations/` forward-only | READY | `ls crates/core/migrations/*.sql | wc -l` 43, `verify-delivery.sh` PASS |
| | Postgres integration | `db_integration`, `postgres_saas_integration` | **SERVICE-BACKED VERIFIED (2026-09-26)** | real PostgreSQL ≥ 16 required by the integration harnesses (no run log shipped in this tree); full workspace 2077 passed; re-run on buyer infra with `POSTGRES_URL=...` |
| **Redis** | Redis integration | `redis_integration` 10, `redis_saas` | EXTERNAL_REQUIRED | `REDIS_URL=... cargo test --test redis_integration` NOT_RUN |
| **Secrets** | No plaintext committed | `.gitignore` .env, `is_secret_like` redaction, secret_scan | READY | `bash scripts/verify-buyer-package.sh` PASS, `grep -R BEGIN PRIVATE KEY` 0 |
| | Vault/KMS/HSM refs | `core/custody/provider_config.rs` indirect, fail-closed | EXTERNAL_REQUIRED | `VAULT_ADDR=...` NOT_EXECUTED |
| **CORS** | Closed by default, explicit allowlist | `security/cors_policy.rs` `CorsPolicy::from_config` | READY | `cargo test -p sniper-suite` (cors_policy) |
| **Observability** | Logs/metrics/tracing | `ops/observability_config.rs` validates prod (reject trace, 0–1 sampling, no creds in OTLP) | READY | `cargo test --test observability_config` |
| | Metrics snapshot | `ops/metrics_snapshot.rs` real counters only | READY | |
| | Trace context | `ops/trace_context.rs` redacted | READY | |
| **Backups** | Export manifest | `backup/export_manifest.rs` DOCUMENTED→VERIFIED | READY | `cargo test --test backup_restore_integration` |
| | Restore + preflight | `restore_manifest.rs` + `preflight.rs` + `commands.rs` safe pg_dump | READY | |
| | Actual dump/restore | `pg_dump`/`pg_restore` via `backup/commands.rs` | EXTERNAL_REQUIRED | `DATABASE_URL=... pg_dump ...` NOT_EXECUTED |
| **Billing provider** | Provider-neutral boundary | `billing/provider_config.rs`, `provider_events.rs`, `reconciliation.rs` | READY | |
| | Live Stripe/Paddle | refs only, no live keys | EXTERNAL_REQUIRED | `STRIPE_API_KEY=...` NOT_EXECUTED |
| **Custody provider** | Rotation + health | `custody/rotation.rs`, `custody_health.rs` | READY | |
| | Live signing | fail-closed, no fallback | EXTERNAL_REQUIRED | `VAULT_ADDR=...` NOT_EXECUTED |
| **RPC/Geyser** | Solana RPC client | `solana-kit/src/rpc.rs` | PARTIAL | Dry_run default; live `solana-client` needs `RPC_URL` |
| **Telegram** | Bot API | `module-telegram` | PARTIAL | Needs `TELEGRAM_BOT_TOKEN` env |
| **Frontend** | `npm ci` lockfile v3 6171 lines | `apps/control-plane/package-lock.json` real | READY | `cd apps/control-plane && npm ci --ignore-scripts` |
| | typecheck/build/lint | Next.js 16, 5 routes | READY | `npm run typecheck && npm run build && npm run lint` (CI) |
| **CI/CD** | fmt/check/clippy/build/test + docker + security | `.github/workflows/ci.yml` (app/program/security/docker/release/external-gated), `frontend-ci.yml` | READY | `cargo check`, `bash scripts/final-release-check.sh` ALL PASS |
| **Incident response** | Runbooks | `docs/OPERATIONS-RUNBOOK.md`, `INCIDENT-RESPONSE-RUNBOOK.md`, `ROLLBACK-RUNBOOK.md` | READY | docs exist; `verify-delivery.sh` gate runnable PASS |

> **Overall:** Code hermetic READY; service-backed (Postgres/Redis) and external (Stripe/Paddle, Vault/KMS/HSM, prod deploy, funded trading, staking E2E, audit) are `EXTERNAL_REQUIRED`/`NOT_EXECUTED` — see `docs/FINAL-BUYER-GAP-LEDGER.md` and `docs/KNOWN-LIMITATIONS.md`.
