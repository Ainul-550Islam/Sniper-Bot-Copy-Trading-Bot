# Security / trust-boundary trace (final buyer handover)

Buyer-readable map of the complete control-and-money path. For every
boundary the eight required questions are answered strictly from the
implementation (file references given; each backed by executed tests inside
the recorded 537/537 + 71/71 + 3/3 evidence runs). No guarantee is claimed
beyond what the code does.

Path: USER → TELEGRAM → API → AUTHORIZATION → RISK → ORDER INTENT →
OWNERSHIP CLAIM → EXECUTION → EXTERNAL NETWORK → CONFIRMATION → DATABASE →
RECONCILIATION → RECOVERY

Legend: ✓ = implemented + test-executed; ⚠ = depends on buyer configuration;
✗ = not present (stated plainly).

## 1. USER → TELEGRAM (`crates/module-telegram/`)

* **Authenticated?** ✓ Telegram chat id against a configured allowlist;
  deny-by-default for unknown chats (`api.rs`, `commands.rs`).
* **Authorized?** ✓ role model — mutating commands require owner role;
  live-mode commands owner-only.
* **Idempotent / replay-resistant?** ✓ commands act through the same
  state/risk/OMS path as the API (idempotency lives there); Telegram is a
  control surface, not a separate money path.
* **Persisted?** ✓ control actions that change runtime state go through
  runtime flags/config in Postgres (`0010_runtime_flags.sql`); chat traffic
  itself is not persisted (by design).
* **Failure-safe?** ✓ module disabled without a bot token; bot token
  redacted from every error string (`error_strings_never_contain_the_bot_token`).
* **Observable?** ✓ structured logs (token-redacted) + audit trail for state
  mutations. **Recoverable?** ✓ state lives in PG, not in the module.
* ⚠ The bot token + owner chat id are buyer-supplied secrets (`.env.template`).

## 2. USER/OPERATOR → HTTP API (`crates/server/src/api.rs`, `ws.rs`)

* **Authenticated?** ✓ `x-api-key` against configured keys (`crates/core/src/auth.rs`);
  unauthenticated requests rejected; loopback-only bind by default
  (compose/Dockerfile).
* **Authorized?** ✓ role per key (owner/admin); live-mode toggles owner-only.
* **Idempotent?** ✓ mutating endpoints route into OMS/state transitions with
  idempotency keys; audit-verify endpoint is read-only.
* **Replay-resistant?** ✓ no bearer-token reuse window beyond the static API
  keys — rotation is an operator procedure (see `docs/FINAL-OPERATIONS-HANDOVER.md`);
  WS feed requires the same key.
* **Persisted / observable?** ✓ every state mutation audit-logged (hash
  chain); `bot_http_*` metrics per route. **Failure-safe?** ✓ HTTP layer
  drains on shutdown after modules stop (phase8b shutdown log).
* ⚠ API key strength + TLS termination are buyer deployment choices.

## 3. API/TELEGRAM → AUTHORIZATION → RISK (`crates/core/src/risk.rs`)

* **Authenticated/Authorized?** ✓ only via boundaries 1–2.
* **Idempotent?** ✓ risk checks are pure reads of config/state — no side
  effects to replay.
* **Failure-safe?** ✓ risk decision `!allowed()` blocks the entry (fail-closed);
  GlobalRiskOracle is tighten-only across replicas (distributed_integration 4/4).
* **Persisted?** ✓ risk config in PG; decisions logged.
* **Observable?** ✓ decision reasons in logs (`d.reason`), rejection metrics.
* **Recoverable?** ✓ config reloaded at startup from PG.

## 4. RISK → ORDER INTENT (`crates/core/src/oms.rs`)

* **Idempotent?** ✓ OMS idempotency keys are Postgres-backed — the
  money-critical dedup layer (documented in `docs/RECONCILIATION.md`; a
  restart or replica race cannot double-create the same intent).
* **Replay-resistant?** ✓ duplicate intent inserts are rejected by the
  idempotency key, not by Redis TTLs.
* **Persisted?** ✓ intent + state machine (`Unknown`/`Reconciled` states) in
  PG (`0002_orders_executions.sql`, `0007_intent_journal.sql`).
* **Failure-safe?** ✓ an intent whose execution outcome is unknown is parked
  as `Unknown` — never assumed filled or failed.
* **Observable?** ✓ audit chain + journal. **Recoverable?** ✓ startup
  recovery re-registers unfinished orders as `Unknown` (boundary 13).

## 5. INTENT → OWNERSHIP CLAIM (`crates/core/src/{ownership,redis_ownership}.rs`, `db/claims.rs`)

* **Authorized?** ✓ only the claim holder (lease + epoch + fencing token)
  may execute an intent; two_replica_mirror 1/1 executed with two real
  processes.
* **Idempotent / replay-resistant?** ✓ fencing tokens invalidate stale
  claimants after restarts/failover; PG is the authoritative claim store,
  Redis only accelerates (never solely holds financial state — master rule).
* **Persisted?** ✓ claims/leases/events in PG (`0009_execution_claims.sql`,
  `0011_execution_claim_events.sql`).
* **Failure-safe?** ✓ expired leases release; kill-switch flag syncs across
  replicas. **Observable?** ✓ claim-event lineage table.
* **Recoverable?** ✓ recovery re-claims or releases per lease state.

## 6. CLAIM → EXECUTION (`crates/solana-kit/src/execute.rs`, `tx.rs`, `signer.rs`)

* **Authenticated?** ✓ signer registry holds only local keypairs; vault/kms/
  hsm backends FAIL startup by design (typed rejection tests).
* **Idempotent?** ✓ simulate-first: a transaction that fails simulation is
  never broadcast (`SimulationFailed`, broadcast_attempts = 0).
