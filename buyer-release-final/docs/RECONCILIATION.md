# Reconciliation & Crash Recovery

How this suite keeps its books identical to reality: what the source of truth
is for every piece of financial state, how ambiguous executions are resolved,
what happens at every crash point, and which corrections are automated versus
reserved for operators.

Code map:

| Layer | File | Role |
|---|---|---|
| Comparison engine (pure) | `crates/core/src/reconciliation.rs` | typed EXPECTED-vs-OBSERVED verdicts, PnL reconstruction — no I/O |
| Queue mechanics | `crates/core/src/recovery.rs` | claim/retry/park worker, startup gate, sweeper |
| Venue adapters | `crates/server/src/recon.rs` | Solana tx, Solana position, Polymarket order truth sources |
| Order state machine | `crates/core/src/oms.rs` | `Unknown`/`Reconciled` states, idempotency keys |
| Persistence hooks | `crates/server/src/persist.rs` | claims at submit/fill time, restart restore |
| Executor ambiguity | `crates/solana-kit/src/execute.rs` | `Sent` / `SendUnknown` vs definite `SendFailed` |
| Balance reads | `crates/solana-kit/src/tokens.rs` | aggregated, identity-validated SPL balance reader |
| Startup wiring | `crates/server/src/main.rs` | gate → block → spawn ordering |

---

## 1. Source-of-truth model (authoritative boundaries)

Five state classes, one direction of authority:

1. **Local intent** — `Order` (OMS): what we *wanted* to do. Never evidence
   of money movement. Idempotency-keyed; duplicates collapse.
2. **Execution attempt** — `transactions` rows + `ExecutionRecord` trails:
   what we *sent* (signature/order-id, signer, venue, side, qty via the
   order, timestamp, slot, broadcast attempts, confirm state).
3. **External observation** — what the venue/chain *actually* shows. The
   **only** authority for whether money moved.
4. **Reconciled state** — positions/orders after a typed engine verdict has
   been applied (`Position`, `OrderStatus::Reconciled/Filled/Failed/...`).
5. **Derived state** — PnL, risk exposure, balances cache. Always
   *recomputed* from reconciled data (`reconstruct_pnl`), never trusted from
   a previous process lifetime.

**Postgres (+ the JSONL journal) is the durable system of record for local
state; the chain/venue is the system of record for money.** Redis is a
cache/dedup accelerator only — no financial state lives solely in Redis (§N):
after a full Redis flush or a stale-cache restart, Postgres + external truth
are sufficient to converge. Dedup degrades to its in-memory `BoundedSet`
semantics (identical behaviour, process lifetime) when Redis is absent; a
Redis outage can therefore weaken *duplicate-event suppression* across
restarts, which is why the OMS idempotency keys (Postgres-backed) are the
money-critical dedup layer, not Redis.

## 2. Transaction lifecycle & claim queue

Every money-moving attempt produces a **claim** in `reconciliation_state`
(kind + subject, `FOR UPDATE SKIP LOCKED` queue):

* Solana orders: `OrderSent` event → `transactions` row (`submitted`, with
  signer/venue/attempts attribution) + claim `transaction:<signature>`.
* Solana exits publish `Fill` directly → the persistence pump claims the
  exit signature too (idempotent insert; overlap with the entry claim is a
  no-op).
