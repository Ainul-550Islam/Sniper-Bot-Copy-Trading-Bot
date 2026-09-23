# Sniper Suite — SaaS Control Plane (Security)

> **No external security audit.** This codebase has NOT been reviewed by an
> external security auditor. The controls below are implemented and unit
> tested in-repo, but an in-repo test suite is not an audit, and nothing
> here should be treated as certified, accredited or compliance-approved.
> Likewise, no institutional production-readiness is claimed.

## Trust model in one paragraph

The browser is never an authorization boundary. Every request is
authenticated and authorized server-side; whatever a client sends as an
organization hint (`x-organization`) can only CONFIRM the credential's own
tenant — platform staff excepted — and can never switch or widen it. Every
refusal is produced by one decision type (`deny_unauthenticated`,
`deny_tenant`, `deny_role`, `deny_permission`, `deny_resource`,
`deny_suspended`, `deny_entitlement`) and security-relevant refusals are
audited.

## Authentication

* **Passwords** — PBKDF2-HMAC-SHA256, 600 000 iterations, per-user salt.
  Plaintext never persisted; login compares in constant time.
* **Sessions** — `POST /api/saas/sessions` returns the plaintext token
  exactly once (32 random bytes, base64url). Only its SHA-256 hash is
  stored, keyed by a public prefix. TTL 12 hours; logout, expiry and
  revocation all end access; `validate()` rejects `Missing`, `Unknown`,
  `Expired`, `Revoked`, `WrongTenant`.
* **Tenant API keys** — same storage discipline (hash + prefix), tenant-bound
  at creation, optionally scoped below the role and expiring.
* **Legacy deployment key** — unchanged; pre-TASK-7A single-operator installs
  keep working verbatim.

Credential order on a SaaS request: tenant API key → session → legacy
deployment key.

## Authorization

One vocabulary of 22 stable `resource.verb` permissions across 8 roles.
Effective permissions = the membership role's set intersected with the
credential's scopes (scopes can only remove power). Tenant lifecycle gates
actions: `active`/`trialing` allow all entitled actions; `past_due` and
`suspended` collapse to read + risk-reduction + billing; `closed` denies.
`is_money_affecting` actions (wallet manage, bot start, order manage, risk
manage) are exactly the four permissions that can move money or exposure.

## Secret-handling rules (all enforced in code)

* No private key, session secret, API secret or billing-provider secret is
  ever placed in frontend source, logs, URLs, or ordinary JSON responses.
* Key/session/API-key secrets are stored as hashes; create-endpoints return
  the secret exactly once; the OpenAPI contract marks such fields
  `writeOnly`.
* Webhook signing secrets are process configuration
  (`SAAS_WEBHOOK_SECRET_{MANUAL,STRIPE,PADDLE}`); their Rust type redacts
  `Debug`, and an empty/missing secret DISABLES the integration instead of
  accepting unsigned traffic.
* The websocket contract refuses tokens in URLs; credentials travel in the
  `Authorization` header or a first auth frame.

## Billing webhooks

`POST /api/saas/billing/webhooks/:provider` is the only billing input, and
it is NOT user-authenticated — it is signature-gated:

1. adapter verifies the signature (boundary scheme:
   `hex(HMAC-SHA256(secret, "{timestamp}.{body}"))`, ±300 s freshness,
   constant-time compare) — an invalid signature is a 401 and **zero state
   change**;
2. the envelope (`{id, type, data}`) is parsed strictly;
3. idempotency: the provider event id is checked and then durably recorded
   (PostgreSQL runtime records) — a replay answers `duplicate` with no
   second mutation; unknown event types answer `ignored` deterministically;
4. state transitions are whitelisted (`plan.changed`,
   `subscription.payment_failed|renewed|canceled|expired`) and derived from
   TASK 7A domain methods — client-supplied status strings are never
   trusted;
5. entitlements change only as a consequence of the plan assignment;
6. everything is audited; the market-truth ledger (`bot_core` accounting)
   is never touched by billing.

Until a provider adapter and its secret are deployed, the route answers
`501` — it cannot be half-enabled.

## The tenant → wallet → strategy boundary

Wallet bindings hold PUBLIC data only (label, public address, module
list). The SaaS layer has no field for, and no path to, signing material;
the signer registry stays where TASK 1–4 put it. Running a module against a
wallet requires all four gates: same-tenant ownership, live binding, the
`wallet.manage` permission, and the plan entitlement for that module. Every
list/read path filters by the caller's own tenant; id-keyed endpoints
refuse cross-tenant access before disclosing existence.

## Transport & headers

Every response carries: CSP (`default-src 'self'`; the dashboard's single
inline script is the documented reason for `script-src 'unsafe-inline'`;
no wildcard hosts), `X-Content-Type-Options: nosniff`,
`Referrer-Policy: strict-origin-when-cross-origin`, `X-Frame-Options: DENY`
plus `frame-ancestors 'none'`, a restrictive `Permissions-Policy`, and HSTS
(`max-age=31536000; includeSubDomains`) when the request arrived over TLS.
API responses are `Cache-Control: no-store`.

## Websocket authorization

The primary stream `GET /api/saas/events` authenticates a session or tenant
API key (header) BEFORE upgrading, or accepts one auth frame from browsers
within 10 s; no data flows before authentication. Events are scoped:
`saas.*` frames carry an organization id and are delivered only to that
organization; market-global frames pass. The credential is revalidated
every 60 s — a revoked session/key, suspended membership or closed
organization closes the socket (1008). The legacy `/api/events` endpoint
keeps its exact previous behaviour so existing installs do not break.

## Known limitations (honest list)

* No rate-limit beyond the per-IP token bucket + per-principal buckets; no
  CAPTCHA or bot defense on signup/login.
* No email-verification enforcement, no password reset flow, no 2FA.
* Webhook signature verification implements ONE scheme in-repo; real
  Stripe/Paddle verification arrives with their adapters (the route refuses
  until then).
* The audit export is bounded (`500` records) and reflects the process's
  audit trail.
* The in-repo test suite (unit + integration, incl. webhook idempotency,
  WS auth, wallet isolation, export isolation) is the only verification
  that exists — again: no external audit.
