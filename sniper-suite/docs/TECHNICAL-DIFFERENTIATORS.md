# Technical differentiation — factual characteristics only

Concrete engineering characteristics that exist in sniper-suite 0.1.0, each
with its location in the tree. This document does **not** rank the project,
compare it to competitors, call it "best"/"enterprise"/"production-proven",
or make performance-superiority claims. Whether these characteristics matter
is the buyer's judgment; that they exist is verifiable.

## Language & build discipline

1. **All-Rust implementation** — memory-safe systems language for every
   component: trading modules, control plane, and the on-chain program
   (91 `.rs` files; no second production language).
2. **Hard-pinned toolchain, three-way enforced** — 1.98.1 in
   `rust-toolchain.toml`, `Dockerfile` (`rust:1.98.1-bookworm`) and CI
   (`dtolnay/rust-toolchain@1.98.1`); `scripts/release-check.sh` fails the
   release if they drift.
3. **`clippy -D warnings` as a hard gate** on both cargo projects (app
   workspace + standalone program), in CI and in the local release gate —
   zero-warning codebase at delivery.
4. **Two committed lockfiles** with MSRV-aware resolution for the on-chain
   crate (`rust-version = 1.79` + `.cargo/config.toml` resolver policy so the
   agave platform-tools compiler can always build the lock).
5. **On-chain release profile hardened** — `overflow-checks = true`,
   `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`
   (`programs/staking-suite/Cargo.toml`).

## Architecture

6. **Modular crate architecture** — modules are independent crates behind a
   shared core (events, risk, OMS, persistence); a venue/module can be
   replaced without touching the money-path invariants.
7. **Single supervised binary** — one process supervises all modules with
   defined startup/shutdown ordering (`crates/server/src/main.rs`), rather
   than a sprawl of services.
8. **Strict configuration contract** — `deny_unknown_fields`: a typo in
   `config.toml` fails startup instead of silently changing behavior;
   unimplemented signing providers (`vault`/`kms`/`hsm`) fail startup rather
   than falling back (`crates/core/src/config.rs`).

## Solana-specific engineering

9. **Protocol-specific instruction builders, hand-rolled** — pump.fun
   bonding curve (incl. v2 variants), PumpSwap AMM, Raydium AMM
   (`SwapBaseIn` + `SwapBaseInV2`), Jupiter routing, from parsed account
   layouts (`crates/solana-kit/src/{pump,pumpswap,raydium,jupiter,layout}.rs`)
   — no heavyweight third-party "sniper SDK" dependency.
10. **Geyser support** — Yellowstone-style `transactionSubscribe` push feeds
    with failed-tx skipping and automatic poll fallback
    (`crates/solana-kit/src/events.rs`; mock-tested).
11. **Warm account cache** — TTL + FIFO-bounded caching of semi-static
    accounts (`crates/solana-kit/src/cache.rs`; env-tunable, `0` disables).
12. **Multi-RPC fan-out** — `BROADCAST_FANOUT` races the same signed
    transaction across primary + fallback RPCs, first accept wins
    (mock-JSON-RPC-pair tested); retry chokepoint with failover after bounded
    consecutive failures and per-method metrics.
13. **Signer abstraction** — `TransactionSigner` + named `SignerRegistry`;
    multi-signer transactions fail structurally if any required signer is
    undeclared/unresolvable; trading modules never touch key material
    (`crates/solana-kit/src/signer.rs`).
14. **Simulate-first execution policy** — transactions are RPC-simulated
    before broadcast by default (`SIMULATE_FIRST`,
    `ABORT_ON_SIMULATION_FAILURE`).

## Correctness / money-path engineering

15. **Deterministic idempotency** — idempotency keys + three-level
    restart-safe dedup (memory/Redis/Postgres) (`crates/core/src/{oms,dedup}.rs`).
16. **Intent journal + reconciliation-before-trust** — intents are recorded
    before send; on restart, unresolved intents are reconciled against venue
    truth before new work; ambiguous outcomes get handoff grace instead of
    blind retry (`docs/RECONCILIATION.md`).
