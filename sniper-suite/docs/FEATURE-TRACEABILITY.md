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
| Collateral reads = live sizing truth (reject-over-fallback) | `crates/module-polymarket/src/collateral.rs`; `lib.rs` | `CollateralClient`, `BalanceUnavailable`, `InsufficientFunding`, freshness bound | `rpc_errors_are_errors_never_zero`, decimals/overflow tests | inside 537 | Polygon RPC + collateral address | EXECUTED |
| Order lifecycle (create→submit→result→persist→reconcile) | `crates/module-polymarket/src/{orders,strategy}.rs` | order state handling | lifecycle tests | inside 537 | CLOB API | EXECUTED (mock); funded = HUMAN ACTION |

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
