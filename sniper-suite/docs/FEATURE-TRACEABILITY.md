# Feature-to-code traceability matrix (final buyer handover)

Every advertised feature traced to actual source, verified by symbol grep on
the final tree (2026-09-19) — not copied from documentation. Verification
method: for each row the file exists, the named function/type exists in it,
and the named test target exists; all listed tests are inside the executed
suites (workspace 537/537, staking host 71/71, validator e2e 3/3 — see
`docs/EVIDENCE-INDEX.md` §Buyer-hardening for logs/hashes).

Status vocabulary: **EXECUTED** = ran green in the recorded evidence runs.
Nothing here is listed merely because a document mentions it.

## MODULE 1 — SNIPER (pump.fun)

| Feature | Source file | Function / type | Unit test | Integration/e2e | Deployment dependency | Status |
|---|---|---|---|---|---|---|
| Launch detection (PumpPortal WS, Geyser `transactionSubscribe`, poll fallback) | `crates/module-sniper/src/detect.rs`; `crates/solana-kit/src/{pumpportal,events,ws}.rs` | feed enums + `LaunchDetector` | in-crate feed tests | `tests/detect_feed.rs`, `tests/geyser_detect.rs`, `crates/solana-kit/tests/mock_pumpportal.rs` | WS/RPC endpoints (buyer-supplied) | EXECUTED (mocks); real provider = buyer config |
| Static screening (lists) → risk entry gate | `crates/module-sniper/src/entry.rs`; `crates/core/src/risk.rs` | `consider_launch`, `check_launch_with_lists`, `check_entry` | risk unit tests | inside 537 | risk config | EXECUTED |
| Bonding-curve buy (pump) | `crates/solana-kit/src/pump.rs`; `crates/module-sniper/src/entry.rs` | instruction builders + `max_sol_cost` | pump builder tests | `latency_bench` simulate leg (devnet, executed) | funded wallet for live | EXECUTED (build+simulate); funded = HUMAN ACTION |
| Exit routing (PumpSwap / Raydium / Jupiter) | `crates/module-sniper/src/exit.rs`; `crates/solana-kit/src/{pumpswap,raydium,jupiter}.rs` | exit router + venue builders | builder/layout tests | inside 537 | venue program ids (constants, verified on-chain earlier) | EXECUTED |
| Live/paper balance separation (fail-closed sizing) | `crates/module-sniper/src/lib.rs` | `available_sol`, `sol_balance_fallback`, `exec_policy` | fallback-rule unit tests (paper-only) | inside 537 | — | EXECUTED |
| Account-layout learning after confirmed live buys | `crates/solana-kit/src/layout.rs` | `LayoutStore` | layout tests | inside 537 | live confirmations | EXECUTED (unit); live learning = buyer op |

## MODULE 2 — COPY TRADING

| Feature | Source file | Function / type | Unit test | Integration/e2e | Deployment dependency | Status |
|---|---|---|---|---|---|---|
| Wallet trade feeds (tx decode → trade events) | `crates/module-copy/src/feeds.rs`; `crates/solana-kit/src/decode.rs` | feed pipeline + decoder | decoder tests | `tests/copy_feed.rs`, `tests/geyser_feed.rs` | RPC/Geyser | EXECUTED (mocks) |
| Mirror engine (proportional sizing, failed-tx skip, poll fallback) | `crates/module-copy/src/mirror.rs` | mirror logic | mirror unit tests | inside 537 | tracked wallets config | EXECUTED |
| Copy exit handling | `crates/module-copy/src/exit.rs` | exit path | exit tests | inside 537 | — | EXECUTED |
| Two-replica safety (claims/leases, no double-mirror) | `crates/core/src/{ownership,redis_ownership}.rs`; `crates/core/src/db/claims.rs` | claim store + fencing | ownership unit tests | `crates/core/tests/distributed_integration.rs` (4/4), `crates/module-copy/tests/two_replica_mirror.rs` (1/1, two real processes) | PG + Redis | EXECUTED vs real services |

## MODULE 3 — POLYMARKET

