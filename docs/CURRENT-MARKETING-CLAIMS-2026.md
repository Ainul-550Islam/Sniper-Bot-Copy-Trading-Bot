# CURRENT MARKETING CLAIMS (2026-10-01)

EVIDENCE-LEVEL: CODE

The current claim vocabulary for marketing-facing text, dated
2026-10-01, reconciled with the machine gate
(`scripts/verify-marketing-claims.sh`) and the evidence levels that
actually exist in this repository. This is the operational answer to
"what may we say" — `docs/MARKETING-CLAIMS.md` remains the canonical
registry this page summarizes.

## The rule (machine-enforced, not a guideline)

A marketing-facing file may not contain a banned phrase unless the very
line carries an inline evidence tag
(`<!-- evidence:LEVEL path/to/evidence -->`) naming an evidence file
that exists, declares `EVIDENCE-LEVEL: <LEVEL>` at least as strong as
the phrase requires, and no evidence above INTEGRATION_TEST exists in
this repository. Therefore **none of the banned phrases can ship at
all today.** The gate is regression-tested by
`tests/release/marketing_claims.sh`, which also plants a violation to
prove the gate can fail.

## Evidence levels that exist (2026-10-01)

CODE, UNIT_TEST, INTEGRATION_TEST — and nothing higher.

* No LIVE_TEST (no provider, exchange, or signing backend has been
  contacted from this repository).
* No FUNDED_TEST (no real funds have ever moved).
* No EXTERNAL_AUDIT (no third-party review commissioned).

## SAFE claims (allowed today, with their evidence)

| Claim | Level | Evidence file |
| --- | --- | --- |
| Multi-tenant SaaS control plane with per-organization data isolation enforced in SQL (forensic sweep: 371 statements classified, 0 missing-tenant-enforcement) | INTEGRATION_TEST | docs/CUSTOMER-SaaS-STATUS-2026.md + docs/FORENSIC-SQL-RESEARCH-2026.md |
| Custody boundary with policy → resolution → signing, org-scoped durable profiles/signers, full audit trail | INTEGRATION_TEST | docs/CUSTODY-STATUS-2026.md |
| Real Vault transit (ed25519) and AWS KMS (SigV4, EdDSA) signing implementations, unit-tested wire protocols | UNIT_TEST | docs/CUSTODY-STATUS-2026.md |
| Fail-closed design: unknown/unconfigured providers refuse naming the exact missing dependency | CODE | docs/CUSTODY-STATUS-2026.md |
| Authoritative billing state machine with deterministic transitions and provider-event idempotency (Stripe + Paddle) | INTEGRATION_TEST | docs/BILLING-STATUS-2026.md |
| Customer trading API (55 documented endpoints) with tenant-scoped orders, positions, executions, reports, module controls, telegram binding | INTEGRATION_TEST | docs/CUSTOMER-SaaS-STATUS-2026.md |
| Polymarket CLOB V2 order domain + explicit V3 position orders with async lifecycle and reconciliation | UNIT_TEST | docs/POLYMARKET-COMPATIBILITY-2026.md |
| Copy-trading engine with leader mirroring, exits, crash recovery, event dedup + ordering | UNIT_TEST | docs/COPY-TRADING-ENGINE.md |
| Native Solana staking/token-fee program, unit-tested, with guarded identity + deploy tooling (pre-deployment) | UNIT_TEST | docs/STAKING-PROGRAM-ID-VALIDATION.md |
| PostgreSQL 17, forward-only migrations (35 to date), tenant-composite upsert arbiters verified in code | INTEGRATION_TEST | docs/CURRENT-STATE.md |
| Byte-parity buyer release tree with drift-proving regression test, SBOM, license report, checksums | CODE (mechanical) | docs/BUYER-PACKAGE-CONTENTS-2026.md |

## UNSUPPORTED claims (banned; the gate rejects them)

| Phrase | Why rejected |
| --- | --- |
| "guaranteed" (any outcome) | No performance or outcome guarantee is proven at any level. |
| "profitable" / profit claims | Market-dependent; no evidence can justify. |
| "risk-free" | False for any trading system. |
| "under 1 second guaranteed" | Latency never measured in a LIVE_TEST. |
| "fully audited" | No EXTERNAL_AUDIT exists. |
| "fully isolated" | Isolation is INTEGRATION_TEST-proven; "fully" implies an external audit that does not exist. |
| "all Raydium" pools/styles | No LIVE_TEST coverage of every pool type. |
| "latest Polymarket V3" | V3 is implemented and tested; "latest" implies live-market verification. |
| "Vault/KMS/HSM included" (production-ready bundle) | Vault/KMS are unit-tested wire protocols, no LIVE_TEST round-trip; HSM is fail-closed unimplemented. |
| "self-service billing" | Live checkout never exercised in LIVE_TEST. |
| "institutional SLA" | No SLA document, audit, or operations contract. |
| "mainnet proven" | No FUNDED_TEST evidence. |
| "battle-tested" / "zero-defect" / "SOC2" / "zero-loss" | Same class: no live, audit, or funded evidence exists. |

## The one-line version for sales

> "A multi-tenant trading platform (sniper, copy, Polymarket V2+V3,
> staking program, Telegram, SaaS control plane with billing and
> custody) that is integration-tested against PostgreSQL 17, ships a
> byte-parity buyer release with SBOM and a forensic tenant-SQL gate,
> and has never been run live, never traded funds, and never been
> externally audited — all three stated plainly in the docs."

Every clause in that sentence is machine-verifiable from this
repository. Nothing stronger is true.

## Re-verification

```bash
scripts/verify-marketing-claims.sh     # must pass (exit 0)
tests/release/marketing_claims.sh      # regression gate incl. planted violation
```
