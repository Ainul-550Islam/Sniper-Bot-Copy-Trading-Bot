# Audit scope — external review of Sniper Suite

Status: **no external audit has been performed yet.** This document defines what an
independent reviewer would be asked to examine. Findings are tracked in the table
at the bottom; nothing here is a claim that a review passed.

## 1. In scope

| Area | Path | Notes |
|---|---|---|
| Control-plane API | `crates/server/src/` (REST, auth chain, tenant scoping) | Axum 0.7, tenant-scoped SQL |
| Trading data plane | `crates/server/src/trading_data_plane/` | backtests, copy, polymarket, strategies |
| Sniper engine | `crates/module-sniper/` | risk gates, entry routing, limit orders, DCA |
| Solana adapters | `crates/solana-kit/` | transaction building, venue adapters |
| Copy trading | `crates/module-copy/` | leader feeds, discovery read model |
| Polymarket | `crates/module-polymarket/` | CLOB signing, strategy registry |
| Telegram control | `crates/module-telegram/` | role-gated commands, alert prefs |
| Secrets handling | `crates/core/src/config.rs`, `crates/core/src/secret.rs` | SecretString redaction |
| Staking program | `programs/staking-suite/` | native Solana program; threat model in `audit/THREAT-MODEL.md`, invariants in `audit/INVARIANTS.md` |
| Control-plane UI | `apps/control-plane/` | Next.js; no fake-data production literals |
| Schema | `crates/core/migrations/` | 0001–0058, contiguous, never edit applied files |

## 2. Out of scope

- Third-party services (Solana RPC providers, Jupiter, Polymarket, Stripe/Paddle, Telegram) — behaviour is assumed, not audited.
- Customer-operated infrastructure (DNS, load balancers, host OS hardening) except where the deployment docs state a requirement.
- Market-risk and profitability of any strategy. Backtests are synthetic unless a recorded dataset is attached; they are not evidence of performance.
- Mainnet funded execution. It has not been generally tested and requires explicit operator approval.

## 3. Commit under review

- Commit hash: **to be set when the audit starts** (record it here, do not guess).
- Repository: no public remote is configured in this workspace yet.

## 4. Deliverables expected (none received yet)

| Deliverable | File | Status |
|---|---|---|
| Staking program audit | `evidence/audits/staking-audit-<firm>-<date>.pdf` | NOT STARTED — EXTERNAL |
| Control-plane penetration test | `evidence/audits/pentest-<firm>-<date>.pdf` | NOT STARTED — EXTERNAL |

`evidence/audits/README.md` lists the same status.

## 5. Findings tracker

| ID | Severity | Status | Fix commit | Source |
|---|---|---|---|---|
| — | — | no findings recorded yet | — | — |

Severity: critical / high / medium / low / info. Status: open / fixed / accepted-risk / wont-fix.
A finding moves to `fixed` only with a commit hash and a test that reproduces it.
