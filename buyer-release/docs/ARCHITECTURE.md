# Architecture

sniper-suite is a Rust workspace: one binary (`sniper-suite`, the server crate)
orchestrates four trading modules, a control API, persistence, reconciliation
and observability. A standalone on-chain program (`programs/staking-suite`,
native Solana, no Anchor) provides staking with fees, rewards, a parameter
timelock and two-step admin transfer.

```
                       ┌────────────────────────────────────────────────┐
                       │                server (bin)                    │
                       │  main.rs — ordered startup, 4-phase shutdown   │
                       │                                                │
 feeds (WS/REST) ────► │  module-sniper ─┐                              │
 pumpportal wss        │  module-copy   ─┼─► RiskEngine (global gate) ──┼──► Executor
 geyser wss            │  module-polym. ─┘        │                     │    (paper/simulate/live)
                       │                          ▼                     │
                       │                    EventBus (broadcast)        │
                       │                     │        │        │        │
                       │              PersistencePump JournalPump obs   │
                       │                     │        │        │        │
                       │  api.rs (Axum REST + WS) ◄────┘        │        │
                       │  recon.rs (3 truth sources)            │        │
                       └───────────┬──────────┬────────────────┴────────┘
                                   ▼          ▼
                              PostgreSQL   Redis (cache/dedup L2 ONLY)
                              (durable)    + JSONL journal (data/*.jsonl)
```

## Crates

| Crate | Role |
|---|---|
| `bot-core` | config (deny-unknown-keys, env overrides), models, error, event bus, risk engine, OMS (idempotent, DB-backed), dedup (L1 bounded set + L2 postgres/redis), auth (sha256 keys, roles, rate limit), audit (hash-chained), lifecycle (orderly shutdown phases), recovery (startup restore), storage (JSONL journal), db (sqlx, all ops timed), redis_kv |
| `solana-kit` | nonblocking RPC wrapper, tx build/sign/send/confirm, signer abstraction + registry (`signer.rs`), block+geyser feed plumbing, pump.fun/raydium account decoders |
| `module-sniper` | pump.fun launch detection → snipe buys → exit strategies (TP/SL/trailing/time) |
| `module-copy` | tracked-wallet mirroring (fixed or fractional sizing, staleness guards) |
| `module-polymarket` | CLOB v2 (EIP-712 orders, neg-risk), Gamma market data, paper fills |
| `module-telegram` | remote control bot with roles (owner/operator/readonly) |
| `sniper-suite` (server) | orchestration, Axum API + dashboard, persistence pumps, reconciliation, observability, WS event feed |
| `programs/staking-suite` | native Solana staking program (see docs/STAKING.md) |

## Data-flow guarantees

* **Risk before execution.** Every module submits intents through the shared
  `RiskEngine` (kill switch, exposure caps, per-trade limits, drawdown
  breaker). No module talks to an executor directly; a rejected intent is
  published as `RiskRejected` and persisted to `risk_events`.
* **Signing behind an abstraction.** Transaction signing goes through
  `solana_kit::signer::TransactionSigner`; the `SignerRegistry` resolves
  logical identities (`primary_trading` always, plus configured `sniper` /
  `copy_trading` / `treasury` / `staking_admin` entries) to signers.
  `TxBuilder` enforces that the compiled message's required-signer set is
  exactly {wallet} ∪ declared `extra_signers`, each resolvable — otherwise
  the build fails with a structured `SignerError`. Strategy modules hold no
  key material; `Wallet::sign_message_sync` is the only local signing choke
  point, and Vault/KMS/HSM backends plug in by implementing the trait.
* **Events → durable state.** Modules publish `AppEvent`s. `PersistencePump`
  materializes them into Postgres (orders via OMS, trades, positions, risk +
  system events, transaction claims for reconciliation). `JournalPump` writes
  the same stream to rotating JSONL files. Both are append-only from the
  application's perspective; the API cannot rewrite or delete audit records.
* **Recovery.** On startup (before modules spawn) the server restores open
  positions, recovers unfinished orders into the OMS as `Unknown`, and sweeps
  unresolved transactions. A `recon_queue` (SKIP LOCKED claim, exponential
  backoff to 1h, `failed` after exhaustion) drives three truth sources:
  Solana tx confirmation, Polymarket order status, and a Solana position
  drift flag (flags, never auto-corrects).
* **Redis is a cache.** Dedup L2 and rate-limit state may live in Redis, but
  durable financial state is Postgres + JSONL. Losing Redis must never lose
  money-relevant data (dedup falls back to its in-process L1 verdict and
  records the degradation).

## Startup order (main.rs)

1. config load + validation (fail fast on unknown keys / invalid combos)
2. secrets seeded from env (`SOLANA_KEYPAIR`), logging, metrics
3. Redis open (optional unless required) → Postgres open + migrations
4. authenticator / audit trail / OMS / risk engine attach to shared state
5. **recovery** (positions, orders, transaction sweep)
6. persistence + journal pumps start consuming the event bus
7. HTTP server binds (fail-closed: non-loopback bind requires API auth)
8. modules spawn per `enabled` flags; each loop selects on the shutdown signal

## Shutdown (lifecycle.rs)

Four phases, each with a deadline; a stuck phase is logged and abandoned so
shutdown always completes:

1. `http-drain` (10s) — stop accepting, finish in-flight requests
2. `module-drain` (15s) — modules observe the signal, exit sweepers flush,
   worker handles are joined
3. `pump-flush` (10s) — persistence/journal pumps drain queued events
4. `db-close` (10s) — pool closed after the last writer

## Observability

`/health` (liveness), `/ready` (dependency readiness), `/metrics` (Prometheus
text: latency histograms, dedup verdicts/degradation, persistence queue
depths, recon outcomes, risk counters). Structured tracing throughout;
secrets are never logged (keys are stored/referenced by sha256 digest).
