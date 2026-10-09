# Current Marketing Claims — 2026 (evidence-gated)

> Rewritten 2026-10-07 per GAP-MAP v2: every claim without a PASSED
> `evidence/live/*.json` file has been DROPPED. The duplicate
> `docs/MARKETING-CLAIMS.md` has been deleted. The enforcement point is
> `scripts/verify-marketing-claims.sh` (fails on VERIFIED / 100% / HSM /
> FIPS / SOC2 / sub-millisecond without backing evidence). The single
> claims table lives in `docs/COMMERCIAL-CLAIM-AUDIT.md`.

## Claims currently permitted

Only the following may be used in buyer-facing material today. Each is a
statement about the shipped source that a buyer can verify by reading or
running the code — not a performance promise.

1. **"Multi-strategy trading suite for Solana launch sniping, copy trading,
   and Polymarket prediction markets, with a multi-tenant control plane."**
   — describes the shipped modules (`crates/module-{sniper,copy,polymarket,telegram}`,
   `crates/server`, `apps/control-plane`).
2. **"Static security review completed across the execution, accounting,
   HA-ownership, global-risk, and tenant data-plane layers; findings and
   fixes documented per round."** — `AUDIT-ROUND-{3..7}-2026-10-07.md`.
   Must always be qualified with: *static review only; the workspace has no
   compiler available in the review environment*.
3. **"On-chain staking program with immutable supply cap, timelocked
   parameter changes, two-step admin transfer, and pause that never blocks
   withdrawals."** — readable in `programs/staking-suite/src/`.
4. **"Tenant authorization chain: every trading-data-plane route is
   authenticate → organization → plane → lifecycle → entitlement → module
   family → per-action permission, fail-closed."** — readable in
   `crates/server/src/trading_data_plane/authorization_chain.rs`.
5. **"Claims gate: marketing terms fail CI unless backed by PASSED evidence
   files."** — `scripts/verify-marketing-claims.sh`.

## Claims DROPPED until evidence exists

- Any latency figure (detect→submit→landed) — needs `evidence/live/latency_report.json`.
- Any funded-trade success rate, PnL, or landing rate — needs the funded
  round-trip evidence files in `docs/COMMERCIAL-CLAIM-AUDIT.md` §1.
- Billing "works with Stripe" as a live claim — needs
  `evidence/live/billing_stripe_test_mode.json` (test-mode evidence permits
  only "Stripe test-mode checkout verified").
- KMS/Vault "HSM-backed signing" as a product claim — the code supports
  KMS/Vault/Hsm providers (code fact), but no live signing evidence exists;
  never describe this as a certification.
- SOC 2, FIPS, "audited" — never; no such evidence can exist yet
  (`evidence/audits/` empty).

## Process

A new claim enters this list only when:
1. an `evidence/live/*.json` (or a re-runnable test) exists with
   `"status": "PASSED"` and identifiers (tx signature / object id / run URL);
2. `scripts/verify-marketing-claims.sh` passes;
3. this file and `docs/COMMERCIAL-CLAIM-AUDIT.md` are updated in the same
   commit.
