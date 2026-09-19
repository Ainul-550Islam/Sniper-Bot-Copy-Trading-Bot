# Commercial artifact index (delivery manifest — human-readable)

Index of every artifact in the sniper-suite 0.1.0 buyer package and what it
is for. The machine-readable manifest is `release-manifest.json` (version,
components, toolchain, test counts, verification status, external blockers);
this document is the human map and does not duplicate source code.

## Delivery identity

- Product: **sniper-suite** — modular crypto trading system (5 modules +
  control plane), version **0.1.0**, MIT license (holder placeholder pending
  transfer).
- Frozen engineering tree: release commit `9c677cd`, freeze commit
  `0e139c3`; 146 tracked files / 2,801,590 bytes / 77,980 lines at freeze;
  final gate 20/20 PASS, 521/521 workspace tests, 0 failures.
- Documentation passes after the freeze (no source bytes changed — proven in
  each pass report): +14 buyer-package docs (160 files / 2,919,949 bytes),
  then +9 final-delivery docs + `scripts/verify-delivery.sh` (170 files).
  A later audit pass changed sources and added `collateral.rs` (171 files),
  and the buyer-hardening pass added `scripts/staking-identity.sh` + 3 docs
  (175 files) — see CHANGELOG.md [Unreleased]; the 0.1.0 snapshot archive
  reflects the pre-audit 170-file state.

## Root artifacts

| Artifact | What it is |
|---|---|
| [`release-manifest.json`](../release-manifest.json) | Machine-readable delivery manifest — version identity, components, migration high-water mark, toolchain pins, exact test counts, verification taxonomy, external handover blockers. Gated by `scripts/release-check.sh`. |
| [`README.md`](../README.md) | Product overview, quick start, configuration reference, API/observability summary, staking deployment, testing, project layout — plus the "Buyer / engineering handover" index section. |
| [`CHANGELOG.md`](../CHANGELOG.md) | Keep-a-Changelog history of 0.1.0 incl. both pre-tag fix passes (release-engineering + engineering-freeze). |
| [`AUDIT.md`](../AUDIT.md) | Full historical audit & build trail with per-pass evidence (27 sections). Historical sections are preserved as-is; later passes append. |
| [`SECURITY.md`](../SECURITY.md) | Vulnerability-reporting policy, supported versions, posture statement (incl. the explicit "no external audit" disclosure). |
| [`LICENSE`](../LICENSE) | MIT text with the documented copyright-holder placeholder + handover note. |
| [`VERSION`](../VERSION) | Release identity (`0.1.0`), gated for consistency with `Cargo.toml` + manifest. |
| [`scripts/release-check.sh`](../scripts/release-check.sh) | One-command, 20-gate local release validation (fmt → tests against real PG/Redis → staking → audit/deny → consistency). |
| [`rust-toolchain.toml`](../rust-toolchain.toml), [`deny.toml`](../deny.toml), [`Cargo.lock`](../Cargo.lock) | Pinned toolchain, supply-chain policy, locked app dependency graph (706 packages). |
| [`Dockerfile`](../Dockerfile), [`docker-compose.yml`](../docker-compose.yml), [`.env.template`](../.env.template), [`config.toml.example`](../config.toml.example) | Deployment assets (image build NOT EXECUTED in delivery sandbox — CI covers it). |
| [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) | 4-job CI: app workspace (services: PG16/Redis7), staking program (build-sbf + gated validator e2e), security (audit/deny), docker (build + smoke). |

## Engineering documentation (13 docs, delivered at freeze)