* Polymarket: `Fill` (matched) → claim `polymarket_order:<orderID>`;
  submit-unknown (transport failure during `POST /order`) → the order id is
  **derived locally** (EIP-712 struct hash = the exchange's `getOrderHash`)
  and claimed via an `OrderSent` event, so even a POST that never answered
  is reconciled against the CLOB after restart.
* Positions: periodic re-verification re-arms `position:<id>` claims for
  every open LIVE Solana position (`position_recheck_interval_secs`).
* Sweeper: `sweep_unresolved_transactions` re-enqueues `submitted` rows
  older than 90 s (crash between broadcast and claim, DB write lost, etc.).

The `RecoveryWorker` claims due items, asks the registered `TruthSource`,
and applies the verdict: **Resolved** (truth established), **Retry**
(backoff, up to `max_attempts`), **GiveUp** (parked in `failed` →
`GET /api/recovery/failed`, alerted at park time). Terminal OMS states are
immutable; corrections to non-terminal state always pass through explicit
transitions with reasons (auditable history in `order_status_history`).

### Executor outcomes (ambiguity is a first-class state)

| `ExecStatus` | Meaning | Downstream |
|---|---|---|
| `Confirmed` | landed, no program error | claim resolves fast |
| `Sent` | broadcast OK, confirmation timed out — *may still land* | claim + recon |
| `SendUnknown` | broadcast ended in a transport failure (no answer) — *may still land* | claim + recon (signature retained) |
| `SendFailed` | every endpoint answered with a **definite rejection** (blockhash, sanitize, rate-limit) — cannot have landed | failure path |
| `LandedFailed` | landed, program error | definitive |
| `SimulationFailed` / `Skipped` / `PaperFilled` | never broadcast / paper | no chain claim needed |

Classification is conservative (`classify_send_error`): anything that is not
a clearly node-produced rejection is ambiguous. An ambiguous outcome is
**never retried with a fresh blockhash** (that would be a second, different
signature for the same intent); instead the executor falls through to
on-chain confirmation of the already-signed transaction, and the claim queue
resolves the rest. Retries inside `Executor::run` only fire **before** any
broadcast (build/blockhash errors) or on a definite `blockhash`-rejection,
which provably never forwarded the transaction.

## 3. Typed reconciliation outcomes

`ReconOutcome` (engine) is the single canonical verdict vocabulary; adapters
map it to queue mechanics and to `bot_reconciliation_outcomes_total{kind,
outcome}`:

`InSync`, `ExternalAhead`, `LocalAhead`, `QuantityMismatch`,
`BalanceMismatch`, `MissingPosition`, `UnexpectedPosition`,
`UnknownExecution`, `MissingTransaction`, `DuplicateExecution`,
`StaleLocalState`, `ExternalStateUnavailable`, `RecoveryRequired`.

Execution classification matrix (`classify_execution`):

| local \ external | Succeeded | Failed | Pending | NotFound |
|---|---|---|---|---|
| SubmittedUnconfirmed | InSync → mark filled | InSync → mark failed | UnknownExecution → retry | MissingTransaction → retry/park |
| RecordedConfirmed | InSync | **RecoveryRequired** (park + alert) | StaleLocalState → retry | **RecoveryRequired** |
| RecordedFailed | **RecoveryRequired** (money moved!) | InSync | **RecoveryRequired** | InSync |
| NoLocalRecord | **RecoveryRequired** | UnknownExecution | **RecoveryRequired** | InSync |

Position comparison (`compare_position`) applies, in order: unreadable
source → retry; unresolved execution in flight → inconclusive; closed
locally + external balance > dust → `UnexpectedPosition`; open locally +
external zero → `MissingPosition` **iff** confirmed exit fills exist (else
`RecoveryRequired`); within 1 % relative tolerance or dust floor → `InSync`;
otherwise `ExternalAhead`/`LocalAhead` → drift-flagged.

## 4. The eight ambiguity cases (§F)

1. **RPC timeout, no signature answer** → `SendUnknown` (signature retained,
   claim enqueued, confirmation probed; never "failed").
2. **Signature exists, never confirms** → claim stays `UnknownExecution` →
   retried with backoff → parked for operators after `max_attempts`; the
   order ends in `Unknown`→`Failed` only via the queue's exhaustion path,
   never optimistically.
3. **RPC connection lost after broadcast** → confirmation errors are
   absorbed by the retry/failover chain; the outcome is `Sent`/
   `SendUnknown`, resolved later by the worker (possibly via a different
   provider).
4. **Tx confirmed, crash before DB commit** → signature recovered from the
   journal/`OrderSent` replay (idempotent OMS key) or from the sweep;
   `classify_execution(NoLocalRecord|SubmittedUnconfirmed, Succeeded)`
   converges the order to `Filled`.
5. **DB write of the tx row lost** → sweep + claim; same convergence.
6. **DB says failed, chain says succeeded** → `RecoveryRequired`: parked,
   risk event, alert — never silently flipped, because fill effects may
   have been (incorrectly) skipped and correction needs authority.
7. **Tx landed but program failed** → `LandedFailed` synchronously, or
   `classify → InSync(mark failed)` from the queue; fees were spent and that
   is recorded, no position effects.
8. **Accidental retry after ambiguous outcome** → impossible by
   construction: ambiguous statuses are never retried in-process; the OMS
   idempotency key collapses re-submitted intents onto the existing
   (possibly terminal) order; Polymarket re-signs of the same intent derive
   the same order id (deterministic salt) so the CLOB itself deduplicates.

## 5. Crash points (§I) — where a kill -9 lands and what recovers it

| Crash point | Durable state at crash | Recovery |
|---|---|---|
| A. before signing | intent (maybe) | idempotency key: intent either absent or re-creatable; nothing moved |
| B. after signing, before broadcast | signature derivable from intent? no — dropped | nothing moved; intent re-fires as a NEW signature safely (nothing was sent) |
| C. during broadcast (ambiguous) | **write-ahead intent row** (`execution_intents`, recorded BEFORE broadcast) + journal `OrderSent` if the event pump got that far | orphan-intent sweep → `intent` claim: ambiguous forever until a late `link` lands or an operator resolves; the intent's symbol is entry-gated meanwhile. Never resubmitted. Balance recon remains the second net |
| D. broadcast answered, pre-confirm | `transactions` row `submitted` | claim/sweep → confirm → Filled/Failed |
| E. confirmed, pre-OMS transition | row `submitted` + sig | same as D (truth re-read; idempotent) |
| F. OMS transitioned, pre-position update | order Filled, position missing/stale | position recheck → `MissingPosition`/drift flag → correction or operator park |
| G. position updated, pre-trade record | position ok, fills missing | PnL replay degrades to `RecoveryRequired` (no fabricated proceeds) + alert |
| H. trade recorded, pre-PnL book | fills durable | `reconstruct_pnl` replay is exact |
| I. PnL booked, pre-journal flush | DB durable (journal is a mirror) | restore() from Postgres |
| J. mid-restore | partial memory | restore is idempotent (upserts, `ON CONFLICT DO NOTHING`) |
| K. post-restore, pre-gate | — | startup gate runs before modules; nothing trades unguarded |

No recovery step relies on process memory: everything above is re-derived
from Postgres + journal + external truth.

## 6. Startup sequence (§H)

```
DB connect → migrations → config version check
→ persist::restore (positions; non-terminal orders → Unknown; 90 s tx sweep)
→ RecoveryWorker built (truth sources registered, incl. `intent`)
→ orphan-intent sweep (pending intents from a previous life → `intent` claims)
→ startup_reconcile(window = recovery.startup_reconcile_secs)   ← GATE
→ unresolved claims (pending/in_progress) are attributed PER SYMBOL where
  possible (intent → journaled symbol; position → its symbol; transaction /
  polymarket_order → via the attributed order row):
     attributable   → only that symbol is entry-gated (exits stay allowed)
     unattributable → conservative module-wide fallback:
        transaction/position/balance/intent → Sniper+Copy blocked
        polymarket_order                    → Polymarket blocked
        unknown kinds                       → all trading modules blocked
   (set_blocked_symbols / set_enabled(false) + audit record + Error event;
    the 60 s sampler recomputes the gate — resolving claims unblocks)
→ worker loop spawned (30 s cadence) + 60 s backlog sampler
   + position recheck ticker (position_recheck_interval_secs)
→ modules spawned (intent sink injected when recovery.intent_journal)
→ API/WS/Telegram
```

**Write-ahead intent journal (§I crash point C).** When
`[recovery] intent_journal` is on (default) every Solana broadcast — pump
buy/sell and Jupiter buy/sell in both Sniper and Copy — is wrapped in
`bot_core::recovery::with_intent`: a durable `execution_intents` row is
written BEFORE the transaction leaves the process, then linked to the
signature (`status='submitted'`) or abandoned (`status='abandoned'`, provably
never broadcast: paper mode, simulation reject, terminal send error). A crash
in between leaves a `pending` orphan: an AMBIGUOUS outcome with no signature
to look up. Orphans are swept into `intent` claims (startup + 60 s cadence),
their symbol is entry-gated, and the claim parks for operators after retries
— the transaction is never blindly resubmitted. Cost: one local INSERT per
execution. Polymarket needs no intent journal: its deterministic salt makes
order submission idempotent at the venue (a retry reuses the same order id).

A blocked module is a *safe state*, not an error state: risk limits computed
from a book that contradicts the chain would be fiction (e.g. local 100
tokens vs on-chain 0 must not trade as 100). Claims already parked in
`failed` (alerted when parked) do not re-block on every restart.

## 7. RPC behaviour (§O/§P)

* **Providers**: primary + failover list (`Rpc::with_urls`); send falls over
  once on primary rejection; reads retry through the chain. Confirmation
  uses `confirmed` commitment (`getTransaction`, `maxSupportedTransactionVersion: 0`);
  finality is a config-level choice for reads elsewhere (`network.commitment`).
* **Commitment levels**: `processed` — node-local, can be dropped;
  `confirmed` — supermajority vote, the suite's execution truth (what
  "landed" means here); `finalized` — irreversible. A tx confirmed but not
  yet finalized can theoretically still be dropped in a cluster partition;
  the reconciliation queue re-reads truth later (the sweep only re-queues
  rows still marked `submitted`), and `transactions.status` supports
  `finalized` for stricter policies.
