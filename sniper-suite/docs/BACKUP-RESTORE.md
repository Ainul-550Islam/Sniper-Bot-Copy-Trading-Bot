# Backup & restore

What is durable, what is ephemeral, and the exact procedures to back up and
restore the system. All commands below match the real compose stack
(`docker-compose.yml`) and the real startup behavior of the binary.

## 1. Data classification

**DURABLE — must be backed up:**

| Data | Location | Contents |
|---|---|---|
| PostgreSQL | `pgdata` volume | orders, executions, positions, trades, transactions (attribution/claims), dedup keys (L2), risk events, audit chain, intent journal, recon queue + checkpoints, execution claims + claim events, runtime flags, api key digests, execution lifecycle `execution_lifecycle` + `execution_lifecycle_events` (0012), copy-trading journal `copy_leaders` / `copy_leader_events` / `copy_events` / `copy_links` (0013), Polymarket journal `poly_signals` / `poly_orders` / `poly_fills` / `poly_recon_findings` (0014 — the order journal is what restart recovery re-adopts open venue orders from), global ledger `ledger_events` / `ledger_postings` / `global_positions` / `global_risk_decisions` / `kill_switches` / `kill_switch_events` / `accounting_recon_findings` (0015 — `ledger_events` is what restart recovery rebuilds the global book and the daily-loss / drawdown state from; `global_positions` is a derived snapshot), HA state `ha_workers` / `ha_worker_events` / `ha_leases` / `ha_lease_events` / `ha_cursors` / `ha_feed_gaps` / `ha_recovery_records` (0016 — `ha_cursors` is what a worker resumes each feed from, `ha_leases` carries the fencing generations that keep singleton work singular, and `ha_feed_gaps` must be empty or explained before a restore is considered complete) |
| JSONL journal | `journaldata` volume (`/app/data`) | trades/positions/events as written by the journal pumps — the fastest forensic copy, independent of Postgres |
| Config + keys | outside the repo | `config.toml`, keypair files, `.env` / secret env values |

**EPHEMERAL — never backed up, loss is safe by design:**

| Data | Location | Loss behavior |
|---|---|---|
| Redis | `redisdata` volume (AOF on in compose) | dedup L2, rate-limit buckets, and — only in Redis-authority deployments — claim/flag state. With Postgres attached (the documented production topology), claims/flags are authoritative in Postgres and Redis loss cannot duplicate a financial execution: store precedence is PG > Redis > memory, and store failures fail closed |
| In-memory state | process | position book, balances, counters — reconstructed at startup from Postgres + journal + on-chain reconciliation |
| Caches | process | account cache, quote cache — refilled on demand |

## 2. Postgres backup

```bash
# Consistent dump of the whole database (compose stack):
docker compose exec postgres pg_dump -U sniper -Fc sniper > backup-$(date +%F).dump

# Plain-SQL alternative (what docs/OPERATIONS.md quick path uses):
docker compose exec postgres pg_dump -U sniper sniper > backup.sql
```

Cron this. `-Fc` (custom format) is preferred: compressed, and
`pg_restore` can select tables. The dump includes schema **and** the
`_sqlx_migrations` bookkeeping table, so the migration state travels with
the data.

A real dump→restore round-trip (dump, create a fresh database, restore,
re-verify row counts and the audit chain) was executed against PostgreSQL
16.4 during the release-engineering pass — see §7.

## 3. Postgres restore

```bash
# Fresh stack, empty database:
docker compose up -d postgres          # waits healthy
docker compose exec -T postgres pg_restore -U sniper -d sniper --no-owner < backup-YYYY-MM-DD.dump

# Then start the bot:
docker compose up -d bot
```

What happens on boot after a restore (all automatic, no manual SQL):

1. `auto_migrate` applies any migrations newer than the dump's high-water
   mark (restoring into an older snapshot + newer binary rolls the schema
   forward correctly). Checksum mismatch on an already-applied migration
   fails startup loudly — never edit applied migration files.
2. Startup recovery reloads open positions, re-registers unfinished orders
   as `Unknown`, and sweeps unresolved transactions into the reconciliation
   queue; `RECOVERY_STARTUP_RECONCILE_SECS` gates module spawn until the
   sweep settles, and `RECOVERY_BLOCK_MODULES_ON_UNRESOLVED` keeps
   safe-state modules blocked while claims are unresolved.
3. Reconciliation re-checks every ambiguous execution against truth sources
   (Solana confirmation, Polymarket order status, on-chain balances).
   **The chain is the arbiter of anything that happened after the backup** —
   fills that occurred between backup and crash are rediscovered here, not
   lost.

Restore-into-running-system note: stop the bot first. Two writers against
one database with divergent in-memory state is exactly what the ownership
layer forbids.

## 4. Redis loss / restore

There is nothing to restore. With Postgres attached:

- Claims/flags: Postgres is authoritative; Redis copies are rebuilt on
  demand.
- Dedup L2 loss: L1 (in-memory) plus the Postgres `dedup_keys` table and
  the reconciliation safety net absorb replays; a degradation metric is
  recorded (see `docs/OPERATIONS.md` matrix).
- In a **Redis-only** deployment (no Postgres), a Redis restart loses claim
  state — this is a documented limitation (`docs/DISTRIBUTED.md` §11), and
  the reason the production compose stack always includes Postgres.

## 5. Journal files

- Append-only JSONL with size-based rotation (`POST /api/journal`, owner
  role). Rotated files should be archived off-box.
- A torn final line after a crash is tolerated: readers **skip and log**
  corrupt lines (`skipping corrupt jsonl line`) — the journal never blocks
  restart. Skipped lines mean potential forensic gaps, which is why
  Postgres (not the journal) is the durable system of record.
- After restoring Postgres from an older backup, the journal is *ahead* of
  the DB — keep it; it is the independent copy used to diff against a
  broken audit chain (`docs/OPERATIONS.md` §"Audit trail").

## 6. Audit chain after restore

```bash
curl -s localhost:8080/api/audit/verify -H "x-api-key: $KEY"
# → "intact" | "broken(at_id)" | "not_chained"
```

Run this after every restore. `broken(at_id)` after a verified-good backup
means the backup itself contains the break — treat as a security incident
per `docs/OPERATIONS.md`. The chain is append-only from every app API; only
direct database access can modify it, which is precisely what verify
detects (regression-tested against modification, reorder, deletion and
duplication in `db_integration.rs`).

## 7. What was actually executed (evidence, not theory)

- **Dump→restore round-trip:** executed against the sandbox PostgreSQL 16.4
  during the release pass (dump with `pg_dump`, restore into a fresh
  database with the SQL dump, row-count and audit-chain re-verification via
  the application's own `verify_chain`). Command transcript kept in the
  release report; the sandbox PG runs durability-off, so this proves the
  procedure and SQL, not disk-crash semantics of the backup tooling.
- **Startup recovery / reconciliation after data loss:** automated tests
  (`startup_reconcile` semantics, PnL replay from persisted fills, OMS
  restart recovery, intent-journal abandonment) run against real Postgres
  in `db_integration` — 23/23 green in the release-pass gate (fresh run,
  rerun, and against the restored database).
- **Full crash→restart→chain-truth convergence:** `recon_crash_e2e`
  (solana-kit) — PREVIOUSLY VERIFIED against a local validator; gated, not
  re-executed in the latest restored sandbox.
