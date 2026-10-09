# BUSINESS MATRIX 2026 (2026-10-01)

EVIDENCE-LEVEL: CODE

Seven business lines × eleven columns, exactly as the PROMPT 6 spec
requires. The **Current completeness %** is machine-measured by
`scripts/generate-business-matrix.sh`: each line's PRESENT capabilities
(spec-listed) are markers the script verifies in the tree — a buyer can
grep every one. 100% means every PRESENT capability is implemented; it
does NOT mean live-proven — the Real-missing-capability, Evidence, and
P0/P1/P2 columns carry that honesty. No subjective "best/worst"
labels anywhere.

## The full business matrix (11 columns, spec-exact)

| Module/Area | Current implementation | Current completeness % | Real missing capability | Competitor/product gap | Evidence | Buyer impact | P0/P1/P2 | Safe marketing claim | Unsafe marketing claim | Next closure |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Sniper | Pump.fun + PumpPortal + Geyser + logs detection; PumpSwap + Raydium AMM v4 + Jupiter routing; priority fee; Jito; deterministic entry/exit; paper + live modes | 100% (9/9 present-capability markers) | Landing-rate proof; latency benchmark evidence; broader direct Raydium protocol coverage | Commercial sniper products ship landing-rate dashboards and measured latency; this tree has neither measurement | UNIT_TEST (72 module test fns) | Core revenue engine works and is tested, but buyers cannot yet show investors a measured landing rate | P0: no live/funded proof. P1: landing-rate + latency evidence. P2: more direct Raydium protocols | deterministic entry/exit detection across Pump.fun/PumpPortal/Geyser feeds with PumpSwap/Raydium/Jupiter routing, priority fees and Jito; unit-tested (no landing-rate or latency proof) | guaranteed 1-second landing (no landing-rate proof exists); profitable / risk-free | Run a funded pilot with landing/latency telemetry and publish the measured rates |
| Copy Trading | Tracked-wallet copying; tenant execution; tenant-local dedup; exit handling; mirror/recovery engine; event ordering | 100% (6/6) | Wallet analytics; wallet discovery; smart-money radar; advanced UI; mobile workflow | Copy-trading SaaS products (e.g. wallet-tracking platforms) sell analytics/discovery/radar as their differentiator; this tree copies but does not analyze | UNIT_TEST + INTEGRATION_TEST (60 module fns + server copy suites) | The mirroring core is solid; the analytics layer competitors monetize is absent | P0: no live proof. P1: — . P2: analytics/discovery/radar/UI/mobile | tracked-wallet copying with tenant execution, tenant-local dedup, exit handling and crash recovery; unit + integration tested (no live proof) | guaranteed fill parity / zero-loss mirroring (no live proof) | Add wallet analytics + discovery surface on the existing event pipeline (P2 roadmap) |
| Polymarket | CLOB REST; Gamma discovery; WebSocket; L1/L2 market data; V2 order domain (EIP-712); explicit V3 position orders; async commit/resolution/reconciliation lifecycle | 100% (6/6) | Live market-data feed operation (implemented client, never run live); per-endpoint live verification | Competing Polymarket bots advertise live-verified uptime; this tree's compatibility is per-endpoint and fixture/integration-tested only | UNIT_TEST + INTEGRATION_TEST (136 module fns + PG suites) | Both order domains and the full async lifecycle are implemented and tested; buyers must still verify endpoints live before funding | P0: no live/funded proof. P1: live endpoint verification. P2: — | CLOB + Gamma + WebSocket + L1/L2 data, V2 order domain and explicit V3 position orders with async lifecycle and reconciliation; unit + PostgreSQL integration tested (no live or funded proof) | all-venues compatibility / exchange-uptime guarantee (compatibility is per-endpoint); latest-V3 claims beyond the implemented domain | Live-test each used endpoint against the CLOB sandbox, then a funded micro-pilot |
| Staking/Token/Fee | Native Solana program: instruction validation; rewards; fee/admin flows; guarded identity + deploy tooling | 100% (5/5) | Actual deployed program ID; deployment evidence; external audit | Competing staking programs are mainnet-deployed with public audits; this program is pre-deployment with a placeholder id | UNIT_TEST (73 program test fns); CODE (identity tooling) | Buyers get full program logic but must generate the final keypair, deploy, and commission an audit themselves | P0: not deployed (by design). P1: deployment + evidence. P2: — | native Solana program with validation, rewards and fee/admin flows, unit-tested; pre-deployment placeholder id with guarded set-id/deploy tooling (not deployed, not audited) | audited / mainnet-live (no external audit, not deployed) | Buyer generates final keypair → `staking-identity.sh set-id` → guarded `deploy` → commission audit |
| Telegram | Control (commands); status alerts; RBAC through the authorization chain; per-tenant binding API (bind/unbind, integration-tested) | 100% (4/4) | Per-tenant OUTBOUND routing: the deployment forwarder routes to the deployment alert chat (verified in source — the binding API stores per-tenant chat ids; the forwarder does not yet dispatch per tenant) | Competing SaaS products deliver per-tenant notifications; this tree stops at the binding API | UNIT_TEST + INTEGRATION_TEST (21 module fns + binding suites) | Alerts/commands work deployment-wide; per-tenant notification is a documented P2 away | P0: — . P1: — . P2: per-tenant forwarder routing | control, status and RBAC surface with per-tenant binding API (integration-tested); the forwarder routes to the deployment alert chat | per-tenant message routing is production-complete (forwarder is deployment-level) | Extend the forwarder to dispatch via the stored per-tenant bindings (small, bounded change) |
| SaaS | Tenant identity; runtime registry (generation/fencing); execution guard; customer API (documented endpoints, one authorization chain); billing state machine (Stripe/Paddle idempotent); custody foundation (Vault/KMS wire-tested) | 100% (6/6) | UI shipped 2026-09-30 and buyer parity is byte-exact (both former gaps CLOSED this cycle); still missing: external custody live round-trip, production evidence | Enterprise SaaS competitors hold external audits and live payment/custody operations; this tree is integration-tested only | INTEGRATION_TEST (328 saas-plane test fns, PostgreSQL 17) | The full multi-tenant plane is test-proven; enterprise buyers must add external audit + live providers | P0: no live providers. P1: external audit. P2: — | multi-tenant control plane: tenant identity, runtime registry, execution guard, customer API, billing state machine, custody foundation — integration-tested against PostgreSQL | SOC2 / externally audited (no external audit commissioned) | Commission the external audit (runbook: docs/PENETRATION-TEST-READINESS.md); wire live Stripe/Paddle + Vault/KMS sandbox credentials |
| BUSINESS / Commercial | Release integrity (parity+manifest+contamination+version); docs set (122); support handover; IP register + handover checklist; licensing (SBOM + license report + compliance doc); external-audit status doc; deployment docs; SLO/DR (backup-restore, rollback, incident runbooks); buyer acceptance test; evidence index | 89% (8/9 topic coverage markers — `docs/BUYER-ACCEPTANCE-TEST.md` archived 2026-10-07; regenerated 2026-10-08 by `scripts/generate-business-matrix.sh`) | No external audit; no live production evidence; no SLA document; no automated CI wiring (local-equivalence documented) | Competing listings sell with third-party audit letters and SLA contracts; this package's verification is mechanical and self-run | CODE (mechanical gates: 6 release tests, forensic SQL gate, claim gate) | Buyers can verify every byte themselves (checksums, parity, SBOM) but must bring their own audit and SLA posture | P0: — . P1: external audit. P2: SLA document; CI wiring | verifiable release package: parity, manifest, SBOM, license report, marketing-claim rejection, forensic SQL gate; every commercial topic covered by a document | zero-defect / fully-parity-verified live operation (verification is static + test-level, not live) | Commission audit; add the buyer's SLA policy; wire the local gates into the buyer's CI |

