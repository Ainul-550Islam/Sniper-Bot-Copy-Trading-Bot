# Final operations handover (day-2 runbook summary)

Operational transfer document: everything needed to run, stop, watch, and
hand over the system. Deep-dive companions: `docs/OPERATIONS.md` (normal
ops + degradation matrix), `docs/RECONCILIATION.md`, `docs/BACKUP-RESTORE.md`,
`docs/DEPLOYMENT.md`, `docs/FINAL-INCIDENT-RUNBOOK.md` (failure playbook).
No real secrets appear in this document; credential NAMES only.

## 1. Startup

```bash
export CONFIG_PATH=/path/to/config.toml          # validated at boot; bad config = fail-fast
export POSTGRES_URL=postgres://USER:PASSWORD@HOST:5432/DB
export REDIS_URL=redis://HOST:6379
export RUST_LOG=info                              # debug for troubleshooting (never trace in prod with keys)
./sniper-suite                                    # or: cargo run --bin sniper-suite
```

Boot order (main.rs): config load+validate → DB pool + **migrations
(auto, forward-only)** → Redis → signer registry (LOCAL keypair only;
vault/kms/hsm = typed startup FAILURE by design) → audit/journal → API
server → recovery pass (reconcile gate) → modules spawn ONLY after the
startup reconcile completes (`RECOVERY_STARTUP_RECONCILE_SECS`). Modules
start disabled unless enabled by config/API. Default `execution_mode =
paper`, `allow_live_trading = false`.

## 2. Shutdown / restart

* **Graceful:** `SIGTERM` → runtime-flag stop → journal-pump stop → HTTP
  drain → module drain → pump flush → `sniper-suite stopped cleanly`
  (observed in evidence `phase8b-shutdown.log`). Never SIGKILL unless hung;
  SIGKILL is crash-safe (recovery on next boot) but skips the drain.
* **Restart:** stop → start. State lives in PostgreSQL + JSONL journal;
  Redis is accelerator-only. Restart runs the recovery pass automatically.
* **Rolling restart of 2 replicas:** safe — ownership leases/fencing tokens
  prevent double-execution (two_replica_mirror test executed).

## 3. Health / readiness / observability

| Surface | What |
|---|---|
| `GET /health` | liveness (no dependencies touched) |
| `GET /ready` | per-component readiness (DB, Redis, modules, RPC) — use for orchestrator probes |
| `GET /api/health` | app-level ok flag |
| `GET /metrics` | Prometheus text: `bot_*` families (health_ready, http request durations per route, execution/recon counters) |
| Logs | structured tracing (stdout); `RUST_LOG` levels; secrets never logged (redaction tests) |
| Audit trail | `GET /api/audit` (paged), `GET /api/audit/verify` (hash-chain verify; `x-api-key` required) — append-only, no mutation API exists |

## 4. Database (PostgreSQL)

* Durable source of truth: orders/executions, positions/trades, dedup +
  risk + audit, reconciliation, transaction attribution, intent journal,
  claim kinds, execution claims, runtime flags, claim events (migrations
  0001–0011).
* Migrations: sqlx, forward-only, checksummed — NEVER edit an applied
  migration; add a new one and restart.
* Connection: single `POSTGRES_URL`; pool sized in config. TLS/roles =
  buyer infrastructure.

## 5. Redis

* Roles: dedup accelerator, ownership claim fast-path, rate-limit state,
  pub/sub hints. **No financial state lives solely in Redis** (design rule;
  verified by distributed tests). Redis outage → degraded mode per
  `docs/OPERATIONS.md` §Dependency degradation matrix (PG fallback paths),
  not data loss.

## 6. Backup / restore

Follow `docs/BACKUP-RESTORE.md`. Summary: `pg_dump -Fc` on a schedule →
restore drill: `pg_restore --no-owner` into a clean DB → verify
`_sqlx_migrations` 11/11 + audit-verify + app boot. Executed round-trip
evidence: dump `5989ecf1…`, identical tables/rowcounts, 23/23 suite on the
restored DB, app ran against it. Journal files (JSONL) are append-only —
back them up alongside PG if you need pre-flush intent history.

## 7. Telegram control plane

Commands (owner/allowlist enforced; unknown commands rejected; token never
in errors):

| Command | Effect | Role |
|---|---|---|
| `/status` | mode, kill-switch, balances, module states | allowlisted |
| `/mode` | show/switch execution mode (live requires owner + gates) | owner for changes |
| `/positions` | open position book | allowlisted |
| `/pnl` | realized/unrealized summary | allowlisted |
| `/kill` | emergency kill switch ON (stops new entries immediately) | owner |
| `/resume` | kill switch OFF | owner |

Setup: bot token + owner chat id + allowlist per `.env.template` /
config `[telegram]`. Without a token the module stays disabled.

## 8. Emergency kill

* Fastest: Telegram `/kill` (owner) or `POST /api/kill` (owner API key) or
  flip the runtime flag via API. Kill switch: new entries stop IMMEDIATELY;
  in-flight orders continue to confirmation/reconciliation (never orphaned);
  live-mode re-activation is refused while the kill switch is set.
