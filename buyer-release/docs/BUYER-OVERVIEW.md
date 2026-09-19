# Buyer technical overview

Part of the buyer / due-diligence package for **sniper-suite 0.1.0** (frozen
engineering tree, release commit `9c677cd`, freeze commit `0e139c3`).
Everything stated here is backed by source files in this repository; pointers
are given inline. This is a technical description, not a sales document. No
market-value, sale-price, or performance-superiority claims are made anywhere
in this package.

## 1. Product overview

sniper-suite is a modular crypto trading system written entirely in Rust. It
bundles five cooperating modules behind one control plane (Axum REST +
WebSocket + embedded HTML dashboard), with PostgreSQL as the durable source of
financial truth, Redis as non-authoritative coordination/cache, and a Telegram
bot for remote on/off control.

The system defaults to **paper** trading. Live execution requires two explicit
configuration gates (`[execution] mode = "live"` **and**
`allow_live_trading = true`) plus real key material; with the second gate off,
live requests are downgraded and nothing is broadcast (README §"Going live",
`crates/core/src/config.rs`).

Delivery size at freeze: **146 tracked files, 2,801,590 bytes (2.80 MB),
77,980 lines** (measurement method in `docs/BUYER-DUE-DILIGENCE.md` §A).

## 2. Module overview

| # | Module | Crate / path | Function |
|---|--------|--------------|----------|
| 1 | Sniper | `crates/module-sniper` | Detects new pump.fun launches (PumpPortal WS, Geyser `transactionSubscribe`, poll fallback) and executes entries, with PumpSwap/Raydium/Jupiter exit routing (`detect.rs`, `entry.rs`, `exit.rs`). |
| 2 | Copy trading | `crates/module-copy` | Mirrors buys (and optionally exits) of tracked wallets with per-wallet rules, sizing and staleness guards (`feeds.rs`, `mirror.rs`, `exit.rs`). |
| 3 | Polymarket | `crates/module-polymarket` | Gamma + CLOB REST/WS integration, EIP-712 v2 order signing, CTF ERC-1155 balance reads (`gamma.rs`, `clob.rs`, `eip712.rs`, `orders.rs`, `ctf.rs`, `ws.rs`, `auth.rs`, `strategy.rs`). |
| 4 | Staking program | `programs/staking-suite` | Standalone native Solana program (no Anchor): reward mint, vault + fee treasury, per-second APY accrual, deposit fee, parameter timelock, two-step admin transfer, pause-deposits-only, hard parameter caps, one-time latched `GenesisMint`. |
| 5 | Telegram control | `crates/module-telegram` | Long-polling bot: deny-by-default RBAC, module on/off, kill switch, rate-limited alerts (`commands.rs`, `alerts.rs`, `api.rs`). |

Shared plumbing: `crates/core` (`bot-core` — config, state, event bus, risk,
OMS, dedup, auth, audit, recovery, journal storage, Postgres repositories,
Redis KV, observability) and `crates/solana-kit` (RPC, WS supervision, account
cache, instruction builders, transaction executor, signer registry, swap
decoding). `crates/server` is the runnable `sniper-suite` binary.

## 3. Architecture

Seven-crate Cargo workspace (`crates/`) plus one standalone program crate
(`programs/staking-suite`, own lockfile, excluded from the workspace). Data
flows over an in-process event bus (`crates/core/src/events.rs`); modules are
supervised by the server binary (`crates/server/src/main.rs`) with defined
startup/shutdown ordering (`docs/ARCHITECTURE.md`). Persistence pumps
(`crates/server/src/persist.rs`) write authoritative state to PostgreSQL;
reconciliation tasks (`crates/server/src/recon.rs`) resolve intent outcomes
against on-chain/venue truth (`docs/RECONCILIATION.md`).

## 4. Control plane

Axum server (`crates/server/src/api.rs`, `ws.rs`, `dashboard.rs`):
23 REST endpoints over 21 `/api` routes, a WebSocket event feed
(`/api/events`), and 4 infrastructure routes (`/`, `/health`, `/ready`,
`/metrics`) — 28 endpoints documented route-by-route in `docs/API.md`.
Mutating routes require the `x-api-key` header when `API_KEY` is set; the
server refuses to bind a non-loopback address without API auth configured.
Every response carries a correlated `x-request-id`. Liveness (`/health`) is
process-only; readiness (`/ready`) reflects component state and returns 503
when degraded.

## 5. Execution path

Every money-moving action follows one pipeline (maintenance invariant,
`docs/HANDOVER.md` §6):

