# CURRENT BUYER FACTSHEET (2026-10-06)

EVIDENCE-LEVEL: CODE

One page for a buyer: what you are buying, what it measures, what is
proven, what is not, and how to check every line yourself. Dated
2026-10-06. Every count is machine-measured; every limitation is
stated plainly.

## The product (8 crates + 1 program + 1 web app)

| Component | What it is | Evidence level |
| --- | --- | --- |
| bot-core (crates/core) | Domain core: orders, positions, executions, transactions, ledger, audit hash chain, tenant model, 43 forward-only migrations | INTEGRATION_TEST |
| solana-kit | Solana/RPC kit used by the modules | UNIT_TEST |
| module-sniper | Sniper engine (deterministic detect/entry/exit) | UNIT_TEST |
| module-copy | Copy-trading engine (event dedup, ordering, mirror/exit/recovery) | UNIT_TEST + INTEGRATION_TEST |
| module-polymarket | Polymarket V2 + V3 position orders, async commit/resolution/reconciliation | UNIT_TEST + INTEGRATION_TEST |
| module-telegram | Telegram alerts + commands | UNIT_TEST |
| sniper-suite (crates/server) | The multi-tenant server: SaaS control plane, billing, custody, tenant data plane, customer API | INTEGRATION_TEST |
| saas-sdk | SDK for the customer API | UNIT_TEST |
| programs/staking-suite | Native Solana staking/token-fee program (PRE-DEPLOYMENT: placeholder id, never on-chain) | UNIT_TEST |
| apps/control-plane | Customer web UI (Next.js): honest empty/error/denied states everywhere | Static typecheck |

## Measured facts (2026-10-06)

* 617 Rust files in `crates/` (+ the staking program), 96 TS/TSX files
  in the control plane, 149 documents in `docs/`.
* 43 forward-only migrations (high-water `0043`), no down-migrations by
  design.
* 55 documented API endpoints; one authorization chain in front of all
  tenant surfaces.
* Buyer source parity: byte-for-byte between `buyer-release/source/`
  and the canonical tree, proven by a regression test that also plants
  drift to prove the checker can fail.
* Forensic SQL sweep: 371 statements classified, ZERO
  missing-tenant-enforcement findings; the gate re-runs on every
  release test pass.
* Full SBOM (`sbom/`), license report, checksums, and manifests ship
  inside `buyer-release/`.

## What is PROVEN (highest evidence per area)

* Tenant isolation on the trading data plane, custody, and lifecycle —
  integration-tested against PostgreSQL 17, including cross-tenant
  404-no-oracle behavior.
* Billing webhook idempotency and HMAC signature verification
  (fixture-tested).
* Vault transit (ed25519) and AWS KMS (SigV4 + EdDSA) custody signing —
  real wire-protocol implementations, unit-tested.
* Audit hash chain tamper-evidence, including concurrent-append safety.

## What is NOT proven (read before buying)

1. **Nothing has run live.** No live exchange, no live Vault/KMS, no
   live payment provider, no live Telegram, no live Solana cluster.
   Every integration is real code + wire-format tests.
2. **Nothing has traded real funds.**
3. **No external security audit / no SOC2.**
4. **HSM custody is not implemented** — it refuses, naming PKCS#11.
5. **Telegram forwarder is deployment-level** (per-tenant binding API
   exists; messages route to the deployment alert chat).
6. **The staking program is undeployed**; its id is the documented
   placeholder (`scripts/staking-identity.sh verify` says so, always).
7. **Marketing claims are machine-constrained**: the release gate
   rejects the banned outcome-guarantee phrase class (the exact list
   lives in docs/MARKETING-CLAIMS.md) unless inline evidence tags
   point at evidence files of sufficient level — none exist above
   INTEGRATION_TEST, so none of those phrases can ship.

## Defects the internal verification FOUND AND FIXED (evidence it works)

| Defect | Fixed by |
| --- | --- |
| Custody profile INSERTs always failed silently (CHECK omitted `pending`) | migration 0035 + integration test |
| Custody FK had no production writer for relational org rows | `ensure_organization_row()` + test |
| Custody rotation status/activate/revoke routes were dead (axum `{id}` vs `:id`) | route fix + test |
| Order-cancel history attributed to deployment org | org-subselect INSERT + forensic sweep |
| Tenant-lifecycle durable phase update enforced org only in Rust | org predicate in durable WHERE |
| Six swallowed durable-write results (`let _ =`) in custody/lifecycle | warn!-logged Results |

## Verify everything yourself (30 minutes)

```bash
scripts/verify-release-integrity.sh        # parity + secrets scan + version
tests/release/buyer_parity.sh              # parity regression incl. drift proof
tests/release/manifest_current.sh          # manifest counts match the tree
tests/release/marketing_claims.sh          # claim gate regression
scripts/forensic-sql-scan.sh               # tenant SQL sweep
tests/forensics/sql-pattern-regression.sh  # sweep gate incl. planted violation
scripts/staking-identity.sh verify         # staking id status (placeholder, honestly)
cargo test -p sniper-suite --lib -- --test-threads=1      # fast suites
# PostgreSQL-backed (needs POSTGRES_URL):
cargo test -p sniper-suite --test custody_rotation_profile_resolution -- --test-threads=1
```

## The papers

* `docs/BUYER-PACKAGE-CONTENTS-2026.md` — what is in the package
* `docs/CURRENT-STATE.md` — measured current state
* `docs/SECURITY-AUDIT-STATUS-2026.md` + `docs/SECURITY-EVIDENCE-MATRIX-2026.md`
* `docs/CURRENT-MARKETING-CLAIMS-2026.md` — what may/may not be said
* `docs/CURRENT-COMMERCIAL-GAP-REGISTER-2026.md` — every gap, priced honestly
* `docs/BUSINESS-MATRIX-2026.md` — 7 business lines × completeness × claims
* `docs/IP-HANDOVER-CHECKLIST.md` — the IP transfer checklist
* `docs/FINAL-16-SECTION-RESULT-2026.md` — the full verification result
