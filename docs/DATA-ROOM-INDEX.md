# Buyer Data-Room Index — sniper-suite 0.1.0

> **SUPERSEDED (2026-09-27):** the single canonical buyer data-room index is
> `docs/FINAL-BUYER-DATA-ROOM.md`. This page is kept for historical reference (its "70 → 101" and
> "(this batch)" markers describe the 2026-09-24 batch that produced it); for current counts and
> paths use the canonical index.

> **Version:** 0.1.0 (2026-09-24) · **Migrations:** 22 (0001–0022, contiguous, forward-only) · **Rust sources:** 343 · **Docs:** 70 → 101 (after this batch) · **Tests (grep):** 1331 · **Workspace members:** 8

This index is the single entry point to the complete transaction evidence set. Every path below exists in this tree at the indicated location — no invented evidence.

## 1. Source & Build
| Area | Path | Evidence |
|---|---|---|
| Workspace manifest | `Cargo.toml` (8 members), `Cargo.lock`, `programs/staking-suite/Cargo.lock` | `release-manifest.json` §components, `rust-toolchain.toml` 1.98.1 |
| Core types & state | `crates/core/src/{config,state,models,billing,custody,db,ownership}` | `cargo check --workspace` PASS |
| Control plane (Axum) | `crates/server/src/{main.rs,api.rs,ops/*,backup/*,saas/*,security/*}` | `crates/server/src/ops/` 41 files, `backup/` 5 files |
| Trading modules | `crates/{module-sniper,module-copy,module-polymarket,module-telegram}` | `crates/module-sniper/src/lib.rs` etc. |
| Solana kit | `crates/solana-kit/src/{rpc,signer,tokens}` | `solana-sdk 2.1` |
| Staking program (separate) | `programs/staking-suite/{src/lib.rs,Cargo.toml,Cargo.lock}` | `cargo build-sbf` (agave 2.1.21) |
| Frontend | `apps/control-plane/{package.json,package-lock.json,src/app,src/lib}` | `npm ci/typecheck/build/lint` |

## 2. Architecture
- `docs/ARCHITECTURE.md` — original freeze architecture
- `docs/ARCHITECTURE-OVERVIEW.md` *(this batch)* — buyer-oriented overview, implemented vs external
- `docs/MODULES.md`, `docs/SNIPER-ENGINE.md`, `docs/COPY-TRADING-*.md`, `docs/POLYMARKET-*.md`, `docs/STAKING.md`

## 3. Testing
- `docs/TESTING.md` + `docs/FEATURE-TRACEABILITY.md`
- `crates/core/tests/{db_integration.rs,redis_integration.rs,distributed_integration.rs,saas_control_plane.rs}` — gated on `POSTGRES_URL`/`REDIS_URL`
- `crates/server/tests/{postgres_saas_integration.rs,redis_saas_integration.rs,billing_integration.rs}` + Batch5 harnesses `observability_config.rs,release_manifest_integration.rs,buyer_package_integration.rs,backup_restore_integration.rs`
- `programs/staking-suite/tests/validator_e2e.rs` — gated `STAKING_E2E=1`

## 4. Security
- `docs/SECURITY.md`, `docs/SECURITY-THREAT-MODEL.md` *(new)*, `docs/SECURITY-CONTROLS-MATRIX.md` *(new)*, `docs/PENETRATION-TEST-READINESS.md` *(new)*
- `docs/SECURITY-BOUNDARY-MAP.md`, `crates/server/src/security/{cors_policy.rs,headers.rs,security_headers.rs,tenant_context.rs,legacy_websocket_guard.rs,websocket.rs}` (6 files)
- Supply chain: `sbom.json`, `sbom.cyclonedx.json`, `licenses.json`, `licenses.csv` (generated 2026-09-24), `deny.toml`

## 5. Deployment & Operations
- `docs/DEPLOYMENT.md`, `docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md` *(new)*, `docs/PRODUCTION-READINESS-MATRIX.md` *(new)*
- `docs/OPERATIONS.md`, `docs/OPERATIONS-RUNBOOK.md` *(new)*, `docs/INCIDENT-RESPONSE-RUNBOOK.md` *(new)*, `docs/ROLLBACK-RUNBOOK.md` *(new)*
- `Dockerfile` (`rust:1.98.1-bookworm`), `docker-compose.yml`, `.env.template`, `config.toml.example`

## 6. Billing / Custody / Lifecycle
- `crates/core/src/billing/{pricing.rs,provider_events.rs,reconciliation.rs,billing_state.rs,dunning.rs,usage_policy.rs,provider_config.rs}`
- `crates/core/src/custody/{credentials.rs,health.rs,resolve.rs,provider_config.rs,rotation.rs}`
- `crates/server/src/saas/{billing*.rs,custody*.rs,tenant_lifecycle.rs,organizations.rs}`, `crates/server/src/provisioning/`
- `docs/SAAS-*.md`, `docs/BUYER-DEPLOYMENT.md`, `docs/BACKUP-RESTORE.md`

## 7. Backup / Restore
- `crates/server/src/backup/{export_manifest.rs,restore_manifest.rs,preflight.rs,commands.rs}` — strict `DOCUMENTED→EXECUTED→PROVEN`
- `docs/BACKUP-RESTORE.md` + scripts `scripts/build-release-package.sh`

## 8. SBOM / License / IP
- Generated: `sbom.json` (34758 B, sha `fd837e42…`), `licenses.json` (107K, 707 entries)
- `docs/THIRD-PARTY-SOFTWARE-INVENTORY.md` *(new)*, `docs/OPEN-SOURCE-COMPLIANCE.md` *(new)*, `docs/IP-*.md` *(new)*
- `LICENSE` (MIT, 2026 sniper-suite authors), `docs/THIRD-PARTY.md`, `docs/IP-COMPONENTS.md`

## 9. Limitations & External Validations
- `docs/FINAL-BUYER-GAP-LEDGER.md` (6 gaps), `docs/KNOWN-LIMITATIONS.md` *(new)*, `docs/TRANSACTION-READINESS-REPORT.md` *(new)*
- `crates/server/src/ops/{external_validation.rs,final_gap_ledger.rs}`

## 10. Buyer Verification
- `docs/BUYER-VERIFICATION-SCRIPT.md` *(new)* — separates HERMETIC / SERVICE-BACKED / EXTERNAL
- `scripts/{verify-delivery.sh,verify-buyer-package.sh,final-release-check.sh,build-release-package.sh,generate-sbom.sh,generate-license-report.sh}`

## 11. Crosswalk & Release Notes
- `docs/FINAL-EVIDENCE-CROSSWALK.md` *(new)*, `docs/RELEASE-NOTES-CURRENT.md` *(new)*, `docs/EVIDENCE-INDEX.md`, `release-manifest.json`

> **How to verify this index:** `ls docs/*.md | wc -l` (live value = `docs_canonical` in `docs/STATS.md`), `cat release-manifest.json | grep docs_files`, `bash scripts/verify-delivery.sh` (PASS after hygiene fix), `bash scripts/verify-buyer-package.sh`.
