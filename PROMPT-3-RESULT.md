# PROMPT 3/10 — RESULT: Enterprise Tenant Isolation of the Trading Core (STEP 10 program)

**Status: COMPLETE.** All planned surfaces shipped and verified end-to-end: the
tenant-scoped `trading_repository` (bot-core), the authenticated tenant trading
data plane (`/api/tenant/*`, sniper-suite), the nine atomic PK/arbiter swaps
(0026–0034), and a full cross-tenant attack-test program — 20 new tests, 18 of
them against a **live PostgreSQL 17.11** database, plus 2 route-level tests
through the real axum router. Both crates compile clean, are `cargo fmt` clean
and `cargo clippy --all-targets -- -D warnings` clean, and the full bot-core
suite is green: **18 test binaries, 727 passed, 0 failed** (including the
previously always-skipped `db_integration` and `tenant_idempotency_isolation`
suites, now executed against a real database for the first time).

> **Verification found and fixed four real defects** that no prior check could
> catch (they only manifest at runtime against a real database, or under
> `--tests`): a positions-upsert SQL column/placeholder mismatch that broke
> every position write; a `$4`-bind bug that broke release/hand-off in the
> legacy ownership plane; a wrong error type in the data plane's `write_scope`;
> and a stale isolation test still pinning the pre-0027 global idempotency
> contract. Details in §5 — this is the strongest possible argument for the
> "actually run the PG suites" discipline this prompt enforced.

---

## 1. What was built

PROMPT 3/10 completes the enterprise tenant-isolation program for the trading
core: **Tenant A must never be able to read, mutate, sign, broadcast,
reconcile or stream Tenant B's trading data — and that guarantee must live in
the schema and the SQL, not only in HTTP middleware.**

Three layers, all delivered:

1. **Schema (migrations 0026–0034)** — nine forward-only, transaction-wrapped,
   idempotent atomic swaps: the business-identity PRIMARY KEYs / `ON CONFLICT`
   arbiters of seventeen trading-truth surfaces become tenant-composite, one
   migration per surface family so each swap is independently reviewable and
   reversible-by-design-decision (forward-only policy unchanged). Plus the new
   `worker_claims` table (0032) and the tenant-leading reporting indexes
   (0034).
2. **Repositories (`crates/core/src/trading_repository/`, 52 files)** — the
   tenant-scoped read/write/query surface for every trading table: orders,
   executions/transactions, claims, idempotency, lifecycle, intents/recovery,
   copy trading, polymarket (incl. reconciliation), positions/trades/balances,
   reporting aggregates, worker lanes, plus the shared machinery (typed scopes,
   pagination with tenant-bound cursors, `RepositoryError`, ownership
   assertions, tenant-scoped transactions with tenant-namespaced advisory
   locks). Every SQL predicate carries `organization_id`; every write binds the
   acting tenant from the scope, never from a default.
3. **Data plane (`crates/server/src/trading_data_plane/`, 8 files)** — 21
   authenticated routes under `/api/tenant/` (orders, executions,
   transactions, positions, trades, balances, copy leaders/links, polymarket
   orders/fills/reconciliation, recovery intents/sweep, reports summary/pnl).
   Every handler authenticates through `authorize_request`, builds the tenant
   scopes from the `SaasContext`, calls **only** the tenant repositories, and
   answers `503 trading_data_plane_unavailable` when no database is attached.
   The legacy deployment API stays untouched beside it (operator plane,
   deployment-organization-bound).

## 2. The seventeen tenant-composite arbiters (and what deliberately stayed global)

