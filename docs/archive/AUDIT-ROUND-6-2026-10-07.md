# Audit Round 6 — Accounting & Ledger: The Money-Truth Subsystem

**Date:** 2026-10-07 · **Scope:** `crates/core/src/accounting/*` (book, posting, ledger, event, reconcile, recovery, store, view), the integer money maths (`maths.rs`), and the durable journal repository (`db/accounting.rs`).

**Why it matters:** if the double-entry book lies, every PnL, risk limit, daily-loss kill check and reconciliation downstream is wrong by the same amount. This is the subsystem everything else quotes.

**Verification status (honest):** static review only — no `cargo`/`rustc` in this workspace; nothing compiled or test-run. CI remains the sign-off gate.

---

## Finding fixed this round

### F-6-1 · P2 — corrupt journal rows vanished silently during replay
`db/accounting.rs` decoded `ledger_events` rows with `filter_map(stored_from_row)`, and a row whose `kind`/`module`/`venue`/`mode` failed to parse became `None` — **dropped with no log** (zero `warn!` in the file). The recovery replay builds the entire money truth from these rows, so a corrupt row would make the rebuilt book drift, and the drift would surface only *indirectly* — as `quantity_mismatch` / `position_mismatch` reconciliation findings with no pointer to the actual cause.
**Fix:** `stored_from_row` now wraps the decoder and, on failure, logs a loud `warn!` with the row's `event_id` + the raw `kind/module/venue/mode` values and an explicit instruction to investigate before trusting the rebuilt book. Applies to both read paths (`load_events` = recovery replay, `recent_events` = API). Derived-view decoders (positions/findings/decisions) keep the tolerant convention — they are re-derived from the source of truth.

---

## Verified clean this round (no changes needed)

| Area | Files | Verdict |
|---|---|---|
| Position book math | `accounting/book.rs` | Average-cost convention matches `reconciliation::reconstruct_pnl` (pinned by cross-check test); over-sells clamp to held quantity — never invent negative inventory or cost; fees tracked separately with `net = realized − fees`; flat positions zero cost basis; exposure = max(cost, marked notional) matching the module risk rule. |
| Double-entry expansion | `accounting/posting.rs` | Every event kind expands to a structurally balanced entry; negative amounts flip side so stored amounts are ≥ 0; the balance invariant holds in RELEASE builds too, because `validate()` (run inside `expand`, all builds) rejects non-finite/negative amounts before expansion — the `debug_assert!` is a belt, not the suspenders. |
| Event validation | `accounting/event.rs` | Rejects non-finite/negative qty/amount/fee/price; fills/settlements require side + qty > 0; corrections REQUIRE a correlation id naming the finding/ticket they answer; transfers require a counterparty. |
| Ledger idempotency | `accounting/ledger.rs` | Single mutation door under ONE lock: validate → dedup by event id → expand → apply → index. A replay with a *different amount* but same fact-id is still `Duplicate` (tested). Durable journal arbitrates across process lives; journal outage parks the event as *pending* (retried by `flush_pending`, reported as `unresolved_financial_event`) — never double-applied, never silently lost. Concurrent 16× submission test pins exactly-one. |
| Reconciliation | `accounting/reconcile.rs` | Closed 8-finding vocabulary; joins trades↔events (trade id, signature, fill-id-in-note), orders↔events (correlation/signature/external/trade-note) with explicit no-double-reporting rule; per-asset qty/PnL comparison only when flat on both sides; orphan events, pending events all surfaced. Detects, never repairs — corrections are explicit journaled events. Findings dedup by digest id per process life. |
| Recovery | `accounting/recovery.rs` | Replay-through-the-idempotent-path (nothing booked twice or blindly); module positions with no ledger history are REPORTED as gaps, never synthesized; closed action vocabulary; journal-unavailable starts empty and lets reconciliation flag everything (fail-loud). |
| Money maths | `maths.rs` | On-chain amounts never multiplied as f64: u128 constant-product with saturating math, checked-arithmetic bonding curve with correct rounding directions (buy cost rounds UP), NaN-safe conversions that saturate instead of panicking, tick rounding via integer scaling. |
| Durable journal writes | `db/accounting.rs` | Event + all postings insert in ONE transaction (half-written entry impossible); `ON CONFLICT (event_id) DO NOTHING` is the durable idempotency arbiter; deployment-org scoping on every read. |

## Design notes accepted as-is
1. **f64 in the book** (not decimal): mitigated by the validate-before-expand finite-amount rule, the 1e-9 relative balance tolerance, clamp-to-zero on dust, and on-chain amounts always going through integer `maths.rs`. Consistent suite-wide convention.
2. **`fold_series` early-return on `net == 0.0`** — exact-zero skip is harmless for the daily-loss series.
3. **Pending events stay applied in memory** while the journal is down — deliberate availability choice, made visible via `unresolved_financial_event` findings + `bot_journal_error_total`.

## Change table (Round 6)

| File | Change |
|---|---|
| `crates/core/src/db/accounting.rs` | loud `warn!` on undecodable journal rows (wrapper around the event decoder; `tracing` import) |

## Sign-off checklist (open, unchanged)
- [ ] `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo build --workspace` / `cargo test --workspace -- --test-threads=1` (needs `POSTGRES_URL`, `POSTGRES_MIGRATION_URL`, `REDIS_URL`)
- [ ] `npm ci && npm run typecheck/test/test:e2e` in `apps/control-plane`
- [ ] `./scripts/verify-migration-graph.sh`, `./scripts/export-openapi.sh --check`
