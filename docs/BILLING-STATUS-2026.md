# Billing Status 2026 (2026-09-30)

EVIDENCE-LEVEL: INTEGRATION_TEST

The authoritative commercial-state capabilities and their limits, as of
today.

## What exists (and its evidence)

| Capability | Where | Evidence |
| --- | --- | --- |
| Authoritative billing state read model | `crates/server/src/saas/billing_view.rs`, `commercial_state.rs` | INTEGRATION_TEST (PostgreSQL-backed `billing_authoritative_state.rs`) |
| Deterministic subscription/payment/invoice transitions | `bot-core` billing state machine + `saas/billing.rs` | INTEGRATION_TEST |
| Provider event persistence + idempotency (webhook replays collapse) | `saas/billing_webhook.rs`, `payment_webhooks.rs` | INTEGRATION_TEST (`billing_integration.rs`, webhook replay tests) |
| Verified event → transactionally applied payment state | payment application path in `saas/billing.rs` | INTEGRATION_TEST |
| Provider adapters (Stripe, Paddle) with a shared live-provider contract | `crates/server/src/billing/{stripe_adapter,paddle_adapter,provider_registry,live_provider_contract}.rs` | UNIT_TEST (contract + adapter unit tests; `live_billing_contract.rs`) |
| Usage metering + entitlement enforcement | `saas/usage_limits.rs`, `bot-core::billing` entitlements | INTEGRATION_TEST (entitlement gating across the customer API chain) |
| Plan catalogue / invoices / checkout endpoints | `saas/checkout.rs`, `invoices.rs` | INTEGRATION_TEST |

## Honest limitations

| Limitation | Detail |
| --- | --- |
| No LIVE payment-provider round-trip | The Stripe/Paddle adapters implement the real API surfaces, but no LIVE_TEST with real provider credentials has been performed. Customer self-checkout is therefore NOT a claimable capability yet. |
| No FUNDED_TEST | No real money movement has ever been exercised. |
| Sandbox/fixture contracts exist and are clearly labeled | `live_provider_fixture.rs` is the NON-LIVE fixture contract — it is test infrastructure, never a production success signal. |
| Dunning/collections policy is code-level | Past-due handling follows `bot_core::tenant::policy`; no collections process has run against real subscribers. |

## Authoritative sources

* Commercial truth: the billing state machine + its PG-backed store.
* NEVER treat provider dashboards or the webhook fixture as truth —
  events are applied transactionally and idempotently to the
  authoritative state; drift between provider and local state is
  reconciled by `billing_reconciliation.rs`.

## Claimable vs not

* ✅ "Deterministic subscription/payment/invoice state machine with
  provider-event idempotency (integration-tested)".
* ❌ Customer self-checkout claims (require LIVE_TEST).
* ❌ "Payments proven in production" (requires LIVE_TEST/FUNDED_TEST).