| surface | old identity | new identity (migration) |
|---|---|---|
| `orders` | `(id)` | `(organization_id, id)` — 0026 |
| `idempotency_keys` | `(scope, key)` | `(organization_id, scope, key)` — 0027 |
| `execution_claims` | `(execution_id)` | `(organization_id, execution_id)` — 0028 |
| `execution_claim_events` | org-less inserts | org VALUES + scoped reads — 0028 |
| `copy_leaders` | `(address)` | `(organization_id, address)` — 0029 |
| `copy_events` | `(event_id)` | `(organization_id, event_id)` — 0029 |
| `copy_links` | `(position_id)` | `(organization_id, position_id)` — 0029 |
| `poly_signals` | `(signal_id)` | `(organization_id, signal_id)` — 0030 |
| `poly_orders` | `(venue_order_id)` | `(organization_id, venue_order_id)` — 0030 |
| `poly_fills` | `(fill_id)` | `(organization_id, fill_id)` — 0030 |
| `poly_recon_findings` | org-less | org VALUES + scoped reads — 0030 |
| `execution_lifecycle` | `(intent_id)` | `(organization_id, intent_id)` — 0031 |
| `execution_lifecycle_events` | org-less | org VALUES + scoped reads — 0031 |
| `ledger_events` | `(event_id)` | `(organization_id, event_id)` — 0033 |
| `ledger_postings` | org-less | org VALUES + **global** `UNIQUE(event_id, seq)` kept — 0033 |
| `global_positions` | `(position_key)` | `(organization_id, position_key)` — 0033 |
| `global_risk_decisions` | `(decision_id)` | `(organization_id, decision_id)` — 0033 |
| `kill_switches` | `(scope)` | `(organization_id, scope)` — 0033 |
| `accounting_recon_findings` | org-less | org VALUES + scoped reads — 0033 |
| `reconciliation_state` | `(kind, subject)` | `(organization_id, kind, subject)` — 0033 |
| `worker_claims` | *(new table)* | `(organization_id, purpose)` + fencing `generation` — 0032 |

**Deliberately still global** (deployment infrastructure, not tenant data —
each with an audited reason): `ha_leases`/HA worker plane (0016, process-level
infra), `runtime_flags` (deployment-wide kill switches), tenant **control-plane
API keys** (already tenant-attributed rows; the *key text* registry is global
by design), custody wallet rows (org-attributed at 0020, no identity swap
needed), HA checkpoints, `audit_events` (org-columned, append-only, global
order), and `transactions.signature` (globally unique on-chain identity — the
0028 batch deliberately kept signature uniqueness global while attribution and
every lookup stayed tenant-scoped; the isolation tests pin both properties).

## 3. Complete file inventory

**bot-core `trading_repository` (52 files):** `mod.rs` (module surface + shared
tests), `query_scope.rs`, `write_scope.rs`, `transaction.rs`, `pagination.rs`,
`repository_error.rs`, `not_found.rs`, `tenant_assert.rs`; `orders/{mod,model,
read,write,conflicts}.rs`; `executions/{mod,model,read,write,claim,lifecycle,
idempotency}.rs`; `intent/{mod,model,read,write,recovery}.rs`;
`copy/{mod,model,read,write,events}.rs`; `polymarket/{mod,model,read,write,
reconciliation}.rs`; `positions/{mod,model,read,write,trades,balances}.rs`;
`reporting/{mod,model,orders,positions,executions,pnl}.rs`;
`worker_claim/{mod,model,acquire,release,recovery}.rs`.

**sniper-suite `trading_data_plane` (8 files):** `mod.rs` (21-route map +
route tests), `service.rs` (plane assembly), `orders.rs` (handlers + shared
`page_request`/`plane_error`), `executions.rs`, `positions.rs`, `copy.rs`,
`polymarket.rs`, `recovery.rs`.

**Migrations:** `0026_orders_tenant_conflict.sql`,
`0027_idempotency_tenant_conflict.sql`,
`0028_execution_claims_tenant_conflict.sql`,
`0029_copy_links_tenant_conflict.sql`,
`0030_polymarket_orders_tenant_conflict.sql`,
`0031_execution_intents_tenant_conflict.sql`,
`0032_tenant_worker_claims.sql`, `0033_accounting_tenant_conflict.sql`,
`0034_reporting_tenant_indexes.sql`.

**Test program (new this session):**