| Doc | What it is |
|---|---|
| [`docs/HANDOVER.md`](HANDOVER.md) | Verify-from-zero procedure, verification-status taxonomy, handover fill-ins, maintenance invariants. **Start here.** |
| [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) | Crate map, data-flow guarantees, startup/shutdown ordering. |
| [`docs/API.md`](API.md) | REST + WebSocket reference (28 endpoints), RBAC matrix, degradation contract. |
| [`docs/SECURITY.md`](SECURITY.md) | Threat model, key management, signer boundary, honest limitations. |
| [`docs/DEPLOYMENT.md`](DEPLOYMENT.md) | Compose + bare-metal setup, production checklist. |
| [`docs/OPERATIONS.md`](OPERATIONS.md) | Day-two runbook: alerts, incidents, journal, audit, backups. |
| [`docs/MODULES.md`](MODULES.md) | Per-module trading guide (feeds, sizing, exits, strategies). |
| [`docs/STAKING.md`](STAKING.md) | Program economics, governance, deploy + genesis sequence. |
| [`docs/TESTING.md`](TESTING.md) | Test layers, what runs where, known gaps. |
| [`docs/RECONCILIATION.md`](RECONCILIATION.md) | Source-of-truth model, ambiguity matrix, crash/startup recovery, PnL replay. |
| [`docs/DISTRIBUTED.md`](DISTRIBUTED.md) | Multi-replica operation: ownership, claims/leases/fencing, flag & book sync. |
| [`docs/RELEASE.md`](RELEASE.md) | Versioning, reproducible-build analysis, release manifest, cut-a-release checklist. |
| [`docs/BACKUP-RESTORE.md`](BACKUP-RESTORE.md) | Durable vs ephemeral data, backup/restore procedures, Redis-loss behavior. |

## Buyer package (14 docs, commercialization pass) + final delivery package (9 docs)

| Doc | What it is |
|---|---|
| [`docs/BUYER-OVERVIEW.md`](BUYER-OVERVIEW.md) | Technical overview: product, modules, architecture, execution path, risk, persistence, reconciliation, distributed ownership, observability, staking, deployment/security/testing/recovery models. |
| [`docs/CAPABILITY-MATRIX.md`](CAPABILITY-MATRIX.md) | Per-capability matrix: implemented / evidence / tested / environment / known limitation (23 capabilities). |
| [`docs/BUYER-DUE-DILIGENCE.md`](BUYER-DUE-DILIGENCE.md) | Independent verification checklist: source, build, testing, security, infrastructure, operations, ownership/IP, open external actions. |
| [`docs/IP-COMPONENTS.md`](IP-COMPONENTS.md) | IP/component inventory with provenance (original code vs external protocol integration) and licensing notes. |
| [`docs/THIRD-PARTY.md`](THIRD-PARTY.md) | Third-party/license inventory: lockfile provenance, major deps, deny policy, advisory scanning, SBOM status, reproduction commands. |
| [`docs/BUYER-DEPLOYMENT.md`](BUYER-DEPLOYMENT.md) | 15-step deployment handover with safety gates; ends in paper mode; live mode gated separately. |
| [`docs/ACCEPTANCE-CHECKLIST.md`](ACCEPTANCE-CHECKLIST.md) | Sign-off checklist with per-item status: VERIFIED / PREVIOUSLY VERIFIED / BUYER ACTION / EXTERNAL. |
| [`docs/RELEASE-NOTES-0.1.0.md`](RELEASE-NOTES-0.1.0.md) | Buyer release notes: identity, test results, components, security/engineering fixes, blockers, explicit non-claims. |
| [`docs/BUYER-FAQ.md`](BUYER-FAQ.md) | Technical FAQ — every answer source-backed (safety defaults, double-execution, crash behavior, Redis/RPC loss, audits, extensibility, multi-replica/tenancy). |
| [`docs/SCOPE-BOUNDARY.md`](SCOPE-BOUNDARY.md) | Commercial boundary: delivered software vs buyer infrastructure vs external services vs human/legal responsibilities. |
| [`docs/SUPPORT-HANDOVER.md`](SUPPORT-HANDOVER.md) | Handover model: source/deployment/config/incident/security-contact/credential-rotation/ownership/repository/staking/production sign-off. No SLA is promised or implied. |
| [`docs/BUYER-RISK-REGISTER.md`](BUYER-RISK-REGISTER.md) | 12 remaining risks with impact, delivered mitigation, evidence, buyer action — plus documented non-risks. |
| [`docs/TECHNICAL-DIFFERENTIATORS.md`](TECHNICAL-DIFFERENTIATORS.md) | 30 concrete engineering characteristics, each with a path. No rankings or superiority claims. |
| [`docs/DELIVERY-MANIFEST.md`](DELIVERY-MANIFEST.md) | This index. |
| [`docs/FINAL-DELIVERY.md`](FINAL-DELIVERY.md) | **Single human-readable starting point**: contents, version/commits, sizes, test & release evidence, taxonomy, components, doc map, infrastructure, buyer actions, limitations, ownership checklist. |
| [`docs/BUYER-QUICKSTART.md`](BUYER-QUICKSTART.md) | 18-step technical quick start: bundle verification → toolchain → PG/Redis → secrets → release-check → paper → probes/metrics/dashboard → Telegram authz → simulate → backup/restore → audit-chain review. |
| [`docs/TECHNICAL-FACT-SHEET.md`](TECHNICAL-FACT-SHEET.md) | One-page-per-topic fact sheet: language, architecture, crates, API, execution, persistence, reconciliation, distributed, observability, staking, tests, CI, scanning, Docker, security, audit status. |
| [`docs/SELLER-FACT-SHEET.md`](SELLER-FACT-SHEET.md) | Factual source document for seller use (listing composition, buyer Q&A). Not an advertisement; explicit non-claims list. |
| [`docs/SELLING-LISTING-SOURCE.md`](SELLING-LISTING-SOURCE.md) | Reusable factual listing material: title candidates, technical summary, feature/architecture/testing/deployment/security facts, deliverables, limitations, transfer requirements. |
| [`docs/DEMO-RUNBOOK.md`](DEMO-RUNBOOK.md) | 10 deterministic buyer demos (paper, simulate, probes/metrics, risk rejection, kill switch, restart/recovery, audit chain, distributed claims, staking, backup/restore) with commands, expected results, verification status. |
| [`docs/EVIDENCE-INDEX.md`](EVIDENCE-INDEX.md) | Claim → evidence file/section/status map for every major assertion in the package, with re-verification instructions. |
| [`docs/REPOSITORY-MAP.md`](REPOSITORY-MAP.md) | Annotated file/folder map of the actual delivered tree with exact counts. |
| [`docs/ARCHIVE-CHECKLIST.md`](ARCHIVE-CHECKLIST.md) | Final seller-archive specification: INCLUDE/EXCLUDE lists, bundle production procedure, integrity requirements. |

