# Execution reliability layer

_Status: shipped 2026-09-20 (TASK 1 — "harden the execution engine")._

This document describes how a transaction intent travels from a module
decision to a reconciled on-chain fact, and which guarantees hold at each
step when the network misbehaves. It is the operator- and reviewer-facing
companion to the code in `crates/solana-kit/src/{provider,rpc,fees,execute,
ws}.rs`, `crates/core/src/execution.rs`, `crates/core/src/db/execution.rs`
and `crates/server/src/{persist,recon,api}.rs`.

Everything below is integrated into the pre-existing `Executor`, `Rpc`,
`SolanaWs`, OMS status vocabulary, audit trail, reconciliation worker and
Postgres schema. There is no second execution path; modules that do not pin
an intent id still get a deterministic one derived by the executor.

---

## 1. RPC provider pool (`solana-kit/src/provider.rs`)

`rpc_url` + `rpc_url_fallbacks` form an ordered **provider pool**. Every RPC
call goes through it.

| Concern | Behaviour |
|---|---|
| Health | Per-provider consecutive-failure counter. After `provider_failure_threshold` (default 3) consecutive failures the provider is **tripped** and skipped for `provider_cooldown_ms` (default 5 s), then re-probed with one request. |
| Failover | A tripped or rate-limited provider is skipped; `Rpc::failover()` rotates explicitly (used by the broadcast fan-out). |
| Retry policy | `RetryPolicy::from_network(&NetworkConfig)`: `max_retries`, exponential backoff from `retry_base_backoff_ms` to `retry_max_backoff_ms`, **full jitter** when `retry_jitter = true` (µs granularity, so replicas never retry in lock-step). |
| Classification | `RpcErrorClass`: `Timeout`, `RateLimited{retry_after}`, `Transport`, `Unavailable`, `Blockhash`, `Permanent`. `retryable()` is true for the first four; `Blockhash` requires a **rebuild** (`needs_rebuild()`), `Permanent` is never retried. |
| Rate limits | HTTP 429 → provider cooled down for `Retry-After` when the header is present, else `rate_limit_cooldown_ms`. |
| Timeouts | `request_timeout_ms` applies to **every** attempt (`MeteredSender`), not only to the whole call. |

Metrics: `bot_rpc_requests_total{method,outcome}`,
`bot_rpc_provider_requests_total{provider,outcome}`,
`bot_rpc_errors_total{provider,class}`, `bot_rpc_retries_total`,
`bot_rpc_failover_total`, `bot_rpc_rate_limited_total`,
`bot_rpc_provider_healthy{provider}` (gauge), `bot_rpc_provider_tripped_total`,
`bot_rpc_consecutive_failures{provider}`, `bot_rpc_attempt_duration_ms{method}`.

## 2. WebSocket resilience (`solana-kit/src/ws.rs`)

`WsPolicy::from_network(&NetworkConfig)` gives every feed the same policy:

* **Reconnect with backoff** — exponential, jittered, capped; the ladder is
  reset once a connection has been healthy for 60 s, so a flapping endpoint
  does not creep back to zero delay.
* **Subscription restoration** — every logical subscription is re-issued
  after a reconnect (`bot_ws_subscriptions_restored_total`).
* **Stale detection** — no inbound frame (including pong) for
  `ws_stale_after_ms` (default 45 s, `0` = off) → `WsStatus::Stale`, the
  socket is dropped and reconnected (`bot_ws_stale_connections_total`).
* **Missed-event recovery** — before the restored subscription reports
  `Connected`, consumers receive `WsMessage::Gap{subscription, last_slot,
  outage_ms}`. The copy-trading feed uses it to **backfill** with
  `getSignaturesForAddress` bounded by the outage's block time
  (`bot_ws_backfilled_events_total{feed}`); the sniper feed logs the gap and
  records an error metric (a launch that happened during the outage is by
  definition no longer a fresh launch, so it is intentionally not replayed).

Metrics: `bot_ws_reconnects_total`, `bot_ws_connection_failures_total`,
`bot_ws_reconnect_delay_ms`, `bot_ws_outage_ms`, plus the two above.

