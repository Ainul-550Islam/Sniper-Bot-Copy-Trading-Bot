# SNIPER-SUITE — GAP / MISSING-FILES MAP

Copy-ready for a coding agent. Every file line ends with `# [ACTION][PRIORITY] what must be inside`.

**Audit basis.** Static read of `sniper-suite-enterprise-release.zip` (root tree; `buyer-release/source/` is a byte-identical copy — `diff -rq` is clean). Nothing was compiled or run: the audit sandbox had no Rust toolchain and no network. Every finding comes from reading code, grep and route diffs. **Run `cargo check --workspace --all-targets` first** — it will list errors this audit could not see.

TOTALS_PLACEHOLDER

---

## 0. Legend

| Tag | Meaning |
|---|---|
| `[NEW]` | create this file |
| `[REWRITE]` | file exists but returns canned data or does nothing — keep path and public route, replace the internals |
| `[MODIFY]` | edit an existing file |
| `[DELETE]` | remove |
| `[VERIFY]` | check only; change code only if the check fails |
| `[EXTERNAL]` | needs a third party (auditor, pentester, counsel) — cannot be coded |
| `P0` | fix before any buyer sees the code (build, truthfulness, safety, licence) |
| `P1` | needed to defend a $30–60k price (proof, security flows, real engines) |
| `P2` | competitive / commercial polish |

## 1. Rules for the coding agent

