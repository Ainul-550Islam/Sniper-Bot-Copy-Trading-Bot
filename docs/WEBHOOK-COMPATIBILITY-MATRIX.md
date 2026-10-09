# Webhook Compatibility Matrix — sniper-suite 0.1.0

> Distinguishes fixture-tested vs live-provider-tested. No live payment claimed.

## Inbound (Provider → sniper-suite)

| Provider | Endpoint | Signature Method | Replay / Idempotency | Event Types | Retry Behavior | Status | Test |
|---|---|---|---|---|---|---|---|
| **Stripe (test)** | `POST /api/saas/billing_webhook` | HMAC-SHA256 `Stripe-Signature` (t=`timestamp`,v1=`hmac`) via `saas/billing_webhook.rs` | `provider_events.rs` normalized + idempotency key (provider `event_id`), secret stripping | `invoice.payment_succeeded`, `invoice.payment_failed`, `customer.subscription.updated`, `checkout.session.completed` (normalized to `ProviderEventKind`) | Provider retries with same `event_id` → idempotent (200, no double-apply) | **FIXTURE-TESTED** (HMAC verify + idempotency tests exist, no live Stripe) | `billing_webhook` 3, `provider_events` 7, `billing_integration` fixture |
| **Paddle (test)** | Same endpoint (provider-neutral boundary `saas/provider.rs`) | HMAC-SHA256 `Paddle-Signature` (similar) | Same idempotency | `subscription_created`, `subscription_updated`, `transaction_completed` | Same | **FIXTURE-TESTED** | `provider.rs` boundary tests exist |
| **Generic** | Same | `provider_events.rs` `ProviderNormalizedEvent` + `hmac` verify | Same | `ProviderEventKind` enum | — | — | — |

**Live vs fixture:**
- **Fixture:** `crates/core/tests/saas_control_plane.rs` + `crates/server/tests/billing_integration.rs` use fixture events, HMAC with test secret (`webhook_secret` env `test_...`), `reconciliation.rs` never invents success.
- **Live:** `EXTERNAL_REQUIRED` / `NOT_EXECUTED` — requires `STRIPE_API_KEY`, `STRIPE_WEBHOOK_SECRET` live, funded account, real webhook delivery (see `docs/FINAL-BUYER-GAP-LEDGER.md` GAP-001). Not claimed.

**Security:**
- Signature **required** — missing/invalid → 401, audited via `AuditTrail::denied`.
- Secret never in logs/URLs/JSON (`is_secret_like` redaction, `saas-sdk` no secrets).
- Replay with same `event_id` → 200 but no state change (idempotency).

## Outbound (sniper-suite → provider / operator)

| Direction | Endpoint | Signature | Events | Retry | Status |
|---|---|---|---|---|---|
| **Outbound billing** | None (pull only) | — | — | — | Not implemented — billing is inbound webhook + `checkout.rs` redirect |
| **Outbound operator** | Telegram `POST https://api.telegram.org/bot<token>/sendMessage` (via `module-telegram`) | `TELEGRAM_BOT_TOKEN` env via `seed_secret_env` | `AppEvent` lifecycle, errors | `backon` retry | **PARTIAL** (needs `TELEGRAM_BOT_TOKEN`) |
| **Outbound custody** | Vault/KMS/HSM signing request (via `core/custody/resolve.rs`) | Indirect ref only | Signing | Fail-closed, no retry to local | EXTERNAL_REQUIRED |

## Compatibility & Versioning

- **Provider API version:** Stripe `2024-06-20` (example, pinned in `provider_config.rs` ref), Paddle Billing v2 — provider-neutral `provider.rs` adapts.
- **Breaking changes:** If provider changes signature method, `billing_webhook.rs` must be updated + `CHANGELOG.md` + major bump (pre-1.0 minor).
- **Idempotency key:** `event_id` + `provider` composite, stored in `*_idempotency` table (Postgres) or memory.

> **Verification:** `grep -R "billing_webhook" crates --include="*.rs"`, `cargo test -p bot-core --test saas_control_plane`, `curl -X POST http://localhost:8080/api/saas/billing_webhook -H "Stripe-Signature: t=..." -d '{}'` (401 without valid HMAC).
