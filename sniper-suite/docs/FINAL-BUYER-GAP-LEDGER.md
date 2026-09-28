# Final Buyer Gap Ledger — Sniper Suite

> Generated: 2026-09-24 · Updated: 2026-09-27 (Batch 10 — commands aligned with executable harnesses; `gap_id` registry mapping)
> Version: 0.1.0
> Source of truth (machine-readable): `crates/server/src/ops/final_gap_ledger.rs` (`FinalGapLedger::default_ledger()`) and `release-manifest.json` + `BUYER-TRUTH-REGISTER.md`.
> Statuses: OPEN / PARTIAL / EXTERNAL_REQUIRED / BUYER_ACTION / SELLER_ACTION — never CLOSED in this ledger (completed Batch 1–4 items are excluded).
> This ledger lists ONLY genuine unresolved gaps. Do not add synthetic gaps for already-complete work.

## How to use

Each gap has: `ID | Area | Status | Severity | Owner | External dependency | Evidence ref | Verification command | Detail`.

- `Owner=Buyer` — buyer must provision / run.
- `Owner=External` — third party (audit firm, chain validator) required.
- `Owner=Seller` — seller-side work still open (none remaining at 0.1.0 — if found, file issue).
- `External dependency` — what must exist before verification can pass.
- `Verification command` — exact command the buyer runs; only after it passes may the gap move to VERIFIED in `crates/server/src/ops/external_validation.rs`.

Completed Batch 1–4 work is intentionally NOT listed as gaps (e.g. billing state machine, custody rotation, tenant lifecycle/retention, websocket auth, OpenAPI+SDK, frontend lockfile+CI, evidence scripts, PG/Redis harnesses). Those are implemented and tested per `docs/FEATURE-TRACEABILITY.md` and `release-manifest.json`.

## Ledger (deterministic sort by ID)

| ID | Area | Status | Severity | Owner | External dependency | Evidence ref | Verification command | Detail |
|---|---|---|---|---|---|---|---|---|
| GAP-001 | billing | EXTERNAL_REQUIRED | HIGH | Buyer | Stripe/Paddle live keys | `docs/BUYER-TRUTH-REGISTER.md` | `LIVE_BILLING=1 STRIPE_API_KEY=... cargo test --test live_billing_contract -- --ignored --nocapture` | Live billing webhook+checkout requires external provider keys and funded provider account. Adapter + end-to-end wiring are verified separately (see MATERIAL-GAP section); only live execution is NOT_RUN — no live payment is claimed. |
| GAP-002 | custody | EXTERNAL_REQUIRED | HIGH | Buyer | Vault/KMS/HSM | `docs/BUYER-TRUTH-REGISTER.md` | `LIVE_CUSTODY=1 VAULT_ADDR=... cargo test --test live_custody_contract -- --ignored --nocapture` | Remote custody **boundary** (provider selection, credential refs, fail-closed refusal) is implemented and tested; the remote signing backends are **not implemented in this build** — selecting `vault`/`kms`/`hsm` fails startup with `SignerError::UnsupportedBackend` and never silently falls back to a local key (`docs/SECURITY.md`, runbook §2). |
| GAP-003 | deployment | BUYER_ACTION | BLOCKER | Buyer | production infra | `docs/BUYER-DEPLOYMENT.md` | `DEPLOYMENT_BASE_URL=... cargo test --test deployment_smoke -- --nocapture` | Production deployment requires buyer-provided host, env, TLS, Postgres+Redis and a read-only smoke against `DEPLOYMENT_BASE_URL`. A local `docker run` + `curl localhost` proves the image only (LOCAL CONTAINER SMOKE — never production verification; a test forbids substituting `localhost`). No prod deployment is claimed. |
| GAP-004 | trading | EXTERNAL_REQUIRED | CRITICAL | Buyer | funded keys + live mode | `docs/LIVE-VALIDATION.md` | `cargo test -p sniper-suite --lib funded_mode_guard` | Funded live trading requires funded keys and explicit live approval. Repo defaults to `EXECUTION_MODE=dry_run`; the guard proves default-never-funded (`live_unfunded` denied), the funded step itself is operator-only (`docs/LIVE-VALIDATION.md` §GAP-004). |
| GAP-005 | staking | EXTERNAL_REQUIRED | HIGH | External | solana-test-validator | `programs/staking-suite/tests/validator_e2e.rs` | `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1` | Staking validator E2E requires local `solana-test-validator` with compiled `.so` and `STAKING_E2E=1`; `programs/staking-suite` is its own (excluded) workspace so the `cd` is required, and the placeholder program id `3vEEMM…` must never be read as a deployed program (it maps to BLOCKED, not PASS). |
| GAP-006 | audit | EXTERNAL_REQUIRED | CRITICAL | External | audit firm | `docs/SECURITY.md` | n/a — external auditor deliverable (handover slot: `docs/EXTERNAL-VALIDATION-RUNBOOK.md` § GAP-006) | External security audit not performed — do not claim audited. Security review is internal only (`cargo audit`, `cargo deny` best-effort). |

