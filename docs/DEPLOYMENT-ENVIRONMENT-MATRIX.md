# Deployment Environment Matrix — sniper-suite 0.1.0

> Distinguishes tested vs not-tested. No claim of prod deployment.

| Component | Local (dev) | CI (GitHub Actions) | Staging (buyer) | Production (buyer) |
|---|---|---|---|---|
| **Database** | `POSTGRES_URL=postgres://sniper:sniper@localhost:5432/sniper` (Docker) — migrations 22 auto | `postgres:16-alpine` service, `POSTGRES_URL=postgres://sniper:sniper@localhost:5432/sniper` — `cargo test --workspace` 26 db tests **actually tested** | **NOT_EXECUTED** — buyer Postgres 16+ required | **EXTERNAL_REQUIRED** — buyer production Postgres, forward-only migrations |
| **Redis** | `REDIS_URL=redis://localhost:6379` (Docker) | `redis:7-alpine` service, `REDIS_URL=redis://localhost:6379` — 10 redis tests **actually tested** | NOT_EXECUTED | EXTERNAL_REQUIRED — buyer Redis 7 |
| **RPC / Geyser** | `network.cluster = devnet` default, `Rpc::new` mock | Mocked in `module-polymarket` mock CLOB | NOT_EXECUTED | EXTERNAL_REQUIRED — live Solana RPC + optional Geyser, funded wallet |
| **Billing** | `core/billing/provider_config.rs` refs only, `billing_reconciliation.rs` 11 tests hermetic | Hermetic tests only | NOT_EXECUTED | EXTERNAL_REQUIRED — live Stripe/Paddle keys, webhook HMAC, `STRIPE_API_KEY=...` |
| **Custody** | `core/custody/provider_config.rs` fail-closed, rotation 7 tests hermetic | Hermetic | NOT_EXECUTED | EXTERNAL_REQUIRED — live Vault/KMS/HSM cluster |
| **Secrets** | `.env.template` + `secrets` env injection via `seed_secret_env` | `POSTGRES_PASSWORD=ci-only docker compose config -q` | NOT_EXECUTED | EXTERNAL_REQUIRED — buyer Vault/env, never committed |
| **Observability** | `ObservabilityConfig::development()` plain, trace allowed | `production()` validated in `cargo test --test observability_config` | NOT_EXECUTED | EXTERNAL_REQUIRED — OTLP endpoint ref (secret-free) |
| **Live trading gate** | `EXECUTION_MODE=dry_run` (default), `allow_live_trading=false` | Same | NOT_EXECUTED | EXTERNAL_REQUIRED — `EXECUTION_MODE=live` + funded keys, explicit approval |
| **Frontend** | `apps/control-plane` `npm ci` 354 packages, `npm run typecheck/build` PASS | `frontend-ci.yml` `npm ci --ignore-scripts` + `typecheck` + `build` + `lint` **actually tested** | NOT_EXECUTED | EXTERNAL_REQUIRED — buyer Node 20, `NEXT_TELEMETRY_DISABLED=1` |
| **CI/CD** | `cargo fmt/check/clippy/test` hermetic | `.github/workflows/ci.yml` app/program/security/docker/release/external-gated **actually tested** | NOT_EXECUTED | EXTERNAL_REQUIRED — buyer runner with `POSTGRES_URL`/`REDIS_URL` |
| **Telegram** | `TELEGRAM_BOT_TOKEN` env optional, disabled if missing | Not tested | NOT_EXECUTED | EXTERNAL_REQUIRED |
| **Backups** | `backup/*` manifests DOCUMENTED, `preflight.rs` | Hermetic | NOT_EXECUTED | EXTERNAL_REQUIRED — `pg_dump`/`pg_restore` with real PG |

## Actually Tested (this repo, hermetic)

- `cargo fmt --all --check` PASS
- `cargo check --workspace --lib` PASS (616 Rust files, 43 migrations, 8 members)
- `cargo test -p saas-sdk` 32/32 + `cargo test --test observability_config` etc. (1331 grep)
- `npm ci --ignore-scripts` + `typecheck` + `build` + `lint` (frontend)
- `docker build -t sniper-suite:ci` + smoke `curl /api/health` (in CI `docker` job)

## Not Tested (marked NOT_EXECUTED / EXTERNAL_REQUIRED)

- Staging / production — no such environment exists in this repo; `docs/BUYER-DEPLOYMENT.md` describes buyer steps.
- Live Stripe/Paddle, live Vault/KMS/HSM, funded trading, `STAKING_E2E=1` validator, external audit — see `docs/FINAL-BUYER-GAP-LEDGER.md` (6 gaps).

> **Verification:** `cat .github/workflows/ci.yml | grep -A2 services:`, `cat rust-toolchain.toml`, `bash scripts/verify-delivery.sh` (7/7), `release-manifest.json` `verification_status.not_executed_environment_blocked`.
