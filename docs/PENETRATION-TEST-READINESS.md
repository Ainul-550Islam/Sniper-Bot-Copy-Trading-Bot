# Penetration-Test Readiness — sniper-suite 0.1.0

> **Explicit:** No external penetration test has occurred. `cargo audit`/`cargo deny` are internal supply-chain checks, not a pentest. Do not claim “audited”.

## 1. Scope & Attack Surface

**In-scope (implemented):**
- `crates/server` Axum control plane: `GET /health`, `/api/*`, `/api/saas/*` (REST), `/api/events` (WS)
- Auth: `x-api-key` or `Authorization: Bearer`, `saas/users` (register/login), `saas/sessions`, `saas/organizations`, `saas/api_keys`
- SaaS middleware: tenant isolation, RBAC (Owner/Operator/Readonly), lifecycle, rate limiting
- Billing: `saas/billing/*`, `billing_webhook`, `checkout`, `invoices`, `commercial_state`
- Custody: `saas/custody/*`, `custody_health`, `custody_rotation` (refs only, no live signing)
- Lifecycle: `tenant_lifecycle`, `data_lifecycle`, retention/purge, provisioning leases
- Observability: `health`, `ready`, `metrics`, `ops/*`
- Frontend: `apps/control-plane` (Next.js 16, 5 routes, session in tab memory)

**Out-of-scope / Not included:**
- Solana RPC/Geyser, Telegram Bot API, Stripe/Paddle live, Vault/KMS/HSM live, funded wallets — all `EXTERNAL_REQUIRED`

## 2. Test Accounts & Roles

| Account | Role | How to create | Permissions |
|---|---|---|---|
| Owner | Owner | `POST /api/saas/users` (register) → first user of org is Owner via `organizations::create_organization` | All: org write, members, api_keys create/revoke, billing read, custody read/rotate (tenant-scoped) |
| Operator | Operator | Owner invites via `organizations::list_members` → membership `operator` | Mutating routes (`POST /api/orders`, etc.) but not key admin |
| Readonly | Readonly | Owner creates | Reads only |
| Platform admin | `DEPLOYMENT_ORG_SLUG` | Seed via `saas::ensure_deployment_organization` | Cross-tenant context resolution (test only) |
| Anonymous | — | — | Only `POST /api/saas/users` + `POST /api/saas/sessions` + `GET /health` |

**Credentials:** Create via `POST /api/saas/users` (email/password) → `POST /api/saas/sessions` → `session_token` (use as `Authorization: Bearer`). API keys via `POST /api/saas/api_keys` (secret shown once, hash stored).

## 3. Endpoints to Test

**Auth required:**
- `GET /api/saas/users/me`, `PATCH /api/saas/users/me`, `POST /api/saas/users/me/logout`
- `GET/PATCH /api/saas/organizations/:id`, `GET /api/saas/organizations/:id/members`
- `POST /api/saas/api_keys`, `GET /api/saas/api_keys`, `DELETE /api/saas/api_keys/:hash`
- `GET /api/saas/billing/*`, `GET /api/saas/custody/*`, `GET /api/saas/backup/*` (if implemented)
- `GET /api/orders`, `GET /api/positions`, `POST /api/mode` etc. (legacy deployment-key gate)

**Public:**
- `GET /health` (no auth), `GET /ready` (depends on DB/Redis), `GET /metrics` (if enabled)

## 4. WebSocket

- Endpoint `GET /api/events` — requires `x-api-key` or `Authorization: Bearer` OR first frame token (see `saas/websocket_auth.rs`). Legacy `?key=` is `LEGACY_ENABLED` only via `security/legacy_websocket_guard.rs` (disabled by default, deprecation header).
- Test: unauthenticated connect → 401; cross-tenant subscription → no events; replay after restart → deduped via `backup/preflight.rs`.

## 5. Billing & Custody

- **Billing:** Attempt price tampering (`POST /api/saas/checkout` with amount field — SDK test `checkout_request_has_no_amount` ensures server-authoritative price); webhook spoof (`POST /api/saas/billing_webhook` without HMAC → 401); replay same webhook → idempotency.
- **Custody:** Attempt to resolve signing identity without Vault/KMS/HSM (`custody/resolve.rs` should fail-closed); attempt rotation race (old revoked before replacement — should fail).

## 6. Lifecycle & Rate Limits

- **Lifecycle:** Suspend org via `data_lifecycle.rs` then attempt `POST /api/saas/api_keys` → should be denied (409/403); purge before retention → denied.
- **Rate limits:** Burst `GET /api/saas/users/me` > `rate_limit_rpm` (default 0 disabled, set via `api.rate_limit_rpm`) → 429 + `Retry-After`.

## 7. Known Exclusions (do not test as flaws)

- `target/` build output — .gitignore'd, excluded from `buyer-release` (see `scripts/verify-delivery.sh` hygiene fix 2026-09-24).
- `node_modules/` — excluded, frontend `package-lock.json` is real but not a finding.
- Example secrets in `crates/core/migrations/0017...` (`sk_live_ab12cd34`) and `is_secret_like` test fakes (`MIIBIj...`, `sk_live_51Hxxx…`) — filtered by secret scans.

## 8. Artifacts to Provide to Tester

- `docs/SECURITY-THREAT-MODEL.md`, `docs/SECURITY-CONTROLS-MATRIX.md`, `docs/API-COMPATIBILITY-MATRIX.md`, `docs/WEBHOOK-COMPATIBILITY-MATRIX.md`
- `release-manifest.json` (43 migrations, 616 Rust files, 1783 `#[test]` attributes), `sbom.json`, `licenses.json`
- `scripts/verify-delivery.sh` (7/7 PASS) + `scripts/build-release-package.sh` (package excludes secrets)

## 9. Success Criteria (for buyer to set)

- No cross-tenant read/write, no secret leakage in logs/URLs, no unauth WS, no price tampering, no signing bypass, rate limiting enforced where enabled.

> **Next step:** Buyer commissions external firm, provides this doc + test accounts, and tracks findings in `docs/INCIDENT-RESPONSE-RUNBOOK.md`.
