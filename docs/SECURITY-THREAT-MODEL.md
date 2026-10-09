# Security Threat Model — sniper-suite 0.1.0

> **Do not claim “secure”.** This model lists threats, existing controls, remaining exposure, and evidence. External assessment still required (see `docs/PENETRATION-TEST-READINESS.md`).

## 1. Tenant Isolation

| Threat | Control | Remaining Exposure | Evidence |
|---|---|---|---|
| Cross-tenant read (list tenants/orders) | `saas/middleware.rs` `authorize_request` → org→membership→permission→lifecycle at app+query (`store.rs` tenant-scoped queries) | If middleware skipped on new route, isolation breaks | `crates/core/tests/saas_control_plane.rs`, `crates/server/tests/tenant_lifecycle_integration.rs` |
| Cross-tenant write (update other org) | Same middleware + `organizations.rs` membership check | Deployment org bootstrap must not be callable by non-admin | `organizations::create_organization` provisioning state machine |
| Data export leaks other org | `saas/export.rs` deterministic tenant-scoped, audit logged | Export must not bypass `authorize_request` | `export.rs` tests 4 |

## 2. Authentication / Session

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Session fixation / theft | Session `hash_token` (SHA256), httpOnly? (frontend `apps/control-plane` stores token in tab memory only, not localStorage) | XSS could still steal from JS memory; frontend must set `Secure`/`SameSite` via `security/headers.rs` | `crates/server/src/security/headers.rs` (CSP, HSTS), `users.rs` login |
| Expired session reuse | `store.rs` + `postgres.rs` revocation, `hash_token` check on each request | Clock skew if DB not authoritative | `store.rs` restart/revocation tests |
| Brute force | `bot_core::auth::RateLimiter` per-IP + per-principal token bucket (`api.rs` `ip_rate_limit` + `require_role`) | Rate limits are in-memory unless Redis backed | `src/ops/rate_limit_report.rs` reports thresholds only |

## 3. API Keys

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Plaintext leakage in logs/URLs/JSON | `is_secret_like` / secret_scan, API key secret shown once at create (`api_keys.rs`), stored as hash via `hash_token`, `saas-sdk` `secret-free Debug`, `cargo test` asserts `!dbg.contains("sk_live_secret")` | If new handler logs `x-api-key` header, leak | `saas/api_keys.rs`  `saas-sdk/src/client.rs`  `ops/security_evidence.rs` |
| Replay of revoked key | `store.rs` revocation + `hash_token` lookup on each request, `api_keys::revoke` | Revocation must propagate to all replicas via Postgres | `store.rs` `test_api_key_restart_revocation_expiry` (real Postgres) |
| Permission escalation via key | `middleware.rs` permission-subset validation (no size-only compare) | If new permission added without subset check, escalation | `middleware.rs` `deny_response` |

## 4. WebSocket Authentication

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Unauthenticated event stream | `security/websocket.rs` authenticated tenant-scoped stream; `security/legacy_websocket_guard.rs` (disabled/compat/legacy-enabled) + deprecation headers | Legacy `?key=` path if enabled without guard | `saas/websocket_auth.rs`  `websocket_auth.rs` header/first-frame |
| Cross-tenant events | WS `SaasContext` checked per connection, `events` filtered by org | If event bus not filtered, leak | `api.rs` `events_ws` |
| Replay of old events | `audit_export.rs` deterministic, not replayed via WS | — | `ws.rs` `event_to_json` |

## 5. Billing Manipulation

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Price/plan tampering by client | Server-authoritative: `core/billing/pricing.rs` immutable snapshot, `checkout.rs` has no amount field (`checkout_request_has_no_amount` test), `provider_config.rs` no secrets | If SDK allowed price field, client could invent price | `billing/pricing.rs`, `saas-sdk` |
| Webhook spoofing | `billing_webhook.rs` HMAC-SHA256 verify, `provider_events.rs` idempotency + secret stripping | Endpoint must not be public without signature | `billing_webhook.rs`  `provider_events.rs` |
| Double billing / replay | `reconciliation.rs` never invents success, `billing_status.rs` + `dunning.rs` 7-state machine, `usage_policy.rs` 80/100% thresholds | Requires `EXTERNAL_REQUIRED` live Stripe/Paddle to verify | `reconciliation.rs` |

