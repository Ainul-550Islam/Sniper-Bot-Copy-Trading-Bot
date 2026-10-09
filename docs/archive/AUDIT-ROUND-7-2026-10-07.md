# Audit Round 7 — Execution Authority, Global Risk, Tenant Trading Data Plane, On-Chain Staking, Modules

**Date:** 2026-10-07 · **Base:** `1d4d5d4` (47 files, +4548/−844 vs pre-audit) · **Method:** full static review, file-by-file.
**Verification status:** ALL findings and fixes are **static-review only** — no `cargo`/`rustc`/`rustfmt` exists in this workspace; nothing here has been compiled or executed. The CI gate remains: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace`, `cargo test --workspace` (+ `POSTGRES_MIGRATION_URL` migration tests), `npm ci && npm run typecheck && npm run test && npm run test:e2e`, `scripts/verify-migration-graph.sh`, `scripts/export-openapi.sh --check`.

---

## Findings (4 fixes this round — 19 total since Round 3)

| ID | Sev | Location | Problem | Fix |
|----|-----|----------|---------|-----|
| **F-7-1** | **P1** | `crates/core/src/execution/mod.rs` | **Lock-ordering deadlock in `ExecutionLedger`.** `begin()` nested `fifo.write()` INSIDE `records.write()`, while `evict_if_needed()` acquires `fifo` FIRST then `records` (and `hydrate()` used a third order). Two concurrent `begin()` calls could deadlock: task A holds `records` waiting for `fifo`, task B (in its own eviction) holds `fifo` waiting for `records`. Under broadcast-driven HFT load this freezes the whole execution guard. | One global lock order — **fifo → records → by_signature** — documented and applied to `begin()`, `evict_if_needed()` and `hydrate()`. `begin()` pushes onto the already-held fifo guard directly; `hydrate()` reordered. No path holds the locks in conflicting order any more. |
| **F-7-2** | **P2** | `crates/server/src/tenant_streams/hub.rs` | **Unbounded tenant-channel leak.** The module docs promised empty channels are reaped "opportunistically on publish/subscribe", but `reap_empty()` was called ONLY from tests. Every organization that ever subscribed left a permanent `broadcast::Sender` entry — unbounded growth with tenant history. | `publish()` and `subscribe()` now perform the reap inline (retain channels with receivers, or the acting org), and a no-listener publish removes its own channel. Tests updated to the new self-reaping contract (`hub.rs` unit tests + `tests/tenant_streams_integration.rs`). `reap_empty()` kept for operators/tests. |
| **F-7-3** | **P2** | `crates/server/src/security/websocket.rs` | **Slow-client stall defeats credential revalidation.** Every `sink.send(...).await` in the SaaS event stream could block indefinitely on a stalled TCP client; while blocked, the 60 s revalidation arm cannot run, so a **revoked** session kept its socket open (and up to 256 buffered events flowing) indefinitely. | Added `SEND_TIMEOUT` (10 s) and a generic `send_with_timeout()` helper (works for both the split sink and an unsplit `WebSocket`); every frame send — market event, saas event, pong, both first-frame-auth error/close sends, and the 1008 "authorization lost" close — is now bounded; a stalled send is treated as a dead connection and the stream returns. |
| **F-7-4** | **P3** | `crates/module-telegram/src/lib.rs` | Fragile `role.expect("checked above")` — provably safe today but one refactor away from a panic on the live Telegram polling loop. | Rebuilt as a structural `let Some(role) = telegram_role(..) else { … continue; }`: the unauthorized path (warn + event log with `accepted: false` + ⛔ reply) lives inside the `else` branch; the authorized path uses `role` by value. No `.expect` remains in production code. |

### Accepted as safe (verified, no change)

- `config_store.rs` lines 78/89/99 — each `.expect` follows an explicit `is_object()` check that returns an error two lines earlier (invariant established immediately above).
- `oms.rs:495` / `execution/mod.rs:899` — `fifo.remove(pos).expect("pos came from the same …")` where `pos` was just produced by `.position()` on the same collection.
- `module-polymarket/orders.rs:295` — `[..16].try_into().expect("keccak output is 32 bytes")` on a keccak256 digest (always 32 bytes).
- `solana-kit/src/consts.rs` — fail-fast `panic!` on malformed compile-time pubkey constants (correct for constants).
- `solana-kit/src/signer.rs:306` — `.expect("fresh registry cannot have duplicates")` inserting into a registry created one line earlier.
- `module-copy/recovery.rs`, `module-copy/tenant_state.rs` — `Mutex::lock().unwrap()` / `.expect("lock poisoned")` accepted under the standing codebase convention (poisoning unreachable under the zero-panic regime; not churn-replaced).
- `ops/*.rs` serialization unwraps — plain structs with string-keyed `BTreeMap`s; serde can only fail on non-string map keys or custom impl errors, neither present.
- **Design notes carried forward:** dedup L2 failure degrades to L1 (availability choice, metered `bot_dedup_l2_degraded_total`); OMS in-memory eviction of terminal orders can drop idempotency keys only when the DB is down (the DB unique key is the final arbiter when present).

---

## Verified CLEAN this round (no changes needed)

**Execution core (`crates/core/src/execution/`)**
- `execution_authority.rs` — 11-check ordered checklist (`AUTHORITY_CHECK_ORDER`), out-of-order/unknown checks refused, `finish` requires all, authority fingerprint = sha256 of `scope.describe()`.
- `execution_scope.rs` — nil runtime rejected fail-closed; fingerprint input covers all 5 identity fields.
- `mod.rs` (rest) — `ExecutionState` machine (terminal states admit only `Reconciled`), `FailureClass::classify_message` **conservative: unknown text → TransportAmbiguous → Pending** (a wrongly-ambiguous verdict delays retry; a wrongly-definite one invites double spend — correct choice); ambiguous classes never re-arm; eviction cleans `by_signature`; `hydrate()` restart policy (Created/Validated → Failed before broadcast, Submitted/Pending → reconcile).
- `tenant_execution_context.rs`, `execution_trace.rs` — no panic sites, scope/authority/wallet/signer refs immutable after issue.

**Global risk (`crates/core/src/global_risk/`)** — all 7 files.
- Kill switch: engage-only-tightens, config-pinned release refused, durable restore only for engaged switches.
- Engine: fixed 9-step check order, missing reference rate ⇒ REJECT (fail closed), every decision journaled, `0 = off` limits.
- `decision.rs` closed reject-reason set; `store.rs` bounded in-memory ring with honest availability flags.

**OMS & dedup (`crates/core/src/{oms,dedup,redis_kv}.rs`)**
- OMS: nothing leaves terminal states; create race-safe via DB unique key; eviction terminal-only-oldest.
- Dedup: L1 bounded + L2 atomic arbiters (`SET NX PX` / `INSERT ON CONFLICT`), L2 error degrades to L1 + meter.
- Redis KV: every op timeout-bounded and metered (`bot_redis_operations_total`), CAS lock release via Lua, pipeline reply destructured correctly, durable state never in Redis.

**Tenant trading data plane (`crates/server/src/trading_data_plane/`, 6,288L)**
- **Every one of the 40+ routes goes through the 8-step fail-closed authorization chain** (`guard`/`guard_manage`): authenticate → org-from-credential → plane → lifecycle → entitlement → module family → per-action permission (`BotStart`/`BotStop` for controls, `OrderRead`/`OrderManage` for orders). The three files with no direct guard call (`module_controls.rs`, `config_store.rs`, `module_control_store.rs`) are library code reachable ONLY through guarded handlers — traced every caller.
- Repository layer: every SQL predicate binds `organization_id = $1` (reads, writes, idempotency lookups); cross-tenant id → 404 without existence leak; `TenantMismatch` → 403.
- `config_store.rs`: row-locked (`FOR UPDATE`) read-modify-write with version bump + `tenant_config_audit` journal in one transaction; version overflow → error.
- `module_control_store.rs`: durable mode ALWAYS reads DB (no cross-replica stale cache), writes fail closed, kill switch is one transaction over all trading modules, cache purged on authoritative absence.
- Zero panic sites outside tests in all 24 files.

**Server ops/backup/staking/security/tenant_streams (12.8k L)** — panic-free; evidence/attestation models honest (`NotRun` never claims success); backup command builders descriptive-only (never executed, secrets never embedded); CORS fail-closed (wildcard explicit-only, credentials disabled with wildcard, empty config ⇒ no CORS).

**Core domains** — `billing/` (integer `amount_cents`, non-negative validated, versioned price snapshots — clients cannot send their own amount), `tenant/`, `provisioning/`, `lifecycle.rs`, `recovery.rs`, `state.rs`, `events.rs`, `models.rs`, `error.rs` — all panic-free, all channels bounded, all statics `OnceLock`-gated.

**`programs/staking-suite` (5,538L, native Solana program — funds-critical)**
- **PDA safety:** config/stake/vault/treasury/metadata all re-derived and `require_address`-checked; canonical token/ATA/system/metadata program IDs pinned before every `invoke`/`invoke_signed` (no signer-substitution attack via malicious program).
- **Integer safety:** reward math in u128 with `checked_mul`, saturates to `u64::MAX` instead of wrapping; `fits_under_cap` fails closed on overflow; supply cap measured against the LIVE mint account.
- **Governance:** immutable `max_supply` (not in `UpdateParams`), one-shot latched `GenesisMint`, timelocked param updates validated at queue AND apply, permissionless `ApplyParams` cannot be griefed, two-step admin transfer rejecting the zero pubkey, pause blocks deposits only — withdrawals can never be frozen.
- **Money path:** fee+net split with `checked_sub`; principal returns only to the staker's own validated token account; reward minting CLAMPED to supply headroom so withdrawals never fail; cooldown via `saturating_sub`; re-initialisation rejected; freeze authority `None`.
- **Metadata:** one-shot, immutable (`is_mutable = false`), byte-length limits enforced client-side and on-chain, PDA mirror-checked against the canonical Metaplex derivation.

**Module crates**
- `module-polymarket` (14.5k L): WS market/user feeds reconnect with capped backoff, 10 s pings, pong replies, `tx.closed()` shutdown; user feed carries a durable HA cursor with replay suppression and gap metering; run loop has a proper `wait_shutdown` arm with optional cancel-all-on-shutdown; restart recovery runs before the first scan; heartbeat task exits on shutdown; EIP-712 uses real keccak256 with explicit typehashes and error-checked u256/address parsing; deterministic order salt (same logical order ⇒ same salt ⇒ venue-side dedup, `| 1` never-zero).
- `module-telegram` (1.5k L): poll loop with bounded backoff on `getUpdates` errors, unauthorized commands logged + refused, no panic sites after F-7-4.
- `module-copy` / `module-sniper`: panic surface reviewed — only accepted mutex conventions.
- `saas-sdk` (with solana-kit src, 27.6k L): zero production panic sites.

---

## Cumulative scorecard (Rounds 3–7)

| Round | Scope | Fixes |
|-------|-------|-------|
| 4 | webhook audit branch, event cap, SHA-256, webhook signature verification (`with_metadata`→Result), `deny_reason` zero-panic, 7 poison-tolerant locks, pin comment | 7 |
| 5 | HA ownership/lease tree | 0 (clean) |
| 6 | accounting/ledger/maths tree | 1 (loud warn on undecodable journal rows) |
| 7 | execution authority, global risk, OMS/dedup, data plane, staking program, modules | 4 (deadlock, hub leak, WS stall, telegram expect) |
| **Total** | | **19 fixes** |

**Remaining unaudited surface:** effectively none of consequence — every workspace member crate plus the excluded `programs/staking-suite` has now been through a panic scan and targeted deep reads of all money/auth/concurrency paths. Recommended next action is CI validation (compile + full test suite) rather than further static rounds.
