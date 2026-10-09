# CURRENT EVIDENCE MATRIX (2026-10-01)

EVIDENCE-LEVEL: INTEGRATION_TEST

Every functional area of the product × the strongest evidence that
exists for it × where that evidence lives × how to re-run it. This is
the product-wide view; `docs/SECURITY-EVIDENCE-MATRIX-2026.md` is the
security-only cut. Levels: CODE < UNIT_TEST < INTEGRATION_TEST <
LIVE_TEST < FUNDED_TEST < EXTERNAL_AUDIT. **The strongest level that
exists anywhere in this repository is INTEGRATION_TEST.**

## The matrix

| Area | Strongest evidence | Evidence location | How to re-run |
| --- | --- | --- | --- |
| Sniper engine (detect/entry/exit) | UNIT_TEST | crates/module-sniper (72 test fns) | `cargo test -p module-sniper` |
| Copy engine (dedup/ordering/mirror/exit/recovery) | UNIT_TEST + INTEGRATION_TEST | crates/module-copy (60) + server copy suites | `cargo test -p module-copy` / server copy tests |
| Polymarket V2 + V3 + async lifecycle | UNIT_TEST + INTEGRATION_TEST | crates/module-polymarket (136) + poly PG suites | `cargo test -p module-polymarket` / server poly tests |
| Telegram alerts/commands | UNIT_TEST | crates/module-telegram (21) | `cargo test -p module-telegram` |
| Staking/token-fee program | UNIT_TEST | programs/staking-suite (73) | `cargo test -p staking-suite` |
| Tenant data plane (orders/positions/executions/reports) | INTEGRATION_TEST | server tenant suites (PostgreSQL 17) | `cargo test -p sniper-suite --test tenant_* -- --test-threads=1` |
| Custody (policy/resolution/signing, durable lifecycle) | INTEGRATION_TEST | 3 custody rotation suites (2026-10-01) | `cargo test -p sniper-suite --test custody_rotation_* -- --test-threads=1` |
| Vault transit signing | UNIT_TEST | custody vault wire-format tests | `cargo test -p sniper-suite custody::vault` |
| AWS KMS (SigV4 + EdDSA) | UNIT_TEST | custody KMS fixture tests | `cargo test -p sniper-suite custody::kms` |
| HSM | UNIT_TEST (refusal) | custody hsm fail-closed test | `cargo test -p sniper-suite custody::hsm` |
| Billing state machine + webhooks | INTEGRATION_TEST | billing suites (PostgreSQL) | `cargo test -p sniper-suite billing -- --test-threads=1` |
| Authorization chain / sessions / API keys | INTEGRATION_TEST | authorization-chain + saas suites | server saas suites |
| Tenant runtime registry / config engine | INTEGRATION_TEST | registry integration suites (PostgreSQL) | `cargo test -p sniper-suite --test tenant_*_integration -- --test-threads=1` |
| Audit hash chain (tamper evidence, concurrency) | INTEGRATION_TEST | db_integration `audit_chain_survives_concurrent_appends` | core db integration suite |
| Accounting ledger / reconciliation | INTEGRATION_TEST | accounting + recon suites | server/core accounting suites |
| HA plane (leases, fencing, cursors, gaps) | INTEGRATION_TEST | ha suites incl. PG claim races | ha suites |
| Customer control-plane UI | Static typecheck (tsc --noEmit) | apps/control-plane | `npx tsc --noEmit` in apps/control-plane |
| saas-sdk | UNIT_TEST | crates/saas-sdk | `cargo test -p saas-sdk` |
| Release integrity / parity / SBOM | CODE (mechanical) | scripts/* + tests/release/* | `scripts/verify-release-integrity.sh`, `tests/release/buyer_parity.sh` |
| Tenant SQL enforcement | CODE (mechanical sweep) + regression gate | scripts/forensic-sql-scan.sh, tests/forensics/ | `tests/forensics/sql-pattern-regression.sh` |
| Marketing claim constraint | CODE (mechanical) | scripts/verify-marketing-claims.sh | `tests/release/marketing_claims.sh` |
| Business matrix completeness | CODE (mechanical) | scripts/generate-business-matrix.sh | `tests/business/business-matrix-completeness.sh` |
| Staking program identity consistency | CODE (mechanical) | scripts/staking-identity.sh | `scripts/staking-identity.sh verify` |

## What NO row claims

* LIVE_TEST — no row has it; no live backend was ever contacted.
* FUNDED_TEST — nothing has traded real funds.
* EXTERNAL_AUDIT — no external review exists.

## Defect-evidence loop (the matrix is not decorative)

The verification program that produced this matrix has found and fixed
real defects — which is the only proof a matrix like this works:

| Found by | Defect | Fix shipped as |
| --- | --- | --- |
| custody rotation integration suite | durable profile INSERT always failed silently (CHECK bug + swallowed error) | migration 0035 + warn-logged writes |
| custody rotation integration suite | custody FK violated (no relational org-row writer) | ensure_organization_row() |
| custody rotation integration suite | rotation status/activate/revoke routes dead (axum path syntax) | `:id` route fix |
| forensic SQL sweep | cancel history attributed to deployment org | org-subselect INSERT |
| forensic SQL sweep | lifecycle durable phase update org-in-Rust-only | org predicate in WHERE |

## Dating

Every entry was re-measured 2026-10-01. After any code change, re-run
the right-hand column — this page does not update itself; the scripts
do.