## External validation mapping

`crates/server/src/ops/external_validation.rs` (`ExternalValidationRegistry::default_registry()`) mirrors these gaps plus DB integrations:

- `stripe_live`, `paddle_live` → GAP-001
- `vault_live`, `kms_live`, `hsm_live` → GAP-002
- `postgres_integration`, `redis_integration` → `gap_id = ""` (deliberately **not** buyer gaps: harnesses exist and the PostgreSQL path is service-backed verified; registry reason stored in the entry detail)
- `staking_e2e` → GAP-005
- `deployment_smoke` → GAP-003
- `funded_trading` → GAP-004
- `external_audit` → GAP-006

Only after the verification command for a gap succeeds may its entry be moved to `VERIFIED` — and only through `mark_verified(id, evidence_ref, verified_at, detail)`, which requires a non-empty evidence file and timestamp. `set_status` refuses `VERIFIED` and `ExternalValidationRegistry::new` rejects a bare `VERIFIED` entry; `gap_mapping_matches_ledger` fails if this table and the registry drift apart. Nothing defaults to VERIFIED.

## Determinism & duplicate rules

- Ledger is sorted by `id` ascending (GAP-001 … GAP-006). Any insertion must re-sort.
- Duplicate `id` is rejected (`FinalGapLedger::new` returns `Err`).
- `CLOSED` entries are rejected from this ledger — closed work belongs in `docs/FEATURE-TRACEABILITY.md` and `release-manifest.json`, not here.
- Severity order is LOW < MEDIUM < HIGH < CRITICAL < BLOCKER for sorting tie-breaks if needed.

## What is NOT a gap

The following are complete and must not be re-listed as unresolved:

- Batch 1 SaaS foundation (tenant scoping, migrations 0001–0022)
- Batch 2 billing/commercial (state machine, webhooks, idempotency)
- Batch 3 custody/rotation (trait + Vault/KMS/HSM adapters + key rotation)
- Batch 4 lifecycle/retention, websocket security, OpenAPI+SDK, frontend `npm ci`/`package-lock.json`+CI, evidence scripts, ops separation, PG/Redis harnesses
- Observability (logs/metrics/tracing), backup/restore manifests, release packaging — these are Batch 5 newly implemented in `crates/server/src/ops/*` and `crates/server/src/backup/*` with tests.
- External validation harness (Batch7 2026-09-24): 24-file reproducible harness (provider_contract/provider_contract_runner/deployment_smoke/network_policy/external_evidence/external_evidence_verify/live_gate/funded_mode_guard + solana connection/geyser + staking deployment/validator + billing live_provider + custody live_provider + 6 integration harnesses + script + runbook) — **harness READY, live execution still EXTERNAL_REQUIRED/NOT_RUN until buyer provisions credentials**. See `release-manifest.json:components.external_validation_batch7_24` and `docs/EXTERNAL-VALIDATION-RUNBOOK.md`.