## The measured matrix (machine-generated snapshot, regenerated 2026-10-08)

Completeness = present required-capability markers ÷ required markers,
measured by `scripts/generate-business-matrix.sh`; the regression gate
`tests/business/business-matrix-completeness.sh` fails the release if
this doc's percentages drift from a fresh measurement.

| Business line | Required markers | Present | Completeness % | Test fns (module paths) | Safe claim | Unsafe claim (do not make) |
| --- | --- | --- | --- | --- | --- | --- |
| Sniper | 9 | 9 | 100% | 170 | deterministic entry/exit detection across Pump.fun/PumpPortal/Geyser feeds with PumpSwap/Raydium/Jupiter routing, priority fees and Jito; unit-tested (no landing-rate or latency proof) | guaranteed 1-second landing (no landing-rate proof exists); profitable / risk-free |
| Copy Trading | 6 | 6 | 100% | 60 | tracked-wallet copying with tenant execution, tenant-local dedup, exit handling and crash recovery; unit + integration tested (no live proof) | guaranteed fill parity / zero-loss mirroring (no live proof) |
| Polymarket | 6 | 6 | 100% | 159 | CLOB + Gamma + WebSocket + L1/L2 data, V2 order domain and explicit V3 position orders with async lifecycle and reconciliation; unit + PostgreSQL integration tested (no live or funded proof) | all-venues compatibility / exchange-uptime guarantee (compatibility is per-endpoint); latest-V3 claims beyond the implemented domain |
| Staking/Token/Fee | 5 | 5 | 100% | 81 | native Solana program with validation, rewards and fee/admin flows, unit-tested; pre-deployment placeholder id with guarded set-id/deploy tooling (not deployed, not audited) | audited / mainnet-live (no external audit, not deployed) |
| Telegram | 4 | 4 | 100% | 42 | control, status and RBAC surface with per-tenant binding API (integration-tested); the forwarder routes to the deployment alert chat | per-tenant message routing is production-complete (forwarder is deployment-level) |
| SaaS | 6 | 6 | 100% | 431 | multi-tenant control plane: tenant identity, runtime registry, execution guard, customer API, billing state machine, custody foundation — integration-tested against PostgreSQL | SOC2 / externally audited (no external audit commissioned) |
| BUSINESS / Commercial | 9 | 8 | 89% | 17 | verifiable release package: parity, manifest, SBOM, license report, marketing-claim rejection, forensic SQL gate; every commercial topic covered by a document | zero-defect / fully-parity-verified live operation (verification is static + test-level, not live) |