| Feature | Source file | Function / type | Unit test | Integration/e2e | Deployment dependency | Status |
|---|---|---|---|---|---|---|
| Gamma market discovery | `crates/module-polymarket/src/gamma.rs` | Gamma client | mock HTTP tests | `tests/mock_clob_gamma.rs` | Gamma API (buyer keys if rate-limited) | EXECUTED (mock) |
| CLOB REST + WS (L1/L2 auth headers) | `crates/module-polymarket/src/{clob,ws,auth}.rs` | CLOB client + auth | auth/wire tests | `tests/mock_clob_gamma.rs` | CLOB API + funder key | EXECUTED (mock); live = HUMAN ACTION |
| EIP-712 v2 order signing (11-field Order, type-3 wrap) | `crates/module-polymarket/src/eip712.rs` | domain/type hashes, `ORDER_TYPE` | signing tests vs known vectors | inside 537 | funder private key (live only) | EXECUTED |
| CTF ERC-1155 position balance | `crates/module-polymarket/src/ctf.rs` | `CtfClient.balance_of` | mock-RPC wire tests | inside 537 | Polygon RPC | EXECUTED (mock) |
| Collateral reads = live sizing truth (reject-over-fallback) | `crates/module-polymarket/src/collateral.rs`; `src/funding.rs` | `CollateralClient`, `BalanceUnavailable`, `InsufficientFunding`, freshness bound | `rpc_errors_are_errors_never_zero`, decimals/overflow tests | inside 537 | Polygon RPC + collateral address | EXECUTED |
| Order lifecycle (create→submit→result→persist→reconcile) | `crates/module-polymarket/src/{orders,pipeline,lifecycle,reconcile,recovery}.rs` | staged pipeline, `TrackedOrder` state machine, `apply_observation`, `reconcile_once`, `recover_after_restart` | lifecycle tests | inside 537 | CLOB API | EXECUTED (mock); funded = HUMAN ACTION |

## MODULE 4 — STAKING / TOKEN (on-chain program)

| Feature | Source file | Function / type | Unit test | Integration/e2e | Deployment dependency | Status |
|---|---|---|---|---|---|---|
| Genesis init (config, caps, latched one-shot mint) | `programs/staking-suite/src/processor.rs` | genesis handler, `GenesisAlreadyDone` (6026) | host tests (71) | validator e2e `governance` | program keypair (buyer) | EXECUTED on real BPF VM |
| IMMUTABLE max-supply cap on every mint (zero/one-over/exact boundaries) | `processor.rs` | `fits_under_cap`, `supply_headroom` | host tests | validator e2e `max_supply_cap_and_metadata` | — | EXECUTED |
| Stake → per-second reward accrual → claim → unstake (checked arithmetic) | `programs/staking-suite/src/state.rs`; `processor.rs` | reward math (`checked_*`/`saturating_*`, `overflow-checks = true`) | host tests | validator e2e `funded lifecycle` | — | EXECUTED incl. funded money flow |
| Zero-headroom safe claim; timelock [0,30d]; fee ≤10%; reward ≤100% APR | `state.rs`, `processor.rs`, `error.rs` | bound validations, custom errors 6000+ | host tests | e2e assertions | — | EXECUTED |
| Token metadata (mpl `CreateMetadataAccountV3` = discriminant 33, one-shot + replay rejection) | `processor.rs` | `MPL_CREATE_METADATA_ACCOUNTS_V3` | host tests | e2e vs REAL mainnet-cloned mpl program | — | EXECUTED (defect D1 found+fixed here) |
| Two-step admin transfer, pause-deposits, vault/fee-treasury PDAs | `processor.rs`, `lib.rs` | authority checks, PDA derivation | host tests | e2e governance/authority | — | EXECUTED |
| Program identity tooling (verify/set-id/deploy guards) | `scripts/staking-identity.sh` | `cmd_verify`, `cmd_set_id`, `cmd_deploy` | — (script; guards exercised) | `verify` re-run at handover (`evidence/phase7-identity-verify.log`) | buyer keypair for real id | EXECUTED (verify); deploy = HUMAN ACTION |

## MODULE 5 — TELEGRAM CONTROL

| Feature | Source file | Function / type | Unit test | Integration/e2e | Deployment dependency | Status |
|---|---|---|---|---|---|---|
| Deny-by-default chat allowlist + roles (owner/admin) | `crates/module-telegram/src/{commands,api}.rs` | role checks | command/role tests | inside 537 | bot token + owner chat id (buyer) | EXECUTED (unit); live bot = HUMAN ACTION |
| Mutating + live-mode commands owner-only | `commands.rs` | authorization gates | authz tests | inside 537 | — | EXECUTED |
| Bot-token redaction in every error path | `api.rs` | redaction | `error_strings_never_contain_the_bot_token` | inside 537 | — | EXECUTED |
| Alerts | `crates/module-telegram/src/alerts.rs` | alert sender | alert tests | inside 537 | bot token | EXECUTED (unit) |

