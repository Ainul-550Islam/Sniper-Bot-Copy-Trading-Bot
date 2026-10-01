# SECURITY EVIDENCE MATRIX (2026-10-01)

EVIDENCE-LEVEL: INTEGRATION_TEST

Every security-relevant claim this repository makes, with the exact
evidence that backs it and the level of that evidence. Levels (weak →
strong): CODE < UNIT_TEST < INTEGRATION_TEST < LIVE_TEST < FUNDED_TEST
< EXTERNAL_AUDIT. **Nothing in this repository is backed above
INTEGRATION_TEST.** Where a cell says NONE, the claim is not made
anywhere in the product — this matrix exists so that never changes
silently.

## Authentication & session security

| Claim | Evidence | Level |
| --- | --- | --- |
| Sessions are server-side records with hashed tokens; logout revokes durably | `saas/sessions` suite + `saas_runtime_records` revocation UPDATEs (warn!-logged) | INTEGRATION_TEST |
| API keys are stored hashed (never plaintext); last-use tracked | `saas/api_keys.rs` + authorization-chain suite | INTEGRATION_TEST |
| Every tenant-surface request passes the one authorization chain (permission + org scope) | `saas/middleware.rs` `authorize_request`; authorization_chain + cross-tenant suites | INTEGRATION_TEST |
| Cross-tenant access returns 404 (not 403) with no oracle | `custody_rotation_cross_tenant`, tenant execution suites | INTEGRATION_TEST |

## Tenant isolation

| Claim | Evidence | Level |
| --- | --- | --- |
| Trading data plane (orders/positions/executions/reports) is org-scoped in SQL | tenant data-plane suites (PostgreSQL) + forensic sweep G1–G8 | INTEGRATION_TEST |
| Every tenant-table upsert arbiter is tenant-composite (0026–0034 swaps hold in code) | forensic sweep G6: 11/11 arbiters carry organization_id | CODE (mechanical) |
| Zero unscoped tenant-table SQL statements ship | `scripts/forensic-sql-scan.sh` — 371 classified, 0 class-4; gate `tests/forensics/sql-pattern-regression.sh` | CODE (mechanical) |
| Custody profiles/signers are org-scoped end-to-end incl. durable writes | 3 custody rotation suites (missing profile, cross-tenant, profile resolution) | INTEGRATION_TEST |
| Sanctioned global access is enumerated and justified | forensic sweep class-2/3 tables in `docs/FORENSIC-SQL-RESEARCH-2026.md` | CODE (mechanical + documented) |

## Custody & signing

| Claim | Evidence | Level |
| --- | --- | --- |
| Vault transit ed25519 signing implements the real wire protocol | `custody/vault/signer.rs` + wire-format unit tests | UNIT_TEST |
| AWS KMS Sign implements real SigV4 request signing with EdDSA | `custody/kms/` + SigV4 fixture tests | UNIT_TEST |
| Vault/KMS have been exercised against live backends | **NONE — never claimed** | — |
| HSM custody fails closed, naming the exact PKCS#11 dependency | `custody/hsm.rs` + refusal test | UNIT_TEST |
| Custody operations are audit-logged with org scope | custody suites assert audit records | INTEGRATION_TEST |

## Billing & provider webhooks

| Claim | Evidence | Level |
| --- | --- | --- |
| Stripe + Paddle webhooks verify HMAC-SHA256 signatures before processing | billing webhook suites (fixture vectors) | UNIT_TEST |
| Provider events are idempotent across replays (deployment-wide dedup) | provider_events suite + forensic sweep class-2 | INTEGRATION_TEST |
| Live payment providers were exercised | **NONE — fixture-tested only, never claimed** | — |

## Audit trail

| Claim | Evidence | Level |
| --- | --- | --- |
| audit_events form a tamper-evident hash chain (genesis, prev_hash, hash) | audit chain unit tests + verification path | UNIT_TEST |
| Concurrent appends cannot fork the chain (advisory-lock serialized) | `audit_chain_survives_concurrent_appends` (PostgreSQL) | INTEGRATION_TEST |
| The chain spans the whole deployment by design (single total order) | forensic sweep class-2 entry (repo.rs) | CODE (documented) |

## Release integrity

| Claim | Evidence | Level |
| --- | --- | --- |
| The buyer tree is in byte parity with the canonical tree | `scripts/compare-canonical-to-buyer-source.sh` + parity regression test (with planted-drift proof) | CODE (mechanical) |
| No secrets/credentials are committed | `scripts/verify-release-integrity.sh` contamination scan | CODE (mechanical) |
| Marketing claims cannot exceed their evidence | `scripts/verify-marketing-claims.sh` (banned-phrase + evidence-tag rules) | CODE (mechanical) |
| The manifest's counts are measured, not hand-entered | `scripts/update-release-manifest.sh` recomputation | CODE (mechanical) |

## Network/edge (explicitly NOT claimed)

| Claim | Evidence | Level |
| --- | --- | --- |
| TLS termination / WAF / DDoS posture | **NONE — deployment concern, out of repository scope** | — |
| Rate limiting on public API surfaces | RateLimiter in server + unit tests | UNIT_TEST |

## How to re-verify this matrix

```bash
scripts/forensic-sql-scan.sh
tests/forensics/sql-pattern-regression.sh
scripts/verify-marketing-claims.sh
scripts/verify-release-integrity.sh
cargo test -p sniper-suite --lib saas::custody -- --test-threads=1
cargo test -p sniper-suite --test custody_rotation_cross_tenant -- --test-threads=1
```

The matrix is dated 2026-10-01; any code change after that date
requires re-running the verification paths above, not trusting this
page.
