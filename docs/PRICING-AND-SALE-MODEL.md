# PRICING-AND-SALE-MODEL.md

**Not legal advice. Not financial advice.** This document frames the three
commercial structures available for this codebase and maps the *existing,
auditable evidence* to the claims each structure can support. Pricing figures
are left as bracketed placeholders for the seller to set with counsel and the
buyer; no number in this file is a valuation.

Aligned with: `legal/SOURCE-CODE-BILL-OF-SALE.md`,
`legal/IP-ASSIGNMENT-TEMPLATE.md`, `legal/MAINTENANCE-AND-SUPPORT-TERMS.md`,
`legal/REGULATORY-CHECKLIST.md`, `docs/COMPETITOR-BENCHMARK.md`,
`docs/TRANSACTION-READINESS-REPORT.md`.

---

## 1. The three structures

### A. Full source sale + IP assignment
Buyer receives the repository, build pipeline, and all IP via
`legal/IP-ASSIGNMENT-TEMPLATE.md` + `legal/SOURCE-CODE-BILL-OF-SALE.md`
(templates for counsel — not legal advice).

- **What the buyer gets:** exclusivity potential, the fee stream (per-trade
  platform fees are coded but their collection wiring is a deployment task),
  freedom to rebrand (P2 branding hooks are scoped in the GAP MAP).
- **What supports the price:** code volume and test coverage (see
  `scripts/generate-stats.sh` output in `docs/STATS.md`), the
  audit rounds in `docs/archive/`, and the frozen gate status — all
  reproducible by the buyer from the delivered tree.
- **What does NOT support any premium yet:** the items in
  `evidence/live/` are all `NOT_RUN` (no funded mainnet run, no external
  audit PDF). Any price band justified by "proven in production" requires
  those files to flip to PASSED first.

### B. Licence (per-deployment or per-seat, source-available)
Seller retains IP; buyer gets build/run rights under a commercial licence.

- **Supports:** recurring-revenue structures; multiple concurrent buyers.
- **Requires (before signing):** the proprietary `LICENSE` at repo root
  defines the grant; counsel completes
  `legal/MAINTENANCE-AND-SUPPORT-TERMS.md` (SLA table §4 is illustrative —
  bracketed) and `legal/REGULATORY-CHECKLIST.md` for the buyer's markets.
- **Risk to price in:** protocol drift is a live cost. The seller's exposure
  is defined by `docs/PROTOCOL-DRIFT-RUNBOOK.md`; licence pricing should
  carry that support obligation.

### C. Hosted SaaS (seller operates it)
Buyer rents capacity; the SaaS layer in this repo (organizations, plans,
subscriptions, entitlements, usage events — migrations 0017+) is the delivery
vehicle.

- **Supports:** the lowest barrier for the buyer; the seller keeps the
  evidence-generation loop running (live runs accrue to the seller).
- **Requires:** the NOT_RUN evidence categories that matter for hosted use —
  `deployment_smoke`, `ci_run`, billing (`stripe_checkout_roundtrip`),
  custody signing (`kms_sign_transit`, `vault_transit`) — plus regulatory
  review per `legal/REGULATORY-CHECKLIST.md` (money-transmission and
  securities questions are open until counsel answers them for each target
  jurisdiction).

## 2. Evidence → price-band mapping

The rule: **a claim may support a price band only if its evidence file is
PASSED.** Today, every row below is NOT_RUN, so no live-performance band is
currently supportable. This table is the checklist that unlocks them.

| Claim category | Evidence file(s) | Status | Price-band effect when PASSED |
|---|---|---|---|
| Builds, boots, migrates cleanly | `deployment_smoke.json`, `ci_run.json` | NOT_RUN | Baseline "it works" band only |
| Solana execution lands real txs | `solana_funded_preflight.json` | NOT_RUN | Unlocks execution-competence band |
| Venue round-trips (pump.fun, PumpSwap, Polymarket) | `pumpfun_buy_sell_roundtrip.json`, `pumpswap_buy_sell_roundtrip.json`, `polymarket_order_roundtrip.json` | NOT_RUN | Unlocks venue-coverage band |
| Billing pipeline (Stripe test mode) | `stripe_checkout_roundtrip.json` | NOT_RUN | Required for structure C at all |
| Custody integrations (KMS, Vault) | `kms_sign_transit.json`, `vault_transit.json` | NOT_RUN | Unlocks institutional-custody band |
| Staking program on devnet | `staking_devnet_e2e.json` | NOT_RUN | Unlocks Module-4 add-on band |
| Latency/landing performance | `latency_report.json` + `docs/LATENCY-AND-LANDING-REPORT.md` | NOT_RUN | The performance-premium band; publication rule in the latency report §6 |
| External security review | `evidence/audits/*.pdf` (EXTERNAL) | NOT ENGAGED | Institutional-band prerequisite |

**Honesty gate:** `scripts/verify-marketing-claims.sh` fails CI if any
buyer-facing document asserts a category above without the PASSED evidence —
the same gate protects the sales conversation from outrunning the repo.

## 3. Market reference points (from COMPETITOR-BENCHMARK.md, sourced)

- Single-user competitors monetise at **0.5–1% per trade** (S1–S6 there);
  rebate ladders are the retention lever. A source sale is priced against
  *ownership of that fee stream*, not against the subscription price of a
  hosted bot.
- Polymarket copy-tooling is an actively monetised category (PolyCop,
  Polycopy); our matching modules are P2 roadmap — a buyer pricing the
  Polymarket copy capability must price it as roadmap, not as shipping
  software, until those modules exist.
- No competitor publishes audited latency; a measured latency report
  (NOT_RUN today) would be a differentiator precisely because the bar is
  "any honest measurement", not a heroic number.

## 4. Decision checklist for the seller

1. Choose structure A/B/C; have counsel complete the matching `legal/`
   templates (all are templates, none are advice).
2. Run `bash tests/release/stats_current.sh` and attach
   `docs/STATS.md` to the data room — machine-generated, no
   hand-typed figures.
3. Decide which evidence rows in §2 will be executed BEFORE closing; budget
   real funds and provider credentials for them (they cannot be simulated).
4. Fill bracketed SLA/pricing values in the legal docs; the runbook
   (PROTOCOL-DRIFT-RUNBOOK.md) defines the support scope those values buy.
5. Re-run the full gate (`scripts/verify-marketing-claims.sh`,
   `scripts/check-openapi-coverage.sh`, CI) on the final commit and attach
   the green run per `evidence/live/ci_run.json`.
