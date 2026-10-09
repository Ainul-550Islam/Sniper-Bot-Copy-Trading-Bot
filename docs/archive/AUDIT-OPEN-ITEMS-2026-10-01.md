# AUDIT OPEN-ITEMS CENSUS — 2026-10-01

Full reconciliation of the standing buyer audit (`AUDIT.md`, original upload of
2026-09-29) against the tree as it stands after PROMPTs 1–6. Purpose: a single
honest answer to "what is actually left?", distinguishing (a) items closed with
verifiable evidence, (b) items still open that are buildable in code, and
(c) items that code alone cannot close (external/commercial/legal), which stay
tracked in `docs/FINAL-BUYER-GAP-LEDGER.md` and `docs/IP-OWNERSHIP-REGISTER.md`.

No new marketing claim is made anywhere in this document; statuses are
measured facts with file pointers and verification commands.

## 1. Method

- Source of asks: the original `uploads/AUDIT.md` (47,796 bytes, md5
  `45342410d81de636cf2df4475cafe00a`) — sections §4–§24, the §26 scorecard,
  the §27 priority list (P0-1…P1-5) and the §22 business list (P0 1–6,
  P1 7–15, P2 16–24).
- Source of state: the canonical tree at the repo root of this workspace,
  checked by direct grep/read on 2026-10-01, plus the release gates
  re-run the same day (all PASS, see §5).
- Every CLOSED row names where the implementation and its tests live.
  Every OPEN row names the exact current evidence that it is still open
  (file and line), so a future prompt can start from a verified baseline.

## 2. CLOSED — verified in the tree (audit ask → remediation → evidence)

| Audit ask (section) | Remediation | Evidence / verification |
|---|---|---|
| §4 / P0-1 / §22-P0-1 buyer package parity | Package regenerated from canonical tree with byte-exact mirror + 4 checksum artifacts | `scripts/rebuild-buyer-release.sh`, `scripts/verify-buyer-package.sh` (PASS 2026-10-01), `tests/release/buyer_parity.sh`, `tests/release/buyer_source_parity.sh` |
| §5 / §22-P0-6 documentation freshness | Unsuffixed living docs (`docs/CURRENT-STATE.md`), measured counts re-derived by script, stale suffixed docs removed | `scripts/update-current-audit.sh` (exit 0), `tests/release/manifest_current.sh` |
| §11 / P0-2 / §22-P0-2 tenant→module runtime wiring | `TenantExecutionContext` reaches sniper/copy/polymarket executors, signing and broadcast guards, persistence | `crates/core/src/execution/tenant_execution_context.rs`, `crates/{module-sniper,module-copy,module-polymarket}/src/tenant_{context,executor}.rs`, `crates/solana-kit/src/tenant_{signing_context,transaction,broadcast_guard}.rs`, `crates/server/src/module_runtime/tenant_module_factory.rs`; tests `tenant_scope_isolation.rs`, `tenant_guard_chain_integration.rs`, `tenant_module_factory_pg.rs` |
| §12 / P0-3 / §22-P0-3 tenant-aware repositories | Production queries scoped by `organization_id` (the tenant column throughout this codebase) | `crates/core/src/db/` repositories; `crates/core/tests/tenant_scope_isolation.rs`; cross-tenant suites `crates/server/tests/custody_rotation_cross_tenant.rs` |
| P0-4 / §22-P0-4 atomic tenant-local uniqueness | Rust `ON CONFLICT` arbiters consistently `(organization_id, …)`; 40 organization-scoped unique constraints in migrations | `grep -rn "ON CONFLICT (organization_id" crates --include="*.rs"`; `crates/core/migrations/*.sql` (0001–0035) |
| §13 / P1-1 / §22-P0-4 remote custody | Vault (transit) and AWS KMS (Ed25519 SigV4, verified against the AWS test vector) real adapters, provider registry, fail-closed HSM refusal, key rotation | `crates/server/src/custody/{vault,kms}/`, `custody_rotation_{missing_profile,cross_tenant,profile_resolution}.rs` (17/0 with batch6 on fresh PG schema, 2026-09-30), `docs/CUSTODY-STATUS-2026.md` |
| §14 / P1-2 / §22-P1-8 authoritative billing | Authoritative plan/usage/payment state; no synthetic status in the production path; Stripe + Paddle adapters with idempotency | `crates/server/src/billing/` (state machine, `stripe_adapter.rs`, `paddle_adapter.rs`), billing suites 39/0; `docs/BILLING-STATUS-2026.md` |
| §15 / P1-3 / §22-P1-7 customer trading dashboard | `trading_data_plane` HTTP service with per-tenant views: orders, positions, executions, bots, copy, polymarket, sniper, telegram, recovery, module controls, authorization chain | `crates/server/src/trading_data_plane/` (13 files); `docs/CUSTOMER-SaaS-STATUS-2026.md` |
| §16 / P0-5 / §22-P0-5 Polymarket current protocol | V3 position-backed orders (`position_orders.rs`, `exchange_v3.rs`), protocol selection, `trade_ids` wire parsing + async lookup/backfill, V2 regression kept | `crates/module-polymarket/src/{position_orders,exchange_v3,async_commit}.rs` (`trade_ids` at `async_commit.rs:146`); polymarket suites 204/0 |
| §22-P1-9 real Stripe/Paddle flows | Live-gated contract tests with explicit `LIVE_BILLING=1`, honest NOT_RUN evidence records | `evidence/external/billing_stripe.json`, `crates/server/tests/live_billing_contract.rs` |
| §24.2 banned claims discipline | Machine-enforced claim gate over the marketing-facing files; regression test plants a banned phrase and requires rejection | `scripts/verify-marketing-claims.sh`, `tests/release/marketing_claims.sh` (PASS 2026-10-01) |

