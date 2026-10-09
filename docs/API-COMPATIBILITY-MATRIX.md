# API Compatibility Matrix — sniper-suite 0.1.0

> No claim of semantic versioning unless enforced. Version 0.1.0, <!-- stat:migrations -->54<!-- /stat --> forward-only migrations (high water `<!-- stat:migrations_high_water -->0054<!-- /stat -->`), <!-- stat:crates -->8<!-- /stat --> members. Legacy path status accurate.

## Versioning

- **App version:** `0.1.0` (`VERSION`, `Cargo.toml` workspace, `release-manifest.json`, `GET /health` `version`)
- **No SemVer guarantee yet:** Pre-1.0, breaking changes allowed with minor bump + `CHANGELOG.md` entry (per `docs/RELEASE.md` §1). No automated semver check in CI.
- **Schema:** DB forward-only <!-- stat:migrations -->54<!-- /stat --> migrations (high water `<!-- stat:migrations_high_water -->0054<!-- /stat -->`), additive.

## REST

| Group | Endpoints | Auth | Status | Compat Note |
|---|---|---|---|---|
| **Health** | `GET /health`, `GET /ready`, `GET /metrics` | `health` public, `metrics` if enabled | **STABLE** | `health` version field typed via `obs.rs` |
| **Legacy control** | `GET /api/status`, `/api/orders`, `/api/positions`, `/api/trades`, `/api/config`, `/api/ha`, `/api/risk/*`, `/api/accounting/*` | Deployment-key (`x-api-key`/`Authorization: Bearer`) via `api.rs` `require_role` | **STABLE** (since freeze) | Single-operator, unchanged by SaaS |
| **SaaS identity** | `POST /api/saas/users` (register), `POST /api/saas/sessions` (login) | Public | **STABLE** | `saas/users.rs` |
| **SaaS user** | `GET/PATCH /api/saas/users/me`, `POST /api/saas/users/me/logout` | Session token | **STABLE** | |
| **SaaS org** | `POST /api/saas/organizations`, `GET/PATCH /api/saas/organizations/:id`, `GET /.../members`, `PATCH /.../suspension` | `authorize_request` | **STABLE** | `organizations.rs` |
| **SaaS api_keys** | `POST /api/saas/api_keys`, `GET /api/saas/api_keys`, `DELETE /api/saas/api_keys/:hash` | Owner | **STABLE** | Secret shown once |
| **SaaS billing** | `GET /api/saas/billing/*`, `GET /api/saas/commercial_state`, `GET /api/saas/readiness` | Tenant-scoped `BillingRead`/`WalletManage` | **STABLE** (Batch3) | `saas/billing_status.rs` etc. |
| **SaaS custody** | `GET /api/saas/custody/*`, `POST /api/saas/custody/rotation` | Tenant-scoped | **STABLE** | `custody_rotation.rs` |
| **SaaS lifecycle** | `GET /api/saas/data_lifecycle`, `POST /api/saas/export` | Tenant-scoped | **STABLE** | `data_lifecycle.rs`, `export.rs` |
| **HA** | `GET /api/ha` | Deployment-key | **STABLE** | `ha.rs` |
| **OpenAPI** | `GET /api/saas/openapi.json` | Session/token | **STABLE** | `saas/openapi.rs` |

## OpenAPI

- Path: `GET /api/saas/openapi.json` (implemented `saas/openapi.rs`, `api/openapi_*.rs`)
- Version: `0.1.0`, `crates/server/src/api/openapi_*.rs`
- No breaking change since Batch3; additive.

## WebSocket

| Path | Auth | Status | Note |
|---|---|---|---|
| `GET /api/events` | `x-api-key` / `Authorization: Bearer` OR first-frame token (`saas/websocket_auth.rs`) | **STABLE** | `security/websocket.rs` tenant-scoped |
| `GET /api/events?key=` (legacy query) | `?key=` | **DEPRECATED** `LEGACY_ENABLED` only, `security/legacy_websocket_guard.rs` (disabled by default, compat mode with deprecation header), `security_headers.rs` | Do not use; prefer header. Test `websocket_auth` |

## SDK

- **Crate:** `saas-sdk 0.1.0` (`crates/saas-sdk/src/{lib.rs,billing.rs,custody.rs,commercial.rs,error.rs,models.rs,client.rs}`)
- **Compat:** Typed, `secret-free Debug`, no secrets in URLs (tests `client::client_debug_never_emits_secrets`),
- **Versioning:** Matches server `0.1.0`; breaking SDK change requires server bump.

## Authentication

- **Deployment-key:** `x-api-key` header first, then `Authorization: Bearer` (`api.rs` `extract_key`), `Role::Owner/Operator/Readonly`, `RateLimiter` per-IP+principal.
- **Session:** `Authorization: Bearer <session_token>` (hashed via `hash_token`), `saas/middleware.rs`.
- **Legacy:** `?key=` deprecated, see above.

## Deprecated Paths

| Path | Replacement | Removal |
|---|---|---|
| `GET /api/events?key=` | `Authorization: Bearer` header or first-frame | Not removed, guarded, WARN header |

> **Verification:** `grep -R "pub fn routes" crates/server/src/saas --include="*.rs"`, `curl -s http://localhost:8080/api/saas/openapi.json | jq`, `cargo test -p saas-sdk` (32), `cargo test -p sniper-suite` `websocket_auth` 7.
