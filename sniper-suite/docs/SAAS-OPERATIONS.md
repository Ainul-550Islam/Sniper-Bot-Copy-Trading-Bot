# Sniper Suite — SaaS Control Plane (Operations)

Running notes for the SaaS layer added by TASK 7A/7B. Everything here
describes the repository as it is; **no institutional production-readiness
is claimed and no external security audit has been performed** (see
`SAAS-SECURITY.md`).

## Binary and process model

One binary (`sniper-suite`) serves the legacy API, the dashboard, the
legacy event websocket AND the SaaS control plane. There are no SaaS
sidecars or microservices. A single-operator install upgraded to this
version changes zero existing calls; the deployment key and `/api/events`
behave as before.

## Environment

| Variable | Effect |
|---|---|
| `POSTGRES_URL` | Enables durable SaaS storage (runtime records, plans, subscriptions, entitlements, usage, audit chaining, durable webhook event ids). Without it the SaaS store runs in the documented in-memory mode (tests/single-process only). |
| `SAAS_WEBHOOK_SECRET_MANUAL` / `_STRIPE` / `_PADDLE` | Per-provider webhook secrets. Absent ⇒ that provider's webhook route answers `501 webhook_not_configured`. Never logged. |
| `NEXT_PUBLIC_API_ORIGIN` | The web console's API origin; unset = same origin. No secret belongs in frontend env. |
| Existing configuration | Unchanged: execution stays paper by default; `DEPLOYMENT_KEY` etc. behave as before. |

## Migrations

SaaS migrations live in `crates/core/migrations/` (TASK 7A files 0001–0018)
and are additive and repeatable; 0017 re-applies as NOTICE-only. Apply in
order; the durable runtime-record store (0018) is what makes webhook
idempotency and wallet bindings survive restarts.

## Daily operation

* **Webhook endpoint**: `POST /api/saas/billing/webhooks/:provider`.
  Responses: `200 applied | ignored | duplicate`, `401` verification
  failure, `422 rejected` (bad organization/plan/closed tenant — safe for
  the provider to retry a corrected event), `501` not implemented/configured.
  Replays are always safe.
* **Sessions**: 12-hour TTL; revocation is immediate (used by logout and
  admin suspension). The websocket re-checks authorization every 60 s.
* **Suspension**: `POST /api/saas/organizations/:id/suspension` — collapsed
  permission set (read + risk-reduction + billing) until restored.
* **Audit**: hash-chained when PostgreSQL is attached
  (`GET /api/audit/verify` still verifies the chain); the ring buffer is the
  fallback without a database.
* **Exports**: rate-limit-friendly deterministic snapshots; the `audit`
  section is bounded at 500 records.

## The web console

`apps/control-plane` (Next.js 15 App Router):

```bash
cd apps/control-plane
npm install
npm run dev        # development
npm run build && npm run start   # production
npm run typecheck  # strict TS gate
npm run lint
```

The console holds no secrets: the session token lives in tab memory only,
and `NEXT_PUBLIC_API_ORIGIN` is the only env var it reads.

## Health and failure modes

* SaaS endpoints ride the same process, rate limiter (per-IP + per-principal
  buckets) and audit trail as the legacy API.
* If PostgreSQL is attached but degrades, SaaS store calls fail with
  explicit errors (`storage_failed`); the in-memory mode is NEVER silently
  substituted in production — absence of `POSTGRES_URL` is a startup-time
  choice.
* Webhook processing marks events durably BEFORE acknowledging; a crash
  between mutation and mark leaves the mutation idempotent to replay.

## Honest capacity statement

The SaaS layer is built and tested for small multi-tenant deployments
(organizations in the tens, not thousands). There is no sharding, no
multi-region story, no SLA tooling. Do not present it otherwise.
