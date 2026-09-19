# Security evidence index (deliverable D)

Audit-ready map: security area → model/design location → code location →
executed evidence → status. Produced in the buyer-hardening pass
(2026-09-18). **No external security audit exists; none is claimed.** This
index aggregates the vendor's own model docs, static scans, dependency
scans, and executed tests — it is NOT a penetration test or third-party
assessment. Findings discovered by execution in this pass (D1 metadata
discriminant, D2 e2e clone flag) are functional defects, fixed and
re-proven; see `AUDIT.md` §29.

| # | Area | Model / design doc | Code locus | Executed evidence (this package) | Status |
|---|---|---|---|---|---|
| 1 | Trust boundaries | `docs/ARCHITECTURE.md` §Crates, §Data-flow guarantees; `docs/SECURITY.md` §Threat model | crate split (core/modules/kit/server), module enablement gates | 537/537 workspace tests incl. module-boundary tests (`evidence/tests/phase2-test-workspace.log`) | Documented + test-pinned |
| 2 | Threat model (summary) | `docs/SECURITY.md` §Threat model, §Known limitations (honest list) | — | — (design artifact) | Documented; residual risks listed honestly |
| 3 | Signer / key management | `docs/SECURITY.md` §Key management | `crates/solana-kit/src/signer.rs` (local signer only; vault/kms/hsm fail startup by design) | signer tests inside 537; live-mode startup rejection path in `docs/BUYER-ACCEPTANCE-TEST.md` step 14 | Documented + test-pinned |
| 4 | Admin / API auth | `docs/API.md` §Authentication | server auth middleware (owner/admin roles, `x-api-key`) | API auth tests inside 537; audit-verify endpoint exercised with key in `evidence/app-startup/phase8b-endpoints.log` | Documented + executed smoke |
| 5 | Telegram authz | `docs/API.md` §Telegram control; `docs/OPERATIONS.md` (RBAC rows) | `crates/module-telegram/` (chat-id allowlist, owner-role mutation gate, token never in errors) | module tests inside 537 (incl. `error_strings_never_contain_the_bot_token`) | Documented + test-pinned; live bot = HUMAN ACTION (token required) |
| 6 | Replay / dedup | `docs/RECONCILIATION.md` (OMS idempotency keys — Postgres-backed, money-critical; Redis = accelerator only) | `crates/core/src/oms.rs`, journal claim scripts (redis `TIME`-based) | db_integration + distributed_integration + two_replica_mirror EXECUTED against real PG+Redis (inside 537); staking metadata replay rejection EXECUTED on validator (`evidence/tests/phase5-full-batch.log`) | Executed |
| 7 | Tx lifecycle / landing uncertainty | `docs/RECONCILIATION.md` (Unknown/Reconciled state machine) | `oms.rs`, reconciliation queue | recon tests inside 537; `recon_crash_e2e` previously executed (earlier session, pre-hardening source — labeled as such in `docs/HANDOVER.md` §3) | Test-pinned; crash-e2e historical |
| 8 | Recovery (crash/restart) | `docs/OPERATIONS.md` §Journal management; `docs/RECONCILIATION.md` §14 | startup recovery + reconcile sweep | recovery tests inside 537; buyer re-verification path = acceptance step 17 | Test-pinned |
| 9 | Shutdown / drain | `docs/ARCHITECTURE.md` §Shutdown (lifecycle.rs) | `crates/server` lifecycle (flag stop → journal pump stop → http drain → module drain → pump flush) | clean SIGTERM observed: "sniper-suite stopped cleanly" (`evidence/app-startup/phase8b-shutdown.log`) | Executed |
| 10 | Rate limits | `docs/API.md` §Degradation contract; `docs/OPERATIONS.md` §Dependency degradation matrix | server/limiter + RPC budget paths | limiter tests inside 537 | Test-pinned |
| 11 | WebSocket feed auth | `docs/API.md` §WebSocket event feed | server ws endpoint (same API-key auth) | ws tests inside 537 | Test-pinned |
| 12 | RPC trust model | `docs/SECURITY.md` §Execution safety; `docs/LIVE-VALIDATION.md` | reject-over-fallback on unverifiable live balance (no paper fallback in live), simulation-before-send, stale-balance rejection (>15 s) | collateral/balance-separation tests inside 537; simulate leg executed vs public devnet (`evidence/benchmarks/benchmarks-2026-09-18.json`) | Executed (simulated; funded = HUMAN ACTION) |
| 13 | Kill switch / live gate | `docs/OPERATIONS.md` §Emergency stop | runtime flag + live preflight chain | `/api/status` shows kill_switch:false + live_allowed:false in paper (`evidence/app-startup/phase8b-endpoints.log`); gate tests inside 537 | Executed smoke + test-pinned |
| 14 | Dependency advisories | `docs/SECURITY.md` §Dependency & supply chain | `.cargo/audit.toml` (9 allow-listed warnings with rationale), `deny.toml` (14 SPDX allow-list) | `evidence/security/audit-app.log`, `audit-staking.log` (0 errors each), `deny.log` (advisories/bans/licenses/sources ok) | Executed |
| 15 | Secret / marker scans | `scripts/release-check.sh` gates 5–6 | — | `evidence/security/static-scans.log`: 0 secret-pattern hits, 0 TODO/FIXME/stub markers, 0 keypair files in tree | Executed |
| 16 | unsafe inventory | `SECURITY.md` §Scope and posture | 0 `unsafe {` blocks in crates/ + programs/; `#![forbid(unsafe_code)]` in module-copy, module-polymarket, module-telegram, staking program | `evidence/security/static-scans.log` | Executed |
| 17 | Immutability of audit records | `docs/OPERATIONS.md` §Audit trail | append-only audit table + hash chain, no API mutation path | db_integration audit-chain tests inside 537; `/api/audit/verify` endpoint served (`phase8b-endpoints.log`, `not_chained` on empty table — chain integrity proven in db_integration 23/23) | Executed |
| 18 | Durable state placement | master directive: no durable financial state solely in Redis | Postgres = source of truth; Redis = cache/claims (documented §N in RECONCILIATION) | distributed/two-replica tests EXECUTED | Executed |

## Honest gaps (never converted to PASS)

* **External security audit:** none exists. Commission one before mainnet
  deployment, especially of `programs/staking-suite` (on-chain, immutable
  once deployed).
* **Penetration test / fuzzing:** not performed in this pass.
* **Funded live-path execution:** procedure only (`docs/LIVE-VALIDATION.md`);
  real-money validation is an explicit HUMAN ACTION with operator approval.
* **Docker/container attack surface:** image build BLOCKED in vendor sandbox
  (no daemon); Dockerfile is non-root + pinned base, but the built image was
  not scanned (no trivy/grype run exists — do not claim one).
* Historical executions on pre-hardening source (`recon_crash_e2e`,
  `devnet_e2e`, full `latency_bench`) remain labeled historical in
  `docs/HANDOVER.md` §3 — not re-claimed as current.
