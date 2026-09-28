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
| Subscription & usage reads | `GET /api/saas/exports?kind=subscription`, `GET /api/saas/exports?kind=usage` | Served through the deterministic exports (there are no separate plan-catalogue or usage endpoints). The provider-neutral catalogue — `starter`, `pro`, `business`, `enterprise` (enterprise is private/invite) — and the plan's feature limits travel inside the `subscription` export; usage covers six metered metrics (orders submitted, fills booked, API requests, module runtime seconds, export rows, active members), recorded idempotently |
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

* The **only implemented billing provider is `manual`**: an operator assigns
  a plan. The `manual` adapter honestly reports that it has no hosted
  checkout and accepts no webhooks.
* The webhook route exists for `stripe`/`paddle` **as code and contract**,
  and returns `501 provider_not_implemented` until an adapter is registered
  AND a webhook secret is configured. There is no self-service checkout
  endpoint today.
* There is no invoicing, no tax handling, no payment-card data anywhere in
  this repository.

## The tenant web console

`apps/control-plane` is a Next.js (App Router) single-page console with
fourteen sections — Dashboard, Bots, Orders, Positions, Risk, Accounting,
Reconciliation, Workers, Wallets, Team, Audit, Billing, Usage, Settings.
The seven trading sections render an explanatory notice for tenant
sessions: that data belongs to the operator console and its deployment
credential. The console keeps the session token in tab memory only (no
localStorage), and the tenant switcher offers exclusively the organizations
the signed-in user actually belongs to.

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
* No self-service checkout or payment collection (see above).
* The audit export reflects the process's audit trail (durable rows when
  PostgreSQL is attached, ring buffer otherwise), scoped to records that
  target the caller's organization.
* Trading-truth data is not re-served per tenant by the SaaS layer; the
  operator console remains its interface.
