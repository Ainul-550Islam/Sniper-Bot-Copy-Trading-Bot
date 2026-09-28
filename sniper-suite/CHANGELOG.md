# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The canonical version lives in `[workspace.package].version` in the root
`Cargo.toml`; the `VERSION` file mirrors it and `scripts/release-check.sh`
fails the release if they ever disagree.

## [Unreleased]

### TASK 7B — SaaS product surface (2026-09-23)

Exactly 21 new source paths were added — 20 numbered files plus the tree's
`apps/control-plane/public/logo.svg` — and 5 existing files received the
minimal integration edits required for compilation/routing. No existing
public path, type or behaviour was changed.

#### Added — tenant web console (`apps/control-plane`, Next.js 15 App Router)
- `package.json` (dev/build/start/lint/typecheck scripts, no bloat),
  `tsconfig.json` (strict + `noUncheckedIndexedAccess` + `@/*` aliases),
  `next.config.ts` (env-driven API origin only; no secrets in client env).
- `src/app/layout.tsx` (metadata/providers/global styles), `src/app/page.tsx`
  (auth-aware landing: sign-in/sign-up when signed out, the shell when in).
- `src/lib/api.ts` — typed API client: every refusal becomes a typed
  `ApiError {kind, reason, status}`; the session token and tenant hint travel
  ONLY in headers (`Authorization`, `x-organization`), never in URLs.
- `src/lib/auth.ts` — session get/login/register/logout/refresh/expiry wired
  to the TASK 7A model; the token lives in tab memory only (no
  localStorage/sessionStorage/cookies); the tenant selector is restricted to
  the memberships the server itself reports.
- `src/components/AppShell.tsx` — fourteen sections (Dashboard, Bots,
  Orders, Positions, Risk, Accounting, Reconciliation, Workers, Wallets,
  Team, Audit, Billing, Usage, Settings); trading-truth sections call only
  endpoints that exist and explain the operator-console boundary when a
  tenant session is refused. No routes are invented.
- `src/components/TenantSwitcher.tsx` — the ONLY tenant chooser: the user's
  own organizations, no free-text organization id (the browser is never an
  authorization boundary).
- `src/styles/globals.css` (responsive + a11y: skip link, focus rings,
  semantic structure; no inline style blocks in components) and
  `public/logo.svg`.

#### Added — server SaaS layer (`crates/server/src/saas/`)
- `provider.rs` — the provider-neutral billing adapter boundary REUSING the
  TASK 7A `BillingProvider`/`Subscription` types (no competing abstraction):
  `BillingProviderAdapter` with a default HMAC webhook verification
  (`hex(HMAC-SHA256(secret, "{timestamp}.{body}"))`, ±300 s freshness,
  constant-time compare), a `ManualProvider` adapter that honestly has no
  checkout and no webhooks, a `ProviderRegistry`, and a redacted-`Debug`
  `WebhookSecret`.
- `billing_webhook.rs` — `POST /api/saas/billing/webhooks/:provider`:
  verify-signature → parse `{id,type,data}` → idempotency by durable
  provider event id (migration-0018 runtime records; process-local only
  without a database, documented) → whitelisted transitions
  (`plan.changed`, `subscription.payment_failed|renewed|canceled|expired`)
  through the existing TASK 7A domain methods → entitlements (via the plan
  assignment) → audit → 200 `applied|ignored|duplicate`. Invalid signature =
  401 with zero state change; unknown type = deterministic `ignored`;
  client-supplied status is never trusted; the accounting ledger is never
  touched.
- `openapi.rs` — `GET /api/saas/openapi.json`: stable `operationId`s, typed
  request/response schemas, `writeOnly` one-time secrets, no internal
  endpoint and no secret material in the document.
- `wallet_access.rs` — the tenant → wallet → strategy boundary:
  public-data-only bindings (no field exists for signing material), every
  answer = ownership ∧ live binding ∧ `wallet.manage` ∧ module entitlement;
  cross-tenant access is refused before existence is disclosed. Durable via
  the runtime-record store when PostgreSQL is attached.
- `export.rs` — `GET /api/saas/exports?kind=…`: seven deterministic,
  tenant-scoped sections (`profile`, `members`, `api_keys` as metadata only,
  `usage`, `subscription`, `wallets`, `audit` bounded at 500 rows), each
  audited; session rows, hashes and secrets are unrepresentable in the
  output.

#### Added — server security (`crates/server/src/security/`)
- `headers.rs` — CSP (no wildcard hosts; the dashboard's single inline
  script is the documented reason for `script-src 'unsafe-inline'`),
  `X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`,
  `Permissions-Policy`, HSTS when TLS, `Cache-Control: no-store` on `/api/*`.
  Appends headers only; never blocks the websocket upgrade.
- `websocket.rs` — `GET /api/saas/events`, the PRIMARY tenant event stream:
  session/API-key auth before the upgrade (401, no socket) or a browser
  first-frame auth (10 s window); tenant-scoped delivery (`saas.*` frames
  carry the organization; market-global frames pass); 60 s revalidation
  closes revoked/expired credentials with 1008. The legacy `/api/events`
  keeps its exact previous behaviour.

#### Added — documentation (`docs/`)
- `SAAS-PRODUCT.md`, `SAAS-SECURITY.md`, `SAAS-OPERATIONS.md` — describing
  ONLY implemented behaviour; both security and operations docs state
  explicitly that NO external security audit has been performed and no
  institutional production-readiness is claimed.

#### Changed (minimal integration edits)
- `crates/server/src/main.rs`: inline `mod security { … }` parent (keeps the
  mandated file tree — no extra `security/mod.rs`).
- `crates/server/src/api.rs`: one `route_layer` for the security headers.
- `crates/server/src/saas/mod.rs`: five `pub mod` declarations + route
  merges (openapi.json, webhooks, wallet-access, exports, saas events).
- `crates/server/Cargo.toml`: `hmac`, `sha2`, `hex` (already workspace deps
  used by bot-core) for webhook signature verification.
- `Cargo.lock` follows.

#### Verification
- Frontend: `tsc --noEmit` clean under `strict` +
  `noUncheckedIndexedAccess`; `next build` succeeds (types validated,
  static prerender).
- Backend: `cargo fmt --all --check` PASS; `cargo clippy -p sniper-suite
  --all-targets` — 0 warnings; new-file unit/integration tests: webhook
  idempotency & replay, signature tampering, WS auth resolution, wallet
  isolation & entitlement gate, export determinism & isolation, header set.
- Full workspace with the TASK-7A methodology (`cargo test --workspace -j 2
  -- --test-threads=1`, PostgreSQL 17.11 live): **1183 passed / 0 failed /
  1 ignored** across 51 test binaries (1153 → 1183).
- Environment note (pre-existing, not a TASK 7B change; `crates/core` is
  byte-identical to the previous deliverable): the three
  `db_integration` `audit_chain_*` tests verify the SHARED audit table and
  can transiently observe each other's deliberate tamper when run in
  parallel against one database; under the established
  `--test-threads=1` methodology (and in isolation) they are deterministic
  and green — 26/26.

#### Corrected (same-day continuation audit)

- The audit continuation (FILE 14/15 of the line-by-line pass) surfaced that
  the 7B contract, product doc and web console referenced four billing-read
  endpoints that the server never mounted (`/api/saas/plans`,
  `/api/saas/subscription`, `/api/saas/entitlements`, `/api/saas/usage/:period` —
  the TASK 7A store has the methods but 7A scoped its routes to
  identity/organizations/api-keys). Removed all four from
  `crates/server/src/saas/openapi.rs` (with their now-orphaned schemas) and
  from `docs/SAAS-PRODUCT.md`; the console's Billing and Usage sections now
  read the REAL deterministic exports (`?kind=subscription`, `?kind=usage`),
  and the contract's description states where billing/usage reads live.
  Every documented route now exists on the server. Gates re-run: fmt,
  clippy, `cargo test -p sniper-suite`, `tsc --noEmit`, `next build` — all
  green (20 operations remain in the public contract).

### TASK 7A specification conformance pass (2026-09-22)

#### Verified
- All 25 TASK 7A files were re-checked one by one against the phase spec:
  migration 0017 (12 tables, hash-only secrets), the tenant / membership /
  session / billing / provisioning / authorization domain modules, the
  server `saas/` boundary, vocabulary completeness (8 roles, 22+1
  permissions, 8-value decision set, 7 provisioning states, 4 plan tiers),
  and the A–L test matrix in `crates/core/tests/saas_control_plane.rs`.
- The SaaS/TASK-5 and SaaS/TASK-6 boundaries gained unit-level regression
  tests inside the authorization module itself: an `ALLOW` from the SaaS
  layer cannot move the TASK 5 global-risk verdict, and no authorization
  decision can renew, verify, or release a TASK 6 lease it does not hold
  (stale generations stay fenced after takeover).
- The durable restart test now also proves the API-key guarantees against
  real PostgreSQL: a key created before the restart still authenticates
  after it (hash-only lookup), a cross-replica revocation stops the key
  with the stable `api_key_revoked` reason, and an expired key stops with
  `api_key_expired`.

#### Verification
- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- Full workspace (`CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace -j 2
  -- --test-threads=1`) with PostgreSQL 17.11 live: **1153 passed /
  0 failed / 1 ignored** across 51 test binaries; the ignored test is the
  opt-in replay fixture generator. Executed against PostgreSQL:
  `db_integration` 26/26, the copy two-replica mirror, both durable SaaS
  tests, and the A–L SaaS suite; Redis-dependent and explicitly
  live-network/broadcast gates self-skipped as designed.
- Full server suite: **49 passed / 0 failed**; bot-core: 349 unit tests
  green.

### File-by-file integrity pass (2026-09-22)

#### Verified
- The canonical tree was re-inventoried file by file (342 files): every Rust
  `mod`, `#[path]`, `include_str!`, and `include_bytes!` declaration resolves
  (209 Rust files, zero missing module files; the three
  `tests/common/mod.rs` harnesses are declared by their integration tests),
  all 7 workspace members exist, all 18 migrations are contiguous
  `0001`–`0018`, no `TODO`/`FIXME`/`todo!()`/`unimplemented!()`/placeholder
  markers remain in code or config, no tracked file is empty, and all release
  scripts are executable.
- Completion and limitation documentation was re-synchronised with the
  current evidence (`docs/TESTING.md`, `docs/EVIDENCE-INDEX.md`,
  `docs/BUYER-DUE-DILIGENCE.md`, `docs/FINAL-KNOWN-LIMITATIONS.md`,
  `docs/REPOSITORY-MAP.md`, `docs/DELIVERY-MANIFEST.md`): the historical
  537-test counts are now explicitly dated, and the current 1028-test
  PostgreSQL-verified state is recorded where a reader looks first.
- `release-manifest.json` gained the integrity-pass record; documented counts
  now match the tree (342 files, 18 migrations, 63 docs).

#### Verification
- `cargo fmt --all -- --check`: PASS.
- `cargo check --workspace`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `scripts/verify-delivery.sh`: **7 PASS / 0 FAIL** (342 files, 18
  migrations, 63 docs).
- Workspace test evidence stands at **1028 passed / 0 failed / 1 ignored**
  with PostgreSQL 17.11 from the SaaS durability pass earlier the same day;
  this pass changed documentation and metadata only, so that run remains the
  current code evidence.

### SaaS durability completion (2026-09-22)

#### Added
- Migration `0018_saas_runtime_records.sql` and a PostgreSQL repository for
  users, organizations, memberships, sessions, tenant API keys, plans,
  subscriptions, entitlements, usage events, and provisioning jobs.
- PostgreSQL-gated restart and replica tests for the durable SaaS projection.

#### Fixed
- Production `SaasStore` reads now use PostgreSQL as the authority; runtime
  identities, revocations, usage idempotency, and provisioning request keys
  therefore survive restarts and are shared between replicas.
- Subscription assignment and replacement of plan-derived entitlements now
  commit atomically. Plan catalogue seeding is restart-safe and race-safe.
- Release migration checks and metadata now cover contiguous `0001` through
  `0018`.

#### Verification
- PostgreSQL 17.11: 26/26 core database integration tests passed.
- Server: 49/49 tests passed, including the PostgreSQL-gated all-record
  two-replica test and durable-store restart/atomic-plan test.
- Full workspace: **1028 passed / 0 failed / 1 ignored** with PostgreSQL
  17.11; the ignored test is the opt-in replay fixture generator.
