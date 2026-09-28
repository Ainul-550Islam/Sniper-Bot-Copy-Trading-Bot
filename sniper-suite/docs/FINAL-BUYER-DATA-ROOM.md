# Final Buyer Data Room — sniper-suite 0.1.0

> **This is the single canonical buyer data-room index** (2026-09-27). `docs/DATA-ROOM-INDEX.md` is a
> superseded 2026-09-24 page kept for history. All paths below are **actual repository paths** — no
> copy-paste of content, only pointers; nothing is named that does not exist in this tree.

**Version:** `0.1.0` · **Rust:** `343` · **Docs:** `101` · **Migrations:** `22` · **Tests:** `1331` · **Members:** `8` · **Toolchain:** `1.98.1`

## 1. Source / build / migrations / SDK
* `crates/` (343 rs) — `core`, `solana-kit`, `module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`, `server`, `saas-sdk`
* **Migrations:** `crates/core/migrations/` — 22 files, contiguous `0001` → `0022`, forward-only (`sqlx migrate run`)
* **SDK:** `crates/saas-sdk/` (typed `billing`, `custody`, `commercial`, `client`, `models`; secret-free `Debug`)
* **Frontend:** `apps/control-plane/` (Next.js 16, `package-lock.json` 6171 lines v3)
* `VERSION`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `deny.toml`, `.cargo/audit.toml`, `docker-compose.yml`, `Dockerfile`, `scripts/` (10 sh)

## 2. API / OpenAPI
* `docs/API.md` — control-API reference (auth, degradation contract, REST endpoints, WebSocket feed)
* OpenAPI: `crates/server/src/saas/openapi.rs`, served at `GET /api/saas/openapi.json` (also checked by the deployment smoke harness)
* Compatibility matrices: `docs/API-COMPATIBILITY-MATRIX.md`, `docs/WEBHOOK-COMPATIBILITY-MATRIX.md`

## 3. Architecture
* `docs/ARCHITECTURE.md` · `docs/ARCHITECTURE-OVERVIEW.md` · `docs/REPOSITORY-MAP.md` · `docs/FORENSIC-FILE-INVENTORY.md` · `docs/HA-ARCHITECTURE.md` · `docs/DISTRIBUTED.md`

## 4. Security
* `docs/SECURITY.md` · `docs/SECURITY-THREAT-MODEL.md` · `docs/SECURITY-CONTROLS-MATRIX.md` · `docs/SECURITY-BOUNDARY-MAP.md` · `docs/SECRETS-MANAGEMENT-MATRIX.md` · `docs/PENETRATION-TEST-READINESS.md`
* `crates/server/src/security/` (6 files — `cors_policy`, `headers`, `security_headers`, `tenant_context`, `legacy_websocket_guard`, `websocket`)

## 5. Tests / evidence records
* `docs/TESTING.md` · `docs/FEATURE-TRACEABILITY.md` · `docs/TESTING.md` harness inventory
* `crates/server/tests/` (15 files) + `crates/server/src/ops/` contract/evidence modules · `programs/staking-suite/tests/validator_e2e.rs` (gated `STAKING_E2E=1`)
* Evidence records: `evidence/external/*.json` (6 redacted `NOT_RUN` records, canonical-hash verified by `cargo test --test provider_contracts`) · claim→source map `docs/EVIDENCE-INDEX.md` · current truth `docs/BUYER-TRUTH-REGISTER.md`

## 6. Deployment
* `docs/BUYER-DEPLOYMENT.md` (buyer sequence §1–§15) · `docs/DEPLOYMENT.md` · `docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md` · `docs/PRODUCTION-READINESS-MATRIX.md` · `docs/ROLLBACK-RUNBOOK.md` · `docs/OPERATIONS-RUNBOOK.md`
* `Dockerfile` · `docker-compose.yml` · `.env.template` · `.github/workflows/ci.yml`

## 7. Billing
* `crates/core/src/billing/` (plan, subscription, entitlement, usage, checkout, payment, invoice, dunning, usage_policy, provider_config, reconciliation)
* `crates/server/src/saas/{billing,billing_status,billing_reconciliation,checkout,invoices,payment_webhooks,commercial_state,usage_limits}.rs`
* `crates/saas-sdk/src/billing.rs` + `commercial.rs`