* Full stop: `SIGTERM` (graceful) — see §2.

## 9. Live-mode activation / deactivation

Activation is a TWO-KEY ceremony by design:
1. `execution_mode = live` (config or `/api/mode`, owner role), AND
2. `allow_live_trading = true` (explicit config flag; `may_broadcast` hard
   gate — without it, live mode still cannot broadcast).
Preflight on activation: signer registry built (local keypair present), RPC
reachable, risk limits loaded; failure = typed refusal, no partial live
state. Deactivation: `/api/mode` back to paper/simulate (owner) — open
positions keep their lifecycle (exits/reconciliation continue).

## 10. Reconciliation & recovery (daily ops)

* Watch recon queue depth/age metrics; work items per `docs/OPERATIONS.md`
  §"Incident: did my order actually land?".
* `GET /api/recovery/failed` lists failed-recovery items; `GET /api/orders`
  shows `Unknown`/`Reconciled` states; resolution = signature lookup →
  attribution write → state converge (idempotent sweeps).
* After any crash: boot recovery re-registers unfinished orders as
  `Unknown` and gates module spawn on the reconcile pass — do not disable
  the gate.

## 11. Key & credential rotation

| Credential | Rotation procedure |
|---|---|
| Solana wallet keypair (`SOLANA_KEYPAIR`/config) | generate new keypair → fund it → stop app → swap file/env → start → verify `/api/status` balance reads from the NEW wallet → keep old key until all in-flight txs confirmed; audit-log the change window |
| API keys (`x-api-key`) | `GET/DELETE /api/keys`, `POST /api/keys` (owner) — issue new key, update clients, revoke old via `DELETE /api/keys/{hash}`; keys are stored hashed |
| Telegram bot token | @BotFather `/revoke` → new token into env → restart → re-verify allowlist; old token dead immediately |
| Polymarket credentials (funder key / API creds) | funder key: move funds to new address, update config, re-check allowance for the exchange contracts (spender addresses in `docs/LIVE-VALIDATION.md`); API creds: regenerate on Polymarket, update env, restart |
| PG/Redis passwords | standard service-side rotation + update `POSTGRES_URL`/`REDIS_URL` + rolling restart |

Rotation never requires code changes; all secrets are env/config-injected.
After ANY rotation: run the health/readiness checks (§3) and one paper-mode
cycle before re-enabling live.

## 12. RPC rotation

`[solana].rpc_url` + failover list are config-driven (client rotates on
failure — failover tests executed). To rotate: add the new endpoint to the
failover list → hot-reload config (or restart) → remove the old endpoint →
watch latency/error metrics. Polymarket reads use `[polymarket].ctf_rpc_url`
(Polygon) — same procedure. Never point production at public rate-limited
endpoints for live trading.

## 13. Deployment rollback (app)

Images/binaries are versioned by release; DB migrations are forward-only:
* Rollback = redeploy the previous binary + keep the DB schema (newer
  migrations are additive; old binary ignores new columns — verified by the
  0001–0011 additive design). If a migration is genuinely incompatible,
  restore from backup (§6) — this is why the backup cadence matters.
* Config rollback: `config.toml` is a file — keep the previous version
  alongside; `CONFIG_PATH` swap + restart.

## 14. Staking program deployment / upgrade notes

* Deployment: HUMAN ACTION via `scripts/staking-identity.sh` (set-id →
  build-sbf → deploy guards; see acceptance E3). Deploy as UPGRADEABLE
  (default `solana program deploy`) and record: program id, upgrade
  authority, payer, deploy tx signature, .so hash.
* Upgrade: `solana program deploy --program-id <KP> <NEW_SO>` with the
  upgrade authority; state (config/stakes) persists across upgrades;
  genesis is latched (re-init rejected) and max-supply cap is IMMUTABLE —
  an upgrade CANNOT change the cap (enforced in state, not just logic).
* Pre-mainnet gate: external audit (category 5 in
  `docs/FINAL-KNOWN-LIMITATIONS.md`) — the vendor claims NO audit.
* Authority hygiene: upgrade authority should be a multisq/offline key in
  production; the two-step admin transfer inside the program covers the
  program's own admin, not the loader upgrade authority.

## 15. Incident response (summary)

Detect (metrics/logs/alerts you wire) → classify → act per
`docs/FINAL-INCIDENT-RUNBOOK.md` (17 scenarios with DETECTION / IMMEDIATE
ACTION / SAFE STATE / RECOVERY / VERIFICATION / POST-INCIDENT EVIDENCE) →
post-incident: preserve logs + journal + audit chain export; never edit the
audit table.

## 16. Support handover checklist

* [ ] Operator has read OPERATIONS.md + this file + INCIDENT runbook.
* [ ] Backups scheduled + one restore drill done on buyer infra.
* [ ] Alerts wired on: /ready != 200, recon queue age, execution error
      rate, kill-switch flag, live_allowed flag changes.
* [ ] Key inventory (which key lives where, who holds it) written by buyer.
* [ ] Acceptance test A–R filled and signed (BUYER-ACCEPTANCE-TEST.md).