## 3. OPEN — buildable in code (candidates for a future prompt)

Each row cites the current in-tree evidence that the item is still open.

| # | Audit ask (section) | Current evidence it is open | Scope class |
|---|---|---|---|
| A | §20.1 audit-attestation terminology: separate actual HMAC verification from future asymmetric/detached signatures in the public data model | `crates/server/src/ops/audit_attestation.rs:19` still reads "hex HMAC-SHA256 or detached signature placeholder (deterministic)"; `:48–49` "In production this would be a real asymmetric signature; here we use HMAC" | Small (comment/API-doc restructure, or a `SignatureKind` distinction + tests) |
| B | §20.2 zeroize for config-held `String` secrets (hardened-build secret lifecycle) | `zeroize` appears in no `crates/*/Cargo.toml`; no `Zeroize` use in non-test sources (Solana `Keypair` path already zeroizes internally) | Small–medium |
| C | §19 strategy registry / versioned strategies / dry-run / replay / per-tenant ownership | `crates/module-polymarket/src/strategy.rs:150` — "The configured strategy name is not implemented." (the exact line the audit flagged) | Medium (audit itself ranks P1/P2, after tenant data-plane and custody — both now closed) |
| D | §17 / §22-P1-13 latency + landing-rate evidence harness and dashboard (p50/p95 detection/build/sign/submit/landing, landed-vs-expired-vs-dropped, by region/tip band) | No latency-measurement surface in `crates/`; the only mentions are the honest gap rows in `docs/BUSINESS-MATRIX-2026.md`, `docs/CAPABILITY-MATRIX.md`, `docs/EVIDENCE-INDEX.md` | Medium for the in-process measurement harness + dashboard; the real numbers still require funded infrastructure (see §4) |
| E | §22-P1-14 customer alerts + operational SLOs | No `alert_rule`/customer-alert surface in `crates/` | Medium |
| F | §22-P1-15 mobile/PWA customer surface | No PWA assets/manifest in the tree | Medium |
| G | §6.4 broader direct Raydium routes (CPMM, CLMM, LaunchLab) beyond AMM v4 (+ Jupiter routing already present) | `EntryRoute` implements direct Raydium AMM v4; other Raydium products route via Jupiter | Medium–large (audit frames as optional breadth) |
| H | §7.3 / §18.1–18.2 / §22-P2-16,17,18 copy-discovery and analytics surface (wallet analyzer, wallet radar, smart-money leaderboard, token audit/risk discovery UI) | No `wallet_analyzer`/`smart_money`/discovery code in `crates/` (grep 2026-10-01: no matches) | Large (audit rates this scope-breadth vs Solana-terminal competitors, 25–40% product-surface) |
| I | §22-P2-19,20 backtesting, strategy catalog/marketplace | No backtest engine in `crates/` (overlaps with C) | Large |
| J | §22-P2-21,22,23,24 multi-exchange, white-label/reseller, affiliate/referral, support tooling | None of these surfaces exist; they are recorded as out-of-scope breadth, not defects (`docs/CAPABILITY-MATRIX.md`) | Large; sell-as-scope decisions |
| K | §10.2 Telegram as a fuller trading surface — note: core control + portfolio reads (Status/Positions/Trades/PnL/Balance with role gating) and per-tenant module instances ARE present; the residue is copy-task UX and discovery/analytics alerts inside Telegram, which overlap H | `crates/module-telegram/src/commands.rs:122–136` (existing role-gated command set); no discovery/analytics commands | Medium (subset of H) |