**risk → ownership claim → idempotency → intent journal → authorization →
execution → persistence → reconciliation → audit**

Concretely: the global risk engine (`crates/core/src/risk.rs`) decides first;
distributed ownership claims the logical execution
(`crates/core/src/ownership.rs`, `redis_ownership.rs`, `db/claims.rs`); the
OMS assigns idempotency keys and drives the order state machine
(`crates/core/src/oms.rs`); the intent journal records intent before send
(`crates/core/src/storage.rs` + migration `0007_intent_journal.sql`);
the Solana executor builds, simulate-first checks, signs via the signer
registry and broadcasts (`crates/solana-kit/src/execute.rs`, `tx.rs`,
`signer.rs`); fills/positions persist to PostgreSQL
(`crates/core/src/db/repo.rs`); reconciliation resolves ambiguous outcomes
(`crates/core/src/reconciliation.rs`); and every step appends to the
hash-chained audit trail (`crates/core/src/audit.rs`).

## 6. Risk engine

`crates/core/src/risk.rs`: global pre-trade checks (position capacity,
exposure limits, daily-loss auto-disable), enforced before execution for all
modules — no module may bypass it. In multi-replica deployments a
cluster-wide `GlobalRiskOracle` propagates limits and may only **tighten**
them (`docs/DISTRIBUTED.md`). Kill switch halts everything
(`POST /api/kill`, Telegram `/kill`) and is synced across replicas via
runtime flags (migration `0010_runtime_flags.sql`).

## 7. Persistence

PostgreSQL is the durable source of truth for orders, executions, positions,
trades, intents, claims, flags and the audit trail — 11 forward-only
migrations (`crates/core/migrations/0001`–`0011`, embedded in the binary via
sqlx, applied at startup when `auto_migrate` is on). Redis is explicitly
non-authoritative (dedup L2, coordination, cache) and may die without losing
money-relevant truth (`docs/BACKUP-RESTORE.md`). A local JSONL journal with
rotation and corrupt-line tolerance provides restart fidelity independent of
the database (`crates/core/src/storage.rs`). Restart-safe dedup spans three
levels: memory / Redis / Postgres (`crates/core/src/dedup.rs`).

## 8. Reconciliation

`crates/core/src/reconciliation.rs` + `crates/server/src/recon.rs`: on startup
and continuously, recorded intents are resolved against venue truth (Solana
transaction status, Polymarket order state). Ambiguous outcomes (e.g.
simulate-mode `Sent`, broadcast timeouts) enter a handoff-grace path rather
than being silently re-executed; the full source-of-truth model, ambiguity
matrix and PnL replay rules are in `docs/RECONCILIATION.md`. Transaction
attribution (matching unknown on-chain transactions to intents) is backed by
migration `0006_transaction_attribution.sql`.

## 9. Distributed ownership

Invariant: **one logical execution ⇒ at most one active owner ⇒ at most one
money-moving submission** (`docs/DISTRIBUTED.md`). Implementation: claim
stores with three backends (Postgres authoritative —
`db/claims.rs`/`0009_execution_claims.sql`; Redis — `redis_ownership.rs`;
in-memory for single-process), leases + epochs + fencing tokens, handoff grace
for ambiguous outcomes, cross-replica kill-switch/module-flag sync, position-
book sync, and the append-only `execution_claim_events` lineage table
(`0011_execution_claim_events.sql`). Tested by `distributed_integration`
(4/4) and `two_replica_mirror` (1/1) against real PG + Redis.

## 10. Observability

Structured logs (text or JSON), exactly one info line per HTTP request with
`request_id` correlation; Prometheus metrics at `/metrics` (text format
0.0.4) with stable `bot_*` names and bounded label sets — never symbols,
wallets, signatures, secrets, or unbounded paths (full metric table in
README §Observability, operational runbook in `docs/OPERATIONS.md`).
Health/readiness semantics: `docs/BUYER-OVERVIEW.md` §4 above and
`crates/core/src/obs/health.rs`, `metrics.rs`.

## 11. Staking program

Native Solana program in `programs/staking-suite` (borsh-encoded
instructions, PDAs `["staking-config"]` / `["staking-stake", staker]`,
custom errors `6000+`). Security model: full account validation, parameter
caps (`MAX_FEE_BPS` 10%, `MAX_REWARD_RATE_BPS` 100% APR), pause that can
never trap funds (withdrawals/claims never gated), two-step admin transfer,
OpenZeppelin-style parameter timelock (queue → wait → permissionless apply,
cancel; delay changes wait out the old delay), one-shot latched `GenesisMint`
(`genesis_done`; second attempt fails `GenesisAlreadyDone` 6026). Economics,
governance and the launch sequence: `docs/STAKING.md`.