## 6. Custody / Signing

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Remote signing bypass → local fallback | `core/custody/provider_config.rs` `local_fallback_allowed=false`, `custody/resolve.rs` fail-closed, `build_signer_registry` fails startup if provider unsupported | If new signer added without fallback check, bypass | `custody/provider_config`, `crates/server/src/ops/external_validation.rs` `vault/kms/hsm` NOT_EXECUTED |
| Private key exfiltration | `custody/credentials.rs` indirect refs only (`VaultRef`/`KmsRef`), never plaintext; `secrets` env injection via `seed_secret_env` without clobber; `AuditTrail` never logs secret | If handler logs `solana_keypair`, leak | `credentials.rs`, `health.rs` |
| Rotation race | `custody/rotation.rs` Pending→Active→Draining→Revoked, old not revoked before replacement | If force-revoked incorrectly, funds stuck | `rotation.rs`, `saas/custody_rotation.rs` |

## 7. Replay / Idempotency / Race

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Order replay | `DedupStore` L1 memory + L2 Redis/Postgres, `dedup_ttl_secs`, `max_dedup_entries` | If Redis off + Memory, multi-replica replay possible (warn in `main.rs`) | `redis_integration.rs` dedup |
| Copy leader race | `provisioning/job_claim.rs` `SELECT ... SKIP LOCKED` leasing, bounded backoff | If SKIP LOCKED not used, duplicate claim | `job_claim.rs` |
| Ha lease split-brain | `OwnershipRegistry` fencing generations, `PostgresClaimStore` atomic, `MemoryClaimStore` warn single-replica | Live trading on Memory store double-executes (warn) | `ha_distributed` |

## 8. Secret Leakage

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| URL/JSON/audit/logs contain secret | `secret_scan` (`grep -R BEGIN PRIVATE KEY` etc. filtered), `security/headers.rs` not rewriting bodies, `ops/health_report.rs` redacts `postgres://`, `redis://`, `sk_live` | New code must use `is_secret_like`/`redacted` helpers | `scripts/verify-buyer-package.sh` + `final-release-check.sh` secret scan PASS, `saas-sdk` secret-free tests |
| .env committed | `.gitignore` `/ .env`, `verify-delivery.sh` hygiene FAIL on `.env` | If `.env` added before gitignore, committed | `verify-delivery.sh` hygiene gate |

## 9. CORS / SSRF / Webhook

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| CORS wildcard on SaaS API | `security/cors_policy.rs` `CorsPolicy::from_config` fails closed, default no CORS, wildcard not default | If `cors_origins=["*"]` configured, still allowed but must be explicit | `cors_policy.rs` |
| SSRF via RPC URL | `config` `network.cluster` validated, `Rpc::new` checks url | If new field fetches arbitrary URL without allowlist, SSRF | `solana-kit/src/rpc.rs` |
| Webhook SSRF | Outbound webhooks not implemented (inbound only) | — | `billing_webhook.rs` inbound only |

## 10. Lifecycle Abuse

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Tenant suspension bypass | `tenant_lifecycle.rs` + `data_lifecycle.rs` orchestration, `readiness.rs` checks lifecycle | If new route skips `authorize_request`, suspension ignored | `tenant_lifecycle_integration.rs` |
| Purge before retention | `data_lifecycle.rs` retention policy, `retention_worker.rs`  only after `lifecycle_worker.rs` | Manual DB delete could bypass | `retention_worker` |

## 11. Operator / Privilege Abuse

| Threat | Control | Exposure | Evidence |
|---|---|---|---|
| Owner-only action by operator | `require_role` `Role::Owner` for key admin/audit verify | If new mutating route requires `readonly`, privilege escalation | `api.rs` `require_role` + `AuditTrail` every denial |
| Audit tamper | `AuditTrail` hash-chained when DB attached, `audit_attestation.rs` HMAC-SHA256 | If DB off, trail is memory-only (no chain) | `db_integration` audit-chain tests |

> **Untested / External:** live Stripe/Paddle, live Vault/KMS/HSM, production deployment, funded trading, staking `STAKING_E2E=1`, external audit — all `EXTERNAL_REQUIRED`/`NOT_EXECUTED` (see `docs/FINAL-BUYER-GAP-LEDGER.md`).

*Verification:* `cargo test --workspace -- --test-threads=1` (hermetic test suites exist in tree — static inventory <!-- stat:test_attrs_plain -->2061<!-- /stat --> `#[test]` attributes per docs/STATS.md; no run log ships), `bash scripts/verify-delivery.sh`, `bash scripts/final-release-check.sh` (secret scan).