17. **Distributed claims with leases, epochs and fencing tokens** — the
    invariant *one logical execution ⇒ ≤1 active owner ⇒ ≤1 money-moving
    submission*, with an append-only `execution_claim_events` lineage table
    for forensics (`crates/core/src/ownership.rs`, migrations `0009`–`0011`).
18. **Global risk oracle, tighten-only** — cluster-wide risk limits can
    propagate in one direction only; a rogue replica cannot loosen risk
    (`docs/DISTRIBUTED.md`).
19. **Hash-chained append-only audit trail** — no app API can mutate or
    delete audit rows; `GET /api/audit/verify` recomputes the chain; appends
    serialized by advisory lock, tested linear under 8 concurrent appenders,
    with modification/reorder/missing/duplicate tamper-detection tests
    (`crates/core/src/audit.rs`).
20. **PostgreSQL as sole financial truth** — Redis is architecturally
    forbidden from holding authoritative state; forward-only migrations
    (0001–0011) embedded in the binary.

## Staking program characteristics

21. **Parameter timelock with permissionless apply** — queued changes can be
    applied by anyone after the delay (unresponsive admin cannot grief),
    cancellable before; changing the delay itself waits out the old delay
    (OpenZeppelin `TimelockController` rule).
22. **Pause that cannot trap funds** — pause halts deposits only; `Unstake`
    and `Claim` are never gated.
23. **Hard parameter caps on-chain** — fee ≤ 10%, reward rate ≤ 100% APR,
    enforced in-program, so a compromised admin cannot set confiscatory or
    inflationary parameters.
24. **One-shot latched genesis mint** — `GenesisMint` latches
    `genesis_done`; any second attempt fails `GenesisAlreadyDone` (6026) —
    supply cannot be silently inflated post-launch.
25. **Two-step admin transfer** — `TransferAdmin` → `AcceptAdmin`; zero pubkey
    rejected; multisig-admin-by-CPI is the documented production pattern
    without embedding M-of-N logic in-program.

## Cross-venue capability

26. **Polymarket V2 order signing implemented from the spec** — EIP-712
    11-field v2 `Order`, correct domain separator, type-3 signature wrapping,
    L1/L2 CLOB auth headers, CTF ERC-1155 balance reads
    (`crates/module-polymarket/src/eip712.rs` et al.) — wire format
    mock-verified.
27. **Three Solana exit venues behind one executor** — PumpSwap, Raydium,
    Jupiter routing for sniper exits (`crates/module-sniper/src/exit.rs`).

## Verification culture

28. **Extensive test matrix with honest labels** — 537 workspace tests
    (521 at freeze; offline-deterministic core + protocol mocks +
    real-service gated integration), 71 staking host tests + 3 executed
    validator e2e, and a VERIFIED / PREVIOUSLY VERIFIED /
    GATED / NOT-EXECUTED taxonomy applied consistently across
    `release-manifest.json`, `docs/HANDOVER.md`, `docs/TESTING.md`,
    `AUDIT.md` — unexecuted things are never labeled as passing.
29. **One-command reproducible release gate** —
    `scripts/release-check.sh`: 20 gates (files, version identity, toolchain
    pin, migration monotonicity, TODO/stub marker scan, secret scan, fmt,
    check, clippy, full test matrix against real PG/Redis, staking suite,
    cargo-audit ×2, cargo-deny), exit-code honest, gated suites announce
    skips instead of faking passes.
30. **Crash/recovery and tamper scenarios are tested, not just described** —
    restart fidelity, corrupt journal lines, OMS restart-recovery, audit
    concurrency, two-process replica mirroring, all in the delivered suite.

---

Each numbered item is checkable at the cited path. If a claim here and the
source ever disagree, the source wins — and `docs/HANDOVER.md` §6 lists the
invariants a maintainer must not regress.