- Workspace check and all-target Clippy with `-D warnings`: passed.

### TASK 7A completeness and repository-integrity pass (2026-09-22)

#### Fixed
- Restored six files omitted from the canonical `sniper-suite/` tree:
  `AUDIT.md`, `docs/{DELIVERY-MANIFEST,EVIDENCE-INDEX,FINAL-DELIVERY,
  FINAL-RELEASE-AUDIT,FORENSIC-FILE-INVENTORY}.md`. This repairs every
  broken handover path and brings the manifest's 63-document count back in
  sync with the tree.
- The deployment organization was described as startup-created but was never
  created. Startup now seeds it before the API is served, so legacy
  deployment credentials can use `/api/saas/*` as documented.
- Human platform administrators could not actually cross tenant boundaries:
  middleware required a membership in the target tenant before it could
  construct platform scope. A persisted `platform_admin` user now receives a
  genuine platform context for the path/header target; ordinary users still
  require exact membership and receive the same non-enumerating refusal.
- Tenant API-key creation compared permission-set *lengths*, which can miss
  different privileges of equal cardinality. Creation now proves the
  requested effective permission set is a subset of the creator's effective
  set, rejects platform roles, unknown scopes, and empty labels.
- Release migration checks now cover contiguous `0001` through `0017` and no
  longer print `0017` as octal `0015`. Delivery/release scripts are executable.
- Formatted the previously unformatted TASK 7A files and resolved every new
  Rust 1.98.1 `clippy -D warnings` finding without changing trading logic.

#### Verification
- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo test -p bot-core -p sniper-suite -j 1 -- --test-threads=1`:
  **492 passed / 0 failed** (all directly changed core/server surfaces).
- `scripts/verify-delivery.sh`: **7 PASS / 0 FAIL**, 63 docs, 17 migrations.
- A full all-crate test link was attempted; the sandbox's 25 GB filesystem
  filled while linking the Solana integration binaries. This is recorded as
  an environment limit, not converted into a passing claim. The unchanged
  staking host suite had already passed 71/71 in this audit session.
- Known boundary kept explicit: migration 0017 defines durable SaaS tables,
  but the current server `SaasStore` adapter remains in-process memory and is
  not yet wired to PostgreSQL; runtime SaaS identities do not survive restart.

### HA — TASK 6 integration completeness pass (2026-09-22)

Audit of the TASK 6 layer against its own specification. Two integration
gaps found and closed; no behaviour removed.

#### Fixed
- `crates/server/src/main.rs` — **the cluster-singleton jobs still ran
  unleased on every replica.** The venue/chain reconciliation sweep
  (`RecoveryWorker`), the hourly retention housekeeping (`run_maintenance`)
  and the periodic position re-verification sweep now run as fenced
  `LeasedWorker`s under the `reconciliation`, `recovery` and `state_sync`
  leases, joining `accounting_maintenance` — four leased singletons. Two
  replicas can no longer race on the same reconciliation claim, delete the
  same retention rows, or enqueue the same position check N times. The
  three remaining unleased loops (reconciliation-backlog snapshot,
  runtime-flag sync, position-book sync) are per-worker local views by
  design and stay per-worker.
- `crates/module-copy/src/feeds.rs` — **the copy poll loop re-seeded its
  cursor with "newest signature now" on every start**, which silently
  skipped every leader trade that happened while the process was down. It
  now resumes each wallet from its durable `copy_logs:<wallet>` cursor and
  advances that cursor after each batch (warm-up seeding is kept only for a
  wallet with no durable position, so a brand-new wallet does not replay its
  history).
- `crates/module-polymarket/src/ws.rs` — **the authenticated user channel
  had no durable position.** It now continues a local delivery counter from
  the `polymarket_user` cursor across process lives, suppresses a replayed
  delivery and records a skipped delivery as a `FeedGap` instead of letting
  it vanish (the fill journal remains the money-level authority).
- Test: `feed_wiring_shapes_survive_a_restart` (opaque per-wallet cursor
  resumes and keeps scopes independent; the sequenced channel cursor
  continues its numbering and reports the gap).
- Docs re-synchronised: `HA-ARCHITECTURE.md` §4 (which singleton jobs are
  leased) and §6 (which feeds are wired and why the other three are not),
  `DISTRIBUTED-OPERATIONS.md` §6b, `FEATURE-TRACEABILITY.md`, `TESTING.md`,
  `release-manifest.json`.

#### Verification
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo test --workspace -j 2 -- --test-threads=1`
**1026 passed / 0 failed / 1 ignored across 50 binaries** (was 1025);
`scripts/verify-delivery.sh` 7 PASS / 0 FAIL (313 files, migrations
0001–0016, 63 docs).

### HA / crash recovery / distributed reliability — TASK 6 (2026-09-22)

One trading system, several workers, one durable state: crash-safe,
deterministic recovery, distributed ownership with fencing, no duplicate
execution, no silently lost events. No new strategy, no new venue; the
TASK 1–5 engines are unchanged except where they needed to become
recoverable/distributed.

#### Added
- `crates/core/src/ha/` — the reliability substrate, one concern per file:
  `worker.rs` (worker identity, registration, heartbeat, the 9-state
  machine `starting → recovering → ready ⇄ running ⇄ degraded`,
  `lease_lost`, `recovery_required`, `draining`, `stopped`; HA modes),
  `lease.rs` (singleton ROLE leases — reconciliation, recovery,
  accounting maintenance, state sync, one per feed — with strictly
  increasing fencing `generation`s and the typed `FenceError`),
  `cursor.rs` (durable feed cursors with duplicate suppression, gap
  detection that is never silently skipped, and deliberate replay),
  `recovery_plan.rs` (the PURE §5/§6 matrices: 12 crash boundaries × 8
  order situations → exactly one deterministic action, and no action that
  resubmits), `store.rs` (`HaStore` contract + `MemoryHaStore` with the
  same atomicity), `runtime.rs` (`HaRuntime`: registration, heartbeat,
  lease guards, `guarded` fenced execution, cursors, the readiness verdict,
  graceful shutdown), `metrics.rs`, `audit.rs`.
- `crates/core/migrations/0016_ha_workers_leases_cursors.sql` — additive:
  `ha_workers` (+ `ha_worker_events`), `ha_leases` (+ `ha_lease_events`,
  fencing generations, one-statement atomic acquisition), `ha_cursors`,
  `ha_feed_gaps`, `ha_recovery_records`.
- `crates/core/src/db/ha.rs` — `HaRepo` over 0016; the acquisition is ONE
  `INSERT … ON CONFLICT DO UPDATE … WHERE expired OR released OR self`, so
  two workers racing cannot both win; renew / release / verify are
  compare-and-set on `(role, holder, generation)`; `now()` exposes the
  shared database clock every liveness comparison uses.
- `crates/server/src/ha.rs` — `DbHaStore`, `attach`, `register_and_recover`
  (journals one deterministic action per unfinished order at startup, plus
  ledger / cursor scope records), `spawn_heartbeat` (heartbeat + stale
  survey + readiness refresh), `LeasedWorker` (acquire → renew → **fence
  before every tick** → step down on loss → release on exit), `shutdown`.
- `GET /api/ha` — worker identity / generation / state / readiness with
  reasons, held roles, the cluster registry with heartbeat ages, all leases
  with holder / generation / expiry, cursors with lag, unresolved gaps and
  recent recovery records.
- `[ha]` configuration: `mode` (`single` / `active_passive` /
  `active_active`), `heartbeat_secs`, `heartbeat_timeout_secs`,
  `role_lease_secs`, `required_roles`; validation refuses an unknown mode,
  an unknown role and a clustered mode without `[database].enabled`.
- Tests: `crates/core/tests/ha_distributed.rs` (17 offline tests against
  real `AppState` workers sharing one store, including the critical proof —
  the same execution event reaching two workers yields exactly one
  execution, one order intent and one ledger effect), 37 unit tests in
  `ha/*`, 4 `[ha]` config tests and a server probe test for the new
  `worker` readiness component.
- Docs: `docs/HA-ARCHITECTURE.md`, `docs/DISTRIBUTED-OPERATIONS.md`,
  `docs/CRASH-RECOVERY.md`; `REPOSITORY-MAP.md`, `FEATURE-TRACEABILITY.md`,
  `BACKUP-RESTORE.md`, `TESTING.md`, `API.md`, `release-manifest.json`,
  `scripts/verify-delivery.sh` (3 more required docs, migrations 0001–0016)
  updated.

#### Changed
- `crates/core/src/state.rs` — `AppState` owns an `HaRuntime`
  (`state.ha()`), built from `[ha]`; the server attaches the durable store
  before registration and recovery.
- `crates/server/src/main.rs` — HA store attached before restore; worker
  registered and recovery journaled right after the TASK 5 ledger recovery;
  heartbeat loop; the accounting maintenance pass now runs as a
  lease-guarded, fenced `LeasedWorker` (exactly one worker in the cluster);
  a dedicated `ha-drain` shutdown phase releases leases and persists cursors
  before the pumps flush.
- `crates/server/src/obs.rs` — the readiness probe gained a `worker`
  component: a worker that has not registered, has not finished recovery,
  lost a required lease (re-verified against the store, never from cache) or
  has an unhealthy dependency never reports READY.
- `crates/server/src/accounting.rs` — the unleased maintenance loop was
  replaced by the leased one; `maintenance_tick` is now that job's body.

#### Verification
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo test --workspace -j 2 -- --test-threads=1`
green; `scripts/verify-delivery.sh` 7 PASS / 0 FAIL (313 tracked files,
migrations 0001–0016, 63 docs). No live venue, funded wallet, database or
multi-host cluster was used — nothing here is a claim of safety,
profitability or institutional production readiness.

### Global risk + accounting — TASK 5 completeness pass (2026-09-22)

File-by-file audit of the TASK 5 layer against the specification. One gap
found and closed; no behaviour removed.

#### Fixed
- `crates/core/src/accounting/reconcile.rs` — **the order layer was missing
  from accounting reconciliation.** Spec §5 requires reconciliation between
  *orders*, fills, ledger and positions; the engine compared trades,
  positions, events and pending ids only, so a `Filled` / `PartiallyFilled`
  OMS order whose fill never reached the ledger (a module fill site failing
  to submit, a crash between the venue fill and the ledger write) produced
  no finding unless a module trade record also existed. `ReconInputs` now
  carries `orders`; an order that reports money moved and that no ledger
  event references — by correlation id (the OMS order id the modules stamp),
  venue signature, external id, or through a module trade naming it
  (`oms=<id>`) — is reported as `unresolved_financial_event` (or
  `missing_ledger_entry` when its trade exists and is itself unbooked).
  Intent-only states (created / validated / queued / submitted / accepted /
  failed / cancelled / expired / unknown / reconciled) are never reported:
  the ledger books fills, not intents. A ledger event whose correlation id
  names a known order is no longer an orphan.
- `AccountingFinding` gained `order_id` (part of the finding identity
  digest and of the audit line); migration `0015` gained the matching
  `accounting_recon_findings.order_id` column and
  `crates/core/src/db/accounting.rs` writes / reads it.
- `crates/server/src/accounting.rs` — the maintenance pass feeds the last
  5,000 OMS orders alongside the positions and trades.
- Tests: `filled_orders_without_a_ledger_event_are_reported`,
  `a_filled_order_matched_by_correlation_signature_or_trade_is_in_sync`,
  `a_ledger_event_explained_only_by_its_order_is_not_an_orphan` (unit),
  `the_order_layer_is_reconciled_against_the_ledger` (integration, against a
  real `OrderManager`), plus an `order_id` round-trip assertion in the gated
  PostgreSQL suite.
- Docs re-synchronised: `ACCOUNTING-LEDGER.md` §7 (four-layer diagram and
  the order rule), `RISK-OPERATIONS.md` §5 (operator response for an
  order-layer finding), `GLOBAL-RISK.md`, `FEATURE-TRACEABILITY.md`,
  `TESTING.md`, `REPOSITORY-MAP.md`, `release-manifest.json`.

#### Verification
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo test --workspace -j 2 -- --test-threads=1`
**966 passed / 0 failed / 1 ignored across 49 binaries** (was 962);
`scripts/verify-delivery.sh` 7 PASS / 0 FAIL (297 files, migrations
0001–0015, 60 docs).

### Global risk + accounting / ledger — TASK 5 (2026-09-21)

