# Operations Runbook — sniper-suite 0.1.0

> **Current:** <!-- stat:migrations -->54<!-- /stat --> forward-only migrations (high water `<!-- stat:migrations_high_water -->0054<!-- /stat -->`), <!-- stat:rust_files -->675<!-- /stat --> Rust files under `crates/`, <!-- stat:crates -->8<!-- /stat --> members, version 0.1.0, `rust-toolchain.toml` pinned. Commands are real and correspond to `scripts/*` and `docs/*`.

## 1. Startup

```bash
# 1) Env (see docs/ENVIRONMENT-VARIABLE-REGISTER.md)
cp .env.template .env  # fill DATABASE_URL, REDIS_URL, API keys (never commit .env)
# 2) DB migrate (forward-only)
sqlx migrate run --source crates/core/migrations  # or auto_migrate via config
# 3) Check
cargo fmt --all --check && cargo check --workspace
# 4) Run (dry_run default)
cargo run -p sniper-suite -- --config config.toml
# or docker
docker build -t sniper-suite:prod . && docker run -p 8080:8080 --env-file .env sniper-suite:prod
# 5) Verify
curl -fsS http://localhost:8080/health          # version, migrations
curl -fsS http://localhost:8080/ready           # DB/Redis health
curl -fsS http://localhost:8080/metrics | head
```

## 2. Shutdown (ordered)

`main.rs` orchestrates: `SIGTERM` → coordinator → HTTP drain (10s) → module drain (15s) → HA drain (persist cursors, release leases) → pump flush (10s) → DB close. See `src/main.rs` `run_phase`.

**Operator:** `kill -TERM <pid>` or `docker stop`.

## 3. Migration

- Location: `crates/core/migrations/0001_*.sql` → `0022_checkout_url.sql` (22, contiguous)
- Policy: forward-only, no down migrations (`docs/BACKUP-RESTORE.md`)
- Check: `ls crates/core/migrations/*.sql | wc -l` (=21), `verify-delivery.sh` PASS
- Run: `sqlx migrate run` or `auto_migrate=true` in `[database]`

## 4. Health Checks

| Endpoint | Meaning |
|---|---|
| `GET /health` | liveness, version `0.1.0`, migrations count |
| `GET /ready` | readiness: state + recovery + leases + dependencies |
| `GET /metrics` | Prometheus, if `observability.metrics_enabled=true` |
| `GET /api/ha` | `crates/server/src/ha.rs` workers/leases/cursors |
| `GET /api/db` | applied migrations + checksum |

Commands:
```bash
curl -s http://localhost:8080/health | jq
curl -s http://localhost:8080/ready | jq
docker logs <container>
journalctl -u sniper-suite -f
```

## 5. Dependency Failures

| Dependency | Behavior | Action |
|---|---|---|
| **Postgres down** (`database.required=false`) | Degrades to memory (warn), `MemoryClaimStore` single replica | Check `DATABASE_URL`, `health_report.rs` redacts, `cargo test --test db_integration` needs PG |
| **Redis down** (`redis.required=false`) | Dedup L1 only, no L2 leases | Check `REDIS_URL`, `redis_integration` |
| **RPC/Geyser down** | Modules stay paper, no broadcasts | Check `network.cluster`, `solana-kit/rpc` |
| **Telegram token missing** | `module-telegram` disabled, log `disabled (no bot token)` | Set `TELEGRAM_BOT_TOKEN` env |

## 6. Queue / Lease Issues

- **Job claim:** `provisioning/job_claim.rs` `SELECT ... SKIP LOCKED` — if stuck, `SELECT * FROM provisioning_jobs WHERE state='pending' LIMIT 5`
- **Leases:** `ha_distributed` fencing generations; if split-brain warn, check `backend` `MemoryClaimStore` → must configure Postgres for multi-replica.
- **Commands:** `cargo test --test tenant_lifecycle_integration` (needs PG)

## 7. Billing Failure

- **Webhook 401:** check `billing_webhook.rs` HMAC, `WEBHOOK_SECRET` env, idempotency key.
- **Reconciliation:** `billing_reconciliation.rs` never invents success; check `provider_events.rs` logs, `billing_status.rs`.
- **Dunning:** 7-state `Current→PaymentFailed→RetryPending→GracePeriod→BillingSuspended→Recovered/ManuallyResolved` (`core/billing/dunning.rs`)

## 8. Custody Failure

- **No Vault/KMS/HSM:** `provider_config.rs` `local_fallback_allowed=false`, `resolve.rs` fail-closed, startup fails if provider unsupported — never falls back.
- **Rotation:** `rotation.rs` Pending→Active→Draining→Revoked, old valid until replacement.
- **Health:** `custody_health.rs` reports `healthy/degraded` without secrets.

## 9. WebSocket Incidents

- **401:** check `x-api-key` or `Authorization: Bearer` or first-frame token (`saas/websocket_auth.rs`); legacy `?key=` only if `legacy_websocket_guard.rs` `LEGACY_ENABLED`
- **Cross-tenant:** `security/websocket.rs` filters by org
- **Reconnect:** client must resend token after restart, dedup via `backup/preflight.rs`

## 10. Tenant Suspension / Closure

- **Suspend:** `organizations::suspension` (PATCH) → `data_lifecycle.rs` marks, `readiness.rs` denies new keys
- **Closure:** `tenant_lifecycle.rs` + `retention_worker.rs` (purge after retention; tests exist in-tree)
- **Verify:** `cargo test --test tenant_lifecycle_integration` (PG)

## 11. Backup

```bash
# Safe commands (never embed secret, use env var)
pg_dump --format=custom --file=/tmp/sniper-$(date +%Y%m%d).dump   # via DATABASE_URL env
# or helper
cargo run -p sniper-suite -- --help # see backup/commands.rs pg_dump_command()
# Manifest
# backup/export_manifest.rs: Documented → Executed (artifact sha) → Verified
```

## 12. Restore

```bash
pg_restore --clean --if-exists --dbname=env:DATABASE_URL /tmp/sniper-*.dump
# Manifest: restore_manifest.rs requires ExportStatus::Verified
# Preflight: backup/preflight.rs checks sha match
```

## 13. Rollback (see docs/ROLLBACK-RUNBOOK.md)

- **App:** `git checkout <previous-tag>` + `cargo build --release` or `docker pull <prev>`
- **Migrations:** forward-only, irreversible — do not `sqlx migrate revert`; check `docs/ROLLBACK-RUNBOOK.md` for compatibility matrix
- **Frontend:** `apps/control-plane` `git checkout` + `npm ci && npm run build`
- **Smoke:** `curl /health`, `curl /ready`, `bash scripts/verify-delivery.sh`, `bash scripts/verify-buyer-package.sh`
