# Demo runbook — deterministic buyer demonstration

Ten demonstrations a seller can run for a buyer (or a buyer can run alone)
against the delivered source. Each demo states: prerequisites, exact
commands, expected observable result, what it proves, and its verification
status. **No demo involves live trading or real funds.** Demos 1–7 and 9–10
run on one machine with PostgreSQL + Redis; Demo 8 needs two processes
(still one machine). Status labels: VERIFIED = the underlying behavior is
covered by tests executed in the final freeze gate; PREVIOUSLY VERIFIED =
covered by tests executed on identical source in an earlier session;
the demo itself is a live re-demonstration on the buyer's/seller's machine.

Global prerequisites: pinned toolchain installed (`rust-toolchain.toml`
auto-selects 1.98.1), `POSTGRES_URL` + `REDIS_URL` exported, migrations
applied (automatic at startup with `auto_migrate`), release build:
`cargo build --release`. Config: `cp config.toml.example config.toml`
(defaults are paper mode, modules disabled).

## Demo 1 — Paper mode end-to-end

- **Prerequisites:** global; a module enabled in `config.toml`
  (e.g. `[sniper] enabled = true`) or enabled at runtime via API/Telegram.
- **Command:**
  ```bash
  CONFIG_PATH=./config.toml ./target/release/sniper-suite
  # in another shell:
  curl -s -X POST localhost:8080/api/modules/sniper/enable -H "x-api-key: $API_KEY"
  curl -s localhost:8080/api/status
  ```
- **Expected:** `mode: paper` in status; the dashboard (`http://localhost:8080/`)
  shows simulated fills against live market data with seeded balances
  (10 SOL / 1000 USDC); positions/trades accumulate; no signature or
  on-chain transaction exists anywhere.
- **Proves:** default-safe operation; the full decision → risk → simulated
  fill → persistence → UI pipeline works without touching a venue.
- **Status:** behavior covered by VERIFIED workspace tests (state machine,
  paper fills, API); live demonstration on the demo machine.

## Demo 2 — Simulate mode (real transactions, zero broadcast)

- **Prerequisites:** Demo 1 setup + `SOLANA_KEYPAIR` (use a fresh empty
  keypair — simulate never sends).
- **Command:**
  ```bash
  EXECUTION_MODE=simulate CONFIG_PATH=./config.toml ./target/release/sniper-suite
  curl -s localhost:8080/api/status   # mode: simulate
  ```
- **Expected:** orders are built as real Solana transactions and sent
  through `simulateTransaction`; logs show simulation outcomes; nothing is
  broadcast (no signatures on-chain); ambiguous synthesized outcomes stay
  `Sent` and are resolved by reconciliation, never shown as confirmed.
- **Proves:** the real execution path (build → sign via signer registry →
  simulate) minus broadcast; honest simulate semantics.
- **Status:** simulate-policy behavior VERIFIED in unit tests; the
  documented simulate status semantics are in `docs/RECONCILIATION.md`.

## Demo 3 — Health / readiness / metrics

- **Prerequisites:** server running (any mode).
- **Command:**
  ```bash
  curl -s localhost:8080/health
  curl -si localhost:8080/ready | head -20
  curl -s localhost:8080/metrics | grep '^bot_' | head -20
  # degrade it: stop Redis (or block RPC), then re-run /ready
  ```
- **Expected:** `/health` always 200 while serving (never reflects
  dependencies); `/ready` 200 with component report, flipping to 503 with
  the affected component marked when Redis/RPC/module degrades, and back to
  200 on recovery; `/metrics` exposes `bot_*` series with bounded labels
  matching README §Observability; every response carries `x-request-id`.
- **Proves:** liveness/readiness separation, visible degradation, metric
  surface, request correlation.
- **Status:** VERIFIED (health/readiness/metrics/correlation tests in the
  521; metric names verified against source).

## Demo 4 — Risk rejection

- **Prerequisites:** server running in paper mode with a module enabled;
  tight risk limits in `config.toml` (`[risk]` — e.g. max positions 1,
  small exposure cap, or a low daily-loss limit).
- **Command:**
  ```bash
  # let it take one position, then watch the next signal get rejected:
  curl -s localhost:8080/api/status          # risk counters
  curl -s localhost:8080/metrics | grep risk_rejections
  # dashboard/events feed shows risk_rejected events
  ```
- **Expected:** signals exceeding a limit produce `risk_rejected` events
  (dashboard + WS feed), `bot_module_risk_rejections_total` increments, and
  **no order is created** — rejection happens before execution.
- **Proves:** the global pre-trade risk engine gates every module.
- **Status:** VERIFIED (risk decision tests + rejection event/metric paths).

## Demo 5 — Kill switch

- **Prerequisites:** server running with a module enabled.
- **Command:**
  ```bash
  curl -s -X POST localhost:8080/api/kill -H "x-api-key: $API_KEY"
  curl -s localhost:8080/api/status            # kill_switch engaged
  curl -s localhost:8080/metrics | grep bot_kill_switch
  # via Telegram (owner id): /kill then /resume
  curl -s -X POST localhost:8080/api/resume -H "x-api-key: $API_KEY"
  ```
- **Expected:** all trading halts immediately on `/api/kill` (or Telegram
  `/kill` from an authorized id; explicit refusal from unauthorized ids);
  `bot_kill_switch` gauge flips; `/resume` clears it. In a multi-replica
  setup the flag syncs to all replicas (Demo 8).
- **Proves:** emergency stop works from both control surfaces and is
  observable.