* **Unavailable ≠ absent**: every reader distinguishes "the chain answered:
  zero/none" from "could not read". `token_balances_for_owner` returns
  `Ok(accounts: 0, total: 0)` only for a real answer; transport failures are
  `Err` → `ExternalStateUnavailable` → retry + `bot_external_state_read_errors_total`.
  Nothing ever reports success from an unreadable source.
* **Balance aggregation (§D)**: all of an owner's token accounts for a mint
  are summed (ATA + auxiliary), each validated for mint+owner identity
  (jsonParsed and raw-base64 wire shapes both handled), decimals taken from
  the chain; closed accounts vanish from the response naturally, so no
  double-count and no stale-account inflation.

## 8. Duplicate-execution prevention (§G)

Layers, outermost first:

1. **Feed dedup** — existing dedup store (Redis L2 + in-memory BoundedSet):
   Geyser/WS/poll duplicates of the same event signature collapse.
2. **OMS idempotency keys** — `sha256("ordersent|module|symbol|signature")`
   (or per-fill keys); duplicate intents return the existing order and meter
   `bot_duplicate_execution_prevented_total{where="oms_memory"|"oms_db"}`.
   Terminal orders never re-execute.
3. **Executor policy** — ambiguous outcomes are never re-broadcast with a
   new blockhash; only provably-unforwarded failures retry.