## CORE

| Feature | Source file | Function / type | Test | Integration | Dependency | Status |
|---|---|---|---|---|---|---|
| Config (hot-reload, validation) | `crates/core/src/config.rs` | `AppConfig` | config tests | inside 537 | config.toml | EXECUTED |
| Shared state + runtime flags + kill switch | `crates/core/src/state.rs` | `AppState`, `may_broadcast` | state tests | db_integration (flags persisted) | — | EXECUTED |
| Risk engine (entry/exit/launch + GlobalRiskOracle tighten-only) | `crates/core/src/risk.rs` | `check_entry/check_exit/check_launch_with_lists` | risk unit tests | distributed_integration (oracle sync) | risk config | EXECUTED |
| OMS (order state machine, idempotency keys, Unknown/Reconciled) | `crates/core/src/oms.rs` | OMS types | oms tests | db_integration | PG | EXECUTED |
| Dedup (Redis accelerator + PG-authoritative) | `crates/core/src/dedup.rs` | dedup claims | dedup tests | redis_integration (10/10) | PG + Redis | EXECUTED |
| Reconciliation queue + sweeps | `crates/core/src/reconciliation.rs` | reconciler | recon tests | db_integration; `recon_crash_e2e` (historical, pre-hardening source — labeled) | PG | EXECUTED (current-source: db_integration) |
| Startup recovery (positions, Unknown re-registration, reconcile gate) | `crates/core/src/recovery.rs` | recovery pass | recovery tests | db_integration | PG | EXECUTED |
| Audit hash chain (append-only, tamper-evident) | `crates/core/src/audit.rs` | chain append/verify | chain tests (tamper/reorder/duplicate/concurrent) | db_integration 23/23 | PG | EXECUTED |
| Storage journal (JSONL, rotation, corrupt-line recovery) | `crates/core/src/storage.rs` | journal | `crates/core/tests/storage_lifecycle.rs` | inside 537 | disk | EXECUTED |
| Auth (API keys, roles) | `crates/core/src/auth.rs` | role/key checks | auth tests | inside 537 | API key env | EXECUTED |
| Lifecycle (ordered startup/shutdown drain) | `crates/core/src/lifecycle.rs` | lifecycle | lifecycle tests | phase8b clean-shutdown log | — | EXECUTED (incl. live smoke) |
| Math/models/events (lamports, ticks, domain events) | `crates/core/src/{maths,models,events}.rs` | helpers | unit tests | inside 537 | — | EXECUTED |

## GLOBAL RISK + ACCOUNTING (TASK 5)

