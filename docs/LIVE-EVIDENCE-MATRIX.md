# Live Evidence Matrix (2026-09-30)

EVIDENCE-LEVEL: CODE

This matrix maps every commercial capability to the strongest evidence
that actually exists for it today. Evidence levels:

| Level | Meaning |
| --- | --- |
| CODE | The implementation exists and compiles; behavior is reviewable in source. |
| UNIT_TEST | Automated tests exercise the code in-process (including wire-protocol tests against documented vectors / local stubs). |
| INTEGRATION_TEST | Automated tests exercise the code against real infrastructure components (PostgreSQL, the full HTTP API surface) in a controlled environment. |
| LIVE_TEST | The system has been exercised against the real external service with real (non-funded) credentials. |
| FUNDED_TEST | The system has been exercised with real funds on mainnet. |
| EXTERNAL_AUDIT | An independent third party has reviewed and attested the claim. |

## Matrix

| Capability | Strongest evidence | Where the evidence lives |
| --- | --- | --- |
| Solana copy-trading (mirror, exits, recovery) | UNIT_TEST | `crates/module-copy` test suites; `crates/server/tests/tenant_background_integration.rs` |
| Sniper strategy execution (Raydium routes that are implemented) | UNIT_TEST | `crates/module-sniper` test suites |
| Polymarket V2 order domain (EIP-712, CTF exchange) | UNIT_TEST | `crates/module-polymarket/src/eip712.rs` tests, `orders.rs` tests |
| Polymarket V3 position orders (negative-risk, `position_id` semantics) | UNIT_TEST | `position_orders.rs`, `exchange_v3.rs`, `tenant_executor.rs` tests |
| Polymarket async order lifecycle (accepted → hash absent/present → trade IDs → resolution) | UNIT_TEST | `async_commit.rs`, `trade_resolution.rs`, `reconcile_async.rs` tests |
| Polymarket live market data / live order placement | CODE only — no LIVE_TEST performed | implementation in `clob.rs`, `gamma.rs`, `ws.rs` |
| Custody boundary (policy → resolution → signing, audited) | INTEGRATION_TEST | `crates/server/tests/custody_boundary.rs`, `crates/server/tests/provider_contracts.rs` |
| Custody tenant binding (tenant A cannot use tenant B signer) | INTEGRATION_TEST | `crates/server/tests/custody_boundary.rs` |
| Vault transit signing (ed25519 via Vault REST) | UNIT_TEST | `crates/server/src/custody/vault/*` — wire protocol, signature envelope parsing, fail-closed paths; SigV4-style strictness; NO live Vault round-trip |
| AWS KMS Ed25519 signing (SigV4-signed KMS API) | UNIT_TEST | `crates/server/src/custody/kms/*` — SigV4 key derivation verified against the AWS-documented test vector; NO live KMS round-trip |
| HSM custody | NONE (fail-closed refusal with exact dependency) | `bot_core::custody::HsmCustodyProvider` refuses; registry names the PKCS#11 dependency |
| Authoritative billing state (transitions, invoices, payment application) | INTEGRATION_TEST | `crates/server/tests/billing_authoritative_state.rs`, `billing_integration.rs` (PostgreSQL-backed) |
| Webhook idempotency (Stripe/Paddle event dedup) | INTEGRATION_TEST | `crates/server/tests/billing_integration.rs`, `live_billing_contract.rs` |
| Live payment provider charges | CODE only — adapters exist, no LIVE_TEST | `crates/server/src/billing/{stripe_adapter,paddle_adapter}.rs` |
| Customer API (orders/positions/executions/reports/controls) | INTEGRATION_TEST | `crates/server/tests/postgres_saas_integration.rs`, `customer_trading_api.rs`-equivalent plane suites |
| Customer UI (trading dashboard, module controls, telegram binding) | CODE (+ typecheck/build gates) | `apps/control-plane/src/app/trading/**`, `src/components/trading/**` |
| Tenant isolation at the repository layer | INTEGRATION_TEST | tenant-scoped predicates across the trading data plane suites |
| PostgreSQL schema + forward-only migrations (34) | INTEGRATION_TEST | `crates/core/migrations/0001…0034` + PG-backed suites |
| Solana staking suite (native program) | UNIT_TEST | `programs/staking-suite` tests, `crates/server/tests/staking_contract.rs` |
| Mainnet trading with funds | NONE | no FUNDED_TEST evidence exists |
| External security audit | NONE | no audit has been commissioned |

## Consequences for marketing

Every claim must stay at or below its row's evidence level. See
`docs/CURRENT-MARKETING-CLAIMS-2026.md` for the enforceable registry and
`scripts/verify-marketing-claims.sh` for the checker.
