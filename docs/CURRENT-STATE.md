# CURRENT STATE (2026-10-01)

EVIDENCE-LEVEL: CODE

The current, dated statement of what this repository IS and MEASURES,
as of 2026-10-01. Counts in the table below are produced by the same
measurement the manifest uses (`scripts/update-release-manifest.sh`)
and are refreshed by `scripts/update-current-audit.sh`; they are facts,
not targets. This document supersedes nothing — it is the newest
member of the dated status set (BILLING-STATUS-2026,
CUSTODY-STATUS-2026, CUSTOMER-SaaS-STATUS-2026,
POLYMARKET-COMPATIBILITY-2026, BUYER-HANDOVER-STATUS-2026,
CURRENT-BUYER-STATE).

## Current counts (canonical product tree, 2026-10-01)

| Measure | Count |
| --- | --- |
| Rust source files (crates/) | 573|
| Docs files (docs/) | 132|
| TypeScript/TSX files (apps/control-plane) | 29|
| Database migrations (forward-only) | 36 (high-water `0036`)|
| Version | 0.1.0 (VERSION = Cargo.toml = manifest) |
| Workspace members | crates/core (bot-core), solana-kit, module-sniper, module-copy, module-polymarket, module-telegram, server (sniper-suite), saas-sdk |
| Standalone program | programs/staking-suite (native Solana program, pre-deployment placeholder id) |

(Numbers are refreshed mechanically by `scripts/update-current-audit.sh`;
if a count above disagrees with a fresh run of that script, the SCRIPT
is right and this page is stale.)

## What the product is (one paragraph)

A multi-tenant trading platform: sniper and copy-trading engines, a
Polymarket V2+V3 client with an explicit async order lifecycle, a
native Solana staking/token-fee program (undeployed), Telegram alerts
and commands, a per-tenant custody boundary (policy → resolution →
signing) with real Vault and AWS KMS integrations, a deterministic
billing state machine with Stripe/Paddle webhook idempotency, a
customer SaaS API + control-plane UI, and release tooling that
measures itself (manifest refresh, buyer parity with drift-proving
regression, marketing-claim rejection, forensic SQL sweep).

## What is real (measured)

* **Tenant isolation**: the 0023–0034 migration program (tenant
  columns, deterministic backfill, tenant-composite arbiters,
  tenant-leading indexes) verified in code by the forensic SQL sweep —
  371 statements classified, zero missing-tenant-enforcement, gated by
  a regression test that also proves the scanner catches planted
  violations.
* **Custody durability**: profile creation, activation, revocation,
  and signer lifecycle persist to PostgreSQL org-scoped — proven by
  three integration suites (missing profile 404-no-oracle,
  cross-tenant, durable resolution), which found and fixed three real
  defects on the way (0020 CHECK omission, missing relational org-row
  writer, dead axum `{id}` routes).
* **Billing**: provider events idempotent across replays; checkout
  URLs persisted (0022); entitlement enforcement integration-tested.
* **Customer API/UI**: 55 documented API endpoints behind one
  authorization chain; the control plane renders honest
  loading/empty/error/denied/disabled/stale states everywhere
  (including the ExecutionStatus lifecycle view and ModuleActionButton
  control surface added 2026-10-01).

## What is NOT real (the honest list, unchanged in kind)

1. No LIVE_TEST: Vault, KMS, Stripe, Paddle, Telegram, Polymarket —
   never exercised against real backends from this repository.
2. No FUNDED_TEST: nothing has traded real funds.
3. No EXTERNAL_AUDIT / no SOC2.
4. HSM custody unimplemented (fail-closed refusal, PKCS#11 named).
5. Telegram per-tenant routing: binding API exists; the forwarder
   still routes to the deployment alert chat.
6. The staking program id is the documented pre-deployment placeholder
   (`scripts/staking-identity.sh verify`).
7. Single-operator deployment model (the HA plane is per-deployment,
   not per-tenant).

## How to verify every statement on this page

```bash
scripts/update-current-audit.sh              # refresh the count table above
scripts/update-release-manifest.sh           # recount into the manifest
scripts/forensic-sql-scan.sh                 # tenant SQL sweep (0 class-4)
tests/forensics/sql-pattern-regression.sh    # the gate, with drift proof
scripts/staking-identity.sh verify           # program id consistency
scripts/verify-release-integrity.sh          # parity + contamination + version
scripts/verify-marketing-claims.sh           # claims vs evidence
tests/release/buyer_source_parity.sh         # parity regression
tests/release/buyer_parity.sh                # parity + package shape
tests/release/manifest_current.sh            # manifest matches reality
tests/release/marketing_claims.sh            # claim rejection regression
```
