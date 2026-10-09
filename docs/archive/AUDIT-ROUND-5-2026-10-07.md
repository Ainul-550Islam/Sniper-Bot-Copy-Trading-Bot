# Audit Round 5 — HA Infrastructure: Leases, Fencing, Ownership Claims

**Date:** 2026-10-07 · **Scope:** the split-brain defense in full — singleton role leases (`ha/lease.rs`, `ha/store.rs`, `db/ha.rs`, `server/ha.rs`), the leased-worker runtime (`ha/runtime.rs`, `LeasedWorker`), per-intent execution ownership (`ownership.rs`, `redis_ownership.rs`), feed cursors/gaps, and the recovery journal.

**Why this matters most:** a split-brain here is not a data-quality bug — two live reconciliation/feed workers, or two replicas both claiming `snipe:<mint>`, means **duplicate trades and double-spent funds**. This is the subsystem whose failure mode is money.

**Verification status (honest):** static review only — no `cargo`/`rustc` in this workspace; nothing compiled or test-run. CI remains the sign-off gate.

## Result: CLEAN — no changes required

The HA layer is the best-engineered subsystem in the codebase. Every property that prevents split-brain was checked and holds:

### Lease core (`ha/lease.rs`, `ha/store.rs`)
- Fencing tokens (`generation`) strictly increase per role via `saturating_add` — never reused, never wrap.
- `renew`/`release`/`verify` are all compare-and-set on `(holder, generation)`; a fenced worker cannot renew, cannot release the new owner's lease, cannot verify.
- Store failures are NEVER "assume ownership": every fallible path returns error → caller fails closed (`FenceError::StoreUnavailable`).
- Released leases are never live; revocation/expiry semantics are a pure function of the shared clock.
- In-memory store has the SAME semantics under one mutex (poison-tolerant lock), so the two-worker race tests are meaningful without a database.

### PostgreSQL implementation (`db/ha.rs`)
- **The safety-critical statement is one SQL statement:** `INSERT … ON CONFLICT (role) DO UPDATE … WHERE expires_at <= now() OR released OR holder = EXCLUDED.holder` — the database alone decides the winner; two racers can never both get a row. WHERE admits exactly expired / released / self-refresh; every successful path bumps generation.
- All liveness comparisons use the **database clock** (`now()`), never worker wall clocks — immune to replica clock drift; `HaStore::now()` exposes it as the shared reference.
- `renew` requires `released = false AND expires_at > now()` in the same CAS; `release` cannot touch a row that's already released or taken over (row-count checked).
- Rejected acquisition re-reads the current row to report the real holder; a vanished row is a hard error, not a silent grant.
- u64→i64 cursor/gap positions clamped with `.min(i64::MAX as u64)`; row decoders defensively default every column.

### Worker runtime (`server/ha.rs`, `ha/runtime.rs`)
- `LeasedWorker` tick order is correct: **acquire (only when unguarded) → renew on schedule → fence immediately before work → step down on any loss**. A fenced worker drops its guard deterministically instead of mutating shared state.
- Heartbeat refusal (`Ok(false)`) moves the worker to `LeaseLost` — a stale life stops acting as owner.
- Graceful shutdown releases leases so a standby takes over at once; release-on-exit is CAS-guarded.
- Recovery journaling at startup records one deterministic action per unfinished order (`plan_order_recovery` over local evidence × venue evidence), and ambiguous execution intents are handed to reconciliation as `HoldAmbiguous` — never finalized optimistically.

### Per-intent execution ownership (`ownership.rs`, `redis_ownership.rs`)
- Claim identity is the LOGICAL intent (`snipe:<mint>`, `copy:<wallet>:<mint>`, `exit:<position>:<rule>`…) — never a wallet address or signature alone.
- Three-state machine is exactly right for transaction ambiguity: `Claimed` → `Released` (determinate outcome, immediately re-acquirable) or `HandedOff` (send outcome UNKNOWN — re-acquirable only after the handoff grace, which is what prevents a second replica from re-broadcasting a possibly-in-flight transaction).
- Redis store implements every transition as ONE Lua script with server-side `TIME` (no client clocks): claim/takeover, CAS renew on (owner, epoch, claimed, unexpired), verify, CAS terminal transition. Expired-lease takeover bumps epoch and meters the event; handoff-grace math lives inside the script, so there is no read-then-write window.
- `OwnershipRegistry::claim` fails CLOSED (`ownership_unavailable` → no execution) when the store is unreachable; `Rejected` is a deterministic "never broadcast" outcome.
- `ClaimGuard` is deliberately non-Clone; a dropped guard without terminal transition expires by lease = the documented crash semantics.

### Residual-panic scan
All 10 HA files scanned with the awk-excluding-tests filter: **zero** `.unwrap()/.expect()/panic!/todo!()` in production code. (The Redis value decoders use `unwrap_or` fallbacks; `ms_to_dt` uses `timestamp_millis_opt().single().unwrap_or_else(Utc::now)` — total functions.)

## Accepted observations (no action)
1. **Self-refresh acquire bumps generation** (Postgres `WHERE holder = EXCLUDED.holder` branch): if a caller re-acquires a role it already holds, the old guard fences out on next tick. This is correct fencing behavior, and `LeasedWorker` never does it (only acquires when unguarded).
2. **`save_cursor` has no generation guard in SQL** — cursor writes are protected at the layer above (feed workers hold the feed lease and fence before writing); adding a SQL-level guard would require plumbing generations into cursor rows and was judged unnecessary churn.
3. **Redis `TERMINAL_TTL_MS` = 7 days** retention for terminal claim records — audit truth lives in Postgres/logs; acceptable.

## Sign-off checklist (open, unchanged)
- [ ] `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo build --workspace` / `cargo test --workspace -- --test-threads=1` (needs `POSTGRES_URL`, `POSTGRES_MIGRATION_URL`, `REDIS_URL`)
- [ ] `npm ci && npm run typecheck/test/test:e2e` in `apps/control-plane`
- [ ] `./scripts/verify-migration-graph.sh`, `./scripts/export-openapi.sh --check`
