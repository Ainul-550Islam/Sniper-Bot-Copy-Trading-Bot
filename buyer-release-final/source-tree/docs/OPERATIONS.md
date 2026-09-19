# Operations runbook

Day-two operations: what to watch, and what to do when something breaks.

## Normal state

```bash
curl -s localhost:8080/ready | jq          # all components ready
curl -s localhost:8080/api/health | jq     # mode, kill switch, uptime
curl -s localhost:8080/api/modules | jq    # per-module enabled/healthy
```

Dashboard: open `http://127.0.0.1:8080/` (live WS feed of every event).

Key metrics to scrape/alert on (`/metrics`, Prometheus text):

| Metric | Meaning / alert idea |
|---|---|
| `bot_health_ready` | 0 → dependency down (alert immediately) |
| `bot_kill_switch` | 1 → trading halted (alert; investigate why) |
| `bot_execution_mode` | mode changed unexpectedly (paper→live!) |
| `bot_app_errors_total` | rate > 0 sustained |
| `bot_module_healthy` / `bot_module_consecutive_errors` | a feed is sick |
| `bot_execution_latency_ms` (histogram) | p99 creeping up vs. your baseline |
| `bot_dup_total` / dedup degradation counters | replay storms / L2 outage |
| `bot_db_pool_active` / `bot_db_pool_idle` | pool exhaustion |
| `bot_events_dropped_total` | WS/broadcast consumers too slow |

## Emergency stop

Any of these halts new intent acceptance immediately (exit handling
continues):

```bash
curl -X POST localhost:8080/api/kill -H "x-api-key: $KEY"   # operator+
# or Telegram: /kill                                          (operator+)
```

Resume after the cause is fixed:

```bash
curl -X POST localhost:8080/api/resume -H "x-api-key: $KEY"
```

Independent of the kill switch, the risk engine **auto-disables all trading
modules** when the daily realized-loss limit (`daily_loss_limit_quote`) is
breached — a `risk_rejected` event announces it and modules stay off until
the next UTC day (or until you re-enable them deliberately). Consecutive
execution failures (`max_consecutive_failures`) auto-disable the affected
module with a fatal error event. Check `/api/health`, `/api/modules` and the event feed for the
trip reason before resuming.

## Incident: did my order actually land?

The OMS + reconciliation stack answers this without trusting the feed:

1. `GET /api/orders?status=unknown` — orders whose outcome is unresolved.
2. The recon queue re-checks them against truth sources: Solana tx
   confirmation, Polymarket order status, and on-chain token balances
   (position drift is **flagged**, never auto-corrected).
3. `GET /api/recovery/failed` — items that exhausted retries (backoff caps
   at 1h). These need a human: inspect the signature/order id, then fix
   state via normal flows (a fill event, or mark closed manually).

After a crash/restart you do NOT need to do this manually — startup recovery
reloads open positions, re-registers unfinished orders as `Unknown`, and
sweeps unresolved transactions into the recon queue automatically.

## Journal (JSONL) management

`JournalPump` writes trades/positions/events to `data/*.jsonl`
(`[storage]` config). Even with Postgres attached this is your fastest
forensic copy.

```bash
curl -s localhost:8080/api/journal -H "x-api-key: $KEY"        # sizes/paths
curl -X POST localhost:8080/api/journal -H "x-api-key: $OWNER" # rotate (owner)
```

Rotation renames current files; the pumps reopen fresh ones. Archive rotated
files off-box; they are append-only and grep-friendly.

Journal readers **skip and log corrupt/truncated lines** (`skipping corrupt
jsonl line`): a torn final line after a crash never blocks restart. Skipped
lines are a forensic gap, which is why Postgres — not the journal — is the
durable system of record. Full backup/restore procedures (incl. Redis-loss
behavior and post-restore reconciliation): `docs/BACKUP-RESTORE.md`.

## Audit trail

```bash
curl -s "localhost:8080/api/audit?limit=50" -H "x-api-key: $KEY"
curl -s localhost:8080/api/audit/verify -H "x-api-key: $KEY"   # intact?
```

If `verify` reports `broken(at_id)`, treat it as a security incident:
someone with DB access modified history. Restore from backup, rotate
credentials, and diff against the JSONL journal (independent copy).

## Backups

* **Postgres:** `docker compose exec postgres pg_dump -U sniper sniper > backup.sql`
  (cron; the DB holds orders/trades/positions/audit/recon).
* **Journal files:** the `journaldata` volume / `data/` dir.
* **Config + keypair:** offline copies, never in the repo.

Restore drill: start a fresh stack, restore the dump, boot the binary —
recovery should bring positions/orders back without manual SQL.

## Dependency degradation matrix

| Dependency down | Behavior |
|---|---|
| Postgres | startup: continues if not `required` (journal-only mode); runtime: persistence pump queues/backs off, DB-backed API routes report `{"available":false}`, dedup falls back to L1 + degraded metric |
| Redis | dedup L2 → L1 verdicts (metric records degradation); rate limiter unaffected (in-process) |
| RPC/WS feed | module marked unhealthy, consecutive-error counter climbs, retries with backoff; no trades are placed on stale data (staleness guards) |
| Polymarket (CLOB/Gamma/WS) | order flow fails closed (nothing broadcasts on venue/auth errors); an ambiguous submit hands the claim off with grace so no second replica resubmits; reconciliation polls CLOB order status as a truth source; sustained failures trip `max_consecutive_failures` → module auto-disabled with a fatal error event |
| Telegram | alerts queue/drop per config; trading unaffected |

## Upgrades & rollbacks

Images are tagged; roll back by pinning the previous tag and restarting —
migrations are additive so an older binary generally runs against a newer
schema (verify release notes per change). Shutdown is drain-ordered
(HTTP → modules → pumps → DB), so restarts mid-trade reconcile on boot.
