# GAP section — Batch 03 row updates

`GAP-MISSING-FILES.md` is owned by Batch 01 and is **not** edited here.
This file records the replacement rows for the Raydium venue adapters
(GAP-MAP v2 §7 P2) that the integration step should merge once Batch 03
lands. It states only what the repository now shows; it is not a release
claim.

## Replacement rows — venue adapters

| Area | Status | Evidence |
|---|---|---|
| Raydium CPMM — program identity | DONE (Batch 03) | `crates/solana-kit/idl/raydium_cp_swap.json`; `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` |
| Raydium CPMM — account layouts | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_cpmm/accounts.rs` — `PoolState` 637 B, `AmmConfig` 236 B; decoded from the official `PoolState::LEN` / `AmmConfig::LEN` |
| Raydium CPMM — PDA seeds | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_cpmm/pda.rs`; ported from `raydium-cp-swap` @ `f4b9b26` |
| Raydium CPMM — quote maths | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_cpmm/quote.rs`; exact integer port of `curve/{constant_product,calculator,fees}.rs` |
| Raydium CPMM — instruction builders | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_cpmm/instructions.rs`; account order from the vendored IDL |
| Raydium CPMM — event decoding | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_cpmm/decode.rs`; `SwapEvent` field order from `states/events.rs` |
| Raydium LaunchLab — program identity | DONE (Batch 03) | `crates/solana-kit/idl/raydium_launchpad.json`; `LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj` |
| Raydium LaunchLab — account layouts | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_launchlab/accounts.rs` — `PoolState` 429 B, `GlobalConfig` 371 B, `PlatformConfig` ≥ 944 B, `VestingRecord` 160 B |
| Raydium LaunchLab — PDA seeds | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_launchlab/pda.rs`; ported from `raydium-sdk-V2` @ `f854914` |
| Raydium LaunchLab — curve maths | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_launchlab/curve.rs`; exact integer port of `launchpad/curve/**` (constant / fixed / linear) |
| Raydium LaunchLab — instruction builders | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_launchlab/instructions.rs`; four trade instructions, 15-account IDL prefix + 3 SDK trailing accounts |
| Raydium LaunchLab — event decoding | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_launchlab/decode.rs`; `TradeEvent`, `PoolCreateEvent`, plus an `initialize_v2` fallback |
| Raydium LaunchLab — migration hand-off | DONE (Batch 03) | `crates/solana-kit/src/venues/raydium_launchlab/migration.rs`; the route/target decision and the SDK's liquidity threshold |
| Venue registry — Raydium entries | DONE (Batch 03) | `crates/solana-kit/src/venues/mod.rs` — `VenueRegistry::default_registry()` |
| Venue registry — Meteora layouts | STILL OPEN | `crates/solana-kit/src/venues/meteora/` — the layouts remain unverified, so the adapters still refuse to build |
| Sniper pipeline → Raydium venues | OPEN (Batch 07) | This batch adds no HTTP route, no frontend and no `module-sniper` wiring |
| Live on-chain evidence for the Raydium venues | NOT_RUN (correct) | No live capture was performed; the batch's evidence is the vendored IDL provenance plus the offline test suite |

## IDL provenance rows

| Area | Status | Evidence |
|---|---|---|
| Vendored official Raydium IDLs | DONE (Batch 03) | `crates/solana-kit/idl/raydium_cp_swap.json` (sha256 `1202f6dc…df81`), `crates/solana-kit/idl/raydium_launchpad.json` (sha256 `96318940…cdb`) |
| Upstream source, commit and URL recorded | DONE (Batch 03) | `crates/solana-kit/idl/SOURCES.md` — `raydium-io/raydium-idl` @ `e7e0c96f` |
| IDL provenance re-checkable offline | DONE (Batch 03) | `scripts/verify-raydium-idl.sh`; also runs in `.github/workflows/venues.yml` |
| Discriminators asserted against the IDL **and** the Anchor sighash | DONE (Batch 03) | `crates/solana-kit/tests/raydium_cpmm_discriminators.rs`, `crates/solana-kit/tests/raydium_launchlab_instructions.rs` |

## Staleness notes for the merge

* The **CPMM `payer` account is signer-only and read-only**. An earlier
  draft of this batch marked it writable; the official IDL and the Anchor
  source both say otherwise, and the builders and tests now follow the IDL.
* The **CPMM `AmmConfig` index seed is big-endian** on chain. The Raydium
  SDK's `getCpmmPdaAmmConfigId` uses little-endian. The two agree only for
  index `0`. This batch follows the *program*; the discrepancy is documented
  in `pda.rs` and asserted in the tests.
* The on-chain CPMM `AmmConfig` has a `creator_fee_share_rate` field that
  the published IDL type does not expose. The decoder follows the on-chain
  struct and fails closed on any length mismatch.
* The neutral `SwapVenue` enum is matched exhaustively by
  `crates/module-copy/src/feeds.rs`, which is owned by another batch. CPMM
  reports the existing `SwapVenue::RaydiumCpmm`; **LaunchLab reports the
  generic `SwapVenue::Other`** and the LaunchLab-specific detail stays in
  `LaunchLabTradeEvent`. Adding a `SwapVenue::RaydiumLaunchLab` variant
  here would break `module-copy`'s build.
* The four JSON fixtures under `crates/solana-kit/fixtures/raydium/` are
  **synthetic** and are labelled as such in the file itself. Their bytes and
  quote/curve vectors are asserted deterministically against the crate's own
  encoders and integer curves by the integration test suite.
* No HTTP route, OpenAPI entry or frontend file was changed by Batch 03, so
  `scripts/check-openapi-coverage.sh` and `scripts/check-routes-vs-ui.sh`
  stay green.