## 3. Transaction lifecycle (`solana-kit/src/execute.rs`)

```
begin ─► created ─► validated ─► submitted ─► pending ─► confirmed
            │           │  (paper) └──────────┐   │          ▲
            │           └─► confirmed          │   ├─► failed  │ (landed, program error)
            │  fee veto / build / simulation   │   └─► expired (blockhash died; rebuild ok)
            └─► failed / expired ◄─────────────┘  definite node rejection
                                    any settled state ─► reconciled
```

Per attempt, `Executor::run` performs, in order:

1. **Intent id** — caller-pinned (`TxRequest::with_intent_id`) or derived
   from `[wallet, label, instruction digest]`. Ids are `int_<sha256 prefix>`
   and deterministic, so the same decision always yields the same id.
   Module conventions:
   * sniper buy: `["sniper", mint, "buy", route, launch signature|slot|observed_at]`
   * copy buy: `["copy", source signature, source wallet, "buy", route, mint]`
   * exits: `[module, position id, "sell", route, sell amount, qty]`
2. **Duplicate protection** — `ExecutionLedger::begin`. A second run for an
   intent that is live (`created…pending`) or already landed (`confirmed`)
   is refused with `ExecStatus::Skipped` + `FailureClass::Duplicate`; the
   result carries the live attempt's signature. A run after a *definite*
   failure (`failed`, `expired`) re-arms the intent and bumps `attempts`.
3. **Fee decision** — see §4. A refusal is a `PolicyVeto` **before any
   network I/O**.
4. **Blockhash freshness** — `Rpc::fresh_blockhash(max_blockhash_age)`
   re-fetches a cached blockhash older than `max_blockhash_age_ms`; the
   `last_valid_block_height` travels with the built transaction.