| file | tests | proves |
|---|---|---|
| `crates/core/tests/trading_isolation_common/mod.rs` | (harness) | gated PG setup/migrate, org factory, scopes, `run_id`, order seeder |
| `orders_cross_tenant_pg.rs` | 3 | CRUD fencing; B denied read/transition/cancel/delete; tenant-local idempotency namespaces (same key both tenants); signature lookup scoped; cross-tenant cursor rejection |
| `execution_cross_tenant_pg.rs` | 3 | execution read/append fencing; transaction signature global-unique but scoped lookup; B's status transition fails closed; claims: cross-tenant independence, same-tenant race exactly one winner, renew/verify/release owner+CAS, lapsed scoped |
| `intents_cross_tenant_pg.rs` | 2 | journal fencing; same intent-id text both tenants; B cannot abandon/link A-only intents; sweep + reconciliation queue scoped |
| `positions_cross_tenant_pg.rs` | 2 | positions/trades/balances fencing; PnL + balance aggregates exclude the other tenant; same-position-id collision cannot mutate |
| `copy_cross_tenant_pg.rs` | 3 | same leader address / event id / position id = independent rows per tenant; stage never cross-overwritten; retention prunes only the acting tenant |
| `polymarket_cross_tenant_pg.rs` | 2 | signal/order/fill same-id independence; monotonic size_matched per tenant; reconciliation drift + findings scoped; cross-tenant cancel impossible |
| `workers_cross_tenant_pg.rs` | 2 | lane independence per tenant; full-identity CAS fencing; lapsed sweep never crosses tenants; takeover bumps the fencing token |
| `reporting_cross_tenant_pg.rs` | 1 | every dashboard aggregate (order counts, position book, execution stats, realized PnL, balance totals, composed summary) excludes the other tenant |
| `crates/server/src/trading_data_plane/mod.rs` (`#[cfg(test)]`) | 2 | the real router: A's list/get/cancel vs B's 404-no-leak; cross-tenant cursor 400; unauthenticated 401; `503` plane-unavailable for six route families without a database |

## 4. Verification evidence

All commands run in this session against the repo as delivered. PostgreSQL
17.11 (Debian package) was installed and run live; the toolchain is the pinned
1.98.1. The full command set, reproducible:

```bash
export POSTGRES_URL="postgres://…/bot_isolation"   # real database, migrations applied by setup()
cargo fmt --all -- --check                          # CLEAN
cargo check -p bot-core --tests                     # 0 errors / 0 warnings
cargo check -p sniper-suite --tests                 # 0 errors / 0 warnings
cargo clippy -p bot-core --all-targets -- -D warnings        # CLEAN
cargo clippy -p sniper-suite --all-targets -- -D warnings    # CLEAN
cargo test -p bot-core --lib --tests -- --test-threads=1     # 18 binaries: 727 passed / 0 failed
cargo test -p sniper-suite trading_data_plane -- --test-threads=1  # 2/2 (1 PG-gated, 1 in-memory)
```

Breakdown of the 727: **599** lib unit tests, **26** `db_integration`
(live-PG — executed for the first time in this program), **18** new
cross-tenant isolation tests, **84** across the pre-existing in-memory suites
(including `tenant_idempotency_isolation`, now live-PG and updated, see §5).
Zero `NOT_RUN` lines with `POSTGRES_URL` set; without it every PG suite
prints `NOT_RUN` and passes through (the gating contract is unchanged).

## 5. Real defects found and fixed by verification

These are the findings that justify the "run the suites against a live
database" discipline — every one of them passed `cargo check` and clippy:

1. **`TenantPositionWrite::upsert` shipped un-runnable** —
   `crates/core/src/trading_repository/positions/write.rs` listed **29 INSERT
   columns but 28 `VALUES` placeholders** (`$1..$28`). Every position upsert
   would fail at runtime with *"INSERT has more target columns than
   expressions"*. Caught by the first live run of
   `positions_cross_tenant_pg`; fixed (`$29`).