**Batch7 truthful harness status (2026-09-24):** GAP-001 Stripe/Paddle **ADAPTER READY LIVE NOT_RUN** (explicit `LIVE_BILLING=1`), GAP-002 Vault/KMS/HSM **BOUNDARY READY NOT_RUN** (explicit `LIVE_CUSTODY=1`, no local fallback, no private-key extraction), GAP-003 deployment **SMOKE HARNESS READY NOT_RUN** (requires `DEPLOYMENT_BASE_URL`, never claims without URL), GAP-004 funded **GUARD READY NOT_RUN** (live-funded requires explicit `execution_mode=live_funded` + owner auth, default never trades), GAP-005 staking **HARNESS READY NOT_RUN/BLOCKED** (requires `STAKING_E2E=1` + validator; placeholder `3vEEMM…` is BLOCKED), GAP-006 audit **NOT DONE**. Every result has `validation_id/provider/env/timestamp/command/status/evidence_hash/redacted metadata` (no `DATABASE_URL`/`REDIS_URL`/secrets); verify with `bash scripts/run-external-validation.sh all-safe` (6/6 NOT_RUN in hermetic, canonical `evidence_hash`) + `cargo test -p sniper-suite --test provider_contracts` (verifies every saved evidence file) `cargo test -p sniper-suite --test deployment_smoke` / `solana_contract` / `staking_contract`.

If you believe a completed item is still open, open an issue referencing `docs/FEATURE-TRACEABILITY.md` and `cargo test` evidence. Batch7 external gaps remain **EXTERNAL_REQUIRED** but are now **reproducibly verifiable** via the harness — they are not claimed VERIFIED.

## Buyer handover pointer

- Handover: `docs/BUYER-HANDOVER-CHECKLIST.md`
- Truth: `docs/BUYER-TRUTH-REGISTER.md` + `docs/BUYER-EVIDENCE-PACK.md`
- Deploy: `docs/BUYER-DEPLOYMENT.md`
- Backup/Restore: `docs/BACKUP-RESTORE.md`
- Security: `docs/SECURITY.md`
- Verify: `scripts/final-release-check.sh` → `scripts/verify-buyer-package.sh` → `scripts/verify-delivery.sh` → `scripts/release-evidence.sh`

## MATERIAL-GAP BATCH status distinction (2026-09-26)

This ledger lists only **open** gaps. The items below are **closed internally** and are
recorded here solely so the buyer can see exactly what is and is not proven:

| Item | Internal status | External status |
|---|---|---|
| GAP-02 checkout durability (`checkout_sessions`, `UNIQUE (organization_id, idempotency_key)`) | **CODE VERIFIED + SERVICE-BACKED VERIFIED** (real PostgreSQL 17.11: 2-way and 4-way concurrent creates converge on one row, restart returns the same durable identity, raw duplicate insert rejected by the database, provider failure leaves `pending` with no fabricated session/URL) | n/a — no external dependency |
| GAP-03 invoice reads (`WHERE organization_id=$1 ORDER BY created_at, id`) | **CODE VERIFIED + SERVICE-BACKED VERIFIED** (real rows: ordering, tenant isolation 200 vs 404, empty result) | n/a |
| GAP-04 provider adapters (`stripe_adapter`, `paddle_adapter`, `provider_registry`) | **ADAPTER + END-TO-END WIRING VERIFIED** — `create_checkout` resolves the adapter, requires `LIVE_BILLING=1` + credentials, invokes it, persists `provider_session_id`/`checkout_url`/`open`, propagates `Idempotency-Key`; typed `NOT_RUN`/`PROVIDER_NOT_CONFIGURED` otherwise, never a Manual fallback and never a fabricated URL. Adapters also enforce a bounded 15 s timeout, require a real provider session id (the Paddle `"paddle_session"` placeholder was removed) and return redacted, status-classified errors | **LIVE EXECUTION NOT_RUN** — needs real Stripe/Paddle keys, funded account, network; remains GAP-001 |
| PostgreSQL concurrency (idempotency under concurrent replicas) | **SERVICE-BACKED VERIFIED** (real PostgreSQL 17.11, migrations 0001–0022, catalog-confirmed unique constraints) | n/a |

Nothing above closes GAP-001 (live provider execution), GAP-002 (remote custody),
GAP-003 (production deployment), GAP-004 (funded trading), GAP-005 (validator E2E) or
GAP-006 (external audit).
