# Competitor benchmark

Updated 2026-11-09. This is a narrow comparison of published product
claims, not an independent audit, endorsement, or measurement of performance.
Fees and features can change; check the linked pages before relying on them.

## Dated sources

- [Polycopy: Polymarket Copy Trading](https://polycopy.app/copy-trading),
  accessed 2026-11-09. The vendor describes manual copying, Premium one-click
  execution, and Auto Copy bots with budget and risk rules.
- [PolyCop documentation](https://docs.polycop.ai/), accessed 2026-11-09.
  The vendor describes a Telegram bot with copy trading, market or limit orders,
  proportional sizing, and per-market caps.
- `CHANGELOG.md`, "Polymarket copy-follow" (2026-10-31), and the current source
  paths below describe this repository, not third-party products.

## Polymarket copy-follow

| Capability | This repository | Published competitor description |
|---|---|---|
| Follow a wallet's entries | Implemented in `crates/module-polymarket/src/copy.rs`, `copy_feed.rs`, and `copy_run.rs`; scheduled by `Tick::Copy` in `crates/module-polymarket/src/lib.rs`. Config defaults to no followed wallets. | Polycopy and PolyCop describe automated wallet-following at their links above. |
| Exit copying | A leader sell is explicitly skipped; no automated Polymarket exit. See `CHANGELOG.md` and `crates/module-polymarket/tests/copy_follow.rs`. | No direct parity claim made. |
| Validation | The repository contains mock-venue tests in `crates/module-polymarket/tests/copy_follow.rs`; no live Polymarket CLOB/data-API round-trip is claimed. | Vendor statements have not been independently tested. |

The earlier broad comparison and fee assertions are not carried forward as
current facts: their original references were not reverified for this revision.
No latency or execution-superiority claim follows from this feature comparison.
See `docs/KNOWN-LIMITATIONS.md` for outstanding validation.