Delivery tooling added in the final pass: [`scripts/verify-delivery.sh`](../scripts/verify-delivery.sh)
(fast, fail-closed bundle-integrity check — required files, version identity,
docs count, hygiene, markdown links, invisible characters; complements, does
not duplicate, `scripts/release-check.sh`).

## Suggested reading order for a technical buyer

1. `docs/FINAL-DELIVERY.md` — the single starting point (identity, evidence,
   statuses, actions).
2. `docs/BUYER-QUICKSTART.md` — hands-on verification walkthrough.
3. `docs/BUYER-OVERVIEW.md` + `docs/TECHNICAL-FACT-SHEET.md` — what the
   system is.
4. `docs/CAPABILITY-MATRIX.md` + `docs/EVIDENCE-INDEX.md` — what is
   implemented, how it was tested, and where each claim is evidenced.
5. `docs/BUYER-DUE-DILIGENCE.md` + `docs/ACCEPTANCE-CHECKLIST.md` — how to
   verify everything independently and sign off.
6. `docs/BUYER-RISK-REGISTER.md` + `docs/SCOPE-BOUNDARY.md` — what remains
   open and who owns what.
7. `docs/BUYER-DEPLOYMENT.md` + `docs/DEMO-RUNBOOK.md` — how to stand it up
   (paper mode first) and demonstrate it.
8. Deep dives as needed: the 13 engineering docs, `AUDIT.md` for evidence
   history, `release-manifest.json` for machine-readable facts,
   `docs/REPOSITORY-MAP.md` + `docs/ARCHIVE-CHECKLIST.md` for the physical
   bundle.