2. **Legacy `PostgresClaimStore::release` broken by the 0028 org bind** —
   `crates/core/src/db/claims.rs` still had `SET status = $4 … AND
   claim_epoch = $4` while **five** values were bound (org, id, owner, epoch,
   mode): the mode never reached the statement and the bind count mismatched
   the placeholders, so every guard `release()`/`hand_off()` on the legacy
   ownership plane returned failure. Caught by the first live run of
   `db_integration` (3 test failures: release/hand-off grace, renew, event
   lineage); fixed (`SET status = $5`). The tenant repo's release was already
   correct — the defect was only in the legacy deployment writer's 0028
   coupling.
3. **`TenantTradingDataPlane::write_scope` had the wrong error type** —
   declared `Result<_, BotError>` but returned `Result<_, WriteScopeError>`;
   a latent compile error under `--tests` (the plain-`check` path masked it).
   Fixed with an explicit `map_err`.
4. **A stale test pinned the superseded idempotency contract** —
   `tenant_idempotency_isolation.rs` asserted the pre-0027 *global* `(scope,
   key)` uniqueness ("tenant B reusing A's key is refused"). Migration 0027
   deliberately made idempotency keys tenant-local (the documented design the
   new orders suite also pins). The test was updated to the 0027 semantics:
   B's same-text key inserts as an independent row, ownership stays
   per-tenant, and a key only A used is invisible to B.

Additionally fixed to reach clean gates (all pre-existing code, no behavior
change): the lib's own `#[cfg(test)]` units didn't compile (missing
`WriteOrigin` import in `transaction.rs`; `assert_eq!` on non-`PartialEq`
`RepositoryError` in `orders/conflicts.rs`; a missing `.expect` and a wrong
`page_where_clause(2)` expectation in `trading_repository/mod.rs`), and 12
clippy violations (3 × `clone_on_copy` on the `Copy` scope in
`transaction.rs`, 2 × `needless_question_mark`, 6 × `too_many_arguments`
given the house `#[allow]` style, 1 × `result_large_err` on the deliberate
early-HTTP-error `page_request` helper).

## 6. Cross-tenant isolation matrix (what the 20 tests prove)

For every surface, the attack program asserts the same invariant set against
a real database with two tenants A and B:

* **Read fencing** — B's read of A's id is `NotFound`/empty, never a row, and
  never distinguishable from a nonexistent id (no existence leak).
* **Write fencing** — B's update/transition/cancel/close/delete on A's id
  affects zero rows and fails closed (`NotFound` / `StaleWrite`), leaving A's
  row bit-identical.
* **Same-identity independence** — where business identity is tenant-local
  (leader address, event id, signal id, venue order id, fill id, position id,
  intent id, idempotency key, worker lane purpose), the *same id text* is two
  independent rows; neither tenant's write can evict, overwrite or advance the
  other's (composite arbiters, tenant-guarded conflict legs).
* **Global uniqueness preserved where it is physical** — transaction
  signatures stay globally unique while lookups answer only for the owner.
* **Aggregates scoped in SQL** — counts, PnL, balance totals and the composed
  dashboard summary are computed `WHERE organization_id = $1` inside the
  aggregate, never filtered post-load.
* **Pagination cannot jump tenants** — a cursor minted by A is rejected for
  B at the repository layer and surfaces as HTTP 400 at the route layer.
* **Race semantics preserved** — the same-tenant claim race still yields
  exactly one winner (Postgres serializes on the composite key); cross-tenant
  claims never contend; worker-lane takeover bumps the fencing generation so
  a crashed leader stays fenced.
* **Route-level end-to-end** — through the real router with real session
  auth: 200/404/400/401/503 exactly as specified, B's cancel of A's order
  changes nothing, and the plane answers 503 (not 500) when unattached.

## 7. Manifest refresh

