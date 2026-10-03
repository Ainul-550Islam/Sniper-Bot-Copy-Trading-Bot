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

`-Fc` (custom format) is preferred: compressed, and `pg_restore` can
select tables. The dump includes schema **and** the `_sqlx_migrations`
bookkeeping table, so the migration state travels with the data.

### 2.1 Scheduled backups (do not hand-roll this)

The paragraph above used to end with the words "Cron this." Documentation
that ends in an instruction to a human is not a backup system: nothing in
this repository took a backup, while `/api/saas/backup/status` told every
customer their data was protected. Both halves of that are now fixed.

```bash
# ship the scheduler with the stack (one backup per BACKUP_INTERVAL_HOURS)
docker compose -f docker-compose.yml \
               -f deploy/compose/docker-compose.backup.yml up -d

# or run one now, by hand
DATABASE_URL=postgres://… ./scripts/backup-postgres.sh
./scripts/backup-postgres.sh --dry-run     # show what it would do
```

`scripts/backup-postgres.sh` dumps, checksums, optionally encrypts with
`age`, prunes by retention, and **appends every run — including failures —
to the ledger** at `BACKUP_LEDGER_PATH`. The failure records are the
point: a backup job that goes quiet when it breaks is worse than none,
because the silence reads as success.

Two behaviours that are deliberate and should not be "fixed" later:

* **Fails closed on encryption.** If `BACKUP_AGE_RECIPIENT` is set and
  `age` is missing, the script refuses rather than writing a plaintext
  dump of a trading database.
* **Prunes only after a successful dump.** A failed backup must never
  also delete the last good copy.

### 2.2 The restore drill (SLO D2)

```bash
./scripts/verify-backup-restore.sh              # newest backup
./scripts/verify-backup-restore.sh pg-2026…Z    # a specific one
./scripts/verify-backup-restore.sh --check      # is a drill overdue?
```

It restores into a **throwaway database**, verifies the checksum against
what the ledger recorded, asserts the restored `_sqlx_migrations`
high-water mark is not newer than this checkout, checks that the tables
the business depends on exist, records the result, and drops the
throwaway database — on failure too, so drills do not litter.

Nothing schedules the drill on purpose: it creates and drops a database,
and a human should read the result. `--check` is cheap and needs no
database, so a quarterly reminder job can run that.

### 2.3 Off-site copies (and why `protected` is not the same question)

Everything above writes to a volume on the same host as the database.
That covers `DROP TABLE`, a bad migration and a corrupted page. It covers
**nothing** that takes the host: a destroyed VM, a deleted volume, a
ransomed filesystem. In that scenario the backups die with the data they
were protecting.

```bash
export BACKUP_OFFSITE_TARGET=s3://my-bucket/sniper/prod   # or an rclone remote
./scripts/backup-offsite-sync.sh            # copy + verify + record
./scripts/backup-offsite-sync.sh --dry-run

# the drill can restore FROM the off-site copy, which is the only copy
# that exists in the disaster the off-site copy is for:
./scripts/verify-backup-restore.sh --from-offsite pg-20261002T020004Z
```

The sync verifies **after** writing (`head-object` / `rclone size` against
the local byte count), because "the upload command exited 0" is not
evidence the bytes are there — a truncated multipart upload, a wrong
prefix and a policy that drops writes all manage to exit 0 somewhere.

The status endpoint therefore answers **two** questions, deliberately kept
apart:

| field | question it answers |
|---|---|
| `protected` | can this data be restored, proven by a drill? |
| `offsite.state` | would a copy survive losing this host? |

Collapsing them would force one of two lies: either a deployment with
proven local restores is reported unprotected (a false alarm operators
learn to ignore), or a deployment whose only copies sit on the same disk
is reported as protected against losing that disk. So `summary` always
ends by saying which one is true — including the blunt
`copies are LOCAL ONLY — losing this host would take the database and its
backups together`.

Credentials are read from the environment by `aws`/`rclone`. **The ledger
records only the KIND of destination** (`s3`/`rclone`) — never a bucket,
prefix or endpoint — because a tenant-facing endpoint reads that file and
an answer about backups must not become a target list.

### 2.4 Point-in-time recovery (the recovery point objective)

With dumps alone the RPO is **one backup interval — up to 24 hours of
trades** on the defaults. Reconciliation rediscovers fills that happened
after the backup (§3), but "the chain will sort it out" is a hope with a
settlement risk attached, not a durability design.

WAL archiving moves the RPO from the backup interval to the WAL segment
interval:

```bash
docker compose -f docker-compose.yml \
               -f deploy/compose/docker-compose.backup.yml \
               -f deploy/compose/docker-compose.pitr.yml up -d
```

| piece | what it is | without it |
|---|---|---|
| `scripts/archive-wal.sh` | `archive_command` target: copies each segment out, verified and atomically published | no roll-forward at all |
| `scripts/backup-basebackup.sh` | physical `pg_basebackup` — the thing WAL is replayed **onto** | an archive that restores nothing |
| `scripts/prune-wal-archive.sh` | deletes only segments no retained base needs | the archive volume fills and the database stops |
| `scripts/restore-pitr.sh` | restores to a chosen moment, and `--verify` proves it | an unproven claim |

**`archive_mode` is not reloadable — applying the overlay restarts
PostgreSQL.** Do it in a maintenance window.