Missing markers by line (the honest gaps):
  Sniper: none
  Copy Trading: none
  Polymarket: none
  Staking/Token/Fee: none
  Telegram: none
  SaaS: none
  BUSINESS / Commercial: docs/BUYER-ACCEPTANCE-TEST.md

## What "completeness %" means — and does not mean

The percentage measures PRESENCE of each line's PRESENT capabilities
(spec-listed, grep-verifiable markers). It does not mean live-proven:
every line's evidence is capped at what exists in this repository
(UNIT_TEST / INTEGRATION_TEST / mechanical CODE gates), and the
Real-missing-capability and P0 columns state plainly that no line has
live or funded evidence. A 100% row with a P0 "never live-tested" gap
is exactly what this matrix is designed to show together.

## Claim governance

* Safe claims are copied verbatim from the generator's validated
  vocabulary (banned-phrase-checked at generation time; parenthetical
  negations like "no live proof" are the honest form).
* The unsafe-claims column intentionally NAMES banned phrases — that is
  its job — which is why `scripts/verify-marketing-claims.sh` exempts
  this document's matrix sections and
  `tests/business/business-matrix-completeness.sh` checks the SAFE
  column column-aware instead.
* The claim vocabulary's canonical registry is
  `docs/CURRENT-MARKETING-CLAIMS-2026.md` (the duplicate
  `docs/MARKETING-CLAIMS.md` was deleted 2026-10-08 per GAP-MAP v2).

## Re-verification

```bash
scripts/generate-business-matrix.sh          # live percentages
tests/business/business-matrix-completeness.sh
```
