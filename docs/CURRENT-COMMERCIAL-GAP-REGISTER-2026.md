# CURRENT COMMERCIAL GAP REGISTER (2026-10-01)

EVIDENCE-LEVEL: CODE

Every gap that matters commercially, stated as a register with
severity, what a buyer would have to spend to close it, and the
evidence that it IS a gap (never a guess). This is the document a
buyer prices the deal from: nothing here is hidden, softened, or
promised-away.

## Severity scale

* **P0 — blocks a specific sale/deployment** if the buyer's use case
  needs it.
* **P1 — expected by most enterprise buyers**; closes with defined
  engineering work.
* **P2 — known limitation, acceptable to most buyers**; documented so
  it cannot surface later as a surprise.

## The register

| # | Gap | Severity | Why it is a gap (evidence) | What closing it takes |
| --- | --- | --- | --- | --- |
| 1 | No live-provider operation (Vault, KMS, Stripe, Paddle, Telegram, Polymarket) | **P0 for live launch** | No LIVE_TEST evidence exists anywhere; wire protocols are unit-tested only (docs/SECURITY-EVIDENCE-MATRIX-2026.md) | Live sandbox credentials + LIVE_TEST evidence per provider (days each, not weeks) |
| 2 | No funded trading | **P0 for any capital deployment** | No FUNDED_TEST evidence; nothing has traded real funds | Buyer-run funded pilot under their own risk policy |
| 3 | No external security audit / SOC2 | **P1** (P0 for regulated buyers) | No EXTERNAL_AUDIT commissioned (docs/SECURITY-AUDIT-STATUS-2026.md) | Commission audit; docs/PENETRATION-TEST-READINESS.md has the runbook |
| 4 | HSM custody unimplemented | P2 | Fail-closed refusal naming PKCS#11 (custody hsm suite) | PKCS#11 integration behind the existing CustodySigner trait |
| 5 | Telegram forwarder is deployment-level | P2 | Binding API exists per tenant; forwarder routes to the deployment alert chat (docs/CURRENT-STATE.md) | Per-tenant outbound routing in the forwarder |
| 6 | Staking program undeployed (placeholder id) | P1 if staking is the use case | scripts/staking-identity.sh verify reports the pre-deployment placeholder | Buyer generates final keypair, set-id, deploy (scripted, guarded) |
| 7 | Single-operator deployment model | P2 | HA plane is per-deployment (docs/DISTRIBUTED.md); no multi-operator tenancy is claimed | Operator-model redesign (out of current scope) |
| 8 | Legacy operator-plane risk-event attribution (deployment org) | P2 | Forensic sweep class-3 entry (repo.rs risk_events); tenant plane is org-scoped | Route legacy risk log through org-aware writer |
| 9 | No per-tenant infrastructure telemetry/metrics pack | P2 | Operations docs cover deployment-level monitoring only | Metrics surface per org (new feature, not a defect) |
| 10 | Control-plane UI is unbranded / no white-label theming | P2 | apps/control-plane ships functional screens only | Theming layer (buyer branding decision) |
| 11 | No automated CI pipeline in-repo | P2 | CI equivalence documented (docs/CI-LOCAL-EQUIVALENCE.md); all gates run as local scripts | Wiring the same scripts into the buyer's CI |
| 12 | Pre-1.0 versioning (0.1.0, no SemVer promise) | P2 | docs/API-COMPATIBILITY-MATRIX.md versioning section | Buyer's release-management policy |

## What is deliberately NOT a gap

These are sometimes raised in diligence and are NOT gaps — with the
reason each is a design decision:

* **No down-migrations** — forward-only by design; rollback is a
  documented operational runbook (docs/BACKUP-RESTORE.md,
  docs/ROLLBACK-RUNBOOK.md), not a schema rewind.
* **`saas_runtime_records` as a generic KV plane for SaaS runtime
  state** — the custody/billing/lifecycle TRUTH tables are relational
  and org-scoped; the KV plane holds identity/session records whose
  org equality is enforced at the service layer (forensic sweep
  class-2, cross-tenant suites prove the boundary).
* **The audit chain being deployment-global** — a tamper-evident chain
  requires a single total order; per-tenant chains would weaken it.
* **Placeholder staking id as such** — the guarded `set-id`/`deploy`
  path IS the deliverable; a burned-in final id would be worse for the
  buyer (see docs/STAKING-PROGRAM-ID-VALIDATION.md).

## Pricing honesty

Every marketing-facing document is machine-gated
(`scripts/verify-marketing-claims.sh`): banned phrases (guaranteed,
risk-free, profitable, battle-tested, …) cannot ship without inline
evidence tags pointing at evidence files of sufficient level — and no
evidence above INTEGRATION_TEST exists. The commercial conversation
therefore starts from this register, not from aspirational claims.

## Related

* `docs/BUSINESS-MATRIX-2026.md` — per-business-line completeness and claim safety
* `docs/CURRENT-MARKETING-CLAIMS-2026.md` — the exact allowed claim vocabulary
* `docs/FINAL-BUYER-GAP-LEDGER.md` — the running gap ledger this register summarizes
* `docs/KNOWN-LIMITATIONS.md` — the technical limitations list
