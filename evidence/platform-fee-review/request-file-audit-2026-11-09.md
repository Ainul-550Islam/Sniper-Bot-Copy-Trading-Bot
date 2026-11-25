# Requested fee files: source audit

Documentation checked: https://docs.polymarket.com/programs/builders/fees on 2026-11-09. CLOB V2 embeds a registered bytes32 builder code in the signed order; matched notional is charged the effective maker/taker builder rate, separately from Polymarket's own fee. Builder earnings and payout are not established by an order signature or a local estimate. No real-funds test performed.

- `crates/module-copy/src/mirror.rs`: direct curve buy calls `direct_fee_instruction`; Jupiter buy calls `jupiter_fee_request`. Its post-execution bookkeeping also accepts `Sent` and `SendUnknown` as filled, so a quoted estimate must not be treated as a confirmed fee or realized revenue. A prepared fee audit is not a durable skip journal.
- `crates/module-copy/src/exit.rs`: direct and Jupiter sell fee preparation exists. Exit order intent identity exists, but no durable per-intent fee-ledger posting or confirmed actual proceeds based assessment is shown; sent/unknown are also booked.
- `crates/module-copy/src/tenant_executor.rs`: tenant context and outcome/position sink exist; no tenant fee policy resolution or ledger posting is wired to these outcomes. Existing plan lookup is read-only and does not snapshot policy on an intent.
- `crates/module-copy/tests/copy_platform_fee.rs`: direct fee instruction, audit and offline signed transaction coverage exists; no end-to-end ledger/idempotency paper/live proof.
- `crates/module-polymarket/src/builder.rs`: V2 code parser and signed builder canonicalization exist; position buy/sell and mock CLOB tests are evidence only for those tested paths, not every strategy, copy or Telegram path.
- `crates/module-polymarket/src/tenant_executor.rs`: fill identity guards and scoped persistence exist; builder revenue calculation/comparison helpers are not connected to fill persistence or an operator ledger.
- `crates/core/src/billing/platform_fee.rs`: FeeSurface, backward-compatible optional attribution, policy validation and HALF-UP integer arithmetic exist. The projections are not a live settlement ledger.
- `crates/core/tests/platform_fee_surfaces.rs`: per-surface in-memory projection tests and seeded arithmetic properties exist. `crates/core/tests/platform_fee_projection_pg.rs` checks scoped durable projection sums but does not reconcile an actual cash ledger.
- `crates/server/src/telegram_trade_port.rs`: both actions route through shared Jupiter request preparation; confirmed live and paper/simulation paths book trades, but no fee posting, frozen tenant policy or chain fee receipt verification is wired in. Simulate is not a cash fill.
- `crates/server/tests/telegram_fee_wiring.rs`: offline request preparation tests do not prove complete command pipeline, balanced durable postings, disabled tenant policy or live settlement.

Prior evidence: `evidence/platform-fee-review/builder-signing-and-policy-2026-11-09.txt`, `tenant-policy-dust-sniper-2026-11-09.txt`, `polymarket-fill-replay-2026-11-09.txt`, `polymarket-builder-estimate-2026-11-09.txt`, `postgres-projection-journal-2026-11-09.txt`. These limitations must remain open until implementation and independent regression evidence exist.