| Feature | Source file | Function / type | Test | Integration | Dependency | Status |
|---|---|---|---|---|---|---|
| One global risk decision in front of every module check (portfolio / wallet / venue / strategy / asset exposure, open positions, order notional, daily loss, drawdown, 14 deterministic reasons) | `crates/core/src/global_risk/{engine,decision}.rs`; `crates/core/src/risk.rs` (step 2b) | `GlobalRiskEngine::decide`, `GlobalRejectReason`, `RiskCode::Global*` | `global_risk/*` unit tests | `crates/core/tests/global_risk_accounting.rs` (18) | `[global_risk]` config + reference rates | EXECUTED (offline) |
| Venue / strategy kill switches (config-pinned + runtime, durable, audited) | `crates/core/src/global_risk/kill_switch.rs`; `crates/server/src/api.rs` (`/api/risk/kill-switch`) | `KillSwitches`, `KillScope` | kill_switch unit tests | `global_risk_accounting.rs`; api tests | PG for durability (memory otherwise) | EXECUTED (offline) |
| Typed accounting events + ONE idempotency identity | `crates/core/src/accounting/event.rs` | `AccountingEvent::event_id`, `fill_event_for_trade` | event unit tests | `same_financial_event_twice_yields_one_ledger_one_position_one_pnl_effect` | — | EXECUTED |
| Append-only double-entry ledger (fills, fees, settlements, deposits, withdrawals, transfers, funding, corrections) | `crates/core/src/accounting/{ledger,posting}.rs` | `GlobalLedger::submit`, `expand` | ledger / posting unit tests (incl. concurrency) | `global_risk_accounting.rs` | PG (0015) or memory | EXECUTED |
| Position aggregation + portfolio view (exposure, realized / unrealized / fees / net, utilization; per venue / wallet / strategy / asset / module) | `crates/core/src/accounting/{book,view}.rs` | `PositionBook`, `PortfolioView` | book / view unit tests | `positions_aggregate_across_modules_with_pnl_and_fees` | reference rates | EXECUTED |
| Accounting reconciliation across the four record layers — OMS orders → fills → ledger → positions (8 finding kinds, never repairs) | `crates/core/src/accounting/reconcile.rs`; `crates/server/src/accounting.rs` (maintenance loop) | `reconcile`, `GlobalLedger::reconcile`, `ReconInputs::orders` | reconcile unit tests (incl. `filled_orders_without_a_ledger_event_are_reported`, `a_filled_order_matched_by_correlation_signature_or_trade_is_in_sync`, `a_ledger_event_explained_only_by_its_order_is_not_an_orphan`) | `ledger_and_positions_reconcile_and_mismatches_are_reported_not_repaired`, `the_order_layer_is_reconciled_against_the_ledger`, `orphan_events_are_reported` | — | EXECUTED |
| Restart recovery (rebuild book + risk state from the journal, gaps reported, replay-safe) | `crates/core/src/accounting/recovery.rs`; `crates/server/src/accounting.rs` | `GlobalLedger::recover` | recovery unit tests | `restart_rebuilds_risk_state_from_the_journal_and_refuses_replays`, `ledger_replay_of_the_whole_journal_is_idempotent` | PG (0015) | EXECUTED (offline) |
| Module integration (explicit typed events at every fill site) | `module-sniper/src/{entry,exit}.rs`, `module-copy/src/{mirror,exit}.rs`, `module-polymarket/src/lifecycle.rs` | `state.ledger().submit(fill_event_for_trade(..))` | — | `module-sniper/tests/concurrency.rs`, `module-copy/tests/dedup_ordering.rs`, `module-polymarket/tests/order_pipeline.rs` | — | EXECUTED (paper) |
| Durable tables (migration 0015) + repository | `crates/core/migrations/0015_global_risk_accounting.sql`; `crates/core/src/db/accounting.rs` | `AccountingRepo` | — | `db_integration.rs::global_ledger_repo_round_trips_and_is_idempotent` (gated on `POSTGRES_URL`) | PostgreSQL | NOT EXECUTED in the sandbox (no PostgreSQL) |
| Metrics + audit (`global_*` series; `global.*` audit actions) | `crates/core/src/{accounting,global_risk}/{metrics,audit}.rs` | helpers | inside unit tests | `global_risk_accounting.rs` audit assertions | — | EXECUTED |
| API (`/api/accounting/portfolio`, `/api/accounting/events` GET/POST, `/api/accounting/findings`, `/api/risk/global`, `/api/risk/kill-switch`) | `crates/server/src/api.rs` | handlers | — | api tests (`accounting_routes_expose_portfolio_events_and_findings`, `kill_switch_scopes_are_operator_actions_and_audited`) | RBAC keys | EXECUTED |

## HA / CRASH RECOVERY / DISTRIBUTED RELIABILITY (TASK 6)