- **Status:** VERIFIED (kill-switch tests; cross-replica flag sync tested in
  `distributed_integration`).

## Demo 6 — Restart / recovery

- **Prerequisites:** paper (or simulate) run with recorded intents/positions.
- **Command:**
  ```bash
  kill -9 $(pgrep -f 'target/release/sniper-suite')
  CONFIG_PATH=./config.toml ./target/release/sniper-suite   # restart
  # watch startup logs: journal replay + reconciliation before new work
  curl -s localhost:8080/api/status
  ```
- **Expected:** startup replays the JSONL intent journal + Postgres intents,
  reconciles unresolved intents against venue truth **before** accepting new
  work; no duplicate executions (dedup + claims); positions/PnL consistent;
  `/ready` returns to 200.
- **Proves:** crash safety: restart neither loses nor double-executes
  recorded intents.
- **Status:** VERIFIED (restart-fidelity, corrupt-line, OMS restart-recovery
  tests); `recon_crash_e2e` against a local validator is PREVIOUSLY
  VERIFIED.

## Demo 7 — Audit-chain verification

- **Prerequisites:** server has run and recorded audit events.
- **Command:**
  ```bash
  curl -s -H "x-api-key: $API_KEY" localhost:8080/api/audit/verify
  # tamper demonstration (on a THROWAWAY database only):
  psql "$POSTGRES_URL" -c "UPDATE audit_events SET detail='x' WHERE id=(SELECT max(id) FROM audit_events);"
  curl -s -H "x-api-key: $API_KEY" localhost:8080/api/audit/verify   # now reports the break
  ```
- **Expected:** valid chain before tampering; after modifying any row the
  verifier reports the exact break. No app API can delete/modify audit rows
  (append-only); the tamper step requires direct DB access and exists only
  to demonstrate detection.
- **Proves:** tamper-evident, hash-chained, append-only audit trail.
- **Status:** VERIFIED (modification/reorder/missing/duplicate detection +
  linear chain under 8 concurrent appenders, `db_integration`).

## Demo 8 — Distributed claim behavior (where local infrastructure permits)

- **Prerequisites:** one machine, shared PG + Redis; two server processes
  with distinct data/HTTP ports (or run the test harness directly).
- **Command (harness — deterministic):**
  ```bash
  POSTGRES_URL=... REDIS_URL=... cargo test -p module-copy --test two_replica_mirror -- --test-threads=1
  POSTGRES_URL=... REDIS_URL=... cargo test -p bot-core --test distributed_integration -- --test-threads=1
  # live variant: start two processes, enable the same module on both,
  # watch claims/leases/fencing in logs + execution_claim_events table
  ```
- **Expected:** for each logical execution exactly one replica holds the
  claim (lease + epoch + fencing token); the mirror test shows one tracked
  trade produces one mirrored execution across two racing replicas;
  `execution_claim_events` records the lineage; kill-switch/module flags
  sync across replicas.
- **Proves:** the invariant one execution ⇒ ≤1 owner ⇒ ≤1 money-moving
  submission under real contention.
- **Status:** VERIFIED (4/4 + 1/1 against real PG/Redis in the freeze gate).

## Demo 9 — Staking tests / documented historical validator evidence

- **Prerequisites:** Rust toolchain (host tests need nothing else).
- **Command:**
  ```bash
  cd programs/staking-suite && cargo test        # 71 host tests
  # full on-chain lifecycle (requires Solana CLI/agave 2.1.21 toolchain):
  cargo build-sbf && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1
  ```
- **Expected:** 71/71 host tests pass anywhere (validation layer, caps,
  pause, timelock queue/apply/cancel, two-step admin, genesis latch +
  max-supply cap math, reward clamping, metadata guards/layout, state
  math, instruction (de)serialization). With the Solana toolchain present:
  the `.so` builds and the 3 e2e tests run the full lifecycle on a
  local `solana-test-validator`, including funded stake → reward → unstake.
- **Proves:** program logic on host; on-chain behavior where the toolchain
  exists.
- **Status:** host 48/48 VERIFIED (freeze gate); build-sbf + validator e2e
  2/2 PREVIOUSLY VERIFIED (agave 2.1.21, identical source; CI `program` job
  re-runs both on every push once CI is active). **Reminder for the demo
  audience: no external audit exists; mainnet deployment is blocked until
  one passes.**

## Demo 10 — Backup / restore

- **Prerequisites:** a database with recorded state (from Demos 1–7).
- **Command:**
  ```bash
  pg_dump "$POSTGRES_URL" -Fc -f sniper_backup.dump
  createdb sniper_restored && pg_restore -d sniper_restored sniper_backup.dump
  POSTGRES_URL=postgres://user:pass@host:5432/sniper_restored \
    cargo test -p bot-core --test db_integration -- --test-threads=1
  ```
- **Expected:** restore succeeds; the full 23-test db_integration suite
  passes **against the restored database** (including audit-chain
  verification of restored rows). Redis needs no restore (non-authoritative).
- **Proves:** the durable-truth model: PostgreSQL + journal files are the
  system of record; backups are complete and functional.
- **Status:** VERIFIED (this exact round-trip ran green in the freeze gate).

---

## Demo sequencing advice

Run 1 → 3 → 4 → 5 first (pure paper, zero setup risk), then 6 → 7 → 10
(recovery/tamper/backup), then 2 (needs a keypair), then 8 (two processes),
then 9 (staking). Total time on a warm build: roughly 1–2 hours. Nothing in
this runbook requires funded keys, mainnet access, or live trading.
