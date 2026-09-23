# Testing guide

## Latest verified source state (2026-09-22)

The current tree passed `cargo fmt --all -- --check`, `cargo check
--workspace`, strict workspace Clippy, and the complete workspace test run:
**1028 passed, 0 failed, 1 intentionally ignored**. PostgreSQL 17.11 was live;
26/26 `db_integration` tests and both new SaaS durability tests executed.
The ignored test is the opt-in replay-fixture generator. Redis-specific and
explicitly live-network/broadcast gates were not enabled in this run. Older
counts below are retained as dated historical evidence, not current totals.

## Layers

| Layer | Command | Needs |
|---|---|---|
| App workspace unit + integration | `cargo test --workspace` | nothing (hermetic) |
| Gated DB integration | `POSTGRES_URL=… cargo test -p bot-core --test db_integration -- --test-threads=1` | real Postgres |
| Gated Redis integration | `REDIS_URL=… cargo test -p bot-core --test redis_integration -- --test-threads=1` | real Redis |
| Gated distributed integration | `POSTGRES_URL=… REDIS_URL=… cargo test -p bot-core --test distributed_integration -- --test-threads=1` | real Postgres AND Redis |
| Gated two-replica module test | `POSTGRES_URL=… cargo test -p module-copy --test two_replica_mirror -- --test-threads=1` | real Postgres |
| Copy engine suites (unit, leader lifecycle, event pipeline, dedup/ordering, policy/sizing, intent execution, reconciliation, crash recovery, concurrency) | `cargo test -p module-copy -- --test-threads=1` | nothing (reuses the sniper's mock JSON-RPC node harness by path, test-only) |
| Sniper engine suites (unit, pipeline, failure injection, exit sweeper, concurrency, replay, property) | `cargo test -p module-sniper -- --test-threads=1` | nothing (mock JSON-RPC node + mock websocket feed on loopback, test-only) |
| Replay fixture regeneration (only after an intentional generator change) | `cargo test -p module-sniper --test replay -- --ignored regenerate_replay_fixtures` | nothing |
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

* **bot-core (155 lib + gated suites):** config parsing/validation (incl. `config.toml.example`
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
* **module-sniper (126 = 60 lib + 66 integration; sniper-engine pass
  2026-09-21, see docs/SNIPER-ENGINE.md §12):** lib tests cover exit
  strategies (TP/SL/trailing/time/stale), `LaunchEvent` identity / shape /
  staleness / consistency, the stage transition table, every
  `RejectReason` ↔ `RiskCode` mapping, gates (pass/fail/skip, strict mode),
  slippage modes + hard max + price-impact arithmetic, the deterministic
  fee estimate / fee budget (route, policy ceiling, tip, rounding,
  saturation, on/off/boundary) and the pump.fun / PumpSwap / Raydium
  decoders. `tests/pipeline.rs` (15): paper and live
  entries walking the full lifecycle against a scripted mock node, the
  authoritative dedup, malformed/stale events, kill switch / disabled
  module / symbol gate, screening denylist, invalid route and graduated
  curve, token-state and liquidity gates, slippage and price-impact limits,
  fee budget and fee-policy veto as `FEE_LIMIT` before risk (no attempt,
  no cooldown, no ledger record), exposure limits, failed-entry cooldown,
  latency budget at submission, the legacy `consider_launch` door.
  `tests/failure_injection.rs` (22):
  websocket disconnect during / after detection, slot regression after
  reconnect, RPC timeout without fallback, failover to a healthy provider,
  tripped provider pool, stale blockhash replaced before broadcast,
  blockhash rejected by the node, failed simulation never broadcasts,
  ambiguous send, confirmation timeout parks the entry, duplicate intent
  refused by the ledger, restart with a failed entry (cleanup without
  selling), intent journal ordering (recorded before broadcast, abandoned
  on rejection), sniper daily-loss limit, kill switch mid-pipeline,
  malformed curve account, stale snapshot at submit time, malformed feed
  payloads, missing liquidity, unreadable mint (default skip vs
  `strict_gates`), PumpSwap event without a pool account.
  `tests/exit_sweeper.rs` (5): stop-loss sell on the curve, kill-switch
  flatten without marking, partial take-profit trim, unpriceable position
  held then force-exited with retry backoff, other venues marked and
  routed by venue. `tests/concurrency.rs` (4): the same launch seen by
  many tasks opens exactly one position, many distinct launches in
  parallel each get one position and one intent, entries racing the exit
  sweeper without a double sell, concurrent sweeps over one position sell
  once. `tests/replay.rs` (5 + 1 ignored generator; 19 fixtures incl. the
  pinned-`[execution]` fee-budget pair): checked-in fixtures
  byte-identical to the generator, every fixture reaches its recorded
  verdict (approved steps carry a fee estimate), replay is deterministic
  and never touches the ledger, AMM fixtures pick the direct venues, JSON
  round-trip. `tests/property.rs`
  (12): randomised invariants over event id, dedup key, staleness
  threshold, sequence tracker, stage machine, gate ordering / thresholds,
  slippage hard max and price-impact bounds.
* **copy engine (module-copy, TASK 3; 51 unit + 3 feed/websocket + 35
  pipeline integration + 1 gated):** unit tests in every new module
  (event identity/validation/stage vocabulary, dedup namespaces, ordering
  cursors and gaps, policy precedence, sizing matrix, entry intent ids and
  hardened exit intent ids, leader registry, reconcile findings, recovery
  planner, in-memory journal, metrics timeline, audit sanitising);
  `tests/leader_lifecycle.rs` (4), `tests/event_pipeline.rs` (7 — incl.
  the live-mode terminal-state flow that reaches `FILLED`, `AMBIGUOUS`,
  `EXIT_MIRRORED`, `REJECTED` and `FAILED` against the mock node and counts
  all 15 stages), `tests/dedup_ordering.rs` (7), `tests/policy_sizing.rs`
  (4), `tests/intent_execution.rs` (6), `tests/reconciliation.rs` (6),
  `tests/crash_recovery.rs` (3), `tests/concurrency.rs` (3) — all offline
  against the mock node (`tests/common/mod.rs` includes
  `module-sniper/tests/common/mod.rs` by path); the pre-existing
  `copy_feed` (1) and `geyser_feed` (2) websocket tests keep their
  behaviour and additionally prove the single-mark dedup contract (a
  feed-delivered trade is fresh for the pipeline's `copy_event` mark
  exactly once). Full map: `docs/COPY-TRADING-ENGINE.md` §13.
* **polymarket (94 unit + 53 integration, TASK 4):** unit tests for
  strategy verdicts and gates, intent identity, the `LocalOrderState`
  machine and cumulative fill accounting (incl. FAK `matched` semantics and
  per-trade deltas reconciled against polled cumulatives), the EOA
  funder/signer guard, user-channel message parsing, EIP-712 v2 vectors and
  clients; integration suites against an in-process axum mock of the CLOB /
  Gamma / Polygon RPC / user websocket (`tests/common/mod.rs`):
  `order_pipeline` (10), `order_lifecycle` (11),
  `user_ws` (5, incl. a real websocket server), `idempotency_concurrency`
  (4), `reconciliation` (6), `crash_recovery` (7), `strategy_sizing` (5)
  and the pre-existing `mock_clob_gamma` (5). Full map:
  `docs/POLYMARKET-ENGINE.md` §13.
* **global risk + accounting (TASK 5; 40 bot-core unit + 19 offline
  integration in `crates/core/tests/global_risk_accounting.rs`, plus 1 gated
  PostgreSQL round-trip in `db_integration.rs` and module assertions in
  `module-sniper/tests/concurrency.rs`, `module-copy/tests/dedup_ordering.rs`,
  `module-polymarket/tests/order_pipeline.rs`):** unit tests for the
  deterministic event identity and validation, balanced double-entry
  postings per event kind, average-cost aggregation (equal to
  `reconstruct_pnl`), fees, exposure / unrealized at the mark, exactly-once
  booking under 16 concurrent submitters, pending / flush when the journal
  is unavailable, journal-held duplicates, the realized series (day totals
  and peak), portfolio slices and missing reference rates, every
  reconciliation finding kind with stable ids (incl. the order layer: a
  filled OMS order with no ledger event is reported, intent-only order
  states are silent, an event explained only by its order is not an orphan),
  replay-safe recovery with gaps reported, kill-switch scopes, config
  validation; integration through
  the real `RiskEngine::check_entry`: portfolio / wallet / venue / strategy /
  asset / open-position / order-notional limits, fail-closed reference
  rates, daily loss and drawdown read from the ledger, global + venue +
  strategy kill switches (config-pinned and runtime, restored from the
  journal), **the same financial event submitted twice → exactly one ledger
  mutation, one position mutation, one PnL effect**, duplicate fee and
  settlement events, 32 concurrent identical events, cross-module
  aggregation with PnL and fees, ledger ↔ position reconciliation
  (quantity mismatch, missing entry, orphan event; nothing repaired),
  order-layer reconciliation against a real `OrderManager` (filled order
  without a ledger event reported, booked and never-executed orders
  silent, the finding clearing once the fill is booked),
  restart recovery rebuilding the daily-loss state and refusing replays,
  whole-journal replay idempotence, config reload, wallet / strategy
  attribution. Full map: `docs/ACCOUNTING-LEDGER.md` §11,
  `docs/GLOBAL-RISK.md` §7.
* **HA / crash recovery / distributed reliability (TASK 6; 37 bot-core unit
  + 18 offline integration in `crates/core/tests/ha_distributed.rs`, plus 4
  config tests and 1 server probe test):** unit tests for the worker state
  machine (every legal and illegal transition, readiness classification,
  staleness as a pure clock function), lease semantics (role keys, liveness,
  strictly increasing fencing generations, renewal cadence, typed fence
  errors), cursor arithmetic (advance / duplicate / gap, opaque tokens,
  rewind, lag), the complete crash-boundary × order-recovery matrix (12
  boundaries, 5 local × 7 venue evidence pairs, every pair deterministic and
  no action that resubmits) and the memory store (generation increments,
  single holder, expiry takeover, fenced release, store failure);
  integration against real `AppState` workers sharing ONE durable store:
  worker registration and generations, heartbeat expiry with stale detection
  that takes nothing over, lease acquire / renew / loss / takeover with the
  old owner fenced on every path, the two-worker race, fail-closed store
  errors, **the critical proof — the same execution event reaching two
  workers yields exactly one execution, one order intent and one ledger
  effect**, duplicate event / order intent / ledger event suppression, feed
  cursor recovery with gap detection and deliberate replay, every crash
  boundary resolving to one action, ledger / position / risk-state restart
  without double booking, partial-fill restart, the recovery journal,
  failover reconciliation reporting (never repairing), single and
  active/passive modes, readiness failure on lost lease / pending recovery /
  unhealthy dependency / `RECOVERY_REQUIRED`, and graceful shutdown
  (cursors persisted, leases released, standby takes over immediately), and
  the feed wiring both money-bearing feeds use (`feed_wiring_shapes_survive_a_restart`:
  the copy poll loop's opaque per-wallet cursor resumes from its durable
  token after a restart and its scopes stay independent; the Polymarket
  user channel continues its delivery numbering across lives and turns a
  skipped delivery into a recorded gap).
  Full map: `docs/CRASH-RECOVERY.md` §6, `docs/HA-ARCHITECTURE.md` §11.
* **other modules (copy legacy, telegram 21):** copy
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
* **server (37):** the worker-readiness health component (a worker that has
  not registered / recovered is never READY, TASK 6 §11); every REST route's
  RBAC matrix (incl. the TASK 5
  accounting / global-risk routes: operator-only event booking and
  kill-switch scopes, duplicate reference → `duplicate`, fills refused over
  the API, audited engage / release), input validation before
  attachment checks, degradation contracts (`available:false`), audit
  verify, journal routes, loopback-bind rules, **startup-gate module
  blocking (kind→module mapping incl. the new `intent` kind, fail-safe on
  unknown kinds)**.
* **solana-kit (253 lib + gated suites):** RPC retry/failover/commitment
  semantics, executor flow incl. **broadcast-failure classification with
  mock endpoints (transport black hole → `SendUnknown` with signature;
  definite rejection → `SendFailed`)**, fee policy (`decide`, escalation,
  adaptive cap, emergency refusal, and the side-effect-free
  `would_refuse` / `max_payable` predicates agreeing with `decide`), warm
  account cache, WS/pump parsing. Gated: `devnet_e2e` (4), `latency_bench` (5),
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