One authoritative global risk and accounting layer over the TASK 1–4
engines. No new bot, no new venue, no strategy change; the existing
`RiskEngine`, OMS, execution ledger, reconciliation and recovery paths are
kept and extended. Every new limit defaults to off, so an unconfigured
suite behaves as before.

#### Added
- `crates/core/src/global_risk/` — `GlobalRiskEngine` (`engine.rs`): the
  ONE portfolio-level decision `RiskEngine::check_entry` takes first (step
  2b) before the module checks — process / venue / strategy kill switches,
  reference-rate presence (fail closed), `max_open_positions`,
  `max_order_notional_ref`, portfolio / wallet / venue / strategy / asset
  exposure caps, `max_daily_loss_ref`, `max_drawdown_ref` /
  `max_drawdown_pct`; 14 closed `GlobalRejectReason`s and a journaled
  `DecisionSnapshot` (`decision.rs`); per-venue / per-strategy
  `KillSwitches` (config-pinned or operator-engaged, durable, restored on
  start; `kill_switch.rs`); `RiskStore` + `MemoryRiskStore` (`store.rs`);
  `global_risk_*` metrics; `global.risk.reject` / `global.kill_switch.*`
  audit actions.
- `crates/core/src/accounting/` — the global ledger: typed
  `AccountingEvent` (fill, fee, settlement, deposit, withdrawal, transfer,
  funding adjustment, correction) with the deterministic `event_id` = ONE
  idempotency identity (`event.rs`); balanced double-entry postings per
  event (`posting.rs`); `PositionBook` aggregation per module / venue /
  wallet / strategy / asset / quote / mode with average-cost realized PnL,
  fees and exposure (`book.rs`); `GlobalLedger::submit` — validate, dedup,
  post, book, journal, audit under one lock, pending queue when the journal
  is unavailable (`ledger.rs`); `PortfolioView` in reference units with
  per-venue / wallet / strategy / asset / module slices, utilization and
  `missing_rates` (`view.rs`); `LedgerStore` + `MemoryLedgerStore`
  (`store.rs`); accounting reconciliation with 8 finding kinds that are
  reported, never repaired (`reconcile.rs`); replay-safe restart recovery
  with gaps reported (`recovery.rs`); `global_ledger_*` /
  `global_portfolio_*` metrics; `global.ledger.* / global.position.* /
  global.recon.* / global.recovery.*` audit actions.
- `crates/core/src/db/accounting.rs` — `AccountingRepo` over migration
  `0015_global_risk_accounting.sql` (`ledger_events`, `ledger_postings`,
  `global_positions`, `global_risk_decisions`, `kill_switches`,
  `kill_switch_events`, `accounting_recon_findings`; additive only). The
  event + postings insert is one transaction, idempotent on the event id.
- `crates/server/src/accounting.rs` — `DbLedgerStore` / `DbRiskStore`,
  startup `attach` + `recover` (kill switches, ledger rebuild against the
  restored positions, one reconciliation pass, gauges), periodic
  maintenance loop (`accounting_reconcile_interval_secs`).
- API: `GET /api/accounting/portfolio`, `GET|POST /api/accounting/events`
  (operator-entered deposits / withdrawals / transfers / funding / fees /
  corrections through the same ledger door; fills refused), `GET
  /api/accounting/findings`, `GET /api/risk/global`, `POST
  /api/risk/kill-switch` (venue / strategy scopes).
- `[global_risk]` configuration section (`config.rs::GlobalRiskConfig` +
  `validate_global_risk`, `config.toml.example`): `reference_asset`,
  `reference_rates`, `capital_base_ref`, the limits above,
  `killed_venues`, `killed_strategies`, `accounting_reconcile_interval_secs`.
- `EntryRequest.wallet` / `EntryRequest.strategy` (attribution for the
  global layer); `RiskCode::{GlobalKillSwitch, GlobalExposure,
  GlobalDailyLoss, GlobalDrawdown, GlobalUnavailable}`;
  `AppState::ledger()`, `AppState::global_risk()`, `AppState::marks()`;
  `Ord` on `BotModule`, `Venue`, `ExecutionMode`.
- Tests: `crates/core/tests/global_risk_accounting.rs` (18), 37 unit tests
  in the new modules + 3 config tests, `db_integration.rs::
  global_ledger_repo_round_trips_and_is_idempotent` (gated), 2 API tests,
  ledger assertions in `module-sniper/tests/concurrency.rs`,
  `module-copy/tests/dedup_ordering.rs`,
  `module-polymarket/tests/order_pipeline.rs`.
- Docs: `docs/GLOBAL-RISK.md`, `docs/ACCOUNTING-LEDGER.md`,
  `docs/RISK-OPERATIONS.md`; `REPOSITORY-MAP.md`,
  `FEATURE-TRACEABILITY.md`, `TESTING.md`, `API.md`, `MODULES.md`,
  `BACKUP-RESTORE.md`, `release-manifest.json`,
  `scripts/verify-delivery.sh` (3 more required docs) updated.

#### Changed
- `module-sniper/src/{entry,exit}.rs`, `module-copy/src/{mirror,exit}.rs`,
  `module-polymarket/src/lifecycle.rs` — every fill site submits one typed
  `fill_event_for_trade(..)` to `state.ledger()` after the module's own
  idempotency and position update (signature / venue fill id / paper
  reference as the identity; intent / OMS order id as the correlation).
  Modules never touch the book or the journal.
- `module-sniper/src/{entry,replay}.rs`, `module-copy/src/event.rs`,
  `module-polymarket/src/pipeline.rs` — `EntryRequest` now carries the
  wallet and the strategy label (`sniper`, `copy:<leader>`, the Polymarket
  strategy); `from_risk_code` maps the new global codes onto the existing
  reject vocabularies.
- `crates/core/src/risk.rs` — `check_entry` step 2b calls the global
  engine; `is_exposure_limit` includes the global exposure / daily-loss /
  drawdown codes.
- `crates/core/src/state.rs` — `AppState::new` constructs the ledger and
  the global engine (memory journals until the server attaches PostgreSQL);
  `update_config` re-applies `[global_risk]` to the running engine.

#### Verification
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo test --workspace -j 2 -- --test-threads=1`
green (counts in `release-manifest.json`); `scripts/verify-delivery.sh` 7
PASS / 0 FAIL. No live venue, funded wallet, database or price feed was
used; nothing here is a claim of safety or profitability.

### Polymarket engine — module layout aligned with TASK 1–3 (2026-09-21)

Structure only — no behaviour change, no new features, no public API
change. `crates/module-polymarket/src/lib.rs` (3,892 lines: engine,
lifecycle, reconciliation, recovery, journal, metrics, audit, funding and
venue session in one file) is split into one file per concern, the layout
`module-copy` already uses. Every function body was moved verbatim; the
only code edits are visibility (`pub(crate)` where a sibling file now calls
a helper) and the repeated inline metric registrations, which now go
through one helper per series in `metrics.rs` (same names, help texts and
labels — `/metrics` output is unchanged).

#### Added
- `module-polymarket/src/discover.rs` — `scan_once`, `discover`,
  `quotes_for`, `process_market`, `build_signal`.
- `module-polymarket/src/pipeline.rs` — `process_signal`, `run_pipeline`,
  `sign_signal`, `post_live`, `record_outcome`, `PipelineCtx`, the
  in-flight guard, `new_tracked`.
- `module-polymarket/src/lifecycle.rs` — `apply_observation`, `book_fill`,
  `poll_orders_once`, `apply_user_event`, `cancel_tracked_order`,
  `cancel_all_tracked`, `finish_locally`, `confirm_fill_and_kill_quantity`,
  tracker-map and OMS-transition helpers, `TERMINAL_RETENTION_SECS`.
- `module-polymarket/src/reconcile.rs` — `ReconKind`, `ReconFinding`,
  `reconcile_once`.
- `module-polymarket/src/recovery.rs` — `RecoveryAction`, `RecoveryReport`,
  `recover_after_restart`.
- `module-polymarket/src/funding.rs` — `CollateralSnapshot`,
  `available_collateral`, `read_collateral`, `ensure_live_funding`,
  `resolve_sizing_balance` (+ its five unit tests), `PAPER_USDC_BALANCE`,
  `COLLATERAL_CACHE_TTL_SECS`.
- `module-polymarket/src/store.rs` — the former inline `store` module
  (`PolyStore`, record re-exports, `MemoryPolyStore`) plus `journal_order`
  and the in-memory journal unit test. The path `module_polymarket::store::*`
  is unchanged.
- `module-polymarket/src/metrics.rs` — one helper per `poly_*` series (and
  the two shared `bot_*` families the engine feeds), the series table,
  `publish_gauges`, `QUOTE_AGE_BUCKETS_MS`.
- `module-polymarket/src/audit.rs` — `AUDIT_ACTOR`, `PolyBot::audit`,
  `sanitize`, the `poly.*` action vocabulary.
- `module-polymarket/src/venue.rs` — `order_status`, `ensure_api_key`,
  `authed_client`, `spawn_heartbeat`, `cancel_all`, `flatten`.

#### Changed
- `module-polymarket/src/lib.rs` — now holds the crate docs (with a module
  map), module declarations, root re-exports (`ReconKind`, `ReconFinding`,
  `RecoveryAction`, `RecoveryReport`, `CollateralSnapshot`, `AUDIT_ACTOR` —
  every previously public path still resolves), `PolyBot` + constructor /
  builders / accessors and the run loop (586 lines).
- `docs/POLYMARKET-ENGINE.md` §2, `docs/REPOSITORY-MAP.md`,
  `docs/FEATURE-TRACEABILITY.md`, `release-manifest.json` — component map,
  tree and counts (272 tracked files under `sniper-suite/`) re-synchronised.

#### Verification
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo test --workspace -j 2 -- --test-threads=1`
901 passed / 0 failed / 1 ignored (unchanged: the same 94 unit tests now
run from `funding::tests`, `store::tests` and the root `tests` module; 53
integration tests unchanged); `scripts/verify-delivery.sh` 7 PASS / 0 FAIL.

### Polymarket engine — TASK 4 verification pass (2026-09-21)

Buyer-grade line-by-line verification of the TASK 4 engine. No new
features; four accounting/confirmation gaps found in the order lifecycle
were closed, each with a regression test.

#### Fixed
- `module-polymarket/src/orders.rs` — `TrackedOrder::apply_venue`: a venue
  `matched` for a **fill-and-kill (`FAK`)** order no longer books the whole
  order size. FAK reports `matched` for any non-zero fill and kills the rest,
  so the killed remainder was being fabricated into a fill (position and
  ledger over-booked). Now: an explicit `size_matched` closes the order at
  exactly that quantity (`filled`; `cancelled` when zero), a status-only
  `matched` (the `POST /order` answer) books nothing and leaves the order
  open, venue-acknowledged, until a poll / user-channel trade / reconciliation
  reports the quantity. `GTC`/`GTD`/`FOK` semantics are unchanged (those types
  report `matched` only once everything matched). New helpers
  `is_fill_and_kill`, `venue_acknowledged`, `remember_trade`; unit test
  `fak_matched_never_fabricates_the_killed_remainder`.
- `module-polymarket/src/lib.rs` — after a FAK `matched` POST answer the
  pipeline reads `GET /data/order` once for the quantity
  (`confirm_fill_and_kill_quantity`); the poll and reconciliation
  "absent from venue" paths distinguish orders the venue never acknowledged
  (`marked_failed`) from acknowledged ones (`marked_unknown`) using the last
  venue status instead of the local state alone; poll / reconciliation /
  cancel observations pass the venue's `size_matched` as reported
  (`ClobOrder::size_matched_opt`, new) so a missing field is "not reported",
  never a fabricated zero, and an open-list row without a size is not a
  `matched_size_mismatch`.
