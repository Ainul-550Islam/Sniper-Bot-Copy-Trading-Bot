# Marketing Claims — safe and unsupported (2026-09-30)

EVIDENCE-LEVEL: CODE

This document is the registry of what may and may NOT be said about this
product. It is enforced by `scripts/verify-marketing-claims.sh`, which
rejects banned phrases unless the exact line cites an existing evidence
file whose declared evidence level justifies the claim.

Evidence levels (weak → strong):

```
CODE < UNIT_TEST < INTEGRATION_TEST < LIVE_TEST < FUNDED_TEST < EXTERNAL_AUDIT
```

No marketing claim may use a higher evidence level than actually exists.
The evidence files referenced below each carry an `EVIDENCE-LEVEL:` line
stating the strongest level they actually prove.

## SAFE CLAIMS

These claims are backed by the evidence file cited on the same line.

| Claim | Evidence level | Evidence |
| --- | --- | --- |
| Multi-tenant SaaS control plane with per-organization data isolation enforced at the repository layer | INTEGRATION_TEST | docs/CUSTOMER-SaaS-STATUS-2026.md |
| Solana copy-trading engine with leader mirroring, exits and crash recovery | UNIT_TEST | docs/CUSTOMER-SaaS-STATUS-2026.md |
| Polymarket CLOB support for the V2 order domain, with explicit V3 position-order support | UNIT_TEST | docs/POLYMARKET-COMPATIBILITY-2026.md |
| Custody boundary with provider registry, sign-request/response contracts, guard-ordered signing and full audit trail | UNIT_TEST | docs/CUSTODY-STATUS-2026.md |
| Authoritative billing state machine with deterministic transitions and provider event idempotency (Stripe + Paddle adapters) | INTEGRATION_TEST | docs/BILLING-STATUS-2026.md |
| Fail-closed design: unknown/unconfigured providers refuse with the exact missing dependency named | CODE | docs/CUSTODY-STATUS-2026.md |
| Customer trading API with tenant-scoped orders, positions, executions, reports and module controls | INTEGRATION_TEST | docs/CUSTOMER-SaaS-STATUS-2026.md |
| PostgreSQL 17 with forward-only migrations (34 to date) | INTEGRATION_TEST | docs/CUSTOMER-SaaS-STATUS-2026.md |

## UNSUPPORTED CLAIMS

These phrases must NOT appear in any marketing-facing text. They are
listed here so the checker — and every reviewer — knows them by name.

| Phrase | Why it is rejected |
| --- | --- |
| "guaranteed" | No performance or outcome guarantee is proven at any evidence level. |
| "profitable" / "profit" claims | Trading outcomes are market-dependent; no evidence can justify this. |
| "risk-free" | False for any trading system. |
| "under 1 second guaranteed" | Latency has never been measured in a LIVE_TEST. |
| "fully audited" | No EXTERNAL_AUDIT exists. Internal reviews are not external audits. |
| "fully isolated" | Isolation is enforced and tested at INTEGRATION_TEST level; the words "fully isolated" imply an external security audit that does not exist. |
| "all Raydium" pools/styles supported | No LIVE_TEST coverage of every Raydium pool type exists. |
| "latest Polymarket V3" | V3 is implemented and unit/integration tested; "latest" implies live-market verification that does not exist. |
| "Vault/KMS/HSM included" (as a production-ready bundle) | Vault transit and AWS KMS adapters are real code with unit-tested wire protocols, but no LIVE_TEST round-trip has been performed, and HSM is an explicit fail-closed unimplemented provider. |
| "self-service billing" | Live provider checkout (Stripe/Paddle) has not been exercised in a LIVE_TEST. |
| "institutional SLA" | No SLA document, no EXTERNAL_AUDIT, no operations contract exists. |
| "mainnet proven" | No FUNDED_TEST evidence exists. |

## How to add a claim

1. Verify the claim is true at a specific evidence level.
2. Ensure the evidence file exists and declares `EVIDENCE-LEVEL: <LEVEL>`.
3. Add the row to SAFE CLAIMS with the evidence file path.
4. If a scanned marketing file needs a phrase from the banned list, the
   exact line must carry `<!-- evidence:LEVEL docs/evidence-file.md -->`
   and the evidence file must declare at least that level.
5. Run `scripts/verify-marketing-claims.sh` — exit 0 or fix the claim.