| Feature | Source file | Function / type | Test | Integration | Dependency | Status |
|---|---|---|---|---|---|---|
| Worker identity, registration, heartbeat, generations, stale detection | `crates/core/src/ha/worker.rs`; `crates/core/src/db/ha.rs`; `crates/server/src/ha.rs` | `WorkerRegistration`, `WorkerState` (9), `WorkerHealth`, `HaRepo::register_worker/heartbeat` | worker unit tests | `worker_registration_generations_and_heartbeat_expiry` | PG (0016) or memory | EXECUTED (offline) |
| Singleton role leases with fencing tokens; safe takeover; stale worker cannot mutate | `crates/core/src/ha/lease.rs`, `runtime.rs`; `crates/core/src/db/ha.rs` | `LeaseRole`, `Lease`, `FenceError`, `HaRuntime::{acquire,renew,fence,guarded,release}`, one-statement `acquire_lease` | lease + store unit tests | `lease_acquire_renew_loss_takeover_and_fencing`, `two_workers_race_for_one_lease_exactly_one_wins`, `store_failure_is_never_ownership` | PG (0016) or memory | EXECUTED (offline) |
| Distributed idempotency proof over TASK 1–5 mechanisms | `crates/core/src/{ownership,oms,accounting}`; TASK 6 tests | claims + `idempotency_key` + `event_id` | — | `critical_proof_same_event_two_workers_one_execution_one_intent_one_ledger_effect`, `two_workers_cannot_execute_create_or_book_the_same_thing_twice` | — | EXECUTED |
| Durable feed cursors, duplicate suppression, gap detection, replay / backfill | `crates/core/src/ha/cursor.rs`, `runtime.rs` | `FeedCursor::offer/offer_token/rewind_to`, `FeedGap`, `GapStatus` | cursor unit tests | `feed_cursor_recovery_gap_detection_and_replay` | PG (0016) or memory | EXECUTED (offline) |
| Cursors wired into the money-bearing feeds: copy poll loop resumes from the durable signature, Polymarket user channel continues its delivery numbering and reports skipped deliveries | `crates/module-copy/src/feeds.rs::run_poll`; `crates/module-polymarket/src/ws.rs::run_user_feed` | `HaRuntime::{offer_token, offer, cursor}` | — | `feed_wiring_shapes_survive_a_restart` | PG (0016) or memory | EXECUTED (offline) |
| Cluster singletons actually leased: reconciliation sweep, hourly retention, position re-verification, accounting maintenance | `crates/server/src/main.rs` + `crates/server/src/ha.rs::LeasedWorker` | `LeaseRole::{Reconciliation, Recovery, StateSync, AccountingMaintenance}` | — | lease tests + `after_takeover_…` | PG for clustered modes | EXECUTED (offline) |
| Crash-boundary → recovery matrix (12 boundaries, 8 order situations, one action each, never resubmit) | `crates/core/src/ha/recovery_plan.rs` | `CrashBoundary`, `plan_order_recovery`, `OrderRecoveryAction` | recovery_plan unit tests (full matrix) | `every_crash_boundary_has_one_deterministic_outcome` | — | EXECUTED |
| Restart rebuild of ledger / positions / risk state without double booking | `crates/core/src/accounting/recovery.rs`; `crates/server/src/{accounting,ha}.rs` | `GlobalLedger::recover`, `register_and_recover` | — | `restart_rebuilds_ledger_positions_and_risk_state_without_double_booking`, `partial_fill_survives_a_restart_and_keeps_the_booked_part` | PG or memory | EXECUTED |
| Reconciliation after failover (reports, never repairs) | `crates/core/src/accounting/reconcile.rs`; `crates/server/src/ha.rs` | `GlobalLedger::reconcile` under a fenced lease | — | `after_takeover_the_new_owner_reconciles_and_reports_findings` | — | EXECUTED |
| HA modes: single / active-passive / active-active | `crates/core/src/ha/worker.rs`; `crates/core/src/config.rs` (`[ha].mode`, validation) | `HaMode`, `validate_ha` | 4 config tests | `single_worker_mode_needs_no_lease_to_be_ready`, `active_passive_standby_is_not_ready_until_it_owns_the_role` | PG for clustered modes | EXECUTED (offline) |
| Readiness that never lies (state, recovery, required leases re-verified, dependencies) | `crates/core/src/ha/runtime.rs`; `crates/server/src/obs.rs` | `Readiness`, `NotReadyReason`, `refresh_readiness`, the `worker` health component | server probe test | `readiness_fails_on_lost_lease_pending_recovery_and_unhealthy_dependency` | — | EXECUTED |
| Graceful shutdown: stop, persist cursors, release leases, audit, stopped | `crates/core/src/ha/runtime.rs`; `crates/server/src/{ha,main}.rs` | `HaRuntime::shutdown`, the `ha-drain` phase | — | `graceful_shutdown_persists_cursors_releases_leases_and_stops` | — | EXECUTED |
| Durable tables (migration 0016) + repository | `crates/core/migrations/0016_ha_workers_leases_cursors.sql`; `crates/core/src/db/ha.rs` | `HaRepo` | — | gated on `POSTGRES_URL` via `DbHaStore` | PostgreSQL | NOT EXECUTED in the sandbox (no PostgreSQL) |
| Metrics + audit (`ha_*` series; `ha.*` actions) | `crates/core/src/ha/{metrics,audit}.rs` | helpers | inside unit tests | audit assertions in the suite | — | EXECUTED |
| API (`GET /api/ha`) | `crates/server/src/api.rs` | `ha_status` | — | RBAC covered by the shared matrix test | RBAC keys | EXECUTED |