- `module-polymarket/src/lib.rs` — **cancel confirmation.** A `DELETE /order`
  answer that names neither `canceled` nor `not_canceled` (empty body /
  unrecognised shape) was treated as a confirmed cancel, closing the order
  locally while it could still be live on the venue — invisible to polling
  (terminal) and to orphan detection (still tracked). Now only the `canceled`
  list confirms; an unrecognised answer triggers a `GET /data/order` re-read
  applied through the lifecycle machine (`cancelled`/`expired` → closed with
  the partial fill kept; still `live` → stays open and is retried; absent →
  left to the poll's "vanished" path). `cancel_all` (kill switch) closes only
  the ids the venue lists as cancelled; orphan cancels in reconciliation
  report `cancelled` / `cancel_refused` / `cancel_unconfirmed` /
  `cancel_failed` (metric `poly_cancel_total{reason="recon"|"cancel_all"}`).
  Integration tests `unrecognised_cancel_answer_is_never_a_confirmation`,
  `fak_matched_books_only_the_venue_reported_quantity`;
  `cancel_all_reflects_the_venue_wipe_locally` now also proves an
  unconfirmed order stays open until venue truth closes it.
- `module-polymarket/src/lib.rs` — **fill replays after a restart.** The
  in-memory `booked_trade_ids` list is lost on restart, so a user-channel
  `trade` re-emitted after the restart (`MINED`/`CONFIRMED` for a trade booked
  before it) advanced the local cumulative even though the durable `poly_fills`
  row stopped the ledger from booking it again — leaving the tracker ahead of
  the venue. `apply_observation` now applies observations on a copy and
  commits only when the fill journal accepted the fill: a per-trade replay is
  discarded (cumulative, state, position and OMS untouched; trade id
  remembered), a cumulative catch-up whose fill row already exists still
  advances the snapshot to venue truth without re-booking (`book_fill` returns
  whether it booked). Integration test
  `fills_replayed_after_restart_are_booked_once`.
- `module-polymarket/src/orders.rs`, `src/ws.rs`, `src/lib.rs` — **per-trade
  deltas vs polled cumulatives.** A user-channel `trade` whose fill a poll had
  already captured (the channel was reconnecting; first sighting = the
  `MINED` re-emission) was booked again as `size_matched + delta`.
  `TrackedOrder` now keeps `trade_matched` (sum of distinct per-trade deltas
  seen in this process) and books only its excess over the reported
  cumulative; cumulative observations carry `associate_trades`
  (`VenueObservation::associate_trades`, parsed from `GET /data/order`, the
  open list and — new field — `UserEvent::Order`) which are remembered as
  booked ids and re-base `trade_matched`, so new trades still book
  immediately while poll-captured ones are no-ops. Unit test
  `late_trade_events_after_a_poll_are_never_booked_twice`; integration test
  `trade_events_a_poll_already_captured_are_not_booked_twice`. The existing
  `trade_deltas_accumulate_and_replays_are_ignored` now asserts the
  corrected semantics (a late trade books the excess only; a distinct-trade
  sum beyond the order size is still a lifecycle error).

#### Changed
- `module-polymarket/src/lib.rs` — the restart-recovery log's `cleaned`
  count includes `failed_unsent`; `post_live` doc comment de-duplicated; the
  run loop's `Tick::User` arm boxes the event (`UserEvent::Order` grew by
  `associate_trades`; clippy `large_enum_variant`).
- `module-polymarket/tests/common/mod.rs` — mock venue gained
  `CancelBehaviour::Unrecognised` (`{}`), `sign_for` (derives the venue
  order id the engine will use so a test can script the venue's answer before
  the POST) and `set_order_with_trades` / `venue_order_with_trades`
  (`associate_trades` payloads); `tests/crash_recovery.rs::journal_row`
  journals the venue status the engine actually writes for each state.
- `docs/POLYMARKET-ENGINE.md` (§8.1, §8.2 incl. replay semantics, §8.4,
  §9, §12 metrics, §13 test map, §14 FAK limitation),
  `docs/POLYMARKET-OPERATIONS.md` (FAK first-run check, cancel semantics),
  `docs/POLYMARKET-RECOVERY.md` (replayed fills, `cleaned` count,
  reconciliation actions), `docs/TESTING.md`, `release-manifest.json`.

#### Verification
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`: clean.
- Workspace: 901 passed / 0 failed / 1 ignored (`cargo test --workspace --
  --test-threads=1`, 48 binaries); module-polymarket 94 unit + 53
  integration; server 34.

### Polymarket engine — TASK 4 completeness pass (2026-09-21)

Follow-up that closes the gaps a repository-wide audit found after TASK 4:
positions were the one Polymarket artefact never checked against the chain,
one signing misconfiguration was only caught by the venue, and several
repository documents still described the tree as it was before TASK 1–4.

#### Added
- `server/src/recon.rs` — `PolymarketPositionTruth` (claim kind
  `polymarket_position`): every open LIVE Polymarket position is re-verified
  against the funder's settled outcome-token balance (CTF ERC-1155
  `balanceOf` via the existing `module_polymarket::ctf` reader, six
  decimals) through the shared reconciliation engine. The Solana position
  source's outcome handling was hoisted unchanged into
  `settle_position_outcome` / `flag_position` / `fills_for_position` so both
  venues share ONE correction policy (flags; fill-justified corrections
  only). A non-terminal Polymarket OMS order on the token counts as
  "execution in flight"; a failed read retries and is never a zero
  (`bot_external_state_read_errors_total{source="polygon_ctf"}`).
  `PolymarketOrderTruth::position_truth()` shares the read-only `PolyBot`
  handle. Unit test: `ctf_observation_uses_six_decimals_and_never_wraps`.
- `server/src/main.rs` — the periodic position recheck
  (`recovery.position_recheck_interval_secs`) now enqueues
  `polymarket_position:<id>` for live Polymarket positions (only when
  `[polymarket].ctf_rpc_url` is configured; `Venue::Paper` still skipped)
  instead of skipping them; startup-gate attribution treats the kind like
  `position` (symbol entry-gate) and `modules_for_recon_kind` maps it to
  Module 3 only (tests extended).
- `module-polymarket/src/orders.rs` — `sign_order_bundle` refuses
  `signature_type = 0` with a `funder_address` that is not the signing key's
  address (`SIGNING_FAILED` before any live POST; types 1–3 unchanged). Unit
  test `eoa_signing_refuses_a_funder_that_is_not_the_signer`.

#### Changed
- `docs/REPOSITORY-MAP.md` — tree and counts now describe the current
  repository (14 migrations, 95 src + 38 test files, 19 replay fixtures,
  57 docs, 262 tracked files; the historical frozen-package table is kept).
- `.env.template` — Polymarket secret env names
  (`POLYMARKET_PRIVATE_KEY` / `POLYGON_PRIVATE_KEY`, `POLY_API_*`).
- `docs/ARCHITECTURE.md`, `docs/SECURITY.md`, `docs/OPERATIONS.md`,
  `docs/BACKUP-RESTORE.md`, `docs/RECONCILIATION.md`, `docs/MODULES.md`,
  `docs/TESTING.md`, `docs/POLYMARKET-ENGINE.md`,
  `docs/POLYMARKET-OPERATIONS.md`, `docs/POLYMARKET-RECOVERY.md` — module
  rows, secret handling, failure modes, durable tables (0012–0014), the new
  claim kind and the position re-verification path.

#### Verification
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`: clean.
- Workspace: 895 passed / 0 failed / 1 ignored (`cargo test --workspace --
  --test-threads=1`, 48 binaries); module-polymarket 92 unit + 49
  integration; server 34.

### Polymarket trading engine — TASK 4 (2026-09-21)

Module 3 extended into a staged, deterministic Polymarket engine on top of
the existing CLOB/Gamma clients, the CLOB V2 EIP-712 signer, the collateral
and CTF readers and the two shipped strategies. Full reference:
`docs/POLYMARKET-ENGINE.md`; runbook: `docs/POLYMARKET-OPERATIONS.md`;
crash/restart procedure: `docs/POLYMARKET-RECOVERY.md`. Paper remains the
default; nothing here is a claim of safety or profitability.

#### Added
- `module-polymarket/src/orders.rs` — `OrderSignal` (frozen intent) with a
  semantic `intent_key` (market, token, side, price on the venue tick, size
  on the 0.01 grid, order type, expiry, mode; `created_at` excluded),
  `PolyStage` (17 stages), 36 machine-readable `RejectReason` codes,
  `LocalOrderState` machine with checked transitions, `VenueOrderState`
  parsing, `TrackedOrder::apply_venue` (cumulative-fill accounting: deltas
  only, over-fill refused, `trade_id` booked once, stale lower cumulatives
  ignored), deterministic `fill_id`s.
- `module-polymarket/src/lib.rs` — `PolyBot::process_signal` pipeline
  (validate → market/quote gates → exposure → size → **the one risk
  decision** → collateral verification → OMS idempotency → fenced ownership
  permit `poly:entry:<token>` → sign → POST), `process_market`,
  `build_signal`, `ingest_quote`; live lifecycle: `poll_orders_once`
  (status poll + TTL / GTD expiry / reprice cancels), `apply_user_event`
  (authenticated user channel), `cancel_all_tracked` (shutdown),
  `reconcile_once` (six finding kinds; orphan cancel opt-in),
  `recover_after_restart` (journal re-adoption with booked quantity,
  ambiguous submits held, OMS-only adoption, stale paper / never-sent
  orders failed, post-recovery reconciliation); write-ahead journaling of
  the venue claim before the POST; `PolyStore` journal trait +
  `MemoryPolyStore`; `poly_*` metrics and `poly.signal.* / poly.order.* /
  poly.recon.* / poly.recovery.*` audit actions.
- `module-polymarket/src/ws.rs` — `run_user_feed` (auth frame, market
  subscription, 10 s PING, 1 → 30 s backoff, clean stop on receiver drop),
  `parse_user_message` → `UserEvent::{Order, Trade}`.
- `module-polymarket/src/clob.rs` — typed `ClobOrder`, `order` (404/null →
  `Ok(None)`), `open_orders`, `trades`, cancel helpers, heartbeat; errors
  classified into definite rejections vs transport ambiguity
  (`PolyError::SubmitUnknown`, `Lifecycle`, `Journal`, `InsufficientFunding`,
  `BalanceUnavailable`, `NotConfigured` in `error.rs`).
- `module-polymarket/src/strategy.rs` — explicit `market_gate` /
  `quote_gate`, `Verdict::{Enter, Skip}` with 18 stable `SkipReason` labels,
  `evaluate_market`, NaN-safe price checks, 0.01 size rounding; decision
  rules of `value` / `search` unchanged.
- `core/src/risk.rs` — `check_polymarket_coded` (resting-order cap, total
  and per-market exposure including resting buys) and the Polymarket branch
  of `check_entry` (`poly_max_position_quote`, `poly_max_concurrent_positions`,
  `poly_daily_loss_limit_quote`, `poly_emergency_disable`, price band,
  liquidity floor); `RiskCode::{PolyOpenOrderCap, PolyMarketExposure,
  PolyDailyLoss, PolyEmergencyDisabled}`.
- `core/src/config.rs` — `[polymarket]` `max_spread`, `min_liquidity_usd`,
  `quote_max_age_secs`, `min_time_to_resolution_secs`, `min_order_size`,
  `order_poll_interval_secs`, `order_ttl_secs`, `reprice_threshold`,
  `reconcile_interval_secs`, `reconcile_cancel_orphans`,
  `use_user_websocket`, `cancel_on_shutdown`; `[risk]`
  `poly_max_position_quote`, `poly_max_total_exposure_quote`,
  `poly_max_market_exposure_quote`, `poly_max_concurrent_positions`,
  `poly_max_open_orders`, `poly_daily_loss_limit_quote`,
  `poly_emergency_disable`; `POLYMARKET_*` / `POLY_*` env overrides;
  validation (ranges, order type, GTD expiry, warnings for reconciliation
  off / orphan cancel on / no heartbeat and no cancel-on-shutdown);
  `config.toml.example` documents every key and
  `bundled_example_config_parses` asserts it.
- `core/migrations/0014_polymarket_trading.sql` (additive) — `poly_signals`,
  `poly_orders`, `poly_fills`, `poly_recon_findings`; `core/src/db/polymarket.rs`
  `PolyRepo`; `server/src/recon.rs` `DbPolyStore` (journal-error metering);
  `server/src/main.rs` wires the store and the ownership registry into the
  bot.
- Tests: `module-polymarket/tests/common/mod.rs` (in-process axum mock of
  the CLOB, Gamma, Polygon JSON-RPC and the user websocket),
  `order_pipeline` (10), `order_lifecycle` (9), `user_ws` (4),
  `idempotency_concurrency` (4), `reconciliation` (6), `crash_recovery` (6),
  `strategy_sizing` (5); unit tests in every touched module (91 in the
  crate); `core/tests/db_integration.rs` migration 0014 + `PolyRepo`
  round-trips; risk-engine unit tests for every `poly_*` cap.