1. **Never return fabricated data.** No hard-coded rows, money, PnL, names or ids (`acme-quant.com`, `usr_operator_01`, `act-01`, `alt-01`, `tkt-2026-9481`, `$48,500`). Empty state = `200` with an empty list/zero totals, or a real error. The UI must show `EmptyState` / `ErrorState`, never sample data inside a `catch`.
2. Every handler: `authorize_request` / `guard` first → scope by `organization_id` → DB-backed → audit write → unit + integration test → present in the code-generated OpenAPI.
3. **No claim without code + evidence.** Remove strings such as "FIPS 140-3", "HSM", "SOC2 Type II", "sub-millisecond", "zero-latency", "0% dummy data" unless an `evidence/live/*.json` with `"status": "PASSED"` backs it.
4. Reuse existing tables before adding new ones (section 4). New migrations continue from `0039_*.sql` in `crates/core/migrations/` (last existing: `0038_authoritative_exact_accounting.sql`).
5. `buyer-release/source/` is generated. Never edit it; regenerate with `scripts/rebuild-buyer-release.sh` after all changes.
6. Keep the frontend rule that the session token lives in memory only (`lib/auth.ts`); no `localStorage`.
7. A gap is closed only by evidence (`evidence/live/*.json`, passing tests) — never by editing a document.
8. After every batch run: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run typecheck`, `npm test`.

## 2. Gap scorecard (my estimate — judgment from static review, not a measurement)

| Area | Weight | Ready | Gap | Evidence |
|---|---|---|---|---|
| Core trading engines (sniper, copy, Polymarket, solana-kit, staking program) | 25% | 75% | **25%** | ~221k lines Rust in 8 crates, 38 migrations, ~2,600 test attributes (not executed) |
| Live proof / evidence | 15% | 10% | **90%** | all 6 `evidence/external/*.json` are `NOT_RUN` |
| Control plane truthful and wired | 20% | 30% | **70%** | 39 pages: 10 real, 6 partial, 23 fake/hollow; 6 UI features call routes that do not exist |
| Security / compliance | 15% | 35% | **65%** | no MFA / SSO / email / reset code; kill switch and "MFA enforced" are fake; no audit, no pentest |
| Sale packaging / IP / docs | 10% | 35% | **65%** | MIT licence, 148 docs with false "VERIFIED" claims, junk files, duplicate source tree |
| Competitive features | 10% | 35% | **65%** | no holder / creator / bundler checks, no fee or referral system, control-only Telegram |
| Deploy / ops | 5% | 70% | **30%** | compose + nginx + TLS + backup scripts + 3 CI workflows; no Helm; OpenAPI covers ~64 of 150 routes |
| **Weighted** | 100% | **≈42%** | **≈58%** | assumes the build blocker in section 3 is fixed |

## 3. P0-0 BUILD BLOCKER — fix before anything else

Static evidence says `crates/server` does not compile: 22 call sites use symbols that are not defined. CI (`.github/workflows/ci.yml`) runs `cargo build --workspace --all-targets`, so it would fail on this tree. These are only what grep found; the compiler may report more.

```text
crates/core/src/authorization/
└── mod.rs                      # [MODIFY][P0] AccessRequest defines read(), manage(), billing() but 11 call sites use AccessRequest::read_only(..) in server/src/saas/{reports,risk_dashboard,notifications,portfolio,alerts,webhooks,support,activity}.rs. Add `pub fn read_only(permission: Permission) -> Self` (same semantics as read) OR change the 11 calls to read().

crates/core/src/membership/
├── permission.rs               # [MODIFY][P0] enum has 22 variants; code uses 3 that do not exist: Permission::UsersManage (7x: saas/security.rs 3, saas/team.rs 3, saas/notifications.rs 1), Permission::ApiKeyRead (1x) and Permission::ApiKeyWrite (3x) in saas/webhooks.rs. Add them (update `ALL: [Permission; 22]`, parse/display, serde names) or map to UsersInvite / UsersRemove and ApiKeyCreate / ApiKeyRevoke.
└── role.rs                     # [MODIFY][P0] role -> permission matrix for any new variant; least privilege (viewer/auditor never get *Manage / *Write).

crates/core/tests/
└── saas_control_plane.rs       # [MODIFY][P0] extend the RBAC matrix test to cover every Permission variant and every role.

.github/workflows/
└── ci.yml                      # [VERIFY][P0] already runs build/clippy/test; after the fix confirm a green run on the exact release commit and save the run URL into evidence/live/ci_run.json.
```

## 4. Existing tables that no (or the wrong) Rust code uses — wire handlers to these first

```text
user_mfa_devices (0038)                       # TOTP / WebAuthn / backup codes; 0 Rust files use it
tenant_sso_configs (0038)                     # OIDC / SAML per org; 0 Rust files use it
portfolio_snapshots_hourly (0038)             # equity curve for portfolio page; 0 Rust files use it
tenant_daily_accounting (0038)                # daily PnL for analytics page; 0 Rust files use it
backtest_runs (0037)                          # backtests; 0 Rust files use it
webhook_endpoints, webhook_deliveries (0037)  # webhooks + delivery log; 0 Rust files use them
usage_events (0017)                           # usage metering for plan limits; 0 Rust files use it
invites (0017)                                # team invites (token_hash, status, expires_at); team.rs invite_member does not write to it
kill_switches, kill_switch_events (0015)      # real kill switch, used by core global_risk + api.rs but NOT by saas/risk_dashboard.rs
tenant_module_controls (0036)                 # durable per-tenant module on/off, already used by module_controls.rs
strategies (0037)                             # referenced by 10 files, yet tenant strategies.rs handlers persist nothing
tenant_configs, config_versions               # versioned per-tenant config; reuse for sniper/copy/polymarket config
```

---

## 5. P0 — Control plane truthfulness and safety (gap 70%)

### 5A. Backend handlers that return canned data or do nothing

```text
crates/server/src/saas/
├── activity.rs                 # [REWRITE][P0] GET /api/saas/activity. Return org-scoped rows from audit_events (limit, cursor, actor, action filters). Delete the hard-coded "act-01.." rows and fake actor "usr_operator_01". Test: fresh org => []; org B never sees org A rows.
├── portfolio.rs                # [REWRITE][P0] GET /api/saas/portfolio. Build from positions + balance_snapshots + portfolio_snapshots_hourly + tenant_daily_accounting via TenantReportingRepo. Zero/empty for a new org. Delete constants (equity 4_850_000 cents, SOL 185.5, PnL 1_420_000...). Keep the JSON keys the UI reads.
├── alerts.rs                   # [REWRITE][P0] GET /api/saas/alerts + POST /alerts/:id/ack on table tenant_alerts. Delete hard-coded "alt-01.." rows. ack = UPDATE + audit.
├── alert_store.rs              # [NEW][P0] struct AlertStore { insert, list(filter severity/unread), ack }. Producers call insert(): risk breach, execution failure, custody health failure, billing past_due, reconciliation finding.
├── status.rs                   # [REWRITE][P0] GET /api/saas/status. Derive each component from real probes (ops/health_report.rs, ops/dependency_health.rs, custody/health.rs, DB + Redis ping). States: operational | degraded | down | not_configured | unknown. DELETE "FIPS 140-3 HSM", "sub-millisecond", fixed latencies and the always-"operational" status.
├── support.rs                  # [REWRITE][P0] GET/POST /api/saas/support/tickets persisted in support_tickets, org-scoped. create_ticket must store + notify (email or webhook). Delete canned ticket "tkt-2026-9481".
├── risk_dashboard.rs           # [REWRITE][P0 SAFETY] GET: real limits (bot_core::risk + global_risk) and real utilization from positions. POST /risk-dashboard/kill-switch today only writes an audit line and echoes `active` => it MUST really halt: apply_control(Disable) on every module of the org (trading_data_plane/module_controls.rs), persist a tenant halt flag (tenant_module_controls / kill_switches + kill_switch_events), make new intents fail (tenant/risk_guard.rs), audit. Authorize with Permission::RiskManage. GET must reflect the true state.
├── notifications.rs            # [REWRITE][P0] GET/PUT /api/saas/notifications/preferences persisted in notification_preferences. PUT currently returns the body without saving; dispatcher must honour the flags.
├── pricing.rs                  # [REWRITE][P0] GET /api/saas/pricing. Build the catalog from bot_core::billing::{plan,pricing,entitlements} + plans table (single source shared with checkout). Remove unbacked feature claims (FIPS 140-3 KMS, dedicated Geyser stream, SLA) unless evidenced.
├── webhooks.rs                 # [REWRITE][P0] CRUD on webhook_endpoints; secret generated server-side, shown once, stored encrypted; POST /:id/test sends a signed test event. Delete fake "acme-quant.com" endpoint. Also fix undefined Permission::ApiKeyRead/Write (section 3).
├── webhook_dispatcher.rs       # [NEW][P1] background worker: events -> HMAC-SHA256 signed POST (X-Signature, X-Timestamp), exponential retry, dead-letter, rows in webhook_deliveries; SSRF guard (block private / link-local / metadata IPs, DNS-rebinding safe), 5s timeout, body cap.
├── reports.rs                  # [REWRITE][P0] GET /api/saas/reports lists real generated reports. Delete fake "SOC2 Type II Audit" and "rep-2026-q3-tax".
├── report_download.rs          # [NEW][P0] GET /api/saas/reports/:id/download (the current download_url points to a route that does not exist). Stream CSV/PDF built from TenantReportingRepo; org-scoped; audit.
├── team.rs                     # [REWRITE][P0] invite_member must INSERT into invites (SHA-256 token_hash, 7-day expiry), send the email, return the real id; add accept_invite, revoke_invite, resend_invite. Today it returns a random UUID and persists nothing. Fix undefined Permission::UsersManage (section 3).
├── organizations.rs            # [MODIFY][P0] (a) members list: accept alias "current" (resolve to caller's active org) or keep UUID-only and fix the UI; (b) align response keys with lib/api/team-api.ts (backend returns {count, members}); (c) add PATCH/DELETE /api/saas/organizations/:id/members/:user_id (UI calls it, no route exists).
├── security.rs                 # [REWRITE][P0] POST mfa-enforce is audit-only today (the repo itself says no MFA code exists). Real: set organizations.mfa_required and refuse sessions without verified MFA; persist ip-allowlist; rotate-tokens must actually revoke sessions. Fix undefined Permission::UsersManage.
├── custody_health.rs           # [REWRITE][P0] GET signer health by id always returns 404 ("simulate not-found path"). Load the custody_signers row, call provider health() (custody/kms/health.rs, custody/vault/health.rs); 404 only when truly absent.
├── audit_export.rs             # [MODIFY][P0] delete the in-memory branch that fabricates mock audit records; no DB => empty set + explicit error.
└── export.rs                   # [MODIFY][P0] audit export emits {records}; settings/audit/page.tsx reads {items} => the page shows an empty list. Pick one key (prefer `items`) and add a contract test.

crates/server/src/trading_data_plane/
├── markets.rs                  # [REWRITE][P0] remove the 3 hard-coded tickers (e.g. SOL/USDC 154.20); delegate to market_service.
├── market_service.rs           # [REWRITE][P1] real feed: Solana prices via solana-kit (jupiter.rs / raydium.rs / pumpswap.rs), Polymarket markets via module-polymarket/src/gamma.rs; TTL cache; explicit `unavailable` state. Wire into markets.rs (MarketService has no callers today).
├── strategies.rs               # [REWRITE][P0] list returns 2 hard-coded "default" strategies, get_one fabricates a record, create/update/archive persist nothing. Implement on the strategies table: versions, archive, params validated per module (SniperConfig / copy policy / polymarket config), audit every change.
├── strategy_runtime.rs         # [REWRITE][P1] StrategyRuntimeBridge is dead code (no callers). Implement activate/deactivate: strategy params -> versioned tenant config (tenant_config/store.rs) -> module reload via module_runtime/tenant_module_factory.rs; paper -> live promotion guarded by ops/funded_mode_guard.rs.
├── backtests.rs                # [REWRITE][P0] create() returns fixed +18.4% ROI / 48 trades / 35 wins / Sharpe 2.34 / 4.8% DD whatever the input; list() always returns a sample run. Read/write backtest_runs; status queued | running | completed | failed.
├── backtest_service.rs         # [REWRITE][P1] dead code ("deterministic mock simulation"); run the module-sniper simulator (section 6A) in a worker (spawn_blocking), persist metrics + equity curve.
├── sniper_config.rs            # [NEW][P0] GET/PUT /api/tenant/sniper/config (UI calls it, no route exists). Validate against bot_core::config::SniperConfig; store as versioned tenant config; permission (TenantUpdate or new BotConfigure) + audit + diff.
├── copy_config.rs              # [NEW][P0] GET/PUT /api/tenant/copy/config; validate with module-copy policy/sizing types; same pattern.
├── polymarket_config.rs        # [NEW][P0] GET/PUT /api/tenant/polymarket/config; validate strategy + sizing limits; same pattern.
├── analytics.rs                # [NEW][P0] GET /api/tenant/analytics?from&to&module: volume, realized PnL, trades, win rate, max drawdown, per-module breakdown from trades / positions / tenant_daily_accounting. Sharpe only with >= 30 daily points, else null.
├── onboarding.rs               # [NEW][P0] GET /api/tenant/onboarding + POST /complete. Steps computed from real state (org provisioned, custody profile + signer healthy, wallet bound, config saved, module enabled). Never static `completed: true`.
├── integrations.rs             # [NEW][P0] GET /api/tenant/integrations: real connectivity of RPC, Geyser, Jito, custody provider, billing provider, Telegram via ops/dependency_health.rs; `not_configured` when absent.
└── mod.rs                      # [MODIFY][P0] register the 6 new routes behind authorization_chain guards (TradingModuleFamily); update api.rs router + OpenAPI.
```

### 5B. Core data layer + migrations

```text
crates/core/src/db/
├── alerts.rs                   # [NEW][P0] sqlx queries for tenant_alerts using tenant_query / tenant_tx helpers (tenant-scoped).
├── support.rs                  # [NEW][P0] support_tickets queries.
├── notifications.rs            # [NEW][P0] notification_preferences queries.
├── strategies.rs               # [NEW][P0] strategies + strategy_versions queries.
├── backtests.rs                # [NEW][P1] backtest_runs queries.
├── webhooks.rs                 # [NEW][P0] webhook_endpoints + webhook_deliveries queries.
├── reports.rs                  # [NEW][P0] reports queries.
└── mod.rs                      # [MODIFY][P0] export the new modules.

crates/core/src/strategy/
└── model.rs                    # [MODIFY][P0] exists as models only: add StrategyVersion and validate_params(module, json) -> Result.

crates/core/src/backtest/
└── model.rs                    # [MODIFY][P1] exists as models only: add BacktestAssumptions, BacktestMetrics, EquityPoint.

crates/core/migrations/         # last existing: 0038_authoritative_exact_accounting.sql
├── 0039_tenant_alerts_support_notifications_reports.sql   # [NEW][P0] tenant_alerts, support_tickets, notification_preferences, reports — each with organization_id FK + index + ownership checks.
├── 0040_org_security_email_flows.sql                      # [NEW][P1] organizations.mfa_required, org_ip_allowlist(cidr), password_reset_tokens(token_hash, user_id, expires_at, used_at), email_verifications, email_outbox(status, attempts, next_attempt_at).
├── 0041_strategy_versions_backtest_ext.sql                # [NEW][P1] strategy_versions; backtest_runs add only the missing columns (dataset_ref, assumptions jsonb, metrics jsonb, equity_curve jsonb, error, started_at, finished_at).
├── 0042_referrals_fees.sql                                # [NEW][P2] referral_codes, referral_attributions, platform_fee_ledger.
└── 0043_wallet_pools.sql                                  # [NEW][P2] wallet_pools, wallet_pool_members.
```

### 5C. Frontend pages that are hard-coded, broken or fall back to sample data

My classification of the 39 pages: **real** = `/`, `/billing`, `/docs`, `/settings/api`, `/settings/data-lifecycle`, `/trading`, `/trading/executions`, `/trading/orders`, `/trading/positions`, `/trading/telegram`; **partial** = `/pricing`, `/custody`, `/settings/audit`, `/trading/sniper`, `/trading/copy`, `/trading/polymarket`; **fake/hollow** = everything marked `[REWRITE]` or listed below.

```text
apps/control-plane/src/app/
├── analytics/page.tsx          # [REWRITE][P0] numbers are literals in the page: volume $4,850,290, PnL +$142,850, Sharpe 2.84, 1,428 trades, win rate 76.4%. Fetch customerTrading.analytics() -> /api/tenant/analytics; EmptyState when no trades; use components/charts.
├── integrations/page.tsx       # [REWRITE][P0] literals "Connected & Verified", "FIPS 140-3 Level 3 Active", fixed latencies. Render /api/tenant/integrations with last_checked and a not_configured state.
├── onboarding/page.tsx         # [REWRITE][P0] static steps; step 2 claims an HSM custody profile was generated. Drive from /api/tenant/onboarding; each step deep-links to the real page (custody, wallets, config).
├── settings/security/page.tsx  # [REWRITE][P0] `useState(true)` for mfaEnforced + fake allowlist (198.51.100.0/24, 203.0.113.45/32). Load real settings (/api/saas/security-summary + new /api/saas/security/settings).
├── settings/team/page.tsx      # [MODIFY][P0] drop the sample-members fallback; use the real org id from sessionStore; pending invites list, resend/revoke, role change.
├── settings/webhooks/page.tsx  # [MODIFY][P0] real CRUD, secret shown once, delivery log.
├── settings/audit/page.tsx     # [MODIFY][P0] fix items vs records, remove the deterministic-sample fallback on error (show ErrorState), add filters, pagination, CSV export.
├── settings/api/page.tsx       # [VERIFY][P0] real api-keys store; confirm no sample-data fallback inside its catch blocks.
├── activity/page.tsx           # [MODIFY][P0] no sample fallback; render real audit events.
├── alerts/page.tsx             # [MODIFY][P0] real alerts + ack; no sample fallback.
├── portfolio/page.tsx          # [MODIFY][P0] real portfolio, DataFreshness "as of" stamp, empty state.
├── reports/page.tsx            # [MODIFY][P0] real reports + working download.
├── risk/page.tsx               # [MODIFY][P0 SAFETY] KillSwitchPanel shows server-confirmed state; on backend failure show the failure, never optimistic success.
├── status/page.tsx             # [MODIFY][P0] show not_configured / unknown honestly.
├── support/page.tsx            # [MODIFY][P0] real tickets.
├── pricing/page.tsx            # [MODIFY][P0] render the catalog from the unified billing source.
├── markets/page.tsx            # [MODIFY][P1] real feed + "data unavailable" state.
├── markets/[marketId]/page.tsx # [MODIFY][P1] price chart via components/charts.
├── backtests/page.tsx          # [MODIFY][P0] add BacktestDisclosure; remove any canned display.
├── backtests/[runId]/page.tsx  # [MODIFY][P0] show assumptions, dataset, fees, "hypothetical results" notice, equity curve.
├── strategies/page.tsx         # [MODIFY][P0] real list; no default-strategies assumption.
├── strategies/new/page.tsx     # [MODIFY][P0] real create + server-side validation errors.
├── strategies/[strategyId]/page.tsx   # [MODIFY][P0] real versions + activate/deactivate (needs strategy_runtime.rs).
├── trading/sniper/config/page.tsx     # [REWRITE][P0] Save calls createStrategy (a no-op). Use PUT /api/tenant/sniper/config via customerTrading.updateSniperConfig.
├── trading/copy/config/page.tsx       # [REWRITE][P0] same for copy.
├── trading/polymarket/config/page.tsx # [REWRITE][P0] same for polymarket.
├── trading/sniper/page.tsx     # [MODIFY][P0] stop showing an invented default config when GET /config fails; show "not configured" or the error.
├── trading/copy/page.tsx       # [MODIFY][P0] same.
└── trading/polymarket/page.tsx # [MODIFY][P0] same.
```

### 5D. Frontend lib + components

```text
apps/control-plane/src/lib/
├── customer-trading-api.ts     # [MODIFY][P0] delete the fallbacks that fabricate sniper/copy/polymarket config, analytics, onboarding and integrations when the HTTP call fails; add updateSniperConfig / updateCopyConfig / updatePolymarketConfig; keep classifyTradingError.
├── api/team-api.ts             # [MODIFY][P0] real invites API; no sample data.
├── api/webhook-api.ts          # [MODIFY][P0] CRUD + deliveries + test.
└── api/security-api.ts         # [MODIFY][P0] enforceMfa becomes real; add mfa enroll/verify and ip allowlist calls.

apps/control-plane/src/components/
├── common/DataFreshness.tsx    # [NEW][P0] "as of <time>" + stale warning, used on portfolio and status.
├── backtest/BacktestDisclosure.tsx   # [NEW][P0] mandatory "hypothetical results, not a prediction" notice + assumptions summary.
├── risk/KillSwitchPanel.tsx    # [MODIFY][P0 SAFETY] confirm dialog; call the real endpoint; show server-confirmed state and errors.
├── settings/security-form.tsx  # [MODIFY][P0] wire to the real endpoints; remove local fake state.
├── charts/LineChart.tsx        # [NEW][P1] dependency-free SVG line chart (equity / PnL), accessible labels.
├── charts/BarChart.tsx         # [NEW][P1] SVG bar chart (volume by module).
├── charts/Sparkline.tsx        # [NEW][P1] small trend chart for cards.
└── AppShell.tsx                # [MODIFY][P2] brand name / "Enterprise" tag from config/branding.ts instead of hard-coded strings.
```

### 5E. Tests and CI gates that would have caught all of this

```text
crates/server/tests/
├── fresh_tenant_contract.rs    # [NEW][P0] boot the app on an empty DB, create a new org, GET every list/summary endpoint; assert empty arrays / zero totals / no sample ids (acme, act-01, alt-01, tkt-2026, "SOC2"). The single most valuable test here.
├── kill_switch_flow.rs         # [NEW][P0 SAFETY] activate via /api/saas/risk-dashboard/kill-switch => all modules disabled, new intents rejected, state visible in GET, audit written; deactivate resumes; other tenants unaffected.
└── openapi_router_conformance.rs # [MODIFY][P1] fail if any registered /api route has no OpenAPI path (grep shows ~64 spec paths vs 150 routes; run this test first).

apps/control-plane/
├── vitest.config.ts            # [NEW][P0] unit-test runner.
├── package.json                # [MODIFY][P0] add scripts test / test:contract / test:e2e and devDependencies (vitest, @testing-library/react, @playwright/test).
├── src/__tests__/no-fake-data.test.ts    # [NEW][P0] scan src/**/*.tsx for forbidden literals: "FIPS", "Math.random", "$4,850,290", "acme", "198.51.100", "Connected & Verified", sample-data fallbacks in catch blocks.
├── src/__tests__/api-contract.test.ts    # [NEW][P0] every "/api/..." string used in src/ must exist in openapi/openapi.json (catches the 6 UI features with no backend route).
└── src/__tests__/kill-switch.test.tsx    # [NEW][P0] UI shows an error, not success, when the backend fails.