**Status honesty:** the program compiles to BPF (5,440-byte `.so`, agave
2.1.21) and passed 2/2 validator end-to-end tests on a local
`solana-test-validator` — both **PREVIOUSLY VERIFIED** on identical source,
not re-executed in the final freeze sandbox. It has **no external security
audit**, and `declare_id!` is a **pre-deploy placeholder**. Do not deploy to
mainnet until an independent audit passes (README, `docs/SECURITY.md`).

## 12. Deployment model

Three supported shapes (`docs/DEPLOYMENT.md`): (a) Docker Compose stack —
bot + PostgreSQL 16 + Redis 7, healthcheck-gated, API published on loopback
by default; (b) bare metal — pinned Rust toolchain (`rust-toolchain.toml`,
1.98.1), external PG ≥ 16 and Redis 7; (c) multi-replica — same binary, N
processes, distributed ownership per §9. CI (`.github/workflows/ci.yml`)
covers fmt/clippy `-D warnings`/build/test with real service containers, the
staking program job (host tests, `build-sbf`, gated validator e2e), security
job (cargo-audit both lockfiles, cargo-deny incl. licenses), and a Docker
image build + container smoke test. Step-by-step buyer deployment:
`docs/BUYER-DEPLOYMENT.md`.

## 13. Security model

Threat model, key management and limitations: `docs/SECURITY.md`; reporting
policy: root `SECURITY.md`. Highlights: paper-by-default with two live gates;
secrets only from environment/secret store, never committed (release-check
secret scan gate); signer boundary — trading modules never touch key
material, signing goes through `TransactionSigner` + named `SignerRegistry`
(`crates/solana-kit/src/signer.rs`), multi-signer transactions require every
signer declared and resolvable; `[signing] provider` backends `vault`/`kms`/
`hsm` fail startup rather than silently falling back; API RBAC + Telegram
deny-by-default roles (owner/operator/readonly; `/mode live` owner-only);
append-only hash-chained audit trail with `GET /api/audit/verify` and
tamper-detection tests (modification/reorder/missing/duplicate, linear chain
under 8 concurrent appenders); Telegram error strings provably never contain
the bot token (regression test `error_strings_never_contain_the_bot_token`).

## 14. Testing model

Layered (`docs/TESTING.md`): deterministic offline unit/integration tests
with protocol mocks (PumpPortal WS, Geyser `transactionSubscribe`, mock
JSON-RPC pair for fan-out, Polymarket CLOB/Gamma HTTP, storage journal);
gated integration suites against real PostgreSQL + Redis
(`db_integration` 23, `redis_integration` 10, `distributed_integration` 4,
`two_replica_mirror` 1 — all executed in the final freeze pass, 38 gated
tests total); network-gated e2e (devnet, latency bench, validator e2e) that
skip cleanly without their env vars and never run silently. Final freeze
numbers: **521/521 workspace tests, 48 staking host tests + 2 gated e2e,
release-check 20/20 gates PASS, 0 failures**; post-delivery audit pass on
the current tree: **537/537 workspace tests, 71 staking host tests + 3
gated e2e, 0 failures** (CHANGELOG [Unreleased], AUDIT.md §28);
buyer-hardening pass 2026-09-18: build-sbf EXECUTED and **all 3 validator
e2e EXECUTED/PASSED** on solana-test-validator 2.1.21 (AUDIT.md §29,
docs/EVIDENCE-INDEX.md). Verification taxonomy
(VERIFIED / PREVIOUSLY VERIFIED / GATED / NOT EXECUTED): `docs/HANDOVER.md` §3.

## 15. Recovery model

Crash recovery is a first-class path, not an afterthought
(`docs/RECONCILIATION.md`, `crates/core/src/recovery.rs`): on startup the
intent journal and Postgres intents are replayed, unresolved intents are
reconciled against venue truth before new work begins, ambiguous outcomes
respect handoff grace, and dedup state (memory/Redis/Postgres) prevents
double execution across restarts. Backup/restore: `pg_dump` → restore → full
`db_integration` suite green on the restored database was **VERIFIED** in the
final pass; migrations are forward-only by design; Redis loss degrades
coordination only (`docs/BACKUP-RESTORE.md`).

---

Next documents in this package: `CAPABILITY-MATRIX.md` (per-capability
evidence), `BUYER-DUE-DILIGENCE.md` (verification checklist),
`DELIVERY-MANIFEST.md` (index of everything delivered).