* **Replay-resistant?** ✓ fresh blockhash per construction; Solana's own
  recent-blockhash window prevents replay of signed wire bytes.
* **Failure-safe?** ✓ `may_broadcast` hard gate: even `mode = Live` cannot
  broadcast unless `allow_live_trading` is true; policy downgrades to
  Simulate otherwise (executed preflight rejections in tests; paper default
  observed live in phase8b `/api/status`).
* **Persisted?** ✓ execution results + signatures journalled (JSONL,
  corrupt-line recovery tested) and written to PG.
* **Observable?** ✓ `bot_*` execution metrics + logs (never log secrets).

## 7. EXECUTION → EXTERNAL NETWORK (RPC / WS / CLOB / Telegram API)

* **Authenticated?** ⚠ outbound credentials are buyer-supplied (RPC keys,
  CLOB L1/L2, bot token); stored in env/config, never in the repo
  (secret-scan gate 0 hits).
* **Failure-safe?** ✓ RPC failover rotation (`rpc.rs`); broadcast errors
  classified terminal vs outcome-unknown (`SendFailure`) — unknown outcomes
  go to reconciliation, never assumed failed.
* **Replay/trust?** ✓ RPC responses are treated as untrusted input: decode
  errors, implausible decimals, and stale reads (>15 s Polymarket collateral
  snapshot bound) reject instead of trading (`rpc_errors_are_errors_never_zero`).
* **Observable?** ✓ per-endpoint latency/error metrics; benchmark harness
  recorded real devnet legs.

## 8. NETWORK → CONFIRMATION (`execute.rs`, `decode.rs`)

* **Idempotent?** ✓ confirmation polls by signature; a confirmed signature
  is decoded once into facts; duplicate confirmations are no-ops.
* **Failure-safe?** ✓ timeout → `SendUnknown` → reconciliation queue
  (never silently dropped, never double-spent).
* **Persisted?** ✓ signature + status in PG transaction attribution
  (`0006_transaction_attribution.sql`).
* **Observable / recoverable?** ✓ recon queue depth metrics; startup sweep.

## 9. → DATABASE (PostgreSQL)

* **Authenticated?** ⚠ connection string is operator-managed; app uses one
  PG URL (trust/localhost in dev evidence; TLS/roles = buyer deployment).
* **Idempotent/replay-resistant?** ✓ migrations forward-only + checksummed
  (sqlx `_sqlx_migrations`, 11/11 verified on restored DB); audit rows are
  insert-only — no API path mutates them (master rule, chain tests).
* **Persisted?** ✓ this IS the durable layer: orders, positions, claims,
  audit chain, flags. Redis holds no sole-copy financial state.
* **Failure-safe?** ✓ PG outage → modules fail their DB ops loudly; no
  in-memory silent continuation of money state (degradation matrix in
  `docs/OPERATIONS.md`). **Recoverable?** ✓ backup/restore round-trip
  EXECUTED (dump `5989ecf1…`, 23/23 suite on restored DB, app started on it).

## 10. → RECONCILIATION (`crates/core/src/reconciliation.rs`, `crates/server/src/recon.rs`)

* **Idempotent?** ✓ sweep is a converging state machine (Unknown →
  Reconciled/WrittenOff via on-chain truth); re-running a sweep is safe.
* **Failure-safe?** ✓ mismatch never auto-mutates balances — it surfaces
  queue items for operator/automation review.
* **Persisted?** ✓ recon state in PG (`0005_reconciliation.sql`).
* **Observable?** ✓ queue depth + age metrics; operator runbook
  (`docs/OPERATIONS.md` §"did my order actually land?").
* **Recoverable?** ✓ crash-mid-recon covered by `recon_crash_e2e`
  (historical execution, pre-hardening source — labeled) + startup
  reconcile gate (current source, db_integration).

## 11. → RECOVERY (`crates/core/src/recovery.rs`)

* **Idempotent?** ✓ recovery is read-then-register; repeated restarts
  converge (journal replay dedups via idempotency keys).
* **Failure-safe?** ✓ module spawn is GATED on the startup reconcile pass
  (`RECOVERY_STARTUP_RECONCILE_SECS`) — trading modules do not start while
  unresolved transactions exist.
* **Persisted/observable?** ✓ journal (JSONL) + recovery logs + metrics.
* **Recoverable?** ✓ this IS the recovery layer; kill -9 → restart path
  documented in acceptance step 17 and exercised by recovery tests.

## 12. ON-CHAIN STAKING PROGRAM boundary (`programs/staking-suite/`)

* **Authenticated?** ✓ every instruction checks its authority (admin ops
  two-step; user ops signer-owned stake accounts) — e2e authority test.
* **Authorized/idempotent?** ✓ genesis latched (replay → `GenesisAlreadyDone`
  6026); metadata one-shot (replay rejected — executed vs real mpl program).
* **Failure-safe?** ✓ all arithmetic checked (`overflow-checks = true` +
  explicit `checked_*`; custom `Overflow` 6024-era errors); supply cap
  IMMUTABLE after genesis; bounds: fee ≤10%, reward ≤100% APR, timelock ≤30d.
* **Persisted/recoverable?** ✓ on-chain state is the ledger; upgrade path =
  upgradeable loader under the buyer's upgrade authority (deployment =
  HUMAN ACTION; no vendor keypair exists).

## What is NOT claimed

* No external penetration test or third-party audit (none exists).
* No DDoS protection beyond axum/limit configuration — rate limits exist in
  code (limiter tests) but capacity numbers are environment-dependent.
* No encryption-at-rest guarantees — that is the buyer's PG/disk layer.
* Telegram transport security is Telegram's (MTProto); the app trusts only
  allowlisted chat ids delivered by the bot API with the buyer's token.