`release-manifest.json` was stale after the STEP 10 program (the same failure
mode PROMPT 2's audit caught). Updated and **verified against both
enforcement formulas** (`scripts/final-release-check.sh` §6 and the
`release_manifest_counts_and_version_are_current` test):

* `rust_files` 436 → **506**; `test_count` → **1553** (`#[test]` formula);
  `migrations` 25 → **34**; `docs_files` **101** (unchanged);
* `components.database_migrations.count` → 34, `high_water_mark` → **"0034"**;
* new `added_0026_0034` entry describing all nine migrations.

## 8. Files added/changed (this session)

**Added (9):** the test files in §3 (8 cross-tenant suites + the shared
`trading_isolation_common` harness module).

**Changed (source fixes):**
`crates/core/src/trading_repository/positions/write.rs` (upsert `$29` fix +
arg-count allow), `crates/core/src/db/claims.rs` (release `$5` fix),
`crates/server/src/trading_data_plane/service.rs` (`write_scope` error
mapping), `crates/server/src/trading_data_plane/orders.rs`
(`result_large_err` allow with rationale), `crates/server/src/
trading_data_plane/mod.rs` (route tests appended),
`crates/core/src/trading_repository/{transaction,orders/conflicts,mod,
executions/write,executions/lifecycle,positions/balances}.rs` (lib-test
compilation fixes, clippy fixes),
`crates/core/tests/tenant_idempotency_isolation.rs` (0027 semantics),
`release-manifest.json` (§7).

## 9. Disciplines honored

* **PG gating** — every database test suite keeps the `db_integration.rs`
  contract: `NOT_RUN` + early return without `POSTGRES_URL`, so CI without a
  database stays green and honest.
* **`--test-threads=1`** for all PG suites (shared database, run-unique keys
  rather than per-test schemas — the established pattern of this codebase).
* **Fail-closed assertions** — cross-tenant reads must be `NotFound` (never a
  leaked row or a distinguishable error), cross-tenant writes must fail
  closed, and the tests pin the error variants.
* **Seeds bypass the code under test** — the suites exercise the repositories
  and routes; direct SQL only arranges the battlefield.
* **No shortcuts in the plane** — route tests run the real router with real
  session authentication, real rate-limit middleware configuration, and a real
  database; nothing is mocked except the SaaS control-plane store, which is
  the in-memory `SaasStore::shared()` fixture the existing suites use.

## 10. Deviations & judgment calls

* The isolation tests deliberately assert **`Ok(())` silent no-op** for B's
  same-global-id position upsert (the tenant-guarded `ON CONFLICT … WHERE
  organization_id = $1` leg matches zero of B's rows) — the implementation's
  exact, safe behavior — rather than demanding an error.
* `set_transaction_status` was found to **fail closed with `NotFound`** on a
  scoped zero-row update (stronger than the "no-op success" the plan memo
  assumed); the test pins the actual, safer contract.
* The suites assert on A-only ids for cross-tenant *mutation* attempts where a
  same-id row legitimately exists for both tenants (intents), so the attack
  and the independence property are proven separately.
* Build environment note: the verification sandbox has 2 cores / 1.9 GB RAM;
  the sniper-suite test binary's link step required a 3 GB swap file and
  `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0`. These are sandbox
  constraints only — the pinned toolchain (1.98.1), migrations and commands
  in §4 are the reproducible contract.

## 11. No-omission declaration

I declare that this report covers **everything** done under PROMPT 3/10 in
this workspace, including all defects found and fixed, all test updates and
their reasons, the manifest refresh, and the environment workarounds. Nothing
known was omitted, deferred, or hidden: every planned surface (17 arbiters +
`worker_claims` + indexes, 52 repository files, 21 routes) is present and
verified; every gate (fmt, check, clippy `-D warnings --all-targets`, full
test run with live PostgreSQL) was run to completion in this session with the
recorded outcomes; and every discrepancy found along the way is recorded in
§5 rather than silently repaired. The one deliberate scope note: the tenant
routes are not yet added to `docs/API.md` (the operator-facing API doc), so
`api_endpoints_documented` in the manifest was left unchanged — documenting
the tenant plane's HTTP contract is carried as follow-up work for the docs
prompt, not silently skipped.
