# COMPETITOR-BENCHMARK.md

- **Date:** 2026-10-08
- **Purpose:** dated, sourced feature/fee/custody benchmark of the commercial
  products this codebase competes with, plus an honest positioning section.
  Replaces `COMMERCIAL-MARKET-BENCHMARK.md` (archived under `docs/archive/`),
  which mixed vendor descriptions with unverified superiority claims.
- **Honesty rules applied here:** every third-party fact carries its source
  and retrieval date; every statement about *this* product is limited to what
  exists in this tree with passing tests or explicitly documented static
  review. No latency numbers are quoted for either side unless measured —
  none have been measured yet (see `docs/LATENCY-AND-LANDING-REPORT.md`,
  status NOT_RUN).

---

## 1. Sources consulted (retrieved 2026-10-08)

| # | Source | Used for |
|---|---|---|
| S1 | solanatools.io — "Solana Trading Bot Fees Compared 2026" (2026-03-02) | fee tiers: Photon 0.5%, Trojan 1.0%, Banana Gun 0.5–1.0% |
| S2 | memegateway.com — "Solana Trading Bot Fees Compared (2026)" (2026-05-15) | Trojan 0.9–1.0% + Arena cashback; Banana Gun revenue share |
| S3 | mayhemcode.com — "Best Telegram Bot for Copy Trading" (2026-10-04) | Trojan/GMGN/Banana Gun chains, fees, copy features |
| S4 | axltoken.com — "GMGN vs Axiom" (2026-09-27) | GMGN vs Axiom features, custody (GMGN platform-managed vs Axiom/Turnkey non-custodial) |
| S5 | solanabox.tools — "GMGN.ai" tool page | GMGN 1% per successful transaction + Anti-MEV priority-fee minimum |
| S6 | telegramtrading.net — "GMGN vs Photon vs Axiom" (2026-04-10) | Axiom tiered 0.95%→0.75%, Photon JITO MEV protection |
| S7 | athenaalpha.xyz — "Best Solana Trading Terminal in 2026" (2026-06-29) | GMGN 2FA-before-withdrawal custody note; feature gaps per platform |
| S8 | polycop.systems — POLYCOP product page | PolyCop model: top-500 leaderboard scan, 5s polling, non-custodial claim |
| S9 | polycopy.app — "Polymarket Copy Trading" (2026-09-07) | Polycopy leaderboard (500K+ wallets), Copy Score, Auto Copy rules |
| S10 | polymarket101.com — whale-tracker guide (2026-04-11) | PolyCop positioning as Telegram copy bot over tracker data |

Third-party facts below are quoted from these sources; they describe the
vendors' *published* behaviour on the source date and may have changed since.
We have not independently audited any competitor.

## 2. Solana memecoin execution (Telegram + web terminals)

| Product | Interface | Published fee (source) | Copy trading | Custody model (source) |
|---|---|---|---|---|
| **Trojan** | Telegram + web terminal | 1.0% base; 0.9% via referral; tier rebates down to ~0.5% (S1, S2, S3) | Included; limit orders, DCA (S3) | Hot keys held by the bot service (standard Telegram-bot model) |
| **Photon (Sol)** | Web terminal | 0.5%–1.0% depending on source/date (S1: 0.5%; S6: ~1%) | N/A (S1) | Browser-session wallets; JITO MEV protection advertised (S6) |
| **GMGN** | Web + Telegram + PWA | 1% per successful transaction; referral can lower effective fee (S3, S5) | Automated copy of up to ~10 wallets (S4, S5) | Platform-managed wallets unless user connects an extension wallet; 2FA required before withdrawal (S4, S7) |
| **Axiom (Trade/Pro)** | Web terminal | 0.95%→0.75% volume tiers + SOL cashback (S6); 1% base with cashback (S4) | Per S4: no automated copy (alerts only); S6 lists copy trading — sources disagree, treat as unverified | Non-custodial via Turnkey; importable recovery phrase (S4) |
| **Banana Gun** | Telegram + Banana Pro web | 0.5–1.0% by order type/chain (S1, S3) | Yes, detailed settings; multi-chain (S3) | Hot keys on bot infrastructure (Telegram-bot model); $BANANA revenue share (S2) |

