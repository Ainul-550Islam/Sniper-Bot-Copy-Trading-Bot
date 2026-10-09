# PROTOCOL-DRIFT-RUNBOOK.md

Who fixes what, within how many hours, when an upstream protocol (pump.fun,
PumpSwap, Raydium, Polymarket, SPL, Jito) ships a change that this codebase
depends on. Aligned with the SLA table in
`legal/MAINTENANCE-AND-SUPPORT-TERMS.md` §4 (Severity 1 — protocol-drift that
blocks trading: response [4] hours 24×7, resolution [48] hours; illustrative
bracketed values pending negotiation).

---

## 1. What counts as drift

Any of:

1. **Program identity drift** — a pinned program id / known account in
   `scripts/protocol-pins.json` no longer matches the code constants
   (`crates/solana-kit/src/consts.rs`, `crates/module-polymarket` addresses,
   EIP-712 domain in `crates/module-polymarket/src/eip712.rs`).
2. **Layout drift** — an on-chain account/instruction layout changed
   (pump.fun discriminators, PumpSwap pool fields, CTF/collateral layouts in
   `module-polymarket/src/ctm.rs`/`collateral.rs`).
3. **Interface drift** — RPC/Geyser/Jito API behaviour change (method
   renames, param changes, auth changes) or Polymarket CLOB/Gamma API change.
4. **Fee-policy drift** — protocol fee recipients/fee math changed so our
   fee transfers or simulations are wrong.

## 2. Detection (already wired)

| Mechanism | What it catches | Cadence |
|---|---|---|
| `.github/workflows/protocol-drift.yml` static job | pins ↔ code disagreement | every push/PR touching pinned files |
| `.github/workflows/protocol-drift.yml` live job | on-chain executability/owner of pinned programs (`getAccountInfo`), writes `self_attesting` evidence | weekly cron + manual dispatch |
| `crates/solana-kit/tests/pump_layout_drift.rs` (`LIVE_SOLANA=1`) | pump.fun account order/count vs layout variants | on demand / release gate |
| Runtime: failed simulations + `simulateTransaction` error fingerprints | layout/fee drift surfacing as execution failures | continuous in operation |
| `scripts/check-protocol-drift.sh` | both modes, operator-invocable | ad hoc |

A drift alert is any of: a failed drift job, a failed layout test, or a
sustained simulation-failure spike in operations telemetry.

## 3. Severity classification

| Severity | Definition | Examples |
|---|---|---|
| **S1 — trading blocked** | live mode cannot trade the affected venue at all, or would trade WRONG | program redeploy with new id; discriminator change; CLOB auth scheme change |
| **S2 — degraded** | trading works but sub-optimally or partially | new optional account ignored by our builder; fee recipient moved; one feed source dead while fallbacks work |
| **S3 — cosmetic/observational** | no behaviour impact | metadata fields, explorer changes, docs |

## 4. Response matrix (roles and clocks)

Roles: **On-call** (first responder), **Protocol engineer** (owns the fix),
**Ops** (deployment + kill switch), **Seller/Buyer** per the support terms.

| Severity | Response (ack + mitigate) | Resolution target | Immediate mitigation |
|---|---|---|---|
| S1 | [4] hours, 24×7 | [48] hours | 1. Ops sets the kill switch for the affected module (tenant_control path — trading stops, withdrawals/reads stay up). 2. Never "fix forward" by editing pins alone. |
| S2 | [8] hours | [5] business days | Disable the affected feed/venue feature flag if one exists; otherwise module-level pause. |
| S3 | next business day | next scheduled release | none required |

## 5. Fix procedure (S1)

1. **Confirm** the drift: run `scripts/check-protocol-drift.sh` (static) and
   the live probe; record the failing pin/layout in the incident note.
2. **Classify** the upstream change: read the protocol's changelog/IDL/
   on-chain accounts; capture evidence (tx ids, account dumps) BEFORE
   changing code.
3. **Code fix** — the change MUST touch, in one commit:
   - the code constant(s) (`consts.rs` / venue module),
   - `scripts/protocol-pins.json` (same change — pins never lead code),
   - the affected layout test or a new regression test reproducing the old
     failure,
   - `evidence/live/*` stays NOT_RUN until a real re-validation run passes.
4. **Verify:** static drift check green; unit/layout tests green; a small
   paper/simulate round-trip on the affected venue; then — only with a real
   funded run — the corresponding `evidence/live/*.json` flips to PASSED with
   attestation (that is the ONLY thing that closes the incident's validation
   gap; per project rule 2, doc edits never close it).
5. **Deploy:** standard release pipeline; keep the kill switch armed until
   one confirmed good live round-trip lands.
6. **Post-incident:** append the pin/layout diff and timeline to the audit
   trail; if the SLA was missed, note it per support-terms §4 (two misses in
   a quarter trigger the remedy clause).

## 6. Venue-specific notes

- **pump.fun / PumpSwap:** discriminators + `pump_fun_accounts` are pinned;
  layout variants live in `solana-kit/src/pump.rs` with the IDL-authoritative
  counts. A new `*_v2` instruction appearing upstream is treated as S2 until
  the old path breaks, then S1.
- **Polymarket:** exchange addresses + EIP-712 domain version pinned. A
  domain-version bump is S1 (signatures fail outright); Gamma API field
  changes are S2 (`market_service` degrades to `unavailable` rather than
  guessing — rule 1).
- **Jito:** tip-floor / bundle API changes are S2 (Race mode falls back to
  sequential send); endpoint retirement without replacement escalates to S1
  for bundles-dependent configurations.
- **SPL / system programs:** identity changes here are effectively
  chain-wide events; treat any pin mismatch as S1 until proven otherwise.

## 7. What this runbook does NOT cover

- Market-risk incidents (a token rugging a tenant) — that is product risk,
  not protocol drift.
- Tenant configuration errors — support tickets, not incidents.
- Anything requiring funds movement to test: live validation always uses the
  smallest viable amount and records attestation; never simulate an
  attestation.
