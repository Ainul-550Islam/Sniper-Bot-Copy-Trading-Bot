# Customer SaaS Status 2026 (2026-09-30)

EVIDENCE-LEVEL: INTEGRATION_TEST

What a customer (tenant) can actually do through the customer API and
UI today, and what they cannot.

## Customer API surface (`/api/tenant/*`)

Every route runs the full authorization chain: authenticate →
organization from the credential (a client can never choose it) →
trading plane attached → lifecycle gate → module-family entitlement →
tenant-scoped repository predicates. Denials carry stable machine codes
(`trading_data_plane_unavailable`, `tenant_lifecycle_blocked`,
`module_not_entitled`).

| Surface | Routes | Evidence |
| --- | --- | --- |
| Bots/runtimes | `GET /api/tenant/bots`, `GET /api/tenant/bots/:module` | INTEGRATION_TEST |
| Orders | `GET /api/tenant/orders` (+cursor pagination), `GET …/:id`, `POST …/:id/cancel` | INTEGRATION_TEST |
| Positions / PnL | `GET /api/tenant/positions`, `…/trades`, `…/balances`, `GET /api/tenant/reports/pnl`, `…/reports/summary` | INTEGRATION_TEST |
| Executions | `GET /api/tenant/executions` (window or per-order) | INTEGRATION_TEST |
| Recovery | `GET /api/tenant/recovery/intents`, `POST …/recovery/sweep` | INTEGRATION_TEST |
| Copy | `GET /api/tenant/copy/leaders`, `…/:address`, `…/links` | INTEGRATION_TEST |
| Polymarket | `GET /api/tenant/polymarket/orders`, `…/fills`, `…/reconciliation` | INTEGRATION_TEST |
| **Module controls (new 2026-09-30)** | `GET /api/tenant/{sniper,copy,polymarket}/status`, `POST …/controls` (enable/disable, tenant-scoped, BotStart/BotStop permission) | UNIT_TEST (module-scoped tests; chain gates integration-tested) |
| **Telegram binding (new 2026-09-30)** | `GET /api/tenant/telegram/status`, `PUT/DELETE /api/tenant/telegram/binding` (chat-id binding, tenant-scoped) | UNIT_TEST |

The customer API client (`apps/control-plane/src/lib/customer-trading-api.ts`)
enforces the same boundary client-side: it refuses to call anything
outside `/api/tenant/*` — customer pages cannot reach operator-global
endpoints even by mistake.

## Customer UI (`apps/control-plane`)

| Page | What it shows |
| --- | --- |
| `/trading` | dashboard: realized PnL (today), runtime/fence table, module cards |
| `/trading/orders` | server-paginated orders + per-order execution drill-down + cancel |
| `/trading/positions` | positions table + PnL card |
| `/trading/executions` | execution lifecycle over a selectable window |
| `/trading/sniper` `/trading/copy` `/trading/polymarket` | module status + tenant-level enable/disable controls |
| `/trading/telegram` | control-plane status + notification binding management |
| `/billing`, `/custody`, `/settings/data-lifecycle` | pre-existing customer surfaces |

Every trading surface renders all its honest states: loading, empty,
error, **suspended tenant**, **entitlement denial**, **module disabled**,
**stale runtime** (no registered runtime), **trading plane unavailable**,
**custody unavailable**. No synthetic demo numbers exist anywhere in the
UI — an absent value renders as absent.

## Honest limitations

| Limitation | Detail |
| --- | --- |
| Module controls are tenant-level | They record enable/disable for the tenant's own organization; runtime lifecycle transitions stay runtime-owned and fenced (by design, not omission). |
| Telegram binding is a declaration | The binding records the tenant's notification chat; the deployment-level alert forwarder currently routes to the deployment alert chat — per-tenant routing is a deployment-side integration step (stated in the UI itself). |
| Controls store is process-local | Same contract as the custody store; the DB-backed path is a migration away and the API surface will not change. |
| No LIVE usage | The whole surface is INTEGRATION_TEST-level (PostgreSQL-backed suites); no production tenant has used it. |

## Infrastructure

* PostgreSQL 17, forward-only migrations `0001`–`0034`
  (tenant-scoped predicates, reporting indexes, worker claims,
  accounting conflict handling).
* Tenant identity: auth principal → membership → organization →
  runtime → entitlement → repository. The client never supplies the
  organization; `x-organization` is a re-verified hint only.
