# CURRENT PROTOCOL COMPATIBILITY (2026-10-01)

EVIDENCE-LEVEL: UNIT_TEST

The current, dated summary of every external protocol this repository
speaks: what is implemented, at what fidelity, with what evidence, and
what is explicitly NOT supported. The detailed per-protocol documents
remain canonical; this page is the 2026-10-01 snapshot.

## Polymarket

| Surface | Status | Evidence |
| --- | --- | --- |
| CLOB REST (auth, order submit, cancel) | Implemented | `crates/module-polymarket/src/clob.rs`, `orders.rs` + unit tests |
| V2 order domain (CTF Exchange, domain version `"2"`, `Order` incl. `metadata`/`builder`) | Implemented | `eip712.rs` (keccak EIP-712 hashing) + unit tests |
| V3 position orders (negative-risk / merged positions) | Implemented explicitly (not inferred from V2); invalid position ids rejected client-side | `exchange_v3.rs`, `position_orders.rs` + unit tests |
| Async order lifecycle (accepted → hash absent/present → trade IDs → resolution → reconciliation) | Implemented; never fake-confirmed; no duplicate on retry | `async_commit.rs`, `trade_resolution.rs`, `reconcile_async.rs` + integration tests |
| `position_id` semantics | Carried through V2 and V3 paths without transformation | unit tests |
| Live market data feed | NOT implemented (no live connection claimed) | — |
| "Latest V3" claim | NOT made — V3 is implemented and tested; "latest" would imply live verification | marketing gate |

Detail: `docs/POLYMARKET-COMPATIBILITY-2026.md`.

## Solana

| Surface | Status | Evidence |
| --- | --- | --- |
| RPC client kit (blocks, transactions, programs) | Implemented | `crates/solana-kit` + unit tests |
| Native staking/token-fee program (instructions, processor, PDAs) | Implemented, PRE-DEPLOYMENT (placeholder id) | `programs/staking-suite` + 73 test fns; `scripts/staking-identity.sh` |
| On-chain deployment | NOT performed (guarded deploy path only) | staking identity script |

Detail: `docs/STAKING-PROGRAM-ID-VALIDATION.md`, `docs/STAKING.md`.

## Payment providers (inbound webhooks)

| Surface | Status | Evidence |
| --- | --- | --- |
| Stripe webhook (HMAC-SHA256 `Stripe-Signature`; invoice/subscription/checkout events) | Implemented, FIXTURE-TESTED (no live Stripe) | `saas/billing_webhook.rs`, `provider_events.rs` — 7 idempotency tests + HMAC vectors |
| Paddle webhook (HMAC-SHA256 `Paddle-Signature`; subscription/transaction events) | Implemented, FIXTURE-TESTED (no live Paddle) | provider-neutral boundary `saas/provider.rs` — 8 tests |
| Provider checkout (Stripe/Paddle redirect flow, durable checkout_url) | Implemented, fixture-tested (0022 persistence) | billing integration suites |
| Live provider round-trips | NOT performed | — |

Detail: `docs/WEBHOOK-COMPATIBILITY-MATRIX.md`.

## Custody signing backends

| Surface | Status | Evidence |
| --- | --- | --- |
| HashiCorp Vault transit (ed25519) | Real wire-protocol implementation, unit-tested | `custody/vault/signer.rs` |
| AWS KMS Sign (SigV4 request signing, EdDSA) | Real implementation, unit-tested against SigV4 fixtures | `custody/kms/` |
| AWS KMS EdDSA availability | Confirmed supported by AWS (2025-11-07 announcement) | external AWS docs (see CUSTODY-STATUS-2026) |
| HSM (PKCS#11) | NOT implemented — fail-closed refusal naming the dependency | `custody/hsm.rs` |
| Live Vault/KMS round-trips | NOT performed | — |

Detail: `docs/CUSTODY-STATUS-2026.md`.

## Telegram

| Surface | Status | Evidence |
| --- | --- | --- |
| Bot API (alerts, commands) | Implemented, unit-tested | `crates/module-telegram` (21 test fns) |
| Per-tenant binding API (bind/unbind chat id) | Implemented, integration-tested | `trading_data_plane/telegram.rs` |
| Per-tenant outbound routing | NOT implemented (forwarder routes to the deployment alert chat) | documented limitation |

## REST API (this product's own surface)

| Surface | Status | Evidence |
| --- | --- | --- |
| 55 documented endpoints (tenant data plane, SaaS control plane, health) | Implemented; one authorization chain | `docs/API.md`, `docs/API-COMPATIBILITY-MATRIX.md` |
| Pre-1.0 (0.1.0), no SemVer guarantee | Stated policy | release docs |
| Legacy paths | Documented per-endpoint | API compatibility matrix |

## The standing rule

Protocol compatibility claims are per-endpoint and evidence-tagged.
No blanket "compatible with X" claim is made for any protocol where
only wire-format fixtures exist; the marketing gate
(`scripts/verify-marketing-claims.sh`) rejects such phrasing on sight.

## Re-verification

```bash
cargo test -p module-polymarket
cargo test -p sniper-suite polymarket -- --test-threads=1
cargo test -p sniper-suite custody -- --test-threads=1
scripts/staking-identity.sh verify
scripts/verify-marketing-claims.sh
```
