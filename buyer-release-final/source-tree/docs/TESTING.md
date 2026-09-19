# Testing guide

## Layers

| Layer | Command | Needs |
|---|---|---|
| App workspace unit + integration | `cargo test --workspace` | nothing (hermetic) |
| Gated DB integration | `POSTGRES_URL=… cargo test -p bot-core --test db_integration -- --test-threads=1` | real Postgres |
| Gated Redis integration | `REDIS_URL=… cargo test -p bot-core --test redis_integration -- --test-threads=1` | real Redis |
| Gated distributed integration | `POSTGRES_URL=… REDIS_URL=… cargo test -p bot-core --test distributed_integration -- --test-threads=1` | real Postgres AND Redis |
| Gated two-replica module test | `POSTGRES_URL=… cargo test -p module-copy --test two_replica_mirror -- --test-threads=1` | real Postgres |
| Staking program host tests | `cd programs/staking-suite && cargo test` | nothing |
| Staking validator e2e | `cargo build-sbf && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | agave 2.1.21 tools |
| Latency benchmark | `cargo run --release -p sniper-suite --bin latency_bench` (see §6 report) | nothing |
| Devnet e2e (read-only) | `cargo run --bin devnet_e2e` — env-gated network tests | internet |
| Crash-recovery e2e | `E2E_NETWORK=1 E2E_LIVE=1 E2E_URL=http://127.0.0.1:8899 cargo test -p solana-kit --test recon_crash_e2e` | local validator (no real funds) |

Without their env vars the gated suites **skip cleanly** (they print a SKIP
line and pass) so `cargo test --workspace` stays hermetic and deterministic.
CI provides Postgres 16 + Redis 7 service containers and a
solana-test-validator, so there the same tests actually execute.

## What is covered where (highlights)

* **bot-core (128):** config parsing/validation (incl. `config.toml.example`
  round-trip), risk engine decisions + daily-loss auto-disable, OMS
  idempotency/state machine (+ duplicate-prevention metric), dedup
  first-arrival-wins (memory + facade), auth roles/rate limiting, audit hash
  chain + tamper detection, lifecycle shutdown phases, JSONL storage
  rotation, recovery planning, maths, the **`GlobalRiskOracle` combine
  semantics** (a shared-DB oracle can only TIGHTEN capacity/daily-loss
  limits; "unknown" falls back to the local view), and the **reconciliation
  engine decision matrix** (position comparison, execution classification, PnL
  reconstruction, dust/tolerance, unavailable-source semantics), plus the
  gap-closure additions: **`with_intent` journal semantics (link on
  signature / abandon on error / abandon on no-signature / no-sink
  passthrough), per-symbol entry-gate state, and recovery/CTF config
  defaults**, plus the Prompt-3 additions: **distributed execution
  ownership (claim/renew/fence/release/hand-off state machine on an
  injected-clock memory store, same-owner reclaim rejection, repeated
  takeover lineage, bounded renewals, guarded-run renewal ticker,
  fail-closed registry/fence/renew under fault-injected store errors,
  permit glue), runtime-flag staleness rules (kill ON immediate, OFF/flags
  recency-gated), position-book merge rules, and replica-id
  configured-vs-generated**.
* Server-side note: `symbol_for_claim`/`block_for_unresolved` and the CTF
  settlement check execute only against a live DB/venue — covered by
  db_integration + the CTF mock tests, and compiled/clippy-gated here.
* **db_integration (23, gated, EXECUTED for real vs PostgreSQL 16.4 (23/23 fresh + rerun)):** migrations, OMS restart-recovery over real
  Postgres, dedup exactly-once across "processes", audit chain tamper
  detection via direct SQL, positions/trades round-trip + idempotent retry,
  recon queue claim/backoff/give-up, transaction/checkpoint/misc repos,
  order signature lookup + status history, **transaction attribution
  columns + claim lifecycle (resolve/reopen/park), PnL replay from
  persisted fills, `startup_reconcile` report semantics, intent-journal
  lifecycle (record idempotence, link/abandon terminality, orphan listing,
  `sweep_orphan_intents` → `intent` claim), cross-replica attempts
  MAX-on-conflict with immutable attribution and terminal rows**, and the
  Prompt-3 ownership layer: **`execution_claims` single-owner + loser sees
  holder, 8-way concurrent race → exactly one winner (atomic upsert),
  lease-expiry takeover with epoch/takeover_count/previous_owner lineage +
  stale-generation fencing (verify/renew/release), release re-acquirable vs
  handoff grace, renewal extending past original expiry,
  `runtime_flags` round-trip with writer identity**, **the
  `execution_claim_events` lineage (acquired → takeover → fenced →
  released across two generations, with owners/epochs/detail)**, and the
  **`PostgresRiskOracle` open-count/realized-PnL queries against real
  position rows (incl. venue-agnostic "unknown" for Contract/Telegram)**,
  **audit-chain tamper evidence beyond content modification: reordered,
  missing and duplicated rows all break the chain, and the chain stays
  linear under 8 concurrent appenders over one shared pool (advisory-lock
  serialization regression — this race was real: a `FOR UPDATE` head read
  forked the chain under concurrent writers and was found + fixed by these
  very tests during the release pass)**.
