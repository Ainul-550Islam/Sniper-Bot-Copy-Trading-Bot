# Sniper Suite — SaaS Control Plane (Product)

Status: **implemented as described below; nothing more.** This document
describes what the code in this repository actually does today. It makes no
claim of institutional production-readiness, and the system has **not been
through an external security audit** (see `SAAS-SECURITY.md`).

The SaaS control plane (TASK 7A + 7B) is the multi-tenant layer over the
existing single-operator trading system. Trading truth — orders, fills,
positions, risk decisions, the ledger, HA leases, feed cursors — remains
owned by the TASK 1–6 engines. The SaaS layer decides **who may ask**; it
never approves a trade and never becomes a second source of truth.

## What a tenant gets

| Surface | Where | Notes |
|---|---|---|
| Account & sessions | `POST /api/saas/users`, `POST /api/saas/sessions`, `GET/PATCH /api/saas/users/me`, `POST /api/saas/users/me/logout` | Passwords are PBKDF2-hashed server-side (600k iterations); the session token is returned exactly once and only its hash is stored; TTL 12 h |
| Organizations | `POST /api/saas/organizations`, `GET/PATCH /api/saas/organizations/:id`, `GET …/members`, `POST …/suspension` | Creation goes through the provisioning state machine; the creator becomes OrgOwner |
| Tenant API keys | `POST/GET /api/saas/api-keys`, `DELETE /api/saas/api-keys/:prefix` | Secret shown once; only the hash is stored; revocation is immediate and restart-safe |
| Subscription & usage reads | `GET /api/saas/billing/status`, `GET /api/saas/usage/limits`, `GET /api/saas/invoices` (+`/:id`), `GET /api/saas/exports?kind=subscription`, `GET /api/saas/exports?kind=usage` | The billing status is the authoritative view (real subscription → plan, real dunning, real usage totals); usage/limits reports current usage versus plan limits — every number is store-derived, and an organization without a subscription sees `plan_code:"none"` (never a default tier). The provider-neutral catalogue — `starter`, `pro`, `business`, `enterprise` (enterprise is private/invite) — and the plan's feature limits travel inside the `subscription` export and the billing status; usage covers six metered metrics (orders submitted, fills booked, API requests, module runtime seconds, export rows, active members), recorded idempotently |
| Wallet access | `POST/GET /api/saas/wallet-access`, `DELETE …/:id`, `POST …/:id/authorize` | Public data only (label, public address, modules). The authorize endpoint answers `ownership ∧ binding ∧ permission ∧ entitlement` |
| Exports | `GET /api/saas/exports?kind=…` | Deterministic tenant-scoped sections: `profile`, `members`, `api_keys`, `usage`, `subscription`, `wallets`, `audit` |
| Event stream | `GET /api/saas/events` (WebSocket) | Session- or key-authenticated, tenant-scoped; replaces the legacy `?key=` stream (which still works unchanged) |
| Public contract | `GET /api/saas/openapi.json` | The machine-readable contract for everything above |
| Billing webhooks | `POST /api/saas/billing/webhooks/:provider` | Signature-verified, idempotent by provider event id |

## Plans and roles

Eight roles with a fixed permission vocabulary of 22 `resource.verb`
permissions: PlatformAdmin (cross-tenant staff) and OrgOwner hold all 22;
OrgAdmin 21; Trader 12; SecurityAdmin 16 but risk-manage is its only
mutating power; BillingAdmin 4; Auditor 11 (read-only + `export.create`);
Viewer 6 (read-only). A tenant's plan resolves to feature entitlements —
for example `module.polymarket` is disabled on `starter`, and member limits
are enforced as plan limits (`limit.max_members`), not conventions.

## Plans and billing reality (read this before selling anything)

* **`manual`** is the always-available provider: an operator assigns a
  plan; the manual adapter honestly reports that it has no hosted checkout
  and accepts no webhooks.
* **`stripe`/`paddle` adapters are REAL code** (`crates/server/src/billing/
  {stripe,paddle}_adapter.rs`): webhook signature verification + freshness,
  idempotent provider-event application, and checkout-session creation via
  `POST /api/saas/checkout` (idempotency key required). **No live
  round-trip has been performed** — provider calls are gated behind
  `LIVE_BILLING=1` + real credentials; without them every provider path
  fails closed with a typed error (501/503), never a fake success
  (GAP-001, `docs/EXTERNAL-VALIDATION-RUNBOOK.md`).
* Invoice records are created and served (`GET /api/saas/invoices`);
  there is no tax handling and no payment-card data anywhere in this
  repository.

## The tenant web console

`apps/control-plane` is a Next.js (App Router) single-page console with
fourteen sections in three groups — Overview (Dashboard), Trading
operator console (Bots, Orders, Positions, Risk, Accounting,
Reconciliation, Workers), and Tenant (Wallets, Team, Audit, Billing,
Usage, Settings). For tenant sessions the seven operator trading
sections render an explanatory notice: that data belongs to the operator
console and its deployment credential.

Customers get their own trading area instead: **eight customer pages
under `/trading/*`** (overview, orders, positions, executions, sniper,
copy, polymarket, telegram), served exclusively by the tenant trading
API through `lib/customer-trading-api.ts`, which refuses any path
outside `/api/tenant/*` — customer pages cannot reach operator-global
endpoints. The console keeps the session token in tab memory only (no
localStorage), and the tenant switcher offers exclusively the
organizations the signed-in user actually belongs to.

## Deployment shapes

1. **Existing single-operator install (unchanged):** run `sniper-suite` as
   always. The legacy API, dashboard and `/api/events` behave exactly as
   before; the deployment key keeps working. SaaS endpoints additionally
   exist.
2. **Multi-tenant:** the same binary with PostgreSQL attached and the SaaS
   signup flow used; tenants sign in and manage their own tenants.

Both shapes run from the same binary; there are no separate SaaS
microservices, and there is intentionally no second ledger, risk engine or
tenant store.

## Explicit non-goals / limitations

* No email verification flow is enforced for function (the field exists and
  is displayed; verification delivery is not built).
* Self-service checkout exists as an API; **payment collection is not
  live-proven** (GAP-001: `LIVE_BILLING=1` + provider credentials required,
  buyer-side). No payment-card data is handled.
* The audit export reflects the process's audit trail (durable rows when
  PostgreSQL is attached, ring buffer otherwise), scoped to records that
  target the caller's organization.
* Trading truth is served per tenant through the tenant data plane
  (`/api/tenant/*`: orders, executions, positions, copy, polymarket,
  recovery, reporting) — every SQL predicate carries `organization_id`,
  cross-tenant reads are not-found, and a runtime-less deployment answers
  `503 trading_data_plane_unavailable`. The operator console keeps its own
  global view.