scripts/
├── forbid-fake-data.sh         # [NEW][P0] CI grep gate: canned-handler patterns in crates/server/src (`vec![ json!({ "id": "...-01"`, "deterministic mock", "acme").
├── check-routes-vs-ui.sh       # [NEW][P0] extract frontend /api paths and Axum routes; fail on any UI path without a route.
└── check-openapi-coverage.sh   # [NEW][P1] fail when a route is missing from the generated spec.

.github/workflows/
├── ci.yml                      # [MODIFY][P0] add forbid-fake-data, check-routes-vs-ui and OpenAPI coverage gates.
└── frontend-ci.yml             # [MODIFY][P0] today: npm ci -> typecheck -> build only. Add `npm test`, the contract test and a Playwright job.
```

---

## 6. P1 — Proof, real engines, security flows

### 6A. Real backtester (or delete the feature and its UI)

```text
crates/module-sniper/src/
├── replay.rs                   # [MODIFY][P1] exists: deterministic replay that stops at EXECUTION_READY (no fills, no PnL). Expose the per-step outcome so a simulator can continue from it.
└── backtest/
    ├── mod.rs                  # [NEW][P1] Backtester::run(dataset, config, assumptions) -> BacktestResult.
    ├── dataset.rs              # [NEW][P1] loader for recorded launches + post-launch price paths (JSON / Parquet).
    ├── fill_model.rs           # [NEW][P1] latency, slippage vs reserves, priority fee / Jito tip, landing probability, partial fills; every assumption echoed in the result.
    ├── exit_sim.rs             # [NEW][P1] apply exit.rs rules (TP / SL / trailing / time-stop) over the price path.
    └── metrics.rs              # [NEW][P1] realized PnL net of fees / tips / rent, win rate, max drawdown, Sharpe / Sortino (only with enough points), exposure.