Shared cost context (S2): on top of platform fees, every trade pays Solana
base fees, priority fees (~0.0005–0.002 SOL), optional Jito tips, plus
slippage and MEV exposure. Round-trip platform-fee cost across these products
is roughly 1–2% of notional before any rebates.

## 3. Polymarket copy tools

| Product | Model (source) | Notes |
|---|---|---|
| **PolyCop** (polycop.systems) | Autonomous Telegram bot: scans top-500 leaderboard wallets, polls active ones every ~5 s, filters (edge/price/age/min size), copies buys and exits; simulation mode available; claims "100% non-custodial" (S8) | The non-custodial claim is vendor-published; copying requires the tool to hold order-signing capability, which we have not verified |
| **Polycopy** (polycopy.app) | Leaderboard of 500K+ tracked wallets, Copy Score, follow feed, Auto Copy with user rules (size/budget/categories) (S9) | Discovery + rules layer over on-chain fills |
| **Polyfollow-class trackers** | Whale/smart-money trackers (e.g. Polywhaler, PolymarketScan) used to find wallets, then fed into copy bots (S10) | Analytics, not execution |

## 4. This codebase vs. the field (implemented features only)

Claims here are limited to what exists in this repository. Verification
status per the project rules: items marked **tested** are covered by
passing tests in the freeze gate; items marked **static review** compile-path
reviewed but not executed in this workspace; items marked **not built** are
stated so nobody mistakes them for features.

| Capability | Status in this codebase | Competitor reference point |
|---|---|---|
| pump.fun / PumpSwap / Raydium AMM v4 execution (paper/simulate/live-gated) | Implemented; IDL-pinned layout checks + drift tests (static review) | Trojan/GMGN/Axiom all trade these venues; their detection/execution latency is unpublished and unmeasured here |
| Launch gating (mint/freeze authority, liquidity, holder checks, bundler heuristics) | Implemented (`module-sniper/src/gates.rs`, tested) | Axiom "Pulse" filters and GMGN GoPlus audits are the closest analogues (S4, S6) |
| Backtesting engine with fill model | Implemented with 56 synthetic fixtures + golden tests (tested); **real recorded launch dataset: NOT captured yet** (see LATENCY report) | S7 notes GMGN has no autosell; none of S1–S7 document a public replay backtester |
| Polymarket CLOB integration (orders, EIP-712 signing, reconciliation) | Implemented (tested); copy/leader/builder modules are P2 roadmap, **not built** | PolyCop/Polycopy already sell copy features (S8, S9) — we do not, yet |
| Multi-tenant SaaS, org-scoped RBAC, billing | Implemented (tested) | Competitors above are single-user tools; none of the sourced pages document multi-tenant org isolation |
| KMS/Vault signing profiles for tenant custody | Implemented in code (static review); live KMS/Vault runs are NOT_RUN in `evidence/` | Axiom advertises Turnkey non-custody (S4); Telegram bots hold hot keys |
| Measured detect→landed latency | **NOT_RUN** — no measurements exist; see LATENCY-AND-LANDING-REPORT.md | Vendors publish no audited latency either (claims like "<400 ms" in S4 are vendor-stated) |
| Per-trade platform fee | Policy + accounting implemented (`billing/platform_fee.rs`, tested); ledger persistence = migration 0049 | Competitor platform fees: 0.5–1% (S1–S6) |

## 5. Pricing-positioning takeaway (facts, not advice)

- The market clears at **~0.5–1% per trade** for single-user tools, with
  rebates as the differentiation lever (S1–S6). A source-code sale of this
  codebase competes on *ownership of the fee stream*, not on undercutting it;
  see `docs/PRICING-AND-SALE-MODEL.md`.
- Copy-tooling for Polymarket is a live, monetised category (S8, S9); our
  equivalent modules are roadmap (P2), and this document must say so until
  they ship.
- Any latency-superiority claim requires the measurement program in
  `docs/LATENCY-AND-LANDING-REPORT.md` to move from NOT_RUN to PASSED first.
  Until then, the only defensible statements are feature-parity statements
  backed by the table in §4.