- Docs: `docs/POLYMARKET-ENGINE.md`, `docs/POLYMARKET-OPERATIONS.md`,
  `docs/POLYMARKET-RECOVERY.md`; `README.md`, `docs/MODULES.md`,
  `docs/TESTING.md` updated.

#### Changed
- `module-polymarket/src/lib.rs` — the legacy scan loop now feeds
  `process_market`; fills are booked exclusively through
  `AppState::record_trade` (the duplicate `Fill` publish was removed);
  resting orders are tracked and cancelled on shutdown.
- `server/src/persist.rs` — `OrderSent` / trade events for venue
  `polymarket` attach the CLOB order id as the OMS external id and enqueue
  `polymarket_order` reconciliation (no Solana-signature assumptions).
- `module-polymarket/src/collateral.rs` — `verify_funding` accounts for
  collateral already reserved by resting buy orders.

#### Fixed
- `post_live` sent the configured `polymarket.order_type` verbatim (a
  lower-case value reached the venue; a config edit between decision and
  POST could change the type). The POST now uses the frozen signal's
  normalised type.
- `strategy.rs` quote gate: `!(x > 0.0)` style checks replaced by explicit
  finiteness checks (`clippy::neg_cmp_op_on_partial_ord`), same NaN
  semantics.
- Restart recovery no longer leaves never-signed or paper/simulate OMS
  orders as `Unknown` forever (`RecoveryAction::FailedUnsent`).
- `risk.rs` unit test `poly_daily_loss_limit_is_module_scoped` disables the
  generic (SOL-denominated) daily cap so it exercises only the USDC-scoped
  one; the currency mix of the shared counter is documented
  (`docs/POLYMARKET-ENGINE.md` §7).

