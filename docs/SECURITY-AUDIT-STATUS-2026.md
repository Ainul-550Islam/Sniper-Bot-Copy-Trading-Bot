# SECURITY AUDIT STATUS (2026-10-01)

EVIDENCE-LEVEL: INTEGRATION_TEST

The single honest statement of the security posture of this repository
as of 2026-10-01: what is verified, at what evidence level, what has
NEVER been tested, and what an external audit would find before it
starts. No external security audit has been commissioned — this
document is the internal evidence inventory, not an audit result.

## Bottom line

| Question | Answer |
| --- | --- |
| External security audit commissioned? | **NO** |
| Live/firewall-edge penetration test? | **NO** |
| Tenant isolation regression-tested? | YES — integration tests against PostgreSQL 17 (cross-tenant suites: custody rotation, tenant execution, orders/positions/executions data plane) |
| SQL tenant-sweep with a regression gate? | YES — `scripts/forensic-sql-scan.sh` (371 statements classified, 0 missing-tenant-enforcement) + `tests/forensics/sql-pattern-regression.sh` |
| Secrets in repository? | NO — verified by release integrity check (contamination scan) |
| Audit trail tamper-evidence? | YES — hash-chained `audit_events` with advisory-lock serialization (unit + concurrency tested) |
| Custody signing? | REAL Vault transit (ed25519) + REAL AWS KMS (SigV4, EdDSA) implementations, unit-tested against wire formats — NEVER exercised against live backends |
| HSM custody? | Explicitly unimplemented; refuses with the exact PKCS#11 dependency named (fail-closed, tested) |

## Verified controls and their evidence levels

| Control | Where | Evidence level | How to re-verify |
| --- | --- | --- | --- |
| Tenant boundary on the trading data plane (orders/positions/executions/reports) | `crates/server/src/trading_data_plane/` | INTEGRATION_TEST | `cargo test -p sniper-suite --test tenant_* -- --test-threads=1` (PostgreSQL) |
| Custody profile tenant boundary (rotation, missing profile, cross-tenant) | `crates/server/src/saas/custody*.rs` + 3 rotation test files | INTEGRATION_TEST | `cargo test -p sniper-suite --test custody_rotation_* -- --test-threads=1` |
| Billing state machine + provider webhook idempotency (Stripe/Paddle HMAC) | `crates/server/src/saas/billing*.rs`, `provider.rs` | INTEGRATION_TEST | billing suites against PostgreSQL |
| Session/API-key authentication chain | `crates/server/src/saas/middleware.rs`, `authorization_chain.rs` | INTEGRATION_TEST | authorization-chain + customer-API suites |
| Audit hash chain (tamper-evident, concurrent-append safe) | `crates/core/src/db/repo.rs` | INTEGRATION_TEST | `audit_chain_survives_concurrent_appends` in db_integration |
| Vault transit signing (ed25519) | `crates/server/src/custody/vault/` | UNIT_TEST | custody vault suite (wire-format fixtures) |
| AWS KMS Sign (SigV4, EdDSA) | `crates/server/src/custody/kms/` | UNIT_TEST | custody KMS suite (SigV4 signing-vector fixtures) |
| HSM provider refusal (fail-closed, names dependency) | `crates/server/src/custody/hsm.rs` | UNIT_TEST | custody hsm suite |
| SQL tenant enforcement sweep | `scripts/forensic-sql-scan.sh` | CODE (mechanical) | script + `tests/forensics/sql-pattern-regression.sh` |
| Marketing claims cannot exceed evidence | `scripts/verify-marketing-claims.sh` | CODE (mechanical) | `tests/release/marketing_claims.sh` |
| Release contamination (secrets/artifacts in tree) | `scripts/verify-release-integrity.sh` | CODE (mechanical) | release integrity script |
| Rate limiting on public surfaces | `crates/server/src/` RateLimiter | UNIT_TEST | server suites |

## What has NEVER been tested (the honest list)

1. **No LIVE_TEST**: Vault, KMS, Stripe, Paddle, Telegram, and
   Polymarket have never been exercised against real backends from this
   repository. Every integration is real code with unit-tested wire
   protocols.
2. **No FUNDED_TEST**: nothing has traded real funds; no real stake,
   order, or payment has ever been submitted.
3. **No EXTERNAL_AUDIT**: no third-party security firm has reviewed
   this codebase. `docs/PENETRATION-TEST-READINESS.md` documents how to
   run one.
4. **No live edge hardening**: TLS termination, WAF, DDoS posture, and
   network-level defenses are deployment concerns outside this
   repository's scope and are NOT tested here.

## Defects found by internal verification and their fixes

The internal verification program (PROMPT 5–6) found and fixed — with
tests and migrations as evidence:

| Defect | Fix | Evidence |
| --- | --- | --- |
| Custody profile durable INSERT always failed silently (0020 CHECK omitted `pending`; error swallowed by `let _ =`) | Migration `0035_custody_profile_status_pending.sql` + all custody durable writes now warn!-log | `custody_rotation_profile_resolution` integration test (PostgreSQL) |
| Custody FK to `organizations(id)` had no production writer for the relational org row | `ensure_organization_row()` in custody creation path | same integration test |
| Three custody rotation routes used axum-0.8-style `{id}` paths under axum 0.7 — status/activate/revoke always 404 | Routes corrected to `:id` | `rotation_uses_the_persisted_profile_id_never_a_new_one` |
| Order-cancel history attributed tenant rows to the deployment org (missing organization_id in INSERT) | Org-subselect INSERT in `trading_repository/orders/write.rs` | forensic SQL sweep (class-4 finding, fixed) |
| Tenant lifecycle durable phase update enforced org only in Rust | `AND organization_id = $4` added to the durable WHERE | forensic SQL sweep (class-12 finding, fixed) |

## What an external auditor should start with

```bash
scripts/forensic-sql-scan.sh            # the SQL tenant sweep (must be 0 class-4)
tests/forensics/sql-pattern-regression.sh
cargo test -p sniper-suite --test custody_rotation_profile_resolution -- --test-threads=1   # needs PostgreSQL
cargo test -p sniper-suite --test tenant_execution -- --test-threads=1                      # needs PostgreSQL
scripts/verify-release-integrity.sh
cat docs/SECURITY-THREAT-MODEL.md       # 11 threat areas with controls + remaining exposure
cat docs/SECURITY-CONTROLS-MATRIX.md
cat docs/SECURITY-BOUNDARY-MAP.md
```

## Related documents

* `docs/SECURITY-EVIDENCE-MATRIX-2026.md` — every security claim × evidence level
* `docs/SECURITY-THREAT-MODEL.md` — threat areas, controls, remaining exposure
* `docs/SECURITY-CONTROLS-MATRIX.md` — control-by-control matrix
* `docs/SECURITY-BOUNDARY-MAP.md` — trust boundaries
* `docs/PENETRATION-TEST-READINESS.md` — how to run a pentest against this tree
* `docs/SECRETS-MANAGEMENT-MATRIX.md` — where every secret lives
