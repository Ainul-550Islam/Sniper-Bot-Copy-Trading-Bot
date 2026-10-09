# Principal Audit — Round 2 — 2026-10-07

Repository: `Ainul-550Islam/Sniper-Bot-Copy-Trading-Bot` at commit `1d4d5d4`
(scope: continuation of AUDIT-ROUND-1-2026-10-05; Rust/Solana/HFT hardening pass
over the websocket layer, PumpPortal feed, module task ownership, SaaS auth,
migrations and control-plane contracts).

**Verification status (read before trusting any claim below).**
`cargo`, `rustc` and `rustfmt` are not installed in this workspace. Every Rust
change in this round is a **static-review edit, uncompiled and untested**. The
frontend typecheck/build and E2E artifacts from earlier rounds remain as
previously recorded. Nothing here may be called production-verified until
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo build --workspace --all-targets` and `cargo test --workspace` pass in CI.

---

## 1. Prioritized findings closed in this round

### P0 — WebSocket supervisor races and unbounded delivery (`crates/solana-kit/src/ws.rs`)

Before: a `SolanaWs` client could spawn multiple supervisors, the outbound frame
queue was recreated per connection (losing subscribe frames queued during a
reconnect), subscription lifecycle bookkeeping raced late subscribe responses,
and a slow consumer could back-pressure the socket reader for every subscription.

Fixes applied (all in `ws.rs` unless noted):

- **Single-supervisor ownership.** `spawn()` uses a `compare_exchange` on
  `supervisor_started`; repeat calls return a non-owning `WsHandle` that can
  never shut down somebody else's connection.
- **Persistent outbound + cancellation channels.** The outbound frame queue and
  the cancellation queue live on `Shared` and are taken exactly once by the
  owning supervisor (`take_outbound` / `take_cancellations`), so frames survive
  reconnects and a second supervisor cannot steal them. Stale frames from a dead
  connection are drained at the top of each reconnect.
- **Bounded per-subscription delivery.** `deliver()` is non-blocking
  (`try_send`); a full queue drops the newest notification, counts
  `bot_ws_subscription_notifications_dropped_total`, and schedules a
  `WsMessage::Gap { dropped_messages, outage_ms, .. }` for the consumer as soon
  as it has room. Overflow gaps carry the wall-clock window since the first drop
  so backfill consumers can bound recovery.
- **Serialized lifecycle.** `subscription_state` mutex serializes subscribe,
  subscribe-response, disconnect and cancellation bookkeeping; a subscribe that
  is cancelled while its response is in flight is unsubscribed on the server
  immediately (`cancelled_requests`), never resurrected with a dead receiver.
- **Drop-safe cancellation.** `SubscriptionHandle::drop` and the
  `SubscriptionRegistration` guard enqueue cancellations through the
  supervisor's channel — no detached cleanup tasks.
- **Bounded waits.** 15 s connect timeout, 5 s timeouts on every write
  (subscribe frame, ping, pong, close, queued frame), and every blocking wait
  selects against the shutdown watch.
- **Duration narrowing.** `duration_ms()` clamps `as_millis()` (u128) before
  narrowing to u64.

### P0 — PumpPortal task ownership, cancellation and shutdown (`crates/solana-kit/src/pumpportal.rs`)

Before: `run_feed` slept through reconnect backoff without watching shutdown;
`connect_once` awaited connect/subscribe/ping/pong/send/stream reads with no
shutdown race; `start()` stored the join handle after spawning (concurrent
start/stop could race registration); `Drop` used `try_lock()` on a Tokio mutex
and could silently detach the network supervisor.

Fixes applied:

- **Cancellation-aware everything.** A `watch` stop signal races connect (10 s
  timeout), subscription writes, keepalive pings, pong replies, message sends
  and the reconnect backoff. `stop()` completes in bounded time instead of
  waiting for the next 500 ms–60 s backoff tick.
- **Race-free start/stop.** A `lifecycle` tokio mutex serializes start/stop
  transitions; the join-handle slot is a `std::sync::Mutex` touched only in
  brief non-awaiting critical sections (`lock_task_slot` recovers from
  poisoning). `start()` joins any leftover previous supervisor before spawning;
  it requires a Tokio runtime (`Handle::try_current`) and errors instead of
  panicking.
- **Abort-safe joining.** `AbortOnDropJoin` aborts the task if the joining
  future is itself cancelled; `stop()` has a 5 s deadline + 1 s abort-join
  grace. `Drop` always aborts the supervisor — the `try_lock` detach hole is
  gone.
- **Status integrity.** `SupervisorStatusGuard` clears `running`/`connected`
  even if the future is dropped mid-flight; the feed also stops when the
  consumer channel closes (`tx.is_closed()`).

### P0 — Detached Tokio tasks in the modules (task ownership / unjoined tasks)

- **`crates/module-sniper/src/detect.rs`** — `LaunchDetector::spawn` now returns
  a `LaunchFeedHandle` that owns the merged receiver **and** every feed task
  (`DetectorTasks`). `shutdown()` closes the receiver and joins all tasks under
  one bounded deadline (5 s join + 1 s abort grace); `Drop` aborts everything
  that is still running, so parent-future cancellation can never detach a
  websocket or PumpPortal forwarder. All three starters (`start_pumpportal`,
  `start_log_subscription`, `start_geyser_subscription`) return their
  `JoinHandle` instead of dropping it.
- **`crates/module-copy/src/feeds.rs`** — same pattern: `CopyFeedHandle` owns
  the stream and `FeedTasks`; the empty-wallet path no longer spawns a detached
  keep-alive sleeper (the handle holds the sender itself). `run_pumpportal`
  selects on `out.closed()` / `wait_shutdown` and calls `feed.stop()` on exit.
- **`crates/module-sniper/src/lib.rs`** — the exit sweeper is wrapped in an
  `OwnedTask` (join-with-timeout + abort-on-drop). The normal-shutdown path now
  joins it; the detector-startup failure path joins it after waiting for
  shutdown; the abort-without-await hole is closed.
- **`crates/module-copy/src/lib.rs`** — the copy exit sweeper is an
  `OwnedTask` joined on the way out of `run()`; the guard-attach failure path
  shuts the feed down before returning. `run()` and `spawn_feed()` now take and
  return the owning handle types.
- **Gap semantics** — sniper (`detect.rs`) and copy (`feeds.rs`) gap consumers
  distinguish *consumer-queue overflow* (`dropped_messages > 0`, triggers a
  tracked-wallet signature backfill bounded by the reported window) from
  *reconnect outage* (recorded, no backfill for launches; backfill for copy).

### P1 — SaaS auth/session hardening (`crates/core/src/session/token.rs`, `crates/server/src/saas/users.rs`, `rate_limit.rs`, `middleware.rs`)

- **PBKDF2 verification is cost-bounded.** `verify_password` rejects encoded
  hashes with iterations above `MAX_PBKDF2_ITERATIONS` (1.2 M), wrong salt/hash
  lengths, or strings longer than 256 bytes — a corrupted or hostile database
  row can no longer force billions of PBKDF2 rounds per login. Hashing clamps
  to the same bound.
- **Secret redaction.** `GeneratedToken` has a manual `Debug` that never prints
  the plaintext or hash (`[REDACTED]`), closing the log-leak vector from
  `{:?}` on a freshly issued token.
- **Timing parity on unknown accounts.** The login dummy hash is now a real,
  correctly-shaped 600 k-iteration PBKDF2 string (`DUMMY_PASSWORD_HASH`), so a
  missing-account login performs the same work as a wrong-password login. The
  previous `$c2FsdA$aGFzaA` constant was invalid and returned instantly.
- **Registration error hygiene.** Duplicate-email conflicts return a generic
  `registration_conflict`; storage failures return 503
  `identity_storage_unavailable` — internal repository error text no longer
  reaches clients.
- **Rate limiting.** `rate_limit::reject_sensitive_attempt` (per-identity
  hashed buckets: registration, login, login_totp, invite_accept, totp_setup,
  totp_verify) sits inside the coarser per-IP API limiter; both emit 429 with
  `Retry-After`. Buckets are capped (`max_buckets`, fail-closed eviction).
- **Session model.** Sessions are one-time plaintext tokens hashed at rest,
  12 h absolute TTL, revocation-on-password-change in one transaction
  (`update_user_and_revoke_sessions`), MFA-enrollment-only sessions scoped
  immutably to the inviting tenant, stale-MFA-proof sessions demoted to the
  TOTP setup/verify routes only. No refresh-token endpoint exists by design:
  expiry requires re-authentication; the control plane keeps the token in
  memory only (`lib/auth.ts`).

### P1 — Migrations and control-plane verification

- `crates/core/tests/migrations_apply_clean.rs` exercises the fresh-database
  contract: refuses a non-empty `POSTGRES_MIGRATION_URL`, applies all embedded
  migrations, asserts operator/tenant strategy-table separation, the
  backtest→`tenant_strategies (organization_id, id)` composite FK (including a
  live cross-tenant rejection probe), the single-pending-TOTP unique index, and
  re-executes migration 0044's runtime-identity backfill statements against
  synthetic legacy rows (migrations 0044–0046 added in earlier rounds).
  CI provisions `sniper_migrations` and sets `POSTGRES_MIGRATION_URL`.
- `apps/control-plane/e2e/mfa-authentication.spec.ts` covers the MFA login
  challenge, enrollment-only sessions, and MFA-enforced invite acceptance
  (token removed from the URL, enrollment session promotion). `error.tsx` logs
  only Next's opaque digest and offers a boundary reset — no raw error text.

### P2 — Accepted residuals (documented, not fixed)

- `execute.rs` uses `Duration::as_millis() as u64` for latency metrics (~24
  sites). This cannot panic; truncation requires >584M years of elapsed time.
  Left as-is deliberately.
- The transaction-build path fetches a fresh blockhash per build and rebuilds
  on expiry (`fresh_blockhash`, `rebuild_with_fresh_blockhash`); there is no
  shared mutable build state to race on. Blockhash caching lives in the RPC
  provider pool (failover-safe), reviewed and accepted.
- Outbound webhook delivery, live provider validation, and funded-trading
  evidence remain gated external work (see `external-gated` CI job).

---

## 2. Files changed in this round

| File | Change class |
|---|---|
| `crates/solana-kit/src/ws.rs` | Supervisor ownership, bounded delivery, overflow gaps, serialized lifecycle, timeouts, cancellation |
| `crates/solana-kit/src/pumpportal.rs` | Cancellation-aware reconnect loop, race-free start/stop, abort-safe joins, Drop safety |
| `crates/module-sniper/src/detect.rs` | `LaunchFeedHandle`/`DetectorTasks` ownership, overflow-vs-outage gap handling |
| `crates/module-sniper/src/lib.rs` | `OwnedTask` sweeper join, launch-handle shutdown on stop |
| `crates/module-copy/src/feeds.rs` | `CopyFeedHandle`/`FeedTasks` ownership, shutdown-aware forwarders, gap backfill |
| `crates/module-copy/src/lib.rs` | Owning feed/sweeper types, ordered shutdown on every exit path |
| `crates/core/src/session/token.rs` | PBKDF2 cost bounds, hash shape validation, `Debug` redaction |
| `crates/server/src/saas/users.rs` | Real dummy hash, generic conflict/503 responses |
| `crates/server/src/saas/store.rs` | `create_user` duplicate check moved under the write lock |
| `apps/control-plane/src/app/error.tsx`, `e2e/*` | Digest-only error boundary, MFA E2E coverage (earlier rounds, re-verified) |

## 3. What must run before sign-off

1. `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
2. `cargo build --workspace --all-targets && cargo test --workspace -- --test-threads=1`
   (with `POSTGRES_URL`, `POSTGRES_MIGRATION_URL`, `REDIS_URL` from the CI job)
3. `npm ci && npm run typecheck && npm run test && npm run test:e2e` in
   `apps/control-plane`
4. `./scripts/verify-migration-graph.sh` and `./scripts/export-openapi.sh --check`

Until steps 1–2 pass, the Rust edits above are **review-only hardening**, not
verified production code.
