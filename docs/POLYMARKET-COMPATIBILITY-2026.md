# Polymarket Compatibility 2026 (2026-09-30)

EVIDENCE-LEVEL: UNIT_TEST

This document states exactly which Polymarket protocol surfaces this
codebase implements, which are explicitly unsupported, and what evidence
exists for each statement.

## What is implemented

### Order domains

| Domain | Status | Evidence |
| --- | --- | --- |
| V2 (CTF Exchange, domain version `"2"`, `Order` struct incl. `metadata`/`builder` fields) | Implemented | `crates/module-polymarket/src/eip712.rs` (domain constants, keccak EIP-712 hashing) + unit tests |
| V3 position orders (negative-risk / merged-position market operations) | Implemented — explicit, not inferred from V2 | `crates/module-polymarket/src/exchange_v3.rs`, `position_orders.rs` + unit tests incl. invalid-position rejection and wrong-domain rejection |
| Order placement via CLOB REST | Implemented | `clob.rs` (auth, order submit, cancel), `orders.rs` |
| Async order lifecycle | Implemented | `async_commit.rs` (accepted without transaction hash), `trade_resolution.rs` (trade IDs present/absent), `reconcile_async.rs` (later hash resolution, final success/failure) |

### Semantics that are preserved exactly

* **`position_id`** — carried through V2 and V3 paths without
  transformation; V3 position orders validate it before submission
  (invalid V3 positions are rejected client-side, unit-tested).
* **No duplicate order on retry** — idempotency keys collapse duplicate
  submissions (unit-tested).
* **Tenant context is attached across the whole pipeline** —
  `tenant_context.rs` → `tenant_executor.rs` → async polling → trade
  resolution → reconciliation → persistence. The tenant identity is set
  at submission and never re-derived from untrusted responses.
* **Async is not fake-confirmed** — an async order NEVER transitions to
  `confirmed` on acceptance; only resolution/reconciliation moves it to a
  final state.
* **V2 regression** — the V2 domain remains supported; nothing V3
  silently downgrades to V2 and nothing V2 breaks (unit-tested).

### Reconciliation compatibility

`reconcile.rs` + `reconcile_async.rs` remain compatible with the
existing reconciliation model: fills and trades are matched back to
orders by venue ids, with tenant-scoped persistence.

## What is NOT implemented / NOT proven

| Area | Status |
| --- | --- |
| Live CLOB market data in production | Code exists (`gamma.rs`, `ws.rs`, `discover.rs`) — no LIVE_TEST has been performed |
| Live order placement on Polymarket mainnet | Not proven — no LIVE_TEST / FUNDED_TEST evidence |
| Claiming current-market V3 primacy ("latest…") | Not claimable — V3 support is UNIT_TEST-level; primacy language implies live-market verification |
| Polymarket UI/API changes after 2026-09-30 | Not tracked — compatibility statements are as of the date above |

## Chain environment

The EIP-712 domain targets the CTF Exchange on Polygon (chain id 137)
with domain name `"Polymarket CTF Exchange"`; V2 domain version is `"2"`.
V3 position orders target the exchange's position-order entrypoint with
the same domain and typed-struct separation.

## Evidence index

* Unit suites: `cargo test -p module-polymarket` (V2 regression, V3
  position order, invalid V3 position, wrong signature domain, tradeIDs
  parsing, missing transaction hash, async resolution, reconciliation).
* Tenant pipeline: `crates/module-polymarket/src/tenant_context.rs`,
  `tenant_executor.rs` + tests.
