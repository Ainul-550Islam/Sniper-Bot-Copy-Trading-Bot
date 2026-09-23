# Repository map — sniper-suite 0.1.0

The actual delivered tree (no invented directories). Generated from the real
file listing; counts are exact. Annotated by role. The tree below is the
**current** one after TASK 1–7A and the SaaS durability completion pass; the
historical count table of the frozen delivery package is kept at the end for
provenance.

```
sniper-suite/
│
│  ── root metadata & release identity ──────────────────────────────
├─ VERSION                        release identity: 0.1.0 (gated vs Cargo.toml + manifest)
├─ LICENSE                        MIT (copyright holder = documented transfer placeholder)
├─ SECURITY.md                    vulnerability-reporting policy + explicit "no external audit"
├─ CHANGELOG.md                   Keep-a-Changelog history (0.1.0 + Unreleased doc passes)
├─ AUDIT.md                       historical audit/build evidence trail (27 dated sections)
├─ README.md                      product overview, quick start, config/API/observability reference
├─ release-manifest.json          machine-readable delivery manifest (versions, counts, statuses)
├─ Cargo.toml                     workspace root: 7 members, [workspace.dependencies] pins
├─ Cargo.lock                     app dependency lockfile (706 packages)
├─ rust-toolchain.toml            pinned Rust 1.98.1 + rustfmt + clippy
├─ deny.toml                      cargo-deny policy (advisories/bans/licenses/sources)
├─ .cargo/
│  └─ audit.toml                  cargo-audit config (app workspace)
│
│  ── deployment assets ─────────────────────────────────────────────
├─ Dockerfile                     multi-stage, non-root, healthcheck, rust:1.98.1-bookworm
├─ docker-compose.yml             bot + postgres:16-alpine + redis:7-alpine, healthcheck-gated
├─ .dockerignore                  build-context exclusions
├─ .env.template                  compose env template (copy to .env; .env never committed)
├─ config.toml.example            annotated reference config (every key, every section)
├─ .gitignore                     repo hygiene (target/, .env, data/, logs, keypairs…)
│
│  ── release tooling & CI ──────────────────────────────────────────
├─ scripts/
│  ├─ release-check.sh            20-gate local release validation (fmt→tests→staking→audit/deny)
│  ├─ verify-delivery.sh          delivery-bundle integrity check (docs, versions, counts, hygiene)
│  └─ staking-identity.sh         program-id identity tooling (show/verify/set-id/deploy; refuses
│                                 keypair≠declare_id and placeholder ids on public clusters)
├─ .github/
│  └─ workflows/
│     └─ ci.yml                   4 jobs: app workspace / staking program / security / docker
│
│  ── application workspace (7 crates) ─────────────────────────────
├─ crates/
│  ├─ core/                       bot-core — shared kernel
│  │  ├─ Cargo.toml
│  │  ├─ migrations/              18 forward-only PostgreSQL migrations (0001–0018):
│  │  │                           bootstrap; orders/executions; positions/trades;
│  │  │                           dedup/risk/audit; reconciliation; tx attribution;
│  │  │                           intent journal; intent claim kind; execution claims;
│  │  │                           runtime flags; execution_claim_events lineage;
│  │  │                           0012 execution_lifecycle (+ events) (TASK 1);
│  │  │                           0013 copy_leaders / copy_leader_events / copy_events /
│  │  │                           copy_links (TASK 3); 0014 poly_signals / poly_orders /
│  │  │                           poly_fills / poly_recon_findings (TASK 4);
│  │  │                           0015 ledger_events / ledger_postings / global_positions /
│  │  │                           global_risk_decisions / kill_switches (+ events) /
│  │  │                           accounting_recon_findings (TASK 5);
│  │  │                           0016 ha_workers (+ events) / ha_leases (+ events,
│  │  │                           fencing generations) / ha_cursors / ha_feed_gaps /
│  │  │                           ha_recovery_records (TASK 6); 0017 normalized SaaS
│  │  │                           control-plane schema; 0018 authoritative SaaS runtime
│  │  │                           record projection (TASK 7A durability)
│  │  ├─ src/
│  │  │  ├─ lib.rs                crate surface
│  │  │  ├─ config.rs             typed config, validation, env overrides, deny_unknown_fields
│  │  │  ├─ error.rs              error model (classification, redaction)
│  │  │  ├─ events.rs             in-process event bus (AppEvent kinds)
│  │  │  ├─ state.rs              authoritative AppState + counters
│  │  │  ├─ models.rs             domain models (Position, Trade, Order, …)
│  │  │  ├─ maths.rs              numeric helpers
│  │  │  ├─ lifecycle.rs          module lifecycle/heartbeat
│  │  │  ├─ risk.rs               pre-trade risk engine (generic + sniper_* + copy_* + poly_* controls; step 2b = the TASK 5 global decision)
│  │  │  ├─ global_risk/          TASK 5 global risk engine — one concern per file:
│  │  │  │  ├─ mod.rs             module map + re-exports
│  │  │  │  ├─ decision.rs        GlobalRiskRequest, 14 GlobalRejectReasons, GlobalRiskDecision + snapshot
│  │  │  │  ├─ engine.rs          GlobalRiskEngine: ordered checks over the ledger's book
│  │  │  │  ├─ kill_switch.rs     per-venue / per-strategy switches (config-pinned or operator-engaged)
│  │  │  │  ├─ store.rs           RiskStore contract + MemoryRiskStore
│  │  │  │  ├─ metrics.rs         global_risk_* series
│  │  │  │  └─ audit.rs           global.risk.* / global.kill_switch.* audit actions
│  │  │  ├─ accounting/           TASK 5 global ledger — one concern per file:
│  │  │  │  ├─ mod.rs             module map + re-exports
│  │  │  │  ├─ event.rs           AccountingEvent, EventKind, the deterministic event_id
│  │  │  │  ├─ posting.rs         double-entry postings / balanced Entry expansion
│  │  │  │  ├─ book.rs            PositionBook aggregation (average cost, realized, fees, exposure)
│  │  │  │  ├─ ledger.rs          GlobalLedger: the single mutation door, idempotency, journal, pending
│  │  │  │  ├─ view.rs            PortfolioView in reference units
│  │  │  │  ├─ store.rs           LedgerStore contract + MemoryLedgerStore
│  │  │  │  ├─ reconcile.rs       orders → fills → ledger → positions, 8 AccountingFinding kinds
│  │  │  │  ├─ recovery.rs        restart rebuild from the journal, gaps reported
│  │  │  │  ├─ metrics.rs         global_ledger_* / global_portfolio_* series
│  │  │  │  └─ audit.rs           global.ledger.* / global.position.* / global.recon.* / global.recovery.*
│  │  │  ├─ execution.rs          ExecutionLedger — one venue-agnostic tx lifecycle state machine for every money-moving attempt (TASK 1)
│  │  │  ├─ oms.rs                order state machine + idempotency keys
│  │  │  ├─ dedup.rs              3-level restart-safe dedup (memory/Redis/PG)
│  │  │  ├─ auth.rs               deployment-key API/RBAC authorization
│  │  │  ├─ authorization/        tenant authorization context, decisions, ordered gate
│  │  │  ├─ tenant/               users, organizations, typed tenant/user identifiers
│  │  │  ├─ membership/           tenant roles, statuses, permissions
│  │  │  ├─ session/              password/token handling and durable session records
│  │  │  ├─ billing/              plans, subscriptions, entitlements, usage metering
│  │  │  ├─ provisioning/         durable signup/provisioning state machine
│  │  │  ├─ audit.rs              hash-chained append-only audit trail
│  │  │  ├─ ownership.rs          PER-EXECUTION claims/leases/epochs/fencing + GlobalRiskOracle
│  │  │  ├─ ha/                   TASK 6 high availability — one concern per file:
│  │  │  │  ├─ mod.rs             module map + re-exports
│  │  │  │  ├─ worker.rs          worker identity, registration, heartbeat, 9-state machine, HA modes
│  │  │  │  ├─ lease.rs           SINGLETON role leases, fencing tokens, FenceError
│  │  │  │  ├─ cursor.rs          durable feed cursors, duplicate suppression, gap detection, replay
│  │  │  │  ├─ recovery_plan.rs   crash-boundary + order-recovery matrices (pure, 12 × 8)
│  │  │  │  ├─ store.rs           HaStore contract + MemoryHaStore
│  │  │  │  ├─ runtime.rs         HaRuntime: leases, cursors, readiness, graceful shutdown
│  │  │  │  ├─ metrics.rs         ha_* series
│  │  │  │  └─ audit.rs           ha.* audit actions
│  │  │  ├─ redis_ownership.rs    Redis claim-store backend
│  │  │  ├─ redis_kv.rs           Redis KV (dedup L2, flags)
│  │  │  ├─ reconciliation.rs     intent → venue-truth resolution, ambiguity matrix
│  │  │  ├─ recovery.rs           startup replay/recovery
│  │  │  ├─ storage.rs            JSONL intent journal (rotation, corrupt-line tolerance)
│  │  │  ├─ db/
│  │  │  │  ├─ mod.rs             sqlx pool + embedded migrate!
│  │  │  │  ├─ repo.rs            repositories (orders/positions/audit append w/ advisory lock…)
│  │  │  │  ├─ claims.rs          Postgres claim store (authoritative)
│  │  │  │  ├─ execution.rs       execution-ledger repository (migration 0012)
│  │  │  │  ├─ copy.rs            copy-trading journal repository (migration 0013)
│  │  │  │  ├─ polymarket.rs      Polymarket journal repository `PolyRepo` (migration 0014)
│  │  │  │  ├─ accounting.rs      global ledger / risk repository `AccountingRepo` (migration 0015)
│  │  │  │  └─ ha.rs              HA repository `HaRepo` (migration 0016)
│  │  │  └─ obs/
│  │  │     ├─ mod.rs             observability surface
│  │  │     ├─ health.rs          health/ready registries
│  │  │     └─ metrics.rs         bot_* metrics registry (bounded labels)
│  │  └─ tests/
│  │     ├─ global_risk_accounting.rs  19 offline TASK 5 tests (limits, kill switches, idempotency, aggregation, order/ledger/position reconciliation, recovery)
│  │     ├─ ha_distributed.rs         17 offline TASK 6 tests (worker identity, leases, fencing, two-worker race, cursors/gaps/replay, crash boundaries, restart, failover, readiness, shutdown)
│  │     ├─ db_integration.rs          26 gated tests vs real PostgreSQL (incl. migrations 0012–0015)
│  │     ├─ redis_integration.rs       10 gated tests vs real Redis
│  │     ├─ distributed_integration.rs 4 gated multi-context tests
│  │     └─ storage_lifecycle.rs       journal restart/rotation/corruption tests
│  ├─ solana-kit/                 Solana integration kit
│  │  ├─ Cargo.toml
│  │  ├─ src/
│  │  │  ├─ lib.rs, consts.rs     crate surface; program IDs / address constants
│  │  │  ├─ rpc.rs                retry/failover chokepoint + broadcast fan-out
│  │  │  ├─ provider.rs           RPC provider pool: per-provider health, failover, retry policy with jittered backoff (TASK 1)
│  │  │  ├─ fees.rs               priority-fee policy: bounds / emergency limit / per-retry escalation, adaptive selection, FEE_LIMIT budget
│  │  │  ├─ ws.rs                 WS supervision + resubscribe
│  │  │  ├─ events.rs             Yellowstone-style Geyser transactionSubscribe client
│  │  │  ├─ pumpportal.rs         PumpPortal WS client
│  │  │  ├─ cache.rs              TTL + FIFO-bounded warm account cache
│  │  │  ├─ pump.rs               pump.fun bonding-curve instruction builders (incl. v2)
│  │  │  ├─ pumpswap.rs           PumpSwap AMM builders
│  │  │  ├─ raydium.rs            Raydium AMM SwapBaseIn / SwapBaseInV2 builders
│  │  │  ├─ jupiter.rs            Jupiter exit routing
│  │  │  ├─ layout.rs             on-chain account layout parsing
│  │  │  ├─ tokens.rs             SPL token / ATA helpers
│  │  │  ├─ decode.rs             transaction/swap decoder
│  │  │  ├─ tx.rs                 transaction assembly + blockhash
│  │  │  ├─ execute.rs            executor: simulate-first, send, confirm
│  │  │  └─ signer.rs             TransactionSigner + SignerRegistry (multi-signer safe)
│  │  └─ tests/
│  │     ├─ mock_pumpportal.rs    PumpPortal WS mock harness
│  │     ├─ devnet_e2e.rs         network-gated devnet e2e (E2E_NETWORK)
│  │     ├─ latency_bench.rs      network-gated latency benchmarks
│  │     └─ recon_crash_e2e.rs    crash-recovery e2e vs local validator
│  ├─ module-sniper/              Module 1 — launch sniper (pump.fun / PumpSwap / Raydium AMM v4)
│  │  ├─ Cargo.toml
│  │  ├─ src/ (lib.rs, detect.rs, event.rs, market.rs, gates.rs, slippage.rs,
│  │  │        pipeline.rs, entry.rs, exit.rs, replay.rs)         — docs/SNIPER-ENGINE.md
│  │  ├─ fixtures/replay/ (19 JSON replay fixtures 01…16b: valid launch, duplicate,
│  │  │        stale, malformed, liquidity, slippage, price impact, risk rejection,
│  │  │        successful intent, failed execution, reconnect gap, PumpSwap, Raydium,
│  │  │        token state / concentration, strict gates, invalid route,
│  │  │        strategy disabled / exposure, fee budget on / off)
│  │  └─ tests/ (common/mod.rs mock node, detect_feed.rs, geyser_detect.rs,
│  │            pipeline.rs, replay.rs, exit_sweeper.rs, failure_injection.rs,
│  │            concurrency.rs, property.rs)
│  ├─ module-copy/                Module 2 — copy trading             — docs/COPY-TRADING-*.md
│  │  ├─ Cargo.toml
│  │  ├─ src/ (lib.rs, feeds.rs, event.rs, event_dedup.rs, event_ordering.rs,
│  │  │        leader.rs, policy.rs, sizing.rs, intent.rs, mirror.rs, exit.rs,
│  │  │        reconcile.rs, recovery.rs, metrics.rs, audit.rs)
│  │  └─ tests/ (common/mod.rs, copy_feed.rs, geyser_feed.rs, two_replica_mirror.rs,
│  │            leader_lifecycle.rs, event_pipeline.rs, dedup_ordering.rs,
│  │            policy_sizing.rs, intent_execution.rs, reconciliation.rs,
│  │            crash_recovery.rs, concurrency.rs)
│  ├─ module-polymarket/          Module 3 — Polymarket CLOB/Gamma     — docs/POLYMARKET-*.md
│  │  ├─ Cargo.toml
│  │  ├─ src/ (one concern per file, the module-copy layout:
│  │  │        lib.rs PolyBot construction + supervised run loop,
│  │  │        discover.rs Gamma discovery → quotes → verdicts → frozen signals,
│  │  │        pipeline.rs staged order pipeline (gates, the one risk decision,
│  │  │        idempotency, ownership, sign, submit), lifecycle.rs venue-order
│  │  │        tracking + fill accounting + polling + user events + cancels,
│  │  │        reconcile.rs order reconciliation, recovery.rs restart re-adoption,
│  │  │        funding.rs collateral reads + pre-broadcast funding checks,
│  │  │        store.rs PolyStore journal contract + MemoryPolyStore,
│  │  │        metrics.rs every poly_* series, audit.rs poly.* audit vocabulary,
│  │  │        venue.rs credentials / heartbeat / kill-switch cancel + flatten;
│  │  │        gamma.rs, clob.rs, ws.rs market + user channel, eip712.rs, orders.rs
│  │  │        intents / stages / state machine, auth.rs, ctf.rs, collateral.rs,
│  │  │        strategy.rs gates + verdicts, error.rs)
│  │  └─ tests/ (common/mod.rs mock CLOB/Gamma/RPC/user-WS, mock_clob_gamma.rs,
│  │            order_pipeline.rs, order_lifecycle.rs, user_ws.rs,
│  │            idempotency_concurrency.rs, reconciliation.rs, crash_recovery.rs,
│  │            strategy_sizing.rs)
│  ├─ module-telegram/            Module 5 — Telegram control
│  │  ├─ Cargo.toml
│  │  └─ src/ (lib.rs, commands.rs, alerts.rs, api.rs — token-redacted Bot API)
│  └─ server/                     sniper-suite binary — control plane + supervision
│     ├─ Cargo.toml
│     └─ src/
│        ├─ main.rs               startup/shutdown orchestration, module supervision
│        ├─ api.rs                REST routes + RBAC + rate limits + request IDs
│        ├─ ws.rs                 /api/events WebSocket feed
│        ├─ dashboard.rs          embedded HTML dashboard
│        ├─ obs.rs                probes + metrics wiring
│        ├─ persist.rs            persistence pumps (state → PostgreSQL; Solana + Polymarket order attribution)
│        ├─ accounting.rs         TASK 5 wiring: DbLedgerStore / DbRiskStore, startup recovery, maintenance pass
│        ├─ ha.rs                 TASK 6 wiring: DbHaStore, registration + recovery journalling, heartbeat loop, LeasedWorker, graceful shutdown
│        ├─ recon.rs              reconciliation tasks (venue truth) + DbCopyStore / DbPolyStore journals
│        └─ saas/                 TASK 7A HTTP boundary and durable control-plane store:
│           ├─ users.rs           register/login/profile/logout
│           ├─ organizations.rs   provisioning, membership, tenant lifecycle
│           ├─ api_keys.rs        tenant-scoped key issue/list/revoke/authentication
│           ├─ middleware.rs      tenant resolution + authorization/entitlement gates
│           ├─ store.rs           PostgreSQL-authoritative store + memory test mode
│           └─ postgres.rs        migration-0018 repository + atomic plan assignment
│
│  ── standalone on-chain program (Module 4) ────────────────────────
├─ programs/
│  └─ staking-suite/              native Solana program (own workspace root)
│     ├─ Cargo.toml               hardened release profile; MSRV 1.79 (agave platform-tools)
│     ├─ Cargo.lock               independent lockfile (580 packages)
│     ├─ .cargo/
│     │  ├─ config.toml           MSRV-aware resolver policy
│     │  └─ audit.toml            cargo-audit config (program)
│     ├─ src/
│     │  ├─ lib.rs                entrypoint, declare_id! (pre-deploy placeholder), PDAs
│     │  ├─ processor.rs          instruction processing + account validation
│     │  ├─ state.rs              Config/Stake state, reward/fee math
│     │  ├─ instruction.rs        borsh instruction (de)serialization + client builders
│     │  └─ error.rs              custom errors 6000+ (incl. GenesisAlreadyDone 6026)
│     └─ tests/
│        └─ validator_e2e.rs      STAKING_E2E-gated on-chain lifecycle (3 tests; all 3 executed + passed in the hardening pass)
│
│  ── documentation (63 files under docs/) ──────────────────────────
└─ docs/
   │  # engine passes (14, TASK 1–6):
   ├─ EXECUTION-RELIABILITY.md  SNIPER-ENGINE.md
   ├─ COPY-TRADING-ENGINE.md  COPY-TRADING-OPERATIONS.md  COPY-TRADING-RECOVERY.md
   ├─ POLYMARKET-ENGINE.md  POLYMARKET-OPERATIONS.md  POLYMARKET-RECOVERY.md
   ├─ GLOBAL-RISK.md  ACCOUNTING-LEDGER.md  RISK-OPERATIONS.md
   ├─ HA-ARCHITECTURE.md  DISTRIBUTED-OPERATIONS.md  CRASH-RECOVERY.md
   │  # engineering set (13, delivered at freeze):
   ├─ ARCHITECTURE.md  API.md  SECURITY.md  DEPLOYMENT.md  OPERATIONS.md
   ├─ MODULES.md  STAKING.md  TESTING.md  RECONCILIATION.md  DISTRIBUTED.md
   ├─ RELEASE.md  HANDOVER.md  BACKUP-RESTORE.md
   │  # buyer package (14, first documentation pass):
   ├─ BUYER-OVERVIEW.md  CAPABILITY-MATRIX.md  BUYER-DUE-DILIGENCE.md
   ├─ IP-COMPONENTS.md  THIRD-PARTY.md  BUYER-DEPLOYMENT.md
   ├─ ACCEPTANCE-CHECKLIST.md  RELEASE-NOTES-0.1.0.md  BUYER-FAQ.md
   ├─ SCOPE-BOUNDARY.md  SUPPORT-HANDOVER.md  BUYER-RISK-REGISTER.md
   ├─ TECHNICAL-DIFFERENTIATORS.md  DELIVERY-MANIFEST.md
   │  # final delivery package (9, this pass):
   ├─ FINAL-DELIVERY.md  BUYER-QUICKSTART.md  TECHNICAL-FACT-SHEET.md
   ├─ SELLER-FACT-SHEET.md  SELLING-LISTING-SOURCE.md  DEMO-RUNBOOK.md
   ├─ EVIDENCE-INDEX.md  REPOSITORY-MAP.md  ARCHIVE-CHECKLIST.md
   │  # buyer-hardening pass (3):
   ├─ LIVE-VALIDATION.md  BUYER-ACCEPTANCE-TEST.md  CI-LOCAL-EQUIVALENCE.md
   │  # final buyer-handover pass (8; BUYER-ACCEPTANCE-TEST upgraded in place):
   ├─ FORENSIC-FILE-INVENTORY.md  SOURCE-OF-TRUTH.md   # forensic cycle (2)
   ├─ FEATURE-TRACEABILITY.md  SECURITY-BOUNDARY-MAP.md
   ├─ FINAL-IP-AND-THIRD-PARTY-INVENTORY.md  BUYER-REPRODUCTION-GUIDE.md
   ├─ FINAL-KNOWN-LIMITATIONS.md  FINAL-OPERATIONS-HANDOVER.md
   ├─ FINAL-INCIDENT-RUNBOOK.md  FINAL-RELEASE-AUDIT.md
```