5. **Simulation / preflight** — when `simulate_first` is on, a program
   error aborts the attempt (`SimulationRejected`, nothing is broadcast);
   an RPC failure of the simulate call itself is logged and the attempt
   continues (the node's own preflight still runs).
6. **Write-ahead `submitted`** — signature, blockhash and
   `last_valid_block_height` are attached to the ledger record and (via
   the sink) persisted **before** `sendTransaction` is called.
7. **Classified send** — `Rpc::send_transaction_classified`. Definite
   rejections → `failed` (`Rejected`, `InsufficientFunds`) or `expired`
   (`BlockhashExpired`); transport failures are **ambiguous**
   (`TransportAmbiguous`) and park the intent in `pending` — never a blind
   retry, because the transaction may still land.
8. **Confirmation tracking** — `Rpc::confirm_tracked(sig,
   last_valid_block_height, timeout, poll)`. Outcomes: `Confirmed` →
   `confirmed`; `Failed` → `failed` (`LandedFailed`); `Expired` (block
   height passed the validity window with no trace) → `expired`;
   `Timeout` (blockhash still valid, nothing seen) → stays `pending`
   (`ConfirmationTimeout`) and reconciliation owns it.
9. **Rebuild loop** — only *definite, rebuildable* failures (`expired`,
   rate-limit) trigger a rebuild with a fresh blockhash and an escalated
   fee, bounded by `max_retries` (`bot_execution_rebuilds_total`).

`send_prebuilt` runs the same lifecycle for externally built transactions
(Jupiter routes) — broadcast is RPC-only when there are no local
instructions.

Every non-success carries a `FailureClass`: `simulation_rejected`,
`blockhash_expired`, `rejected`, `insufficient_funds`, `rate_limited`,
`transport_ambiguous`, `confirmation_timeout`, `landed_failed`,
`policy_veto`, `duplicate`, `internal`.

## 4. Priority-fee policy (`solana-kit/src/fees.rs`)

`FeePolicy::decide(requested, oracle quote, attempt)`:

| Setting | Meaning |
|---|---|
| `fee_mode = "fixed"` | pay `priority_fee_micro_lamports` (the existing key). |
| `fee_mode = "adaptive"` | pay the `fee_percentile` of `getRecentPrioritizationFees`, floored at `priority_fee_micro_lamports`. Samples are cached for `fee_oracle_ttl_ms`. |
| `fee_min_micro_lamports` / `fee_max_micro_lamports` | clamp range for every decision, including escalations (`bot_priority_fee_clamped_total`). |
| `fee_escalation_pct` | +N % per rebuild attempt (capped by `fee_max`). |
| `fee_emergency_max_micro_lamports` | a request or a clamped result above this is **refused, not clamped** (`bot_priority_fee_refused_total`, `PolicyVeto`). |

Metrics: `bot_priority_fee_decisions_total{source=requested|adaptive|escalated}`,
`bot_priority_fee_last_micro_lamports`,
`bot_priority_fee_oracle_quote_micro_lamports`,
`bot_priority_fee_oracle_samples_total`.

## 5. State machine (`core/src/execution.rs`)

`ExecutionState::can_transition_to` is the single source of truth:

```
created   → validated | failed | expired
validated → submitted | confirmed (paper) | failed | expired
submitted → pending | confirmed | failed | expired | reconciled
pending   → confirmed | failed | expired | reconciled
confirmed | failed | expired → reconciled
```

`to_order_status()` projects each state onto the OMS `OrderStatus`, so
orders and executions never disagree about what a state means. Illegal
transitions are rejected with `BotError::Invalid` and logged.

## 6. Persistence and crash recovery

* Migration `0012_execution_lifecycle.sql`: `execution_lifecycle` (one row
  per intent, upserted on every transition) and the append-only
  `execution_lifecycle_events` (one row per transition).
* `ExecutionLedgerSink` (server) is attached right after the audit trail is
  built; a DB failure never blocks the trade path
  (`bot_execution_persist_failures_total{op}`).
* On startup `restore_execution_ledger` hydrates open + the 2 000 most
  recent rows and runs `resolve_after_restart`:

  | State at crash | Resolution |
  |---|---|
  | `created`, `validated` | → `failed` (`internal`, "process restarted before submission"). Nothing left the process. |
  | `submitted`, `pending` | kept **blocked** (duplicate protection stays in force); the signature is enqueued for the `transaction` truth source (`bot_execution_restart_recovered_total{disposition=ambiguous}`). |
  | settled states | informational; evicted from memory after the bounded window. |

* The reconciliation worker closes the loop: when the `transaction` truth
  source resolves a signature (confirmed / failed / expired on chain) it
  calls `ledger().reconcile_signature`, moving the intent to `reconciled`
  and releasing its duplicate-protection slot, in the same step that
  finishes the OMS order.

## 7. Observability

* Metrics: `bot_execution_transitions_total{from,to}`,
  `bot_execution_failures_total{class}`, `bot_execution_attempts_total`,
  `bot_execution_rebuilds_total`, `bot_execution_state_duration_ms{state}`,
  `bot_execution_stage_ms{stage=build|simulate|send|confirm}`,
  `bot_execution_active` (gauge), `bot_execution_restart_recovered_total`,
  `bot_execution_persist_failures_total`.
* Audit trail: actor `executor`, action `execution.<state>` for
  `submitted`, `confirmed`, `failed`, `expired`, `reconciled`; the JSON
  detail carries intent id, attempt, signature, failure class and reason.
* Structured logs (`tracing`) on every transition with `intent`, `from`,
  `to`, `attempt`, `signature`.
* API: `GET /api/executions` (`?limit=`, `?state=`, `?open=true`) and
  `GET /api/executions/:id` (intent id or signature, DB fallback with the
  event history). See `docs/API.md`.

## 8. Failure-injection coverage

All tests are offline (mock HTTP / WS nodes) and run in CI with
`cargo test --workspace -- --test-threads=1`.

| Scenario | Test |
|---|---|
| RPC timeout | `solana_kit::rpc::tests::timeout_is_classified_and_retried_then_exhausted` |
| RPC failure + failover | `rpc::tests::failing_primary_fails_over_to_the_fallback_within_one_call`, `provider::tests::*` (breaker, cooldown, re-probe) |
| Permanent error not retried | `rpc::tests::permanent_errors_are_not_retried` |
| Rate limit | `rpc::tests::rate_limited_primary_is_cooled_down_and_skipped` |
| Transient node error retried | `rpc::tests::transient_node_error_is_retried_on_the_same_provider_and_recovers` |
| WS disconnect / stale / restore | `ws::tests::stale_connection_is_detected_and_resubscribed_after_reconnect`, `ws::tests::restored_subscription_gets_a_gap_before_connected`, `ws::tests::backoff_policy_is_exponential_capped_and_jittered` |
| Stale blockhash | `execute::tests::stale_blockhash_is_detected_and_rebuilt_with_a_fresh_one`, `execute::tests::definite_failure_is_rearmed_and_retried_with_escalated_fee`, `rpc::tests::confirm_tracked_reports_expiry_instead_of_burning_the_timeout` |
| Failed simulation | `execute::tests::failed_simulation_aborts_before_any_broadcast` |
| Submission failure | `execute::tests::definite_rejection_yields_send_failed`, `execute::tests::transport_failure_yields_send_unknown_not_send_failed` |
| Confirmation timeout | `execute::tests::confirmation_timeout_parks_the_intent_in_pending` |
| Process restart | `bot_core::execution::tests::restart_resolution_fails_unsent_and_hands_off_ambiguous`, `solana-kit/tests/recon_crash_e2e.rs` |
| Duplicate retry | `execute::tests::duplicate_run_is_refused_while_the_first_attempt_is_pending`, `execution::tests::duplicate_attempts_are_refused_while_live_or_landed` |
| Fee emergency veto | `execute::tests::fee_above_emergency_limit_is_vetoed_before_any_network_io`, `fees::tests::*` |

## 9. Configuration reference

```toml
[network]
max_retries = 3
request_timeout_ms = 10000
retry_base_backoff_ms = 50
retry_max_backoff_ms = 2000
retry_jitter = true
rate_limit_cooldown_ms = 1000
provider_failure_threshold = 3
provider_cooldown_ms = 5000
ws_stale_after_ms = 45000

[execution]
priority_fee_micro_lamports = 0      # existing key: fixed fee / adaptive floor
fee_mode = "fixed"                   # "fixed" | "adaptive"
fee_min_micro_lamports = 0
fee_max_micro_lamports = 5000000
fee_emergency_max_micro_lamports = 20000000
fee_percentile = 75
fee_escalation_pct = 50
fee_oracle_ttl_ms = 2000
max_blockhash_age_ms = 20000
```

Every key is optional; the defaults reproduce the pre-existing behaviour
(fixed fee, three retries, no emergency refusal below 20 000 000 µ-lamports).
Environment overrides: `RPC_RETRY_BASE_BACKOFF_MS`, `RPC_RETRY_MAX_BACKOFF_MS`,
`RPC_RETRY_JITTER`, `RPC_RATE_LIMIT_COOLDOWN_MS`,
`RPC_PROVIDER_FAILURE_THRESHOLD`, `RPC_PROVIDER_COOLDOWN_MS`,
`WS_STALE_AFTER_MS`, `FEE_MODE`, `FEE_MIN_MICRO_LAMPORTS`,
`FEE_MAX_MICRO_LAMPORTS`, `FEE_EMERGENCY_MAX_MICRO_LAMPORTS`,
`FEE_PERCENTILE`, `FEE_ESCALATION_PCT`, `MAX_BLOCKHASH_AGE_MS`.

## 10. Known boundaries

* Ambiguous outcomes are resolved by reconciliation, never by a blind
  resend. Operators who need faster resolution should shorten the
  reconciliation worker's interval, not `max_retries`.
* The sniper feed does not replay launches missed during a WS outage (a
  stale launch is not a valid snipe). The copy feed's backfill is bounded
  by `getSignaturesForAddress` pagination (one page per gap).
* The adaptive fee oracle uses the cluster-wide `getRecentPrioritizationFees`
  sample; account-scoped fee estimation is a possible follow-up.