crates/module-sniper/tests/
├── fixtures/launches/*.json    # [NEW][P1] >= 50 recorded real launches (pump.fun + Raydium) with price paths and capture metadata (date, source).
└── backtest_golden.rs          # [NEW][P1] golden metrics on the fixtures + determinism test.

crates/module-copy/src/
└── backtest.rs                 # [NEW][P2] replay leader trades through sizing.rs / policy.rs.
```

### 6B. Live evidence (every claim you keep in docs needs one)

```text
evidence/live/                  # [NEW][P1] one JSON per validation; same schema as evidence/external/*.json (validation_id, gap_id, provider, environment, timestamp, command, mode, status, evidence_hash, redacted_metadata, endpoint_ref) but "status": "PASSED" with real ids.
├── solana_mainnet_small_funded.json    # [NEW][P1] small funded run: tx signatures, slots, landed latency (redacted).
├── pumpfun_buy_sell_roundtrip.json     # [NEW][P1] buy + sell on pump.fun with current account layout.
├── pumpswap_roundtrip.json             # [NEW][P1] same on PumpSwap.
├── polymarket_order_roundtrip.json     # [NEW][P1] place + cancel + fill on the current CLOB.
├── billing_stripe_test_mode.json       # [NEW][P1] checkout created + webhook signature verified (run existing live_billing_contract test).
├── custody_kms_signing.json            # [NEW][P1] AWS KMS signing round-trip (existing live_custody_contract test).
├── custody_vault_transit_signing.json  # [NEW][P1] Vault transit signing round-trip.
├── deployment_smoke.json               # [NEW][P1] output of the deployment_smoke test against a real deployment.
├── staking_devnet_e2e.json             # [NEW][P1] staking program on devnet / validator.
├── latency_report.json                 # [NEW][P1] produce with existing crates/solana-kit/tests/latency_bench.rs.
└── ci_run.json                         # [NEW][P1] URL + commit of a green CI run.

scripts/
├── run-live-validation.sh      # [NEW][P1] orchestrates the live runs and writes evidence/live/*.json.
└── run-external-validation.sh  # [MODIFY][P1] write into evidence/live; refuse PASSED without a tx signature / provider object id; redact secrets.

crates/server/src/ops/
└── final_gap_ledger.rs         # [MODIFY][P1] derive each GAP status from evidence/live hashes instead of hard-coded values.

docs/
└── LATENCY-AND-LANDING-REPORT.md       # [NEW][P1] measured p50 / p95 detect -> submit -> landed per region and provider, method, link to raw data.
```

### 6C. Protocol drift

```text
crates/solana-kit/
├── src/pump.rs                 # [MODIFY][P1] account-count statements disagree: file header says 17 for buy, a unit test asserts 16 (buy_layout_has_sixteen_accounts_in_order), pump's April 2026 notice says 18. Verify on-chain, fix comment + test, keep layout variants as fallback.
├── src/pumpswap.rs             # [VERIFY][P1] same check against the current PumpSwap layout (cashback / holder-reward variants).
└── tests/pump_layout_drift.rs  # [NEW][P1] ignored by default (LIVE_SOLANA=1): fetch the current IDL or simulate a tiny buy; assert account order/count against buy_layout_variants() / sell_layout_variants().

crates/module-polymarket/tests/
└── clob_v2_vectors.rs          # [NEW][P1] sign fixed orders and compare to vectors from the official client (domain version "2", order fields, pUSD collateral); extends exchange_v3_signing.rs.

scripts/
└── check-protocol-drift.sh     # [NEW][P1] compare pinned program ids / IDL hashes, Polymarket exchange addresses and EIP-712 domain version with live sources.

.github/workflows/
└── protocol-drift.yml          # [NEW][P1] scheduled daily; runs the script; opens an issue on drift.

docs/
└── PROTOCOL-DRIFT-RUNBOOK.md   # [NEW][P1] who fixes what within how many hours when pump.fun / Polymarket ship breaking changes.
```

### 6D. Email, MFA, SSO, password flows (schema for MFA and SSO already exists, code does not)

```text
crates/server/src/email/
├── mod.rs                      # [NEW][P1] trait EmailProvider { send(EmailMessage) }, config from env.
├── smtp.rs                     # [NEW][P1] SMTP transport (TLS required), e.g. lettre.
├── http_provider.rs            # [NEW][P1] HTTP providers (Resend / Postmark / SES) behind the same trait.
├── outbox.rs                   # [NEW][P1] email_outbox worker: retry with backoff, dedupe, rate limit.
└── templates.rs                # [NEW][P1] verify, reset, invite, alert, invoice templates (text + HTML); no secrets in logs.

crates/server/src/saas/
├── password_reset.rs           # [NEW][P1] POST /api/saas/auth/forgot + /reset; SHA-256 token hash, 30 min, single-use, constant-time compare, no user enumeration, rate limited, revoke all sessions after reset.
├── email_verification.rs       # [NEW][P1] verify-email token flow.
├── mfa.rs                      # [NEW][P1] TOTP (RFC 6238) enroll / verify / disable + 10 hashed backup codes on user_mfa_devices; secret encrypted with the custody master key; replay protection via counter; enforced when organizations.mfa_required.
├── sso.rs                      # [NEW][P1] OIDC authorization-code + PKCE (SAML2 optional) on tenant_sso_configs; allowed_domains; JIT provisioning + role mapping; enforce_sso.
├── ip_allowlist.rs             # [NEW][P1] enforce the org CIDR allowlist.
├── users.rs                    # [MODIFY][P1] login returns an MFA challenge when required; throttle / lockout.
└── middleware.rs               # [MODIFY][P1] apply the IP allowlist + `mfa_verified` session claim.

crates/core/src/session/
└── mfa.rs                      # [NEW][P1] session claim `mfa_verified`, step-up window.

crates/server/tests/
├── mfa_flow.rs                 # [NEW][P1] enroll -> login challenge -> backup code -> disable.
├── password_reset_flow.rs      # [NEW][P1] token expiry, reuse, enumeration safety, session revocation.
├── invite_flow.rs              # [NEW][P1] invite -> email outbox -> accept -> role applied; revoke.
├── strategy_lifecycle.rs       # [NEW][P1] create -> version -> activate -> config applied to the module -> archive.
└── webhook_delivery.rs         # [NEW][P1] signed delivery, retry / backoff, SSRF blocks.

apps/control-plane/src/
├── app/page.tsx                # [MODIFY][P1] sign-in / sign-up already live here; add "Forgot password", the MFA step and a ToS / Privacy consent checkbox (store consent version + timestamp).
├── app/forgot-password/page.tsx        # [NEW][P1]
├── app/reset-password/page.tsx         # [NEW][P1]
├── app/verify-email/page.tsx           # [NEW][P1]
├── app/accept-invite/page.tsx          # [NEW][P1]
├── app/settings/security/mfa/page.tsx  # [NEW][P1] TOTP enroll (secret + QR as inline SVG), verify, backup codes shown once, disable.
├── app/settings/sso/page.tsx           # [NEW][P1] OIDC / SAML config per org, enforce_sso, allowed_domains.
├── app/legal/terms/page.tsx            # [NEW][P1] placeholder text, to be completed by counsel.
├── app/legal/privacy/page.tsx          # [NEW][P1] placeholder text, to be completed by counsel.
├── app/legal/risk-disclosure/page.tsx  # [NEW][P1] trading risk, no performance guarantee, hypothetical-results language.
├── app/error.tsx               # [NEW][P1] global error boundary.
├── app/not-found.tsx           # [NEW][P1]
├── app/loading.tsx             # [NEW][P1]
├── lib/api/auth-flows-api.ts   # [NEW][P1] forgot / reset, verify email, accept invite, MFA challenge.
├── lib/auth.ts                 # [MODIFY][P1] MFA challenge step; optional httpOnly-cookie session mode (today a page reload signs the user out because the token is in memory).
├── components/legal/ConsentCheckbox.tsx    # [NEW][P1]
├── components/settings/mfa-enroll.tsx      # [NEW][P1]
└── components/settings/webhook-deliveries.tsx # [NEW][P1] delivery log table with retry status.

apps/control-plane/e2e/
├── playwright.config.ts        # [NEW][P1]
├── auth.spec.ts                # [NEW][P1] sign-up -> verify -> login -> MFA -> logout; reload behaviour.
├── kill-switch.spec.ts         # [NEW][P1] end-to-end halt.
├── strategy-config.spec.ts     # [NEW][P1] config save -> reload -> value persisted.
└── billing-checkout.spec.ts    # [NEW][P1] Stripe test mode checkout.
```

### 6E. Audit prep and operator custody

```text
programs/staking-suite/         # ~4.1k lines src, 73 test attributes, 1,404-line e2e, no external audit
├── audit/THREAT-MODEL.md       # [NEW][P1] actors, trust boundaries, privileged instructions.
├── audit/INVARIANTS.md         # [NEW][P1] conservation of funds, reward-math rounding, authority checks, close / re-init.
├── fuzz/                       # [NEW][P1] Trident or honggfuzz harness over instruction.rs sequences.
└── tests/property_rewards.rs   # [NEW][P1] proptest on reward accrual.

evidence/audits/
├── staking-audit-<firm>-<date>.pdf     # [NEW][P1][EXTERNAL] independent audit of programs/staking-suite.
└── pentest-<firm>-<date>.pdf           # [NEW][P1][EXTERNAL] penetration test of control plane + API.

docs/audit/
└── AUDIT-SCOPE.md              # [NEW][P1] scope, commit hash, findings and fixes tracker.

crates/solana-kit/src/
└── signer.rs                   # [VERIFY][P1] README says [signing] provider = vault | kms | hsm fails at startup, while tenant custody has real KMS / Vault clients (crates/server/src/custody/kms, vault). Decide: wire operator mode to those signers or delete the config values; make docs match.

crates/core/src/
└── config.rs                   # [MODIFY][P1] SigningProvider::{Vault,Kms,Hsm} parse today; validate end-to-end or remove.
```

---

## 7. P2 — Competitive features, white-label, ops

```text
crates/module-sniper/src/risk_intel/
├── mod.rs                      # [NEW][P2] RiskIntel aggregator that enriches MarketSnapshot.
├── holders.rs                  # [NEW][P2] top-N holder concentration excluding LP / bonding curve.
├── creator_history.rs          # [NEW][P2] deployer's previous launches and rug rate.
├── bundler.rs                  # [NEW][P2] same-slot / same-funder buyer cluster detection.
├── honeypot.rs                 # [NEW][P2] simulateTransaction of a sell + transfer-fee / tax check.
└── external.rs                 # [NEW][P2] optional scanner adapters (RugCheck / Birdeye / GoPlus) behind a trait with circuit breaker + cache; off by default.

crates/module-sniper/src/
├── gates.rs                    # [MODIFY][P2] existing gates: pool state / open time, token state, freeze authority, decimals, min liquidity, price sanity, creator concentration, pool supply fraction, snapshot freshness. Add top-holders, creator-history, bundler and sell-simulation gates; extend reason_for_gate() and RejectReason.
├── limit_orders.rs             # [NEW][P2] persistent limit / trigger orders evaluated by tenant_background/scheduler.rs.
├── dca.rs                      # [NEW][P2] scheduled DCA with budget caps.
├── exit.rs                     # [VERIFY][P2] trailing stop and laddered take-profit; add if missing.
└── tenant_executor.rs          # [MODIFY][P2] spread buys across a wallet pool.

crates/module-sniper/tests/
└── gates_risk_intel.rs         # [NEW][P2]

crates/solana-kit/src/
├── holders.rs                  # [NEW][P2] getTokenLargestAccounts / program-account parsing helpers.
└── fee_transfer.rs             # [NEW][P2] atomic platform-fee transfer appended to swaps (or Jupiter platformFeeBps); skip dust; fee vault from config.

crates/core/src/billing/
└── platform_fee.rs             # [NEW][P2] fee config per plan (bps, caps, minimum) + accounting hooks (post via accounting/posting.rs).

crates/server/src/saas/
├── referrals.rs                # [NEW][P2] referral codes, attribution, payout ledger.
└── wallet_pools.rs             # [NEW][P2] named pools of bound wallets (org already supports multiple wallet bindings); strategy -> pool assignment, round-robin / split sizing.

crates/module-telegram/src/
├── commands.rs                 # [MODIFY][P2] today control / read-only (/status /kill /resume /positions /pnl /mode ...). Add /buy <mint> <sol>, /sell <mint> <pct>, /snipe on|off, /wallets, /limit.
├── callbacks.rs                # [NEW][P2] inline-keyboard confirm / cancel to prevent mis-clicks.
├── trade_session.rs            # [NEW][P2] chat -> tenant binding checks, per-chat limits, confirmation TTL.
└── alerts.rs                   # [MODIFY][P2] fill / exit / risk alerts honouring notification_preferences.

crates/module-polymarket/src/
├── copy.rs                     # [NEW][P2] follow selected wallets: sizing, slippage, market filters; reuse pipeline.rs / orders.rs (module has only value + search strategies today).
├── leaders.rs                  # [NEW][P2] leaderboard / discovery + stats.
└── builder.rs                  # [NEW][P2] builder-code attribution per the current CLOB V2 docs — verify before implementing.

crates/module-copy/src/
└── tracker.rs                  # [NEW][P2] alert-only leader tracking (no trading).

crates/server/src/security/
└── geo_policy.rs               # [NEW][P1] configurable per-module country blocklist using a proxy-provided country header; confirm Polymarket's current restricted jurisdictions with counsel.

apps/control-plane/src/
├── config/branding.ts          # [NEW][P2] product name, logo, colours, support email, legal links from env.
├── theme/tokens.css            # [NEW][P2] design tokens for white-label theming.
├── app/manifest.ts             # [NEW][P2] PWA manifest.
├── app/referrals/page.tsx      # [NEW][P2]
└── app/trading/polymarket/copy/page.tsx   # [NEW][P2]

deploy/
├── helm/sniper-suite/Chart.yaml            # [NEW][P2]
├── helm/sniper-suite/values.yaml           # [NEW][P2]
├── helm/sniper-suite/templates/            # [NEW][P2] deployment, service, ingress, migration job, hpa, networkpolicy, secret refs.
├── terraform/                              # [NEW][P2][OPTIONAL] cloud skeleton.
├── demo/docker-compose.demo.yml            # [NEW][P1] one-command demo stack.
└── demo/seed-demo-tenant.sh                # [NEW][P1] seed clearly labelled DEMO rows (is_demo flag) + UI banner; replaces canned handler data for sales demos.

docs/
└── WHITE-LABEL-GUIDE.md        # [NEW][P2]
```

---

## 8. Packaging, IP, docs, cleanup

```text
./
├── LICENSE                     # [REWRITE][P0] currently MIT "Copyright (c) 2026 sniper-suite authors". MIT lets any holder of a copy redistribute or resell it, which defeats exclusivity of a paid transfer. Replace with a proprietary / commercial licence or an IP-assignment model chosen with counsel; put the real legal entity name.
├── Cargo.toml                  # [MODIFY][P0] license field + publish = false to match (also crates/*/Cargo.toml and programs/staking-suite/Cargo.toml).
├── README.md                   # [MODIFY][P0] stale numbers (says 36 docs, there are 148; test and migration counts differ). Generate counts by script.
├── rustup-init.sh              # [DELETE][P0] 29.9 KB toolchain installer, not product code.
├── .cargo/env                  # [DELETE][P0] sandbox artifact (keep .cargo/config.toml and .cargo/audit.toml).
├── .config/solana/install/     # [DELETE][P0] dev-sandbox directory.
├── data/events.jsonl           # [DELETE][P0] runtime artifact; add data/ to .gitignore.
└── buyer-release/source/       # [DELETE][P0] byte-identical copy of the root tree; regenerate at release time with scripts/rebuild-buyer-release.sh.