## SOLANA KIT

| Feature | Source file | Function / type | Test | Integration | Dependency | Status |
|---|---|---|---|---|---|---|
| RPC client with failover rotation | `crates/solana-kit/src/rpc.rs` | `Rpc`, rotation | failover tests | inside 537; devnet reads executed (bench) | RPC URLs | EXECUTED |
| Tx construction + simulate-first execution + confirmation classification | `crates/solana-kit/src/{tx,execute}.rs` | `Executor`, `SendFailure`, `broadcast_signature` | executor tests + mock JSON-RPC fan-out pair | `latency_bench` simulate leg (devnet, executed); `devnet_e2e` (historical, labeled) | RPC | EXECUTED (simulate); funded broadcast = HUMAN ACTION |
| Signer registry (local-only; vault/kms/hsm fail startup) | `crates/solana-kit/src/signer.rs` | signer backend enum | rejection tests | inside 537 | keypair file (buyer) | EXECUTED |
| Venue clients (pump/pumpswap/raydium/jupiter) + constants + tokens | `crates/solana-kit/src/{pump,pumpswap,raydium,jupiter,consts,tokens}.rs` | builders | builder tests | inside 537 | venue programs | EXECUTED |
| WS event plumbing (PumpPortal/Geyser) + account cache | `crates/solana-kit/src/{ws,pumpportal,events,cache}.rs` | feeds/cache | mock WS tests | `mock_pumpportal.rs` | endpoints | EXECUTED (mocks) |
| Decode (confirmed-tx → swap facts) | `crates/solana-kit/src/decode.rs` | decoder | decoder tests | inside 537 | — | EXECUTED |

## SERVER / DATABASE / DEPLOYMENT / OBSERVABILITY / RECOVERY

| Feature | Source file | Function / type | Test | Integration | Dependency | Status |
|---|---|---|---|---|---|---|
| REST API (28 endpoints, x-api-key auth) | `crates/server/src/api.rs` | routers | api tests | phase8b endpoint smoke (executed) | API_KEY env | EXECUTED |
| WebSocket event feed (authenticated) | `crates/server/src/ws.rs` | ws route | ws tests | inside 537 | — | EXECUTED |
| Dashboard (single-file HTML, no CDN) | `crates/server/src/dashboard.rs` | inline HTML | dashboard test | phase8b (served) | — | EXECUTED |
| Persistence bridge + recon API | `crates/server/src/{persist,recon,obs}.rs` | glue | unit tests | db_integration | PG/Redis | EXECUTED |
| Health/readiness/metrics | `crates/core/src/obs/{health,metrics}.rs` | `/health`,`/ready`,`bot_*` | obs tests | phase8b smoke (executed: /ready 200, 4 components, `bot_health_ready 1`) | — | EXECUTED |
| Migrations 0001–0011 (forward-only, checksummed) | `crates/core/migrations/*.sql` | sqlx `migrate!` | — | db_integration + phase8 restore (11/11 success on restored DB) | PG | EXECUTED |
| Docker/compose packaging | `Dockerfile`, `docker-compose.yml`, `.dockerignore` | multi-stage non-root image | — | **BLOCKED (no daemon in vendor sandbox)**; CI docker job wired | Docker (buyer) | NOT RUN (vendor); buyer step 19 |
| CI pipeline | `.github/workflows/ci.yml` | 4 jobs | — | **BLOCKED (no runner)**; 1:1 local map in `docs/CI-LOCAL-EQUIVALENCE.md` | GitHub (buyer) | NOT RUN (vendor); buyer step 20 |
| Release gates | `scripts/{release-check,verify-delivery}.sh` | 20-gate + 7-check | — | final runs 20/0/0 + 7/0 (executed) | toolchain/services | EXECUTED |
| Startup recovery + reconcile gating | `crates/core/src/recovery.rs`; `crates/server/src/main.rs` | recovery spawn gate | recovery tests | db_integration | PG | EXECUTED |

**Row-count note:** every row above was symbol-verified against the final tree
on 2026-09-19 (file exists + named symbol grep + named test target exists).
Rows marked NOT RUN are environment-blocked vendor-side and are wired for
buyer execution — never counted as passes.