4. **Venue-level** — Polymarket salts are derived from the intent's semantic
   content (maker, signer, token, amounts, side, expiry), so a re-signed
   identical intent maps to the same order id and the CLOB rejects the
   duplicate. (Consequence, deliberate: re-ordering an *identical* GTC
   intent after a cancel requires changing size/price, or using GTD whose
   expiry rotates the id.)
5. **Persistence** — `transactions` PK on signature, `trades` PK on id,
   claim queue PK (kind, subject): replays are no-ops.

## 9. Position reconciliation & PnL (§J/§K)

* Live Solana positions are re-verified against the aggregated on-chain
  balance (per-execution claims + periodic rechecks). Tolerance: 1 %
  relative or dust floor (fee/rent artifacts on tiny positions).
* **Flags, not silent rewrites**: `ExternalAhead`/`LocalAhead`/
  `QuantityMismatch` raise a `drift_flag` risk event + Error alert and
  resolve as `drift-flagged` — the book is *not* rewritten from a balance
  snapshot alone (a balance cannot tell us *price*, so PnL cannot be
  honestly recomputed from it).
* **One automated correction** (deterministic, auditable): chain says zero,
  book says open, no execution in flight, and persisted fills include a
  confirmed exit → close the position with economics **recomputed from the
  fill history** (`reconstruct_pnl`: average-cost; realized =
  proceeds-of-sold − avg-cost-of-sold; `realized_quote`/`cost_basis` set to
  the replayed totals so the existing `realised()` invariant holds), risk
  event `recon_correction`, operator-visible alert.
* Zero on-chain *without* an exit fill → `RecoveryRequired` → parked:
  tokens left with no authoritative execution data is operator territory
  (external transfer? lost trade record?); no PnL is fabricated.
* `UnexpectedPosition` (balance with no open local position) → parked +
  risk-visible: exposure that local state doesn't know about must surface,
  never be adopted silently.
* **PnL discipline (§K)**: quantity, average cost, exit proceeds and fees
  are separate inputs; realized PnL only ever comes from *persisted fills*
  (unconfirmed sends are claims, not fills); reconstruction is a pure
  function — replaying the same history after any number of restarts yields
  byte-identical numbers (tested, including the DB round-trip).

## 10. Risk-engine interaction (§L)

Drift flags and `RecoveryRequired` parks write risk events through the
existing `RiskEventRepo` pipeline (the same channel daily-loss and
consecutive-failure tripwires use). Blocked modules are disabled through
`set_enabled(false)` — the same lever the risk engine's kill paths use — so
no module can trade while its claims are unresolved, and no reconciliation
path bypasses the risk engine or the signer abstraction (corrections never
broadcast anything; they only re-derive local state from external truth).

## 11. Visibility (§R/§S) and metrics (§T)

* `GET /api/status` and Telegram `/status` show the reconciliation backlog
  per kind (`recon_unresolved`); `GET /api/recovery/failed` lists parked
  claims. Telegram is strictly read-only here — no command can clear,
  hide or "resolve" a claim; re-enabling a blocked module is an explicit,
  audited operator action.