## 4. OPEN — not closable by code alone (tracked, honest, fail-closed)

These stay exactly as the gap ledger records them; none is claimed done:

| Audit ask (section) | Tracking |
|---|---|
| §17 / P1-5 / §22-P1-12 funded live proof + measured latency | `GAP-004` (`docs/FINAL-BUYER-GAP-LEDGER.md`, `evidence/external/funded-preflight_funded.json` — `status: NOT_RUN`, `live_funded: false`; guard proves default-never-funded) |
| §20.3 / §22-P1-10 independent external audit | `GAP-006` (external auditor deliverable; internal review only) |
| §9.2 / §21.4 / §22-P1-11 staking deployment identity | placeholder program id `3vEEMM…` maps to BLOCKED, never PASS (`docs/STAKING-PROGRAM-ID-VALIDATION.md` documents the guarded placeholder→final path; deploy authority intentionally not committed) |
| §21.1 LICENSE holder | `LICENSE` handover note (transferring party must substitute the legal entity) |
| §21.2 repository URL | `docs/IP-OWNERSHIP-REGISTER.md:31` — LEGAL_REVIEW_REQUIRED |
| §21.3 third-party UNKNOWN license | `licenses.csv:416` (`solana-config-program-client 0.0.2`) + register row — LEGAL_REVIEW_REQUIRED |
| §21.5 branding/trademark/domain | `docs/IP-OWNERSHIP-REGISTER.md` (explicitly not included) |
| Vault/KMS/HSM live round-trips | `GAP-002` (adapters real + unit-tested; live validation EXTERNAL_REQUIRED; HSM fail-closed unimplemented) |
| Deployment smoke against production | `GAP-003` (BUYER_ACTION) |

## 5. Gates re-verified on census day (2026-10-01)

- `tests/release/manifest_current.sh` — PASS
- `tests/release/buyer_parity.sh` — PASS
- `scripts/verify-buyer-package.sh` — PASS
- `scripts/forensic-sql-scan.sh` — PASS (0 class-4)
- `tests/business/business-matrix-completeness.sh` — PASS (7×11, machine-matched)
- `tests/release/marketing_claims.sh` — PASS (real tree clean; gate proven to reject)

Rust toolchain gates (fmt / check / clippy `-D warnings`) and the full
1,740-test workspace suite were last run green on 2026-09-30 after the
PROMPT 6 fixes (`docs/FINAL-16-SECTION-RESULT-2026.md` §16 records the
link-time OOM + swapfile environment note); no Rust source changed since,
so those results remain the current evidence for the unchanged code.

## 6. Suggested ordering for the next work prompt (no scope invented here)

1. A + B (small, security-adjacent, quick verifiable wins);
2. C (strategy registry — the audit's own "after custody/tenant" sequencing is
   now satisfied);
3. D-harness + E (measurement and alerting build on the trading data plane);
4. F, G, H, I, J are product-breadth decisions best taken with the buyer's
   positioning choice (`docs/BUSINESS-MATRIX-2026.md` pricing rows).

## 7. Determinism notes

- This census adds one file to the product tree; the release manifest,
  buyer package, checksums and the count-bearing docs were refreshed by the
  standard cycle (`update-current-audit.sh` → `update-release-manifest.sh` →
  `rebuild-buyer-release.sh`) immediately after this file was written, and
  the §5 gates re-run.
- Statuses use the ledger vocabulary (OPEN / EXTERNAL_REQUIRED /
  BUYER_ACTION / SELLER_ACTION-equivalent legal rows) and never mark an
  external item closed.