#### The three rules that make this safe

1. **Exit 0 only when the segment is stored.** Postgres recycles the
   local segment the moment `archive_command` succeeds, so a lie there is
   unrecoverable loss rather than a failed job. Every doubt in
   `archive-wal.sh` exits non-zero; Postgres then retains WAL, `pg_wal`
   grows, and the database eventually refuses to write. That is the safe
   direction — **monitor the archive volume**, because it is not a quiet
   one.
2. **Never overwrite an archived segment with different content.**
   Re-invocation for an already-archived segment is legitimate and is
   treated as success when the bytes are identical; different bytes are a
   hard failure (two clusters sharing an archive, or a reused timeline).
3. **Prune WAL only after a new base backup succeeds**, and never past
   the start of the oldest base still on disk. `prune-wal-archive.sh`
   refuses to guess: no base backup means no pruning.

#### Restoring to a moment

```bash
./scripts/restore-pitr.sh --base base-20261002T020000Z \
                          --target-time '2026-10-02T09:15:00Z'
./scripts/restore-pitr.sh --latest --verify    # quarterly drill
./scripts/restore-pitr.sh --check              # is a PITR drill overdue?
```

Always pass the target in **UTC with an explicit `Z`**. A PITR that lands
an hour from the intended moment is worse than none, because it will be
believed. Recovery pauses at the target instead of promoting, so the
cluster can be inspected and re-targeted; promotion stays a deliberate
human act.

The drill restores into a throwaway directory on a throwaway port
(`BACKUP_PITR_PORT`, default 55432), waits for recovery, checks the same
business tables the logical drill checks, records `pitr_verified`, and
stops the cluster. It never touches the live data directory.

#### What the endpoint reports

`/api/saas/backup/status` gains a third axis:

| field | meaning |
|---|---|
| `pitr.state` | `not_configured` · `failing` · `stale` · `unverified` · `current` |
| `rpo_estimate_seconds` | worst-case data loss **as evidenced by the ledger** — `null`, never `0`, when nothing is recorded |
| `rpo_basis` | `wal_archive` · `last_backup` · `unknown` |

A **failing or stale archive is never used as the RPO basis.** That is
exactly the situation in which the optimistic number is wrong, so the
answer falls back to the dump interval and says so.

One subtlety worth knowing: an **idle** database produces no WAL, so a
quiet archive would look broken. The overlay sets
`archive_timeout = 300`, forcing a segment switch on a timer — which is
what makes "the archive has been quiet" mean something.

### 2.5 Getting the PITR artefacts off-site

Dumps going off-site while the WAL archive stays on the host is the
combination that reads best and protects least. Recovery then offers a
choice between "off-site, hours old" and "minutes old, on the machine
that just died" — which is not the choice anyone thinks they bought.

```bash
BACKUP_OFFSITE_INCLUDE_PITR=true ./scripts/backup-offsite-sync.sh
```

This mirrors `base/` and `wal-archive/` alongside the dumps and records
`scope: "dumps+pitr"` in the ledger, which is what turns
`offsite.includes_pitr` true. The sync script warns on every run when
this host archives WAL and the flag is off.

WAL segments are mirrored with `aws s3 sync` / `rclone sync --size-only`
rather than one upload per file. A size comparison is sufficient **only**
because WAL segments and base tarballs are written once and never
modified; do not copy that flag onto anything mutable.

### 2.6 Should you keep these scripts, or adopt pgBackRest / wal-g?

An honest comparison, because four shell scripts in a trading repository
are a maintenance commitment:

| | these scripts | pgBackRest / wal-g |
|---|---|---|
| dependencies | `pg_*`, `age`, `aws`/`rclone` | one binary |
| parallel WAL push | no — serial `archive_command` | yes |
| incremental / differential base backups | no (full every time) | yes |
| compression, encryption, retention policies | basic, ours | mature, configurable |
| restore to a point in time | yes (`restore-pitr.sh`) | yes, with more options |
| **integration with this system's tenant-facing status** | **yes — the ledger drives `/api/saas/backup/status`** | would need an adapter |
| who fixes it at 3am | you | an upstream project with users |

**Recommendation.** If this deployment is run by a team that will not
maintain backup tooling, adopt **pgBackRest** and write a small shim that
appends the same ledger records — the posture endpoint, the drills and
the SLO wiring all stay. These scripts exist because the repository had
*nothing*, and "nothing, but documented" was being reported to customers
as protection. They are deliberately small, boring and auditable; they
are not an attempt to out-engineer pgBackRest.

What you should NOT do is run both. Two things archiving WAL into one
destination is the "already archived with different content" failure
`archive-wal.sh` refuses on — by design.

### 2.7 What the customer is told

`/api/saas/backup/status` derives its answer from the ledger and nothing
else:

| state | meaning |
|---|---|
| `verified` | current backup **and** a restore drill inside the 90-day window — the only state that reports `protected: true` |
| `unverified` | backups run, but no restore has ever been proven. An untested backup is an assumption |
| `stale` | the last successful backup is older than 36h — the schedule is not running |
| `failing` | the most recent attempt failed |
| `not_configured` | no ledger: this deployment is not backing anything up |

plus an `offsite` object (`not_configured` | `current` | `stale` |
`failing`, with `includes_pitr`) and a `pitr` object, each on its own
axis.

The response never carries a path, bucket, hostname or backup id.

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
