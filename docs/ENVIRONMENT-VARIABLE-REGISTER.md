# Environment / Configuration Register — sniper-suite 0.1.0

> Complete inventory. Never include actual values. Reconciled with `config.toml.example`, `.env.template`, `crates/core/src/config.rs`, `crates/server/src/main.rs::seed_secret_env`, `docs/SECRETS-MANAGEMENT-MATRIX.md`.

| Variable | Required | Env | Secret | Source Config | Default Behavior | Failure Behavior |
|---|---|---|---|---|---|---|
| `DATABASE_URL` | Optional (if `database.required=false`) / Required (if `true`) | `DATABASE_URL` | Yes (password) | `[database]` `url` or env `DATABASE_URL` | No DB → memory store, warn, single replica | `GET /ready` not ready if required, `db_integration` NOT_RUN |
| `REDIS_URL` | Optional | `REDIS_URL` | Yes | `[redis]` `url` | No Redis → dedup L1 only | `redis_integration` NOT_RUN |
| `WEBHOOK_SECRET_ENCRYPTION_KEY` | Required for webhook create/delivery | `WEBHOOK_SECRET_ENCRYPTION_KEY` | Yes | SaaS webhook AES-256-GCM envelope | No key → webhook creation and delivery refuse; legacy plaintext rows must be rotated | `webhook_secret_storage_unavailable` / `webhook_secret_unavailable` |
| `MFA_ENCRYPTION_KEY` | Required when MFA is enabled | `MFA_ENCRYPTION_KEY` | Yes | SaaS TOTP AES-256-GCM envelope | No key → TOTP enrollment, verification, and MFA-protected authentication refuse; use a stable base64-encoded 32-byte key | `mfa_encryption_key_missing` / `mfa_secret_unavailable` |
| `RUST_LOG` | No | `RUST_LOG` | No | `[observability]` `log_level` (overridden by RUST_LOG) | `info` via `EnvFilter` | Falls back to `info` if invalid |
| `SOLANA_KEYPAIR` | No (paper) | `SOLANA_KEYPAIR` | Yes (private key JSON) | `secrets.solana_keypair` → env `SOLANA_KEYPAIR` | Ephemeral `Wallet::generate()` (paper only, warn) | `load_wallet` fails if malformed |
| `TELEGRAM_BOT_TOKEN` | No | `TELEGRAM_BOT_TOKEN` (or `telegram.bot_token_env`) | Yes | `secrets.telegram_bot_token` → env | `module-telegram` disabled, log | — |
| `POLYGON_PRIVATE_KEY` / `PRIVATE_KEY_ENV` | No | `module_polymarket::PRIVATE_KEY_ENV` | Yes | `secrets.polygon_private_key` → env | Polymarket disabled if missing | — |
| `STRIPE_API_KEY` / `PADDLE_API_KEY` | External | `STRIPE_API_KEY` etc. | Yes | `billing/provider_config.rs` indirect refs only | `EXTERNAL_REQUIRED` — no live billing | Webhook 401 if wrong |
| `STRIPE_WEBHOOK_SECRET` | External | `WEBHOOK_SECRET` | Yes | `billing_webhook.rs` HMAC | — | Webhook spoof if missing |
| `VAULT_ADDR` / `VAULT_TOKEN` / `KMS_KEY_ID` | External | `VAULT_ADDR` etc. | Yes | `custody/provider_config.rs` `VaultRef` | Fail-closed, startup fails if provider unsupported | No fallback to local |
| `RPC_URL` / `CTF_RPC_URL` | No | `RPC_URL` | No/Yes | `network.rpc_url`, `polymarket.ctf_rpc_url` | Devnet, mock CLOB | Live trading needs funded RPC |
| `API_KEY` / `AUTH_KEYS` | No (loopback) / Yes (non-loopback) | `API_KEY` / `[[auth.keys]]` | Yes | `api.api_key_env`, `auth.keys` | No key + non-loopback → fail closed (`is_loopback`) | `serve_api` bails |
| `POSTGRES_PASSWORD` (compose) | No | `POSTGRES_PASSWORD` | Yes | `docker-compose.yml` | `ci-only` in CI `docker compose config` | — |
| `EXECUTION_MODE` | No | `EXECUTION_MODE` | No | `execution.mode` | `dry_run` (default, safe) | `live` + `allow_live_trading=true` needs funded keys |
| `STAKING_E2E` | No | `STAKING_E2E` | No | test flag | `0` (validator e2e not run) | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e` needs validator (excluded workspace) |
| `POSTGRES_URL` (test) | No | `POSTGRES_URL` | Yes | test harness | `NOT_RUN` if missing | `db_integration` prints NOT_RUN |
| `REDIS_URL` (test) | No | `REDIS_URL` | Yes | test harness | `NOT_RUN` if missing | `redis_integration` prints NOT_RUN |

## Config Templates

- `.env.template` — placeholders, never real values, `cp .env.template .env`
- `config.toml.example` — annotated, `[secrets]` inline only via env injection
- `docker-compose.yml` — `POSTGRES_PASSWORD` env, `rust:1.98.1-bookworm`

## Defaults & Failures (from code)

- **No DB + no Redis:** `MemoryClaimStore` (single replica, paper only) + warn `EXACTLY ONE replica`
- **Live on Memory store:** warn `WILL double-execute` if multi-replica
- **No API key + non-loopback bind:** `anyhow::bail` refuses to bind
- **Invalid log filter:** `eprintln` + fallback to `info`
- **Missing wallet:** `Wallet::generate()` ephemeral (paper)

> **Verification:** `grep -R "env::var" crates --include="*.rs" | grep -E "DATABASE_URL|REDIS_URL|SOLANA_KEYPAIR"` etc., `.env.template` vs `config.rs` fields, `bash scripts/verify-delivery.sh` hygiene (no .env committed).