legal/
├── IP-ASSIGNMENT-TEMPLATE.md           # [NEW][P0][EXTERNAL] template for counsel to complete (not legal advice).
├── SOURCE-CODE-BILL-OF-SALE.md         # [NEW][P0][EXTERNAL] template for counsel to complete.
├── THIRD-PARTY-NOTICES.md              # [NEW][P0] generate from licenses.csv; include MPL-2.0 packages (e.g. webpki-roots); resolve the UNKNOWN licence of solana-config-program-client 0.0.2.
├── MAINTENANCE-AND-SUPPORT-TERMS.md    # [NEW][P1][EXTERNAL] support window, protocol-drift fix SLA, escrow of credentials.
└── REGULATORY-CHECKLIST.md             # [NEW][P1][EXTERNAL] jurisdictions, who may use Polymarket, money-transmission / securities questions for the buyer's markets.

scripts/
├── generate-stats.sh           # [NEW][P1] write line / test / migration / route counts to docs/STATS.md and fail CI if README disagrees.
└── generate-claims-from-evidence.sh    # [NEW][P1] build the claims tables from evidence/live + test results instead of hand-written "VERIFIED".

docs/                           # 148 files -> keep about 15 canonical ones; move the rest to docs/archive/ or delete
├── ARCHITECTURE.md             # [MODIFY][P0] merge ARCHITECTURE-OVERVIEW.md, HA-ARCHITECTURE.md, DISTRIBUTED*.md.
├── OPERATIONS.md               # [MODIFY][P0] merge OPERATIONS-RUNBOOK.md, ONCALL.md, *-OPERATIONS.md.
├── SECURITY.md                 # [MODIFY][P0] merge SAAS-SECURITY.md, SECURITY-*-MATRIX.md, SECURITY-THREAT-MODEL.md.
├── DEPLOYMENT.md               # [MODIFY][P0] merge BUYER-DEPLOYMENT.md, DEPLOYMENT-ENVIRONMENT-MATRIX.md, TLS-REVERSE-PROXY.md.
├── API.md                      # [MODIFY][P0] merge API-VERSIONING.md, API-COMPATIBILITY-MATRIX.md; point at openapi/.
├── MODULES.md                  # [MODIFY][P0] merge SNIPER-ENGINE.md, COPY-TRADING-ENGINE.md, POLYMARKET-ENGINE.md.
├── TESTING.md                  # [MODIFY][P0] real commands + how to run live tests.
├── KNOWN-LIMITATIONS.md        # [MODIFY][P0] merge FINAL-KNOWN-LIMITATIONS.md; list every [VERIFY] still open.
├── EVIDENCE.md                 # [NEW][P1] how evidence/live is produced and reproduced.
├── BUYER-HANDOVER.md           # [MODIFY][P0] merge the 15+ BUYER-* and FINAL-* handover documents.
├── COMMERCIAL-CLAIM-AUDIT.md   # [REWRITE][P0] says VERIFIED / 100% / "0% dummy data" / keys "never loaded into memory"; replace with the generated claims table.
├── COMMERCIAL-BATCH-1-50-GAP-DELTA.md      # [REWRITE][P0] claims ~95% readiness; replace with the scorecard from this file.
├── COMMERCIAL-MARKET-BENCHMARK.md          # [REWRITE][P0] compares with 3Commas / Hummingbot / WunderTrading; replace by COMPETITOR-BENCHMARK.md.
├── COMMERCIAL-PAGE-BY-PAGE-MATRIX.md       # [REWRITE][P0] regenerate from the real page classification (section 5C).
├── CURRENT-MARKETING-CLAIMS-2026.md        # [REWRITE][P0] drop every claim without evidence/live backing.
├── MARKETING-CLAIMS.md         # [DELETE][P0] duplicate of CURRENT-MARKETING-CLAIMS-2026.md.
├── BATCH-*-COMPLETION-RECORD.md            # [DELETE][P0] 13 files of self-reported completion; archive or delete.
├── COMMERCIAL-BATCH-*-COMPLETION-RECORD.md # [DELETE][P0] same.
├── COMPETITOR-BENCHMARK.md     # [NEW][P1] dated, sourced comparison with Trojan, Photon, Axiom, Banana Gun, BonkBot / GMGN and Polymarket bots (PolyGun, Polyman): features, fees, custody model.
├── PRICING-AND-SALE-MODEL.md   # [NEW][P1] source + IP assignment vs licence vs hosted SaaS; maintenance retainer; which evidence supports which price band (not legal or financial advice).
└── BUYER-DEMO-SCRIPT.md        # [NEW][P1] 20-minute demo path using deploy/demo seed data.
```

---

## 9. Execution order and Definition of Done

**Order**

1. Section 3: make `cargo check --workspace --all-targets` pass, then run the full test suite and record the result.
2. Section 5E gates first (`fresh_tenant_contract.rs`, `forbid-fake-data.sh`, `check-routes-vs-ui.sh`, frontend contract test) — they fail on every fake and define "done".
3. Safety: real kill switch, real MFA truth, remove false claims (FIPS, HSM, SOC2).
4. Section 5A–5D: replace canned handlers, add the 6 missing routes, rewrite hard-coded pages.
5. Section 8 `P0`: licence, cleanup, claim documents.
6. Section 6: live evidence, backtester (or delete it), email / MFA / SSO / reset, drift gates, audit and pentest.
7. Section 7: competitive features, white-label, Helm.

**Definition of Done**

- `cargo fmt`, `clippy -D warnings`, `cargo test --workspace`, `npm run typecheck`, `npm test`, Playwright and all CI gates are green on one commit.
- `fresh_tenant_contract.rs` passes: a new tenant sees empty or zero data everywhere and no sample ids.
- 39 / 39 pages are backed by a real endpoint or removed; no page contains a hard-coded metric or a sample-data fallback.
- Every `/api` route is in the OpenAPI spec and every UI path has a route.
- Every claim left in README / docs / UI has a matching `evidence/live/*.json` with `"status": "PASSED"`.
- `LICENSE` and `legal/*` match the chosen sale model and were reviewed by counsel.
