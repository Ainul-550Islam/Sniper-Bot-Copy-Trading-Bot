# CURRENT BUYER STATE (2026-09-30)

EVIDENCE-LEVEL: CODE

The single, current, honest statement of what the buyer package is,
measured today. Counts below are produced by the same measurement the
manifest uses (`scripts/update-release-manifest.sh`); they are facts,
not targets.

## Current counts (canonical product tree, 2026-09-30)

| Measure | Count |
| --- | --- |
| Product files (all) | 828 (manifest `verification.manifest.product_files`) |
| Rust source files (crates/) | 564 (manifest `rust_files`; programs/staking-suite counted separately as a component)|
| Docs files (`docs/`) | 122 (includes this file)|
| TypeScript/TSX files (`apps/control-plane`) | 30|
| Database migrations (forward-only) | 35 (high-water `0035`)|
| Version | 0.1.0 (VERSION = Cargo.toml = manifest) |
| Workspace members | crates/core (bot-core), solana-kit, module-sniper, module-copy, module-polymarket, module-telegram, server (sniper-suite), saas-sdk |
| Standalone program | programs/staking-suite (native Solana program) |

## What is in the package

* **Trading core**: sniper, copy-trading (mirror/exit/recovery),
  Polymarket V2 + explicit V3 position orders with the full async
  lifecycle (accepted → hash absent/present → trade IDs → resolution →
  reconciliation; never fake-confirmed; no duplicate on retry; tenant
  context attached end-to-end).
* **Custody**: the tenant-scoped custody boundary (policy → resolution
  → signing, fully audited), with REAL Vault transit (ed25519) and REAL
  AWS KMS (SigV4, EdDSA) integrations — unit-tested, fail-closed, not
  live-proven — and an explicitly unimplemented HSM provider that
  refuses with its exact dependency.
* **Billing**: authoritative deterministic state machine with
  provider-event idempotency (Stripe/Paddle adapters), usage limits,
  entitlement enforcement — integration-tested against PostgreSQL.
* **Customer SaaS**: the full customer API (orders, positions,
  executions, reports, recovery, copy, polymarket, module
  controls/status, telegram binding) behind one authorization chain,
  and the customer UI (`apps/control-plane`) with honest empty/error/
  suspended/entitlement/disabled/stale/unavailable states everywhere.
* **Release tooling**: parity compare, buyer rebuild, manifest refresh,
  integrity verification, marketing-claim rejection, parity regression
  test.
* **Docs**: 108 documents including the 2026 status set
  (MARKETING-CLAIMS, LIVE-EVIDENCE-MATRIX, POLYMARKET-COMPATIBILITY,
  CUSTODY-STATUS, BILLING-STATUS, CUSTOMER-SaaS-STATUS,
  BUYER-HANDOVER-STATUS — this file, and CURRENT-BUYER-STATE).

## Current limitations (the honest list)

1. **No LIVE_TEST anywhere**: Vault, KMS, and payment providers have
   never been exercised against real backends. The integrations are
   real code with unit-tested wire protocols — not live-proven.
2. **No FUNDED_TEST**: nothing has traded real funds.
3. **No external security audit** has been commissioned.
4. **HSM custody is not implemented** (fail-closed refusal naming the
   PKCS#11 dependency).
5. **Telegram per-tenant routing**: the tenant binding API exists; the
   deployment-level forwarder still routes to the deployment alert chat.
6. **Tenant module controls and the telegram binding store are
   process-local** (same contract as the custody store) — the
   DB-backed path is a migration, not a redesign.
7. **Buyer source parity**: the canonical tree moved on 2026-09-30;
   run `scripts/rebuild-buyer-release.sh` then
   `scripts/verify-release-integrity.sh` to re-mirror and verify.

## How to verify every statement in this file

```bash
scripts/update-release-manifest.sh          # recount and see the same numbers
scripts/compare-canonical-to-buyer-source.sh  # source parity (after rebuild)
scripts/verify-release-integrity.sh          # parity + manifest + contamination + version
scripts/verify-marketing-claims.sh           # no claim exceeds its evidence
tests/release/buyer_source_parity.sh         # parity regression incl. drift detection
```
