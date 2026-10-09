# Coding-Agent Remediation Tree — $20k–$60k Sale Gaps

> **Legend:** `NEW` = create; `MODIFY` = change existing; `VERIFY` = run/produce evidence. Every line has a `#` comment describing the intended implementation.

```text
sniper-suite/
├── crates/
│   ├── module-sniper/
│   │   └── src/
│   │       ├── limit_orders.rs                         # MODIFY: production-grade order model; remove reliance on in-memory implementation for server runtime; atomic state transitions, idempotency, expiry, trigger evaluation, retry/backoff.
│   │       ├── dca.rs                                  # MODIFY: production DCA state machine; durable budget accounting, lease-safe scheduling, exactly-once intent creation, pause/resume/cancel semantics.
│   │       ├── market.rs                               # MODIFY: require live holder/bundler/creator risk snapshots for enabled gates; provider-specific sell simulation; never silently downgrade a required risk signal.
│   │       ├── entry.rs                                # MODIFY: integrate persistent creator deny-list and risk-intelligence decisions into the authoritative entry gate; preserve existing execution pipeline.
│   │       ├── risk_intel.rs                            # MODIFY/EXPAND: normalize creator, holder concentration, bundler, liquidity and rug signals into deterministic risk decisions with source timestamps.
│   │       ├── sell_probe.rs                            # NEW: venue-specific sell simulation/probe adapters for Raydium v4/Jupiter where supported; explicit Unknown/Unavailable reasons and metrics.
│   │       └── tests/                                  # NEW/MODIFY: integration fixtures for every sniper venue, risk gate, limit order, DCA schedule and restart/recovery case.
│   │
│   ├── module-polymarket/
│   │   └── src/
│   │       ├── copy.rs                                 # MODIFY: production copy-follow lifecycle, live provider IDs, idempotent order/fill reconciliation, tenant isolation and audit events.
│   │       ├── leaders.rs                              # MODIFY: expose leaderboard through tenant/customer API with freshness, sample-size filters, PnL/ROI and anti-sybil rules.
│   │       ├── collateral.rs                            # VERIFY/MODIFY: validate current pUSD/collateral contract addresses, allowance flow and balance semantics against live provider data.
│   │       ├── builder.rs                               # VERIFY/MODIFY: keep CLOB V2 builder attribution aligned with current provider documentation and live signed-order evidence.
│   │       ├── orders.rs                                # VERIFY/MODIFY: verify current CLOB V2 order schema, fee handling, nonce/timestamp and settlement/reconciliation against live API.
│   │       └── tests/                                  # NEW/MODIFY: funded/devnet-safe Polymarket round-trip tests with immutable evidence capture.
│   │
│   ├── server/
│   │   └── src/
│   │       ├── trading_data_plane/
│   │       │   ├── limit_orders.rs                     # NEW: authenticated tenant CRUD for limit orders; SQLx repository calls; optimistic/atomic state changes; audit events; pagination.
│   │       │   ├── dca.rs                              # NEW: authenticated tenant CRUD for DCA schedules; SQLx repository calls; budget validation; audit and event emission.
│   │       │   ├── limit_order_worker.rs               # NEW: durable scheduler worker using Postgres row locking/lease; trigger evaluation; handoff into normal execution; retry and dead-letter handling.
│   │       │   ├── dca_worker.rs                       # NEW: durable DCA scheduler; one-run-per-interval, budget caps, lease recovery, restart-safe execution.
│   │       │   ├── market_service.rs                   # MODIFY: expand beyond SOL/USD + Polymarket into a real Solana market/discovery aggregation layer.
│   │       │   ├── solana_market_feed.rs               # NEW: adapter for real-time token/new-LP/trending/liquidity feed; normalized timestamps/source/quality metadata.
│   │       │   ├── wallet_intelligence.rs              # NEW: wallet tracker, PnL, win-rate, token hit-rate, recent trades, smart-money labels and risk signals.
│   │       │   ├── backtest_worker.rs                  # MODIFY: support a real historical dataset adapter; retain synthetic mode but make dataset mode explicit and auditable.
│   │       │   ├── backtest_dataset.rs                  # NEW: versioned historical dataset reader (Parquet/object storage/Postgres), timestamp filtering, venue-state reconstruction and reproducible dataset hashes.
│   │       │   └── backtest_evidence.rs                # NEW: store dataset hash, strategy/config hash, run ID, source window, fee/slippage model and reproducibility metadata.
│   │       │
│   │       └── saas/
│   │           ├── sso.rs                               # MODIFY: replace process-local pending OAuth map with durable/TTL state store while preserving PKCE, nonce, one-shot consume and tenant scoping.
│   │           └── sso_state.rs                         # NEW: Postgres/Redis pending-auth repository with atomic consume and expiry cleanup.
│   │
│   ├── core/
│   │   └── migrations/
│   │       ├── 0052_limit_orders_dca.sql                # VERIFY: keep existing schema; add only fields required by the production runtime (lease, attempts, last_error, executed_at) if necessary.
│   │       └── 0054_trading_runtime_integrity.sql      # NEW IF NEEDED: durable lease/idempotency/reconciliation fields for limit/DCA/backtest live runs; preserve migration order and rollback semantics.
│   │
│   └── solana-kit/
│       └── src/
│           ├── risk_feed.rs                             # NEW: live RPC/indexer adapter for token holders, creator history, bundler signals and token-account concentration.
│           ├── sell_probe.rs                            # NEW: shared protocol sell-probe primitives with deterministic result classification.
│           └── tests/                                  # NEW: provider fixture coverage and failure-mode tests.
│
├── apps/
│   └── control-plane/
│       └── src/
│           ├── app/
│           │   ├── trading/
│           │   │   ├── limits/page.tsx                 # NEW: customer limit-order creation/list/cancel/edit workflow with tenant-safe validation.
│           │   │   ├── dca/page.tsx                    # NEW: DCA schedule creation/pause/resume/cancel, budget visibility and run history.
│           │   │   ├── wallets/page.tsx                # MODIFY: real multi-wallet management, execution-wallet selection, permissions and balances.
│           │   │   └── discovery/page.tsx              # NEW: Solana token/new-LP/smart-wallet discovery workspace.
│           │   ├── backtests/page.tsx                  # MODIFY: clearly distinguish Synthetic vs Historical dataset modes and show dataset hash/source.
│           │   └── backtests/[runId]/page.tsx           # MODIFY: show dataset provenance, fee model, slippage, reproducibility and caveats.
│           │
│           ├── components/
│           │   ├── backtest/backtest-table.tsx          # MODIFY: remove “Historical” label for synthetic runs; show dataset type and provenance.
│           │   ├── market/MarketScreener.tsx            # NEW/MODIFY: real-time Solana discovery, new LP, liquidity, holders, creator risk and smart-wallet signals.
│           │   ├── wallet/WalletRadar.tsx               # NEW: tracked-wallet analytics, recent trades, PnL/win-rate and one-click copy setup.
│           │   └── trading/LimitDcaPanels.tsx            # NEW: shared limit/DCA controls used by trading and copy workflows.
│           │
│           └── lib/api/
│               ├── limit-orders-api.ts                 # NEW: typed tenant API client for limit orders.
│               ├── dca-api.ts                          # NEW: typed tenant API client for DCA.
│               ├── wallet-intelligence-api.ts          # NEW: typed client for wallet tracker/radar/discovery data.
│               └── backtest-api.ts                     # MODIFY: datasetType/source/hash/reproducibility fields; never call synthetic data historical.
│
├── docs/
│   ├── BUYER-TRUTH-REGISTER.md                          # NEW: every material product claim mapped to source file, live evidence ID, status and buyer-safe wording.
│   ├── BUYER-EVIDENCE-PACK.md                           # NEW: indexed external/live evidence, tx/order IDs, timestamps, hashes, screenshots and provider responses.
│   ├── BUYER-HANDOVER-CHECKLIST.md                      # NEW: deployment, secrets, KMS/Vault, DB, Redis, domains, billing, monitoring, backup/restore, rollback and IP handover.
│   ├── LIVE-TRADING-RUNBOOK.md                          # NEW: safe launch, funded preflight, kill switch, rollback, reconciliation and emergency procedures.
│   ├── BACKTEST-DATA-SPEC.md                            # NEW: historical dataset schema, provenance, retention, versioning and replay/reproducibility rules.
│   ├── COMPETITOR-BENCHMARK.md                          # MODIFY: current Trojan/GMGN/Maestro/Polymarket feature comparison, clearly marking vendor claims vs verified facts.
│   └── COMMERCIAL-CLAIM-AUDIT.md                        # MODIFY: update every claim after live evidence is produced; no unsupported performance wording.
│
├── legal/
│   ├── SOURCE-CODE-BILL-OF-SALE.md                     # MODIFY: replace placeholders with actual buyer/seller terms after counsel review.
│   ├── IP-ASSIGNMENT-TEMPLATE.md                       # MODIFY: complete chain-of-title/assignment fields.
│   ├── MAINTENANCE-AND-SUPPORT-TERMS.md               # MODIFY: define support period, SLA, exclusions and handoff.
│   └── THIRD-PARTY-NOTICES.md                          # VERIFY: complete dependency/license notices and buyer obligations.
│
├── LICENSE                                               # MODIFY: replace seller legal entity and governing-law placeholders before any transfer.
│
├── evidence/
│   ├── external/                                        # VERIFY: run all six external gates; no NOT_RUN records at final delivery.
│   └── live/                                            # VERIFY: run all 13 live gates; preserve immutable raw evidence and hashes.
│
└── scripts/
    ├── verify-buyer-package.sh                         # MODIFY: require the three buyer docs; distinguish actual secret matches from explanatory test strings; fail only on real secret evidence.
    ├── verify-release-integrity.sh                     # MODIFY/VERIFY: validate actual buyer-release contents and canonical-source comparison in a clean Git checkout.
    ├── check-openapi-coverage.sh                       # VERIFY: keep scoped 148/148 API/documentation gate green.
    ├── check-routes-vs-ui.sh                           # VERIFY: keep server/UI route parity green.
    ├── verify-delivery.sh                              # VERIFY: keep release contamination checks green.
    └── verify-script-modes.sh                          # VERIFY: execute from a real Git checkout so executable modes are preserved.
```

---