## Counts (exact, current tree — `git ls-files` under `sniper-suite/`)

| Category | Files |
|---|---|
| Root-level files | 17 |
| Root `.cargo/` policy | 1 |
| Scripts (`scripts/`) | 3 |
| CI (`.github/`) | 1 |
| App workspace `crates/` — 162 source Rust + 41 test Rust + 7 crate manifests + 18 SQL migrations + 19 replay fixtures | 247 |
| Staking program `programs/staking-suite/` (5 source Rust + 1 test Rust + 3 TOML + lockfile) | 10 |
| Docs (`docs/`) | 63 |
| **Total files (excluding generated targets and Git metadata)** | **342** |

## Counts (historical, at the final delivery package)

| Category | Files |
|---|---|
| Root metadata / release identity (incl. root `Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `.cargo/audit.toml`) | 12 |
| Deployment assets (Dockerfile, compose, templates, ignores) | 6 |
| Scripts (`release-check.sh`, `verify-delivery.sh`, `staking-identity.sh`) | 3 |
| CI workflow | 1 |
| App workspace `crates/` — Rust (72 src + 14 tests) + 7 crate Cargo.tomls + 11 SQL migrations | 104 |
| Staking program `programs/staking-suite/` (5 src + 1 test + Cargo.toml + Cargo.lock + 2 `.cargo/` files) | 10 |
| Docs (`docs/`) | 49 |
| **Total tracked files** | **185** |

Note: the frozen software tree (146 files) plus 14 buyer docs plus 9 final
delivery docs plus `scripts/verify-delivery.sh` = 170, plus
`crates/module-polymarket/src/collateral.rs` added by the post-delivery
audit pass = 171; the buyer-hardening pass added `scripts/staking-identity.sh`
plus 3 docs (LIVE-VALIDATION, BUYER-ACCEPTANCE-TEST, CI-LOCAL-EQUIVALENCE)
= 175; the final buyer-handover pass added 8 docs (feature traceability,
security boundary map, IP/third-party inventory, reproduction guide,
known-limitations register, operations handover, incident runbook, release
audit) = 183; the forensic-engineering cycle then added
FORENSIC-FILE-INVENTORY.md + SOURCE-OF-TRUTH.md = 185, and modified `scripts/staking-identity.sh` (identity-tracking
defect fix: BUYER-ACCEPTANCE-TEST.md added to TRACKED_FILES;
release-manifest.json excluded from the post-set-id stale sweep as a
delivery-time record). The audit pass also MODIFIED existing sources (live/paper
balance separation in modules 1+3, staking max-supply cap + token metadata
in module 4), so the frozen-tree byte identity applies to commit `0e139c3`
only; the current tree differs as described in CHANGELOG.md [Unreleased].
Re-check with the commands in `docs/BUYER-DUE-DILIGENCE.md` §A.

## Where to look first

- Understand: `docs/FINAL-DELIVERY.md` → `docs/BUYER-OVERVIEW.md`
- Verify: `docs/EVIDENCE-INDEX.md` → `AUDIT.md` → run the scripts
- Deploy: `docs/BUYER-QUICKSTART.md` → `docs/BUYER-DEPLOYMENT.md`
- Accept: `docs/ACCEPTANCE-CHECKLIST.md`
- Transfer: `docs/IP-COMPONENTS.md` §Ownership transfer checklist →
  `docs/SUPPORT-HANDOVER.md` → `docs/ARCHIVE-CHECKLIST.md`