## 8. Custody
* `crates/core/src/custody/` (model, provider, credentials, health, rotation, resolve) · `crates/server/src/saas/{custody,custody_health,custody_rotation}.rs` · `crates/saas-sdk/src/custody.rs` · `crates/server/src/custody/live_provider_*.rs`

## 9. Backup/restore
* `docs/BACKUP-RESTORE.md` · `crates/server/src/backup/{export_manifest,restore_manifest,preflight,commands}.rs` (5 files incl. `mod.rs`) · `docs/OPERATIONS-RUNBOOK.md` (§ backup/restore)

## 10. SBOM
* `sbom.json` (200 components, 34,758 B, sha256 `fd837e42…`) · `sbom.cyclonedx.json` (CycloneDX, sha256 `fd837e42…`) · `crates/server/src/ops/sbom_report.rs` · delivered copy under `buyer-release/sbom/`

## 11. License report
* `licenses.json` (707 entries, 108,621 B, sha256 `c1c051ca…`) · `licenses.csv` · `crates/server/src/ops/license_report.rs` · `docs/OPEN-SOURCE-COMPLIANCE.md` · `docs/THIRD-PARTY-SOFTWARE-INVENTORY.md` · delivered copies under `buyer-release/licenses/`

## 12. IP handover
* `docs/IP-OWNERSHIP-REGISTER.md` · `docs/IP-HANDOVER-CHECKLIST.md` · `docs/FINAL-IP-AND-THIRD-PARTY-INVENTORY.md` · `LICENSE` (MIT, generic holder → LEGAL_REVIEW)

## 13. External validation
* `docs/EXTERNAL-VALIDATION-RUNBOOK.md` (§0.1 is the canonical handover table for the six gaps) · `docs/FINAL-EXTERNAL-VALIDATION-MATRIX.md` · `scripts/run-external-validation.sh` (modes) · `crates/server/src/ops/{external_validation,provider_contract,deployment_smoke,external_evidence}.rs` · `evidence/external/*.json` (redacted)

## 14. Gap ledger
* `docs/FINAL-BUYER-GAP-LEDGER.md` (GAP-001…GAP-006) · `crates/server/src/ops/final_gap_ledger.rs` + `external_validation.rs` · `docs/FINAL-BUYER-STATUS.md`

## 15. Known limitations
* `docs/KNOWN-LIMITATIONS.md` (16 rows) · `docs/FINAL-KNOWN-LIMITATIONS.md` · `docs/RELEASE-NOTES-CURRENT.md`

## 16. Buyer verification procedure
* `docs/BUYER-VERIFICATION-SCRIPT.md` (HERMETIC / SERVICE-BACKED / EXTERNAL separation) · `docs/BUYER-ACCEPTANCE-TEST.md` (A2 = `sha256sum -c`) · `docs/BUYER-REPRODUCTION-GUIDE.md` · `docs/BUYER-HANDOVER-CHECKLIST.md`
* Scripts: `scripts/{verify-delivery,verify-buyer-package,final-release-check,build-release-package,run-external-validation}.sh`

## 17. Final checksums
* Delivered package: `buyer-release/checksums/SHA256SUMS` (per-artifact, `sha256sum -c`) · `buyer-release/checksums/all-files.sha256` (every package file except the manifest itself) · `buyer-release/checksums/SOURCE-TREE.sha256` (canonical `source/` tree digest)
* `release-manifest.json` (top level and `buyer-release/manifests/release-manifest.json`) · `docs/BUYER-VERIFICATION-SCRIPT.md` explains the three layers

> **Verify this data room:** `bash scripts/verify-delivery.sh` (7/7) → `bash scripts/verify-buyer-package.sh` (PASS) → `cd buyer-release && sha256sum -c checksums/SHA256SUMS` (8/8) → `bash scripts/final-release-check.sh` (ALL PASS) → `bash scripts/run-external-validation.sh all-safe` (6/6 `NOT_RUN`).