* **redis_integration (10, gated, EXECUTED for real vs Redis 7.2.10 (10/10 fresh + rerun)):** SET NX TTL first-arrival, INCR+EXPIRE,
  token-guarded locks, dedup facade restart semantics (L2 survives an empty
  L1), env-based open helper, plus the Prompt-3 Redis claim store (Lua CAS
  over `own:claim:{id}` hashes, Redis-TIME clock): **two-replica single
  owner, expiry takeover + fencing, release/handoff grace, renewal
  extension, and `own:flag:*` runtime-flags round-trip**.
* **distributed_integration (4, gated on BOTH Postgres and Redis, EXECUTED
  for real (4/4 fresh + rerun)) — Prompt 3 §X two-context test:** two
  fully independent replica contexts (own PG pool, own Redis connection,
  own AppState, own registry) sharing the same servers — **concurrent
  claim races on the Postgres AND Redis stores elect exactly one owner;
  kill-switch engaged on context A propagates through `runtime_flags` and
  converges context B (asserting B's `may_broadcast` gate is actually
  closed, plus module-flag convergence and release propagation); the
  position book written by A converges onto B via `list_open` +
  `merge_positions`, and both contexts then compete for the SAME
  `exit:{position_id}:{rule}` claim identity — exactly one may sell**.
* **modules (104 + 1 gated):** sniper exit strategies (TP/SL/trailing/time), copy
  sizing/staleness/mirroring, **`two_replica_mirror` (gated on Postgres,
  EXECUTED for real: two independent `CopyBot` instances, same whale trade
  concurrently, exactly one passes the claim gate — proven by it reaching
  the network stage — the loser leaves zero trace, and the shared claim row
  plus event lineage name the winner)**, polymarket EIP-712 v2 signing + order types +
  paper matching + **deterministic order-id derivation (duplicate-order
  prevention)** + **CTF ERC-1155 balance reader (ABI encoding incl. 77-digit
  token ids, uint256 decode with no-truncation rule, errors-are-“could not
  read”-never-zero against a mock JSON-RPC endpoint)**, telegram parsing +
  role gating + **bot-token redaction in every Telegram API error path
  (`without_url`; closed-port regression test)**.
* **server (32):** every REST route's RBAC matrix, input validation before
  attachment checks, degradation contracts (`available:false`), audit
  verify, journal routes, loopback-bind rules, **startup-gate module
  blocking (kind→module mapping incl. the new `intent` kind, fail-safe on
  unknown kinds)**.
* **solana-kit (202 lib + gated suites):** RPC retry/failover/commitment
  semantics, executor flow incl. **broadcast-failure classification with
  mock endpoints (transport black hole → `SendUnknown` with signature;
  definite rejection → `SendFailed`)**, warm account cache, WS/pump
  parsing. Gated: `devnet_e2e` (4), `latency_bench` (5),
  `recon_crash_e2e` (2 — full crash→restart→chain-truth convergence and
  the ambiguity no-double-spend proof; see docs/RECONCILIATION.md §14).
* **staking (71 host + 3 validator e2e):** all 3 e2e EXECUTED and passed on a
  real `solana-test-validator` (Agave 2.1.21) in the hardening pass
  2026-09-18 — see docs/STAKING.md and docs/EVIDENCE-INDEX.md.

## Rules this test suite follows

1. **Deterministic:** no wall-clock races (injected timestamps/clocks),
   `Pubkey::new_unique()` only where order is controlled, unique keys per
   integration run (process id + nanos) so parallel CI jobs never collide.
   (Learned the hard way in the gap-closure pass: `Pubkey::new_unique()` is
   a per-process counter — the SAME sequence every run — so anything used
   as a cross-run DB key must be derived from the run tag instead.)
2. **Network-gated:** anything touching the internet is behind env vars
   (`POSTGRES_URL`, `REDIS_URL`, `STAKING_E2E`, devnet gates) — default runs
   are offline.
3. **No fake assertions:** skipped tests announce themselves; a green suite
   means executed-or-explicitly-skipped, never silently missing.
4. **Single-threaded where state is shared:** DB/validator suites pin
   `--test-threads=1` (one database / one ledger dir / RAM limits).

## Known gaps (NOT EXECUTED locally, executed in CI or gated)

* Live Geyser endpoint feeds (no reachable provider in the dev sandbox) —
  covered by mock-WS tests; real-feed behavior validated against devnet
  `transactionSubscribe` wire shapes captured from `api.devnet.solana.com`.
* Funded mainnet/devnet landing-rate statistics — requires a funded key and
  explicit approval; the latency bench measures the local pipeline only.
* Docker image build — no docker daemon in the dev sandbox; CI has a
  dedicated `docker` job (build + health-endpoint smoke test).