#### Verification
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`: clean.
- Workspace: 893 passed / 0 failed / 1 ignored (`cargo test --workspace --
  --test-threads=1`, 48 binaries); module-polymarket 91 unit + 49
  integration.

### Copy-trading engine — TASK 3 file-layout reconciliation (2026-09-21)

Follow-up pass that reconciles the implementation with the TASK 3 file
tree, file by file. No behaviour change for the pipeline, the feeds, the
risk engine, the ledger or the journal; intent ids are byte-identical.

#### Changed
- `module-copy/src/event.rs` — now also hosts the staged pipeline:
  `CopyBot::process_event`, its exit stage (`mirror_exit`) and the terminal
  bookkeeping (`finish_rejected` / `finish`) moved here from `mirror.rs`
  unchanged, next to the `CopyStage` / `RejectReason` vocabulary they
  drive.
- `module-copy/src/mirror.rs` — reduced to what the tree assigns it: the
  legacy `CopyBot::mirror_trade` door (kept, routes through
  `process_event`), the entry execution paths (`buy`, `buy_on_curve`,
  `buy_via_jupiter`, `record_buy`, `BuyReport`), `journal_record`,
  `size_for`, `short`.
- `module-copy/src/intent.rs` — owns the hardened exit identity:
  `ExitRoute`, `exit_intent_id` (position id + mint + open time + route +
  raw quantity + quantity held; same parts as the `exit.rs` helper it
  replaces), `exit_label`, `is_exit_label`; unit test for determinism per
  decision and uniqueness per position lifetime.
- `module-copy/src/exit.rs` — consumes `intent::exit_intent_id` /
  `intent::exit_label` (local `exit_intent_id` removed); executor / ledger
  / write-ahead journal / ownership flow untouched.
- `module-copy/src/feeds.rs` — documentation only: the two marks are now
  spelled out (feed `mark_signature_seen` = fetch suppression in the `sig`
  namespace; the pipeline's `AppState::mark_copy_event_seen` = the one
  authoritative, exactly-once decision mark) and why the run loop must
  never re-mark `sig`. Feed behaviour is byte-for-byte unchanged.
- `module-copy/src/lib.rs` — module map updated.
- `docs/COPY-TRADING-ENGINE.md`, `docs/TESTING.md` — component map, exit
  id parts and test counts updated.

#### Tests
- `module-copy/tests/event_pipeline.rs` —
  `live_terminal_states_filled_ambiguous_and_exit_mirrored`: live mode
  against the mock node reaches `FILLED`, `AMBIGUOUS` (dropped broadcast,
  parked intent, position booked, no cooldown), `EXIT_MIRRORED` (ledger
  record under the hardened exit id, `copy-exit-*` label, link closed),
  `REJECTED` (duplicate) and `FAILED` (node rejection) in one flow and
  asserts every one of the 15 `copy_stage_total` stages moved.
- `module-copy/tests/geyser_feed.rs` — a Geyser-delivered trade is fresh
  for the authoritative dedup after the feed's `sig` mark (the pre-TASK-3
  starvation would fail this), decided exactly once.
- `module-copy/tests/copy_feed.rs` — PumpPortal deliveries pre-mark
  nothing; buy and sell of one mint are distinct events, each decided once.
- `module-copy/src/intent.rs` — `exit_ids_are_deterministic_per_decision_and_unique_per_position_lifetime`.
- Workspace: 816 passed / 0 failed / 1 ignored (`cargo test --workspace
  -- --test-threads=1`, 41 binaries).

### Copy-trading engine — TASK 3 (2026-09-21)

Module 2 extended into a staged, deterministic copy-trading engine on top of
the existing feeds, execution paths and exit sweeper. Full reference:
`docs/COPY-TRADING-ENGINE.md`; runbook: `docs/COPY-TRADING-OPERATIONS.md`;
crash/restart procedure: `docs/COPY-TRADING-RECOVERY.md`.

#### Added
- `module-copy/src/event.rs` — canonical `LeaderTradeEvent` (deterministic
  `cev_…` id from leader + signature + mint + side, source + delivery
  sequence, chain vs observed time, shape validation `EventDefect`), the
  stage vocabulary `CopyStage`, 29 machine-readable `RejectReason` codes,
  `Rejection`, `CopyOutcome`.
- `module-copy/src/leader.rs` — `LeaderRegistry`: `ACTIVE ⇄ PAUSED → REMOVED`
  lifecycle with re-follow, config hot-reload sync (follow / rule change /
  pause / resume / unfollow), per-leader counters, restore from the journal.
- `module-copy/src/event_dedup.rs` — the one authoritative dedup
  (`AppState::mark_copy_event_seen`, facade namespace `copy_event`), seeding
  from the journal.
- `module-copy/src/event_ordering.rs` — per-leader slot cursors,
  advisory/strict out-of-order handling, per-source sequence-gap detection.
- `module-copy/src/policy.rs` — pure mirror/skip decision (leader state,
  replay guard, venue decoders, leader minimum, two staleness knobs, symbol
  gate, sniper overlap, already-holding; exit fraction for leader sells).
- `module-copy/src/sizing.rs` — pure sizing (`fixed` / `proportional`,
  wallet cap, `max_sol_per_trade`, `max_balance_fraction`, dust floor,
  NaN/∞/≤0 handling) with the clamps that bound.
- `module-copy/src/intent.rs` — deterministic entry intent ids (same parts
  as before), labels, write-ahead journal records, `CopyIntent`.
- `module-copy/src/reconcile.rs` — leader ↔ follower reconciliation over
  links, the position book, journaled leader activity and the execution
  ledger: `leader_exited_we_hold`, `leader_removed_we_hold`,
  `link_without_position`, `quantity_mismatch`, `orphan_position`,
  `ambiguous_entry` with `Flag` / `MirrorExit` / `CloseLink` / `UpdateLink`.
- `module-copy/src/recovery.rs` — `CopyStore` trait + `MemoryCopyStore`,
  pure `plan_recovery` (`SeedDedup`, `SeedCursor`, `HoldAmbiguous`,
  `CleanupFailedEntry`, `RestoreLink`, `CloseLink`).
- `module-copy/src/metrics.rs` — `copy_*` counters / gauges / histograms
  and `LatencyTimeline`; `module-copy/src/audit.rs` — `copy.entry.*`,
  `copy.exit.*`, `copy.leader.*`, `copy.recon.*`, `copy.recovery.*`.
- `CopyBot::process_event` (staged pipeline), `with_copy_store`,
  `sync_leaders`, `recover_after_restart`, `reconcile_once`, `leaders()`,
  `store()`, `ordering()`; the run loop now recovers at startup, syncs
  leaders on hot reload and reconciles on an interval.
- `crates/core/migrations/0013_copy_trading.sql` (`copy_leaders`,
  `copy_leader_events`, `copy_events`, `copy_links`) and
  `crates/core/src/db/copy.rs` (`CopyRepo`); server `DbCopyStore` wired in
  `spawn_modules` when Postgres is configured.
- Risk: `RiskEngine::check_copy_coded`, `leader_exposure`,
  `pending_copy_entries`, copy throttles inside `check_entry`, `RiskCode`
  variants `CopyEmergencyDisabled`, `CopyDailyLoss`, `CopyPendingCap`,
  `CopyFailedEntryCooldown`, `CopyLeaderExposure`, `CopyCooldown`,
  `StaleSignal`; `RiskConfig::copy_position_cap` / `copy_position_limit`.
- Config: `[copy]` `max_event_age_secs`, `strict_ordering`,
  `max_sol_per_trade`, `max_balance_fraction`, `min_mirror_sol`,
  `reconcile_interval_secs`, `reconcile_auto_exit`,
  `recovery_lookback_hours`; `[[copy.wallets]]` `paused`,
  `max_exposure_sol`, `max_open_positions`; `[risk]`
  `copy_max_position_quote`, `copy_max_total_exposure_quote`,
  `copy_max_concurrent_positions`, `copy_max_pending_executions`,
  `copy_failed_entry_cooldown_secs`, `copy_daily_loss_limit_quote`,
  `copy_max_leader_exposure_quote`, `copy_emergency_disable`; matching
  `COPY_*` env overrides; `validate_copy_engine`.
- State: `mark_copy_event_seen`, `copy_event_seen`,
  `seen_copy_event_count`.
- Tests: 8 new module-copy integration suites (39 tests) reusing the sniper
  mock-node harness, 50 unit tests across the new modules, bot-core risk /
  config tests, gated `pg_copy_journal_roundtrip`.

#### Changed
- `module-copy/src/mirror.rs` — `mirror_trade` now wraps the staged
  pipeline; the execution paths (`buy_on_curve`, `buy_via_jupiter`,
  `record_buy`) are unchanged in behaviour and report a `BuyReport`;
  `size_for` delegates to `sizing::rule_size` (same arithmetic).
- `module-copy/src/exit.rs` — copy exit intent ids now include the
  position's mint and open time (the sniper's scheme) so two positions can
  never share an exit identity across process lives.
- `AppState::note_failed_entry` retention now also honours
  `risk.copy_failed_entry_cooldown_secs`; `CopyWallet` derives `PartialEq`.
- `scripts/verify-delivery.sh` requires migrations 0001–0013;
  `release-manifest.json` high-water mark `0013`.

#### Fixed
- Events from the `logs_poll` and `transaction_subscribe` feeds were dropped
  as duplicates before ever reaching the mirror: the feeds mark each decoded
  signature as seen and `CopyBot::run` re-marked the same key. The pipeline
  now owns dedup in its own namespace keyed by the event; the feed mark is
  fetch suppression only (regression test
  `dedup_ordering::feed_signature_marks_never_starve_the_pipeline`).
- `[copy].decode_*` flags were accepted but never enforced; the policy stage
  now refuses trades on a venue whose decoder is off (`VENUE_DISABLED`).

### Sniper engine — TASK 2 "production-grade sniper engine" (2026-09-21)

Module 1 rebuilt as one deterministic, staged pipeline over a unified launch
event, for pump.fun, PumpSwap and Raydium AMM v4 launches, on top of the
execution reliability layer below. Full reference: `docs/SNIPER-ENGINE.md`.

#### Added
- `module-sniper/src/event.rs` — unified `LaunchEvent` (deterministic
  `evt_…` id from protocol + mint + pool + signature/slot/sequence,
  protocol, source feed, slot, signature, mint, creator, pool, base/quote,
  liquidity and initial price when known, event/source/observed timestamps,
  per-source sequence, raw payload hash, legacy `TokenLaunch`), shape
  validation (`EventDefect`), staleness, cross-feed consistency, the one
  dedup key, `SequenceTracker` for gap/reorder detection.
- `module-sniper/src/pipeline.rs` — lifecycle `DETECTED → VALIDATED →
  RISK_APPROVED → EXECUTION_READY → SUBMITTED → CONFIRMED` (+ `REJECTED` /
  `FAILED`) with an enforced transition table, 19 machine-readable
  `RejectReason` codes (`INVALID_EVENT`, `STALE_EVENT`, `DUPLICATE_EVENT`,
  `INSUFFICIENT_LIQUIDITY`, `SLIPPAGE_LIMIT`, `PRICE_IMPACT_LIMIT`,
  `FEE_LIMIT`, `EXPOSURE_LIMIT`, `RISK_REJECTED`, `EXECUTION_UNAVAILABLE`,
  `KILL_SWITCH`, `STRATEGY_DISABLED`, `INVALID_ROUTE`, `INVALID_STATE`,
  `TOKEN_STATE_INVALID`, `POOL_NOT_READY`, `CONCENTRATION_LIMIT`,
  `SYMBOL_GATED`, `OWNERSHIP_LOST`), `LatencyTimeline`, route selection,
  the pure `precheck` shared by the live path and replay, and the pure fee
  budget `check_fee_budget` / `estimate_entry_fee` (check 15: the executor's
  `FeePolicy` must accept the configured priority fee, and the worst-case
  transaction fee — 5 000 lamports base + priority fee at the policy
  ceiling × compute units + Jito tip — must fit
  `sniper.max_entry_fee_lamports`; a certain fee-policy veto is therefore
  reported as `FEE_LIMIT` before any attempt or cooldown instead of as a
  failed submission).
- `module-sniper/src/gates.rs` — configurable, individually testable safety
  gates over a protocol-neutral `MarketSnapshot` (pool state, Raydium open
  time, mint/freeze authority, minimum liquidity, sane price/decimals,
  creator concentration, pool supply fraction, snapshot freshness) with
  pass/fail/skip outcomes and `strict_gates`.
- `module-sniper/src/slippage.rs` — slippage engine (`fixed`,
  `liquidity_aware`, `price_impact`; per-token → per-protocol → strategy
  precedence; hard maximum `risk.max_slippage_bps`; `u128` price-impact
  model; property-tested arithmetic).
- `module-sniper/src/market.rs` — fresh per-protocol venue reads (pump
  curve, PumpSwap pool, Raydium AMM v4 pool, Jupiter quote) → snapshot +
  venue context.
- `module-sniper/src/replay.rs` + `fixtures/replay/*.json` (19 fixtures) —
  deterministic replay through the live validation code; never submits.
  A fixture may pin an `execution` section so the fee budget is priced from
  fixture data rather than the engine's defaults.
- `module-sniper/tests/{pipeline,failure_injection,exit_sweeper,concurrency,
  replay,property}.rs` + `tests/common/mod.rs` (scripted mock JSON-RPC node
  and mock websocket feed, test-only).
- `solana-kit`: `raydium::{PoolInitEvent, parse_initialize2_log,
  find_initialize2_log}`, `decode::{DecodedInstruction, decode_instructions}`,
  `events::find_pool_creation`, `tokens::{MintInfo, SPL_MINT_LEN}`,
  `WsMessage::Logs { slot }`, `fees::FeePolicy::{would_refuse, max_payable}`
  (side-effect-free predicates; `decide` now uses `would_refuse` for its
  refusal verdict, unchanged behaviour).
- `bot-core`: `risk::RiskCode` on every `RiskDecision`, sniper exposure
  controls inside `RiskEngine::check_entry` (per-token cap, total sniper
  exposure, concurrent positions, pending executions via the execution
  ledger, token cooldown, failed-entry cooldown, sniper daily loss,
  emergency disable), `ExitRule::StalePosition`, `AppState` entry-attempt /
  failed-entry timestamps and module-scoped daily realized PnL,
  `maths::{PUMP_TOKEN_DECIMALS, PUMP_TOTAL_SUPPLY_TOKENS,
  LAMPORTS_PER_SIGNATURE, MAX_TRANSACTION_COMPUTE_UNITS}`.
- Metrics: `sniper_events_total`, `sniper_stage_total`,
  `sniper_rejections_total`, `sniper_gate_results_total`,
  `sniper_slippage_bps`, `sniper_{detection,validation,risk,build,
  submission,confirmation,total}_latency_ms`, `sniper_feed_{events,
  reconnects,gaps,out_of_order}_total`, `sniper_exit_actions_total`.
  Audit events `sniper.entry.<stage>` and `sniper.exit.failed_entry_cleanup`.
- Config (all optional; defaults preserve previous behaviour): `[sniper]`
  `slippage_mode`, `pumpswap_slippage_pct`, `raydium_slippage_pct`,
  `slippage_overrides_bps`, `max_price_impact_bps`, `max_entry_fee_lamports`
  (`0` = off; validation rejects `1..4999` and warns when the configured
  first attempt already exceeds the budget), `min_liquidity_sol`,
  `require_mint_authority_revoked`, `require_freeze_authority_revoked`,
  `max_creator_initial_buy_sol`, `min_pool_supply_fraction`,
  `max_snapshot_age_ms`, `strict_gates`, `stale_position_exit_secs`,
  `exit_retry_backoff_secs`, `failed_entry_cleanup`; `[risk]`
  `sniper_max_position_quote`, `sniper_max_total_exposure_quote`,
  `sniper_max_concurrent_positions`, `sniper_max_pending_executions`,
  `sniper_token_cooldown_secs`, `sniper_failed_entry_cooldown_secs`,
  `sniper_daily_loss_limit_quote`, `sniper_emergency_disable`; matching
  `SNIPER_*` environment overrides (see README).

#### Changed
- `module-sniper/src/detect.rs` — every feed emits `LaunchEvent`s;
  `logsSubscribe` now covers the PumpSwap and Raydium AMM v4 programs when
  those protocols are traded (Raydium `initialize2` → creating transaction
  fetched and decoded); Geyser subscription includes all three programs;
  reconnects, gaps and slot regressions are counted.
- `module-sniper/src/entry.rs` — `Sniper::consider_event` runs the staged
  pipeline (checks 1–19 in the file header); `consider_launch` remains as
  the legacy door; kill switch re-checked and latency budget / snapshot
  age enforced immediately before the hand-off to the executor;
  `EntryOutcome.fee_estimate_lamports` and `fee_est_lamports=` in the
  accepted audit line.
- `docs/SNIPER-ENGINE.md` §8 now states how partial exits and
  reconciliation mismatches of sniper positions are handled (the existing
  reconciliation engine's verdicts; no code change in `exit.rs`).
- `module-sniper/src/exit.rs` — failed-entry cleanup from the execution
  ledger, ambiguous-entry hold, venue-aware mark and sell (curve / PumpSwap /
  Raydium / Jupiter), stale-position exit, per-position retry backoff,
  exit intent id now includes symbol + opening time.
- `bot-core/src/maths.rs` — `pump_spot_price_sol` corrected to the curve's
  6-decimal raw units (the previous 1e18-scale constants produced marks
  ~10⁶× too high on real curves, firing take-profit immediately).
- `release-manifest.json` / `scripts/verify-delivery.sh` — migration count
  and docs count brought in line with the tree (0012, 51 docs).
- `docs/TESTING.md` — sniper suites added to the layer table and the
  coverage map; per-crate counts refreshed from the run below.

#### Verification (2026-09-21, Rust 1.98.1, hermetic — no Postgres / Redis /
network, so the gated suites ran in their self-skipping mode)
- `cargo fmt --all -- --check` — clean (app workspace and staking program).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo test --workspace -- --test-threads=1` — 723 passed, 0 failed,
  1 ignored (the fixture generator), 33 test binaries. Module 1 alone:
  `cargo test -p module-sniper` — 126 passed (60 lib + 15 pipeline +
  22 failure injection + 5 exit sweeper + 4 concurrency + 5 replay +
  12 property + 3 feed) + 1 ignored. (The fee-budget follow-up added 2
  `solana-kit` fee-policy tests, 2 pipeline unit tests and 1 pipeline
  integration test; `bot-core` config tests grew assertions, not cases.)
- `scripts/verify-delivery.sh` — 7 PASS / 0 FAIL.
- `cargo audit` / `cargo deny` were not available in the build sandbox and
  were not run. `Cargo.lock` gained no new crates: `module-sniper` now
  lists the workspace-pinned `bs58` (dependency) and `async-trait`
  (dev-dependency), both already in the lockfile via other crates.

### Execution reliability layer — TASK 1 "harden the execution engine" (2026-09-20)

Production-grade reliability for the build → simulate → submit → confirm
path. Integrated into the existing `Executor`, `Rpc`, `SolanaWs`, OMS
status vocabulary, audit trail, reconciliation queue and Postgres schema —
no parallel stack, no behaviour change for callers that do not opt in.
Full design notes: `docs/EXECUTION-RELIABILITY.md`.

#### Added
- `solana-kit/src/provider.rs` — RPC provider pool (primary + fallbacks)
  with per-provider health (consecutive-failure breaker, cooldown/re-probe),
  automatic failover, a shared `RetryPolicy` (exponential backoff with full
  jitter, `from_network(&NetworkConfig)`), `RpcErrorClass` classification
  (`Timeout | RateLimited{retry_after} | Transport | Unavailable | Blockhash |
  Permanent`, with `retryable()` / `needs_rebuild()` / ambiguity flags) and a
  `MeteredSender` that honours `Retry-After` on HTTP 429 and applies the
  request timeout to every call. Metrics `bot_rpc_requests_total`,
  `bot_rpc_errors_total{class}`, `bot_rpc_retries_total`,
  `bot_rpc_failover_total`, `bot_rpc_rate_limited_total`,
  `bot_rpc_provider_healthy`, `bot_rpc_provider_tripped_total`,
  `bot_rpc_consecutive_failures`, `bot_rpc_attempt_duration_ms`.
- `solana-kit/src/fees.rs` — priority-fee infrastructure: `FeePolicy`
  (`fixed` | `adaptive`, min/max clamp, **emergency ceiling that refuses
  instead of clamping**, per-attempt escalation), `FeeDecision` /
  `FeeSource`, and a TTL-cached `FeeOracle` over
  `getRecentPrioritizationFees` (percentile quote). Metrics
  `bot_priority_fee_decisions_total{source}`, `bot_priority_fee_clamped_total`,
  `bot_priority_fee_refused_total`, `bot_priority_fee_last_micro_lamports`,
  `bot_priority_fee_oracle_quote_micro_lamports`,
  `bot_priority_fee_oracle_samples_total`.
- `core/src/execution.rs` — the execution state machine
  `created → validated → submitted → pending → confirmed | failed | expired
  → reconciled` (`ExecutionState::can_transition_to`, projection onto
  `OrderStatus`), `FailureClass` (`simulation_rejected`, `blockhash_expired`,
  `rejected`, `insufficient_funds`, `rate_limited`, `transport_ambiguous`,
  `confirmation_timeout`, `landed_failed`, `policy_veto`, `duplicate`,
  `internal`), deterministic intent ids (`intent_id(&[parts])` →
  `int_<sha256 prefix>`), and the process-wide `ExecutionLedger`
  (`ledger()`): duplicate protection (`begin` refuses a second attempt while
  an intent is live or already landed), attempt counters, per-state timing,
  `ExecutionSink` fan-out, `hydrate` + `resolve_after_restart` for crash
  recovery. Metrics `bot_execution_transitions_total{from,to}`,
  `bot_execution_failures_total{class}`, `bot_execution_attempts_total`,
  `bot_execution_rebuilds_total`, `bot_execution_state_duration_ms{state}`,
  `bot_execution_stage_ms{stage}`, `bot_execution_active`.
- `core/migrations/0012_execution_lifecycle.sql` + `core/src/db/execution.rs`
  (`ExecutionRepo`): durable `execution_lifecycle` rows (one per intent,
  upserted on every transition, **written before broadcast**) and the
  append-only `execution_lifecycle_events` history.
- `server/src/persist.rs` — `ExecutionLedgerSink` (DB upsert + event +
  audit records `execution.<state>` for submitted / confirmed / failed /
  expired / reconciled; `bot_execution_persist_failures_total{op}`) and
  `restore_execution_ledger` (startup hydration; unsent intents → `failed`,
  submitted/pending intents stay blocked and their signatures are enqueued
  for reconciliation; `bot_execution_restart_recovered_total{disposition}`).
- API: `GET /api/executions` (`?limit=`, `?state=`, `?open=true`) and
  `GET /api/executions/:id` (intent id **or** signature, DB fallback with
  event history). Documented in `docs/API.md`.
- Config (all optional, defaults preserve previous behaviour):
  `[network] retry_base_backoff_ms`, `retry_max_backoff_ms`, `retry_jitter`,
  `rate_limit_cooldown_ms`, `provider_failure_threshold`,
  `provider_cooldown_ms`, `ws_stale_after_ms`; `[execution] fee_mode`,
  `fee_min_micro_lamports`, `fee_max_micro_lamports`,
  `fee_emergency_max_micro_lamports`, `fee_percentile`, `fee_escalation_pct`,
  `fee_oracle_ttl_ms`, `max_blockhash_age_ms`. Environment overrides follow
  the existing un-prefixed convention (`RPC_RETRY_BASE_BACKOFF_MS`,
  `RPC_RETRY_MAX_BACKOFF_MS`, `RPC_RETRY_JITTER`, `RPC_RATE_LIMIT_COOLDOWN_MS`,
  `RPC_PROVIDER_FAILURE_THRESHOLD`, `RPC_PROVIDER_COOLDOWN_MS`,
  `WS_STALE_AFTER_MS`, `FEE_MODE`, `FEE_MIN_MICRO_LAMPORTS`,
  `FEE_MAX_MICRO_LAMPORTS`, `FEE_EMERGENCY_MAX_MICRO_LAMPORTS`,
  `FEE_PERCENTILE`, `FEE_ESCALATION_PCT`, `MAX_BLOCKHASH_AGE_MS`).
  `config.toml.example` documents each key.
- Failure-injection tests (all offline, mock HTTP/WS nodes): RPC timeout,
  RPC failure + failover, permanent error not retried, 429 cooldown, WS
  disconnect / stale connection / subscription restore + gap signal, stale
  blockhash detected and rebuilt, failed simulation aborts before broadcast,
  submission rejection vs. ambiguous transport failure, confirmation timeout
  parks the intent in `pending`, process-restart resolution, duplicate retry
  refused, fee above the emergency limit vetoed before any network I/O.
  Workspace total: 594 tests (was 537), `cargo clippy -D warnings` and
  `cargo fmt --check` clean.

#### Changed
- `solana-kit/src/rpc.rs` — `Rpc` now routes every call through the
  provider pool (`failover()`, `provider_label()`), tracks blockhash
  freshness (`Blockhash{blockhash,last_valid_block_height,fetched_at}`,
  `fresh_blockhash`, `max_blockhash_age_ms`), classifies send failures
  (`send_transaction_classified` → `RpcFailure{class,…}`), and `confirm` /
  `confirm_tracked` return `ConfirmOutcome::Expired{last_valid_block_height,
  block_height}` when the blockhash lapses without a landing (distinct from
  `Timeout`). `get_transaction` reads the raw JSON so a `null` result maps to
  `Ok(None)` instead of a deserialization error (previously masked expiry
  detection on real nodes).
- `solana-kit/src/execute.rs` — `Executor::run` drives the full lifecycle:
  deterministic intent id (caller-pinned or derived from wallet + label +
  instruction digest), ledger `begin` duplicate guard, fee decision (veto
  before any I/O), fresh blockhash, simulation gate, write-ahead `submitted`,
  classified send, tracked confirmation, expiry → rebuild with escalated fee
  (bounded by `max_retries`), and a definite `FailureClass` on every
  non-success. `ExecutionResult` gains `intent_id`, `state`, `failure`,
  `priority_fee_micro_lamports`, `broadcast_signature()`, `is_duplicate()`.
  `send_prebuilt` accepts externally built (Jupiter) transactions with a
  pinned intent id and broadcasts RPC-only when there are no local
  instructions. `fee_policy_from_config` builds the policy from `Config`.
- `solana-kit/src/tx.rs` — `BuiltTx::from_signed / with_intent_id /
  attributed`, `TxRequest::with_intent_id / attributed / intent_digest`,
  last-valid-block-height carried with every built transaction.
- `solana-kit/src/ws.rs` — `WsPolicy` (jittered exponential reconnect
  backoff that resets after 60 s of healthy connection, ping interval,
  stale-after, subscribe timeout; `from_network(&NetworkConfig)`),
  `WsStatus::Stale`, stale-frame detector, slot tracking, subscription
  restoration after reconnect and a `WsMessage::Gap{subscription,last_slot,
  outage_ms}` emitted before `Connected` so consumers can backfill. Metrics
  `bot_ws_reconnect_delay_ms`, `bot_ws_outage_ms`,
  `bot_ws_stale_connections_total`, `bot_ws_subscriptions_restored_total`.
- `module-sniper` (`detect.rs`, `entry.rs`, `exit.rs`, `lib.rs`) and
  `module-copy` (`feeds.rs`, `mirror.rs`, `exit.rs`, `lib.rs`) — pin
  deterministic intent ids on every buy/sell (`snipe_intent_id`,
  `copy_intent_id`, `exit_intent_id`), route Jupiter-built transactions
  through `Executor::send_prebuilt`, apply `WsPolicy::from_network`, handle
  `Gap` (sniper: warn + error metric; copy: bounded `backfill_after_gap`
  using `getSignaturesForAddress`, `bot_ws_backfilled_events_total`), and
  attach the configured fee policy to every `Executor`.
- `server/src/recon.rs`, `solana-kit/tests/recon_crash_e2e.rs` — handle
  `ConfirmOutcome::Expired` explicitly (→ failed-on-external with reason).
- `server/src/main.rs` — installs the `ExecutionLedgerSink` right after the
  audit trail so no transition can be lost between startup and persistence.

### Repository hygiene — git publication pass (2026-09-20)

- Fixed `.gitignore`: added `*-keypair.json` (the `cargo build-sbf` /
  `solana-keygen` output name — the previous `*.keypair.json` pattern did not
  match it), an anchored `/config.toml` (+ `!config.toml.example`) so a live
  config with an inline `[secrets]` table can never be committed, `*.pem`,
  `**/data/`, `*.pdb`, `docker-compose.override.yml` and editor noise. Cargo's
  own `programs/staking-suite/.cargo/config.toml` stays tracked (anchored
  pattern).
- Added a root `.gitignore` for the publication repository (the canonical
  tree lives under `sniper-suite/`): toolchain / home-directory remnants
  (`.cargo/`, `.config/`, `.profile`, `.wget-hsts`, `rustup-init.sh`),
  `work/solana-release/` (~70 MB toolchain remnant), `work/artifacts/`
  (local `build-sbf` output incl. its program keypair), `work/cratesrc/`,
  `work/app-run/`, root-level stray `*.log`, and the same secret / runtime
  patterns as above. Frozen packages and `evidence/` remain tracked.
- Untracked (kept on disk, removed from the index) the 81 files those rules
  cover — including a committed program keypair
  (`work/artifacts/staking_suite-keypair.json`, pubkey
  `8tpPo8PMU4e8VdsScd3Wnb9w3CEVV8uRvYiSitoWNZbp`). That key must be treated
  as compromised: never deploy it; generate a fresh program keypair via
  `scripts/staking-identity.sh` before any public-cluster deployment.
- Verified on Rust 1.98.1: `cargo build -p sniper-suite` (0 warnings),
  paper-mode startup, `/health` `/ready` `/api/status` `/metrics` and the
  API-key gate (401 without `x-api-key`), and the offline workspace test
  suite. No production source changed.

### Forensic engineering cycle — Phase 0/1 (2026-09-19)

- Added `docs/FORENSIC-FILE-INVENTORY.md` (full-workspace 992-file forensic
  scan: per-file classification A–K, duplicate-group analysis, canonical
  repo per-file table, static gap confirmations) and
  `docs/SOURCE-OF-TRUTH.md` (canonical tree declaration, never-source
  directories, duplicate precedence, cycle/re-packaging discipline).
- No production source changed; v0.1 handover baseline (tree `033b582f…`,
  `buyer-release-final/`) remains the frozen, verifiable delivery snapshot.

### Final buyer-handover pass (2026-09-19)

- Fixed (release defect, found by the handover-pass identity audit):
  `scripts/staking-identity.sh` TRACKED_FILES missed
  `docs/BUYER-ACCEPTANCE-TEST.md` (it quotes the declared program id), so a
  post-`set-id` tree would have kept a stale placeholder in a buyer-facing
  doc; the verify stale-sweep also now excludes `release-manifest.json`
  (delivery-time record, like AUDIT.md) so post-set-id verification cannot
  false-FAIL. Re-verified: `verify` passes with 4 tracked docs
  (`evidence/phase7-identity-verify.log`).
- Added docs (8): FEATURE-TRACEABILITY (symbol-verified feature→code→test
  matrix), SECURITY-BOUNDARY-MAP (13-boundary trace),
  FINAL-IP-AND-THIRD-PARTY-INVENTORY (factual IP/dependency inventory),
  BUYER-REPRODUCTION-GUIDE (clean-machine procedure),
  FINAL-KNOWN-LIMITATIONS (6-category register), FINAL-OPERATIONS-HANDOVER
  (day-2 ops incl. rotation procedures), FINAL-INCIDENT-RUNBOOK (17
  scenarios), FINAL-RELEASE-AUDIT (pass summary + counts).
- Upgraded: BUYER-ACCEPTANCE-TEST.md to the formal A–R record format
  (PRECONDITIONS/COMMAND/EXPECTED/ACTUAL/EVIDENCE/PASS-FAIL/INITIALS/DATE;
  nothing pre-marked PASS).
- Corrected (commercial-claims boundary): README multisig sentence no
  longer asserts third-party audit status it cannot evidence.
- Money-path audit (inspection + executed-test review): Polymarket
  reject-over-fallback, Solana paper/live balance separation, staking
  checked arithmetic (`overflow-checks = true`) all confirmed fail-closed;
  no new defects; no application logic changed.

### Fixed (buyer-hardening pass — 2026-09-18, found by EXECUTING the gated e2e)

- **Staking program, metadata CPI discriminant (real on-chain bug):** the
  hand-rolled `CreateMetadataAccountV3` instruction used discriminant `19`,
  but in the mpl-token-metadata source deployed to mainnet-beta (tag
  `token-metadata@v1.14.0`) variant 19 is `Utilize` and
  `CreateMetadataAccountV3` is variant **33**. The real mainnet-cloned mpl
  program rejected the CPI with `InvalidInstructionData` under
  `solana-test-validator`. Fixed the constant + the pinned byte-layout test;
  verified by the executed e2e (metadata created, replay rejected).
- **validator e2e harness, mpl clone flag:** `--clone <program>` copies only
  the 36-byte program account WITHOUT its programdata account, so the
  upgradeable loader reported "Program is not deployed" at execution time
  (Agave 2.1.21). The harness now passes `--clone-upgradeable-program`.

### Added (buyer-hardening pass)

- `scripts/staking-identity.sh` — program-identity guard: `verify` (declare_id
  vs every tracked reference, placeholder detection), `set-id <keypair>`
  (updates source of truth + buyer-facing docs atomically, re-verifies),
  `deploy --keypair --url` (refuses keypair/declare_id mismatch, refuses the
  placeholder id on public clusters, freshness-checks/rebuilds the .so,
  verifies the on-chain account after deploy). Round-trip tested.
- `docs/LIVE-VALIDATION.md` — operator-controlled live-validation runbook
  (Polymarket live-read checks with raw `eth_call` selectors, in-app
  simulate→live ladder, Solana canary rules, evidence-labeling taxonomy).
- `docs/BUYER-ACCEPTANCE-TEST.md` — 22-step independent buyer acceptance
  procedure.

### Verified by execution (buyer-hardening pass — previously environment-blocked)

- `cargo build-sbf` on the audit-pass source: Agave 2.1.21 / platform-tools
  v1.43 → 187,504-byte `staking_suite.so` (SHA-256 `57a890fa…` after the
  discriminant fix; the pre-fix build `9e113678…` is superseded).
- ALL THREE validator e2e tests EXECUTED and PASSED on a real
  `solana-test-validator` (3/3, 160.72 s batch, `--test-threads=1`).
- pg_dump→restore round-trip re-executed on PostgreSQL 17.11 (dump SHA-256
  `5989ecf1…`; tables/migrations/rowcounts identical; db_integration 23/23
  against the restored DB; application startup + health/ready + audit-verify
  + graceful SIGTERM shutdown against the restored DB).
- Latency benches executed against public devnet (read-only + simulate;
  getSlot p50 65 ms, getLatestBlockhash p50 65 ms, simulateTransaction p50
  66 ms from the sandbox — see `evidence/benchmarks-2026-09-18.json` in the
  release package; NOT product performance claims).

### Fixed (audit pass — live/paper money separation)

- **Module 3 (Polymarket) live sizing balance** — `available_usdc` silently
  returned the cached dashboard balance (which a paper start seeds with a
  1,000 USDC demo figure that survives a runtime mode switch) and otherwise
  fell back to the paper figure **in live mode**. Replaced by
  `available_collateral`: LIVE entries now require a verified on-chain read
  of the funder's collateral (new `collateral.rs` ERC-20 client:
  `balanceOf`/`decimals`/`allowance` via `[polymarket].ctf_rpc_url` against
  `collateral_address`, 15 s freshness bound, decimals plausibility check).
  Unverifiable balances REJECT the entry with typed errors
  (`BalanceUnavailable` / `InsufficientFunding`) — no fallback exists. Live
  orders additionally verify funding and (for EOA signing) the settling
  exchange's ERC-20 allowance before broadcast. Paper/simulate keep the
  demo figure, and only there. Regression tests pin the separation matrix
  (poisoned cache seed, failed/missing/implausible reads).
- **Module 1 (Sniper) `available_sol`** — on RPC failure the cached balance
  was used as a fallback **regardless of execution mode** (contradicting its
  own comment); after a paper→live mode switch a stale/demo seed could size
  live orders. The fallback is now paper-mode-only; simulate/live propagate
  the RPC error into a risk rejection. Unit-tested via a pure fallback rule.
- Module 2 (Copy) audited: already correct (paper cache in paper mode only;
  real RPC read otherwise) — unchanged.

### Added (audit pass — staking program: max supply + token metadata)

- **Immutable max-supply cap** — `Initialize` gained a required `max_supply`
  parameter (> 0, stored in `Config`, deliberately NOT changeable via
  `UpdateParams`). `GenesisMint` now fails with `MaxSupplyExceeded` (6028)
  unless the LIVE mint supply plus the amount stays at or below the cap
  (checked arithmetic; overflow fails closed). Reward minting
  (`Claim`/`Unstake`) is clamped to the remaining headroom so withdrawals
  can never fail because of the cap; the shortfall is forfeited and logged
  on-chain. New error `InvalidMaxSupply` (6029) for a zero cap.
- **Token metadata** — new one-shot admin instruction
  `CreateTokenMetadata{name,symbol,uri}` performing a hand-rolled borsh CPI
  to mpl-token-metadata `CreateMetadataAccountV3` (discriminant 33 — the
  enum index in the mpl source deployed to mainnet-beta, verified by
  executing the CPI against the real mainnet-cloned mpl program in the
  validator e2e — pinned by a byte-layout test): immutable metadata
  (`is_mutable = false`), config
  PDA as mint/update authority, canonical mpl PDA + program-id validation,
  byte-length limits (32/10/200) enforced before the CPI. New errors:
  `MetadataAlreadyExists` (6030), `InvalidMetadataProgram` (6031),
  `MetadataFieldTooLong` (6032). Client builder `create_token_metadata_ix`.
- Staking host unit tests 48 → 71 (cap boundaries incl. exact-cap and
  one-over, live-supply authority, overflow fail-closed, reward clamping at
  zero/partial headroom, claim-succeeds-at-cap, metadata guards + layout).
  New gated validator e2e `validator_e2e_max_supply_cap_and_metadata`
  (cap + metadata against a mainnet-cloned mpl program; NOT executed in the
  audit sandbox — no build-sbf/validator/internet there).
- New module file `crates/module-polymarket/src/collateral.rs` (ERC-20
  collateral reader + unit conversions with mock-RPC wire tests);
  `ctf.rs` address/word helpers shared crate-internally.
- Docs/config updated to match: `config.toml.example` (collateral/ctf_rpc
  semantics, `token_supply` ↔ on-chain `max_supply` mapping),
  `docs/STAKING.md`, `docs/MODULES.md`, `README.md` launch sequence +
  security model.

### Added (buyer / due-diligence documentation pass — no source code changed)

- 14 buyer-package documents under `docs/`: BUYER-OVERVIEW,
  CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE, IP-COMPONENTS, THIRD-PARTY,
  BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST, RELEASE-NOTES-0.1.0, BUYER-FAQ,
  SCOPE-BOUNDARY, SUPPORT-HANDOVER, BUYER-RISK-REGISTER,
  TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST (index).
- README "Buyer / engineering handover" section; `docs/HANDOVER.md` §1 and
  `release-manifest.json` `docs_count` updated for the new document set
  (13 → 27 files under `docs/`). The frozen engineering tree (commit
  `0e139c3`) is untouched: no Rust, SQL, config or CI file changed.

### Added (final delivery package — documentation + bundle tooling only)

- 9 final-delivery documents under `docs/`: FINAL-DELIVERY (single starting
  point), BUYER-QUICKSTART (18-step walkthrough), TECHNICAL-FACT-SHEET,
  SELLER-FACT-SHEET, SELLING-LISTING-SOURCE (factual listing source
  material — not advertising), DEMO-RUNBOOK (10 deterministic demos),
  EVIDENCE-INDEX (claim → evidence map), REPOSITORY-MAP (annotated tree),
  ARCHIVE-CHECKLIST (seller archive INCLUDE/EXCLUDE spec).
- `scripts/verify-delivery.sh` — fast, fail-closed bundle-integrity check
  (required files, version identity, docs count, hygiene, markdown links,
  invisible characters). Complements `release-check.sh`; builds/tests
  nothing.
- CAPABILITY-MATRIX and BUYER-RISK-REGISTER finalized to the delivery column
  schema; IP-COMPONENTS gained the ownership-transfer checklist;
  DELIVERY-MANIFEST indexes all 23 buyer/delivery docs; README/HANDOVER
  pointers and `release-manifest.json` `docs_count` (27 → 36) updated.
  Still zero changes to Rust, SQL, migrations, configs, Docker or CI files.

## [0.1.0] — initial handover release

First complete, internally verified release of the suite. Delivered state
(full evidence trail in `AUDIT.md`, test inventory in `docs/TESTING.md`):

### Added

- **Module 1 — Sniper** (`module-sniper`): pump.fun launch detection
  (PumpPortal WS, Yellowstone-style Geyser `transactionSubscribe`, poll
  fallback) and entry execution with PumpSwap/Raydium/Jupiter exit routing.
- **Module 2 — Copy trading** (`module-copy`): tracked-wallet mirroring with
  per-wallet rules, sizing, staleness guards and mirrored exits.
- **Module 3 — Polymarket** (`module-polymarket`): Gamma + CLOB REST/WS
  integration with EIP-712 v2 order signing and CTF ERC-1155 balance reads.
- **Module 4 — Staking program** (`programs/staking-suite`): native Solana
  program — reward mint, vault + fee treasury, per-second APY accrual,
  parameter timelock (queue/apply/cancel), two-step admin transfer,
  pause-deposits-only, hard parameter caps, one-time latched `GenesisMint`.
- **Module 5 — Telegram control** (`module-telegram`): deny-by-default RBAC,
  kill switch, module on/off, rate-limited alerts.
- **Control plane** (`server`): Axum REST (23 REST endpoints over 21
  `/api` routes) + WebSocket event feed (`/api/events`) + 4 infra routes —
  28 endpoints documented route-by-route in `docs/API.md` +
  embedded dashboard; liveness/readiness probes; Prometheus metrics with
  bounded labels; request-ID correlation; per-IP and per-principal rate
  limits; refusal to bind non-loopback without API auth.
- **Core** (`bot-core`): typed config with validation and env overrides,
  global risk engine (capacity, exposure, daily-loss auto-disable), OMS state
  machine with idempotency keys, restart-safe dedup (memory/Redis/Postgres),
  hash-chained append-only audit trail, JSONL journal with rotation and
  corrupt-line tolerance, intent journal + startup reconciliation,
  Postgres repositories with 11 forward-only migrations.
- **Distributed execution ownership** (`docs/DISTRIBUTED.md`): one logical
  execution ⇒ at most one active owner ⇒ at most one money-moving submission.
  Claim stores (Postgres authoritative, Redis, memory), leases + epochs +
  fencing, handoff grace for ambiguous outcomes, cross-replica kill-switch /
  module-flag sync, position-book sync, cluster-wide `GlobalRiskOracle`
  (tighten-only), and the append-only `execution_claim_events` lineage table.
- **Solana kit** (`solana-kit`): RPC retry/failover/fan-out, WS supervision
  with resubscribe, account cache (TTL + FIFO bounds), pump/raydium/pumpswap
  instruction builders, transaction executor with simulate-first policy and
  signer registry (multi-signer safe).
- **Operations**: Dockerfile (multi-stage, non-root, healthchecked),
  docker-compose stack (Postgres 16 + Redis 7, healthcheck-gated),
  `.env.template`, single-workflow CI (fmt/clippy `-D warnings`/build/test
  with real service containers, staking `build-sbf` + validator e2e,
  cargo-audit + cargo-deny hard gates, docker image build + smoke test),
  `scripts/release-check.sh` local release gate, machine-readable
  `release-manifest.json`, and thirteen docs under `docs/`.

### Fixed (during the release-engineering pass, pre-tag)

- **Audit chain append serialization** — `AuditRepo::append` previously read
  the chain head with `SELECT … ORDER BY id DESC LIMIT 1 FOR UPDATE`, which
  does not serialize concurrent writers under READ COMMITTED (a blocked
  writer's snapshot never sees the winner's new head row → the chain forks
  and `/api/audit/verify` reports a false break). Appends are now serialized
  by a transaction-scoped advisory lock
  (`pg_advisory_xact_lock(hashtext('audit_events_chain'))`). Regression
  tests: concurrent-append linearization + reordered/missing/duplicate row
  detection (`db_integration`).
- Toolchain-pin drift: the Dockerfile built on `rust:1.82` and the CI
  `program` job on unpinned `stable`, contradicting the `rust-toolchain.toml`
  pin (1.98.1). Both now use 1.98.1; `scripts/release-check.sh` gates the
  three-way consistency.
- Removed the placeholder `repository` URL (`example.com/...`) from the
  workspace manifest; stale test counts and an undocumented route-subset
  table in README corrected.

### Fixed (engineering-freeze pass, pre-tag)

- **Telegram bot-token leak into error strings** — the Bot API embeds the
  token in every request URL and `reqwest::Error`'s `Display` appends
  ` for url (…)` on send errors, so failed Telegram calls put the token into
  tracing logs / audit detail / alert text. Every reqwest error mapping in
  `module-telegram` now strips the URL (`Error::without_url()`); regression
  test `error_strings_never_contain_the_bot_token` exercises all four API
  methods against a closed loopback port and fails if the token ever appears
  in an error string.
- **Unused dependencies removed** (verified zero code references before
  removal, `cargo check` + full gate re-run after): `tokio-util` (core,
  solana-kit, server), `sha3` (module-polymarket — EIP-712 uses
  `tiny-keccak`), `serde_with` (workspace entry no crate referenced). They
  remain in `Cargo.lock` only where still required transitively.
- **Release metadata drift corrected:** control-plane route count and docs
  count in this file now match the source (26 `.route()` registrations /
  28 documented endpoints; 13 docs); `release-manifest.json` added as the
  machine-readable delivery manifest and wired into `release-check.sh`
  (required file + version consistency).

### Verification status at cut

- 521 application workspace tests (incl. 38 gated Postgres/Redis/
  distributed/two-replica integration tests), 48+2 staking host/e2e-gated
  tests — 0 failures; `scripts/release-check.sh` 20/20 gates PASS; fmt, clippy `-D warnings`, cargo-audit, cargo-deny
  clean. Per-pass evidence and the honest NOT-EXECUTED /
  ENVIRONMENT-BLOCKED list: `docs/HANDOVER.md` and `docs/TESTING.md`.
- The staking program has **not** had an external security audit; the
  declared program id is a pre-deploy placeholder. Do not deploy to mainnet
  until an independent audit passes (see `docs/SECURITY.md`).

[0.1.0]: initial release — no previous tags exist.