* Alerts (existing event→Telegram pipeline): `drift_flag`,
  `recon_correction`, `recon_recovery_required`, `unexpected_position`,
  `recon_conflict` (DB-says-success/chain-says-failed), startup module
  blocks, and give-up parks.
* Metrics (fixed label sets — no wallets, signatures or token ids):
  * `bot_reconciliation_verdicts_total{kind,verdict}` (resolved/retry/given_up)
  * `bot_reconciliation_outcomes_total{kind,outcome}` (13 typed outcomes)
  * `bot_reconciliation_duration_ms{kind}` (histogram)
  * `bot_reconciliation_unresolved` (gauge, 60 s sampler)
  * `bot_external_state_read_errors_total{source}`
  * `bot_duplicate_execution_prevented_total{where}`

## 12. Migrations (§U)

Lexicographic order is the apply order (sqlx migrator, `auto_migrate`):
`0001` bootstrap → `0002` orders/executions/**transactions** → `0003`
positions/trades → `0004` dedup/risk/audit → `0005` **reconciliation_state +
recovery_checkpoints** → `0006` **transaction attribution** (`signer`,
`venue`, `attempts` — additive, `IF NOT EXISTS`, defaulted; restart-safe on
partially-migrated databases, preserves all rows). Indexes back every queue
query (`status,next_attempt_at`) and operator lookup (`signer`,
`status,submitted_at`, `position_id,ts`).

## 13. Honest limitations (after the gap-closure pass)

Closed since Prompt 2 (all with tests, see §14): crash point C (write-ahead
intent journal), per-symbol startup gating, cross-replica
`transactions.attempts` (MAX-on-conflict while a row is still `submitted`;
attribution columns stay immutable), on-chain CTF settlement reads for
Polymarket, and fill-justified drift correction. What remains, honestly:

* **Intent journal cost & residual window**: one extra local INSERT before
  each Solana broadcast (sub-millisecond, but non-zero); disable with
  `RECOVERY_INTENT_JOURNAL=false` if a deployment accepts the old
  balance-recon-only discovery. A crash *before* the intent INSERT is crash
  point A/B again: nothing moved, nothing to recover. An orphan intent can
  never be auto-resolved to Filled/Failed (no signature exists to look up)
  — it parks for operators by design; the symbol gate limits the blast
  radius to that one symbol.
* **Symbol attribution needs the order row**: a `transaction` claim maps to
  a symbol only when `transactions.order_id` is set and the order persists;
  unattributable claims (e.g. `balance:<addr>`) keep the conservative
  module-wide block.
* **Polymarket CTF checks verify, they do not fabricate**: a matched order
  whose fill event was missed is now DETECTED on-chain (ERC-1155
  `balanceOf` on the CTF contract via `[polymarket].ctf_rpc_url`) and
  flagged with balance evidence — but the position is still not
  auto-created: cost basis must come from authoritative fill data (§Y),
  and a balance alone cannot reconstruct it. `ctf_rpc_url = ""` disables
  the check (venue API remains the order-lifecycle truth).
* **Drift correction stays fill-justified**: quantities are adopted from
  the chain only when `reconstruct_pnl` over the durable fill history
  independently reproduces the observed balance within tolerance; every
  other divergence flags for operators. This is a policy choice (§Y: no
  silent overwrites), not a technical limit.
* **Telegram stays read-only for reconciliation** (by design, §Y): the
  backlog and entry-gated symbols are visible in `/status`, alerts flow
  through the event bus, and no command may mutate claims.

## 14. Testing map (§V/§W)

* Engine decision matrix, PnL replay, dust/tolerance, unavailable-source
  semantics: `crates/core/src/reconciliation.rs` (unit, offline).
* Broadcast classification + `SendUnknown`/`SendFailed` end-to-end against
  mock endpoints: `crates/solana-kit/src/execute.rs` tests.
* Deterministic Polymarket order ids: `crates/module-polymarket/src/orders.rs`.
* Startup-gate module blocking: `crates/server/src/main.rs` tests.
* DB-gated (CI, `POSTGRES_URL`): attribution columns, claim lifecycle
  (enqueue/resolve/reopen/park), PnL replay from persisted fills,
  `startup_reconcile` report semantics: `crates/core/tests/db_integration.rs`.
* Validator-gated (`E2E_NETWORK=1` [+ `E2E_LIVE=1`], `E2E_URL` → local
  `solana-test-validator`): full crash→restart→truth→convergence loop with
  exactly-one-transfer assertion, and the transport-black-hole ambiguity
  test: `crates/solana-kit/tests/recon_crash_e2e.rs`.
