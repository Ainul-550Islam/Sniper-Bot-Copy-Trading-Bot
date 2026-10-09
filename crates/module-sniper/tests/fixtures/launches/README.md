# launches/ — recorded REAL token-launch dataset (currently EMPTY by design)

GAP MAP v2 asks for ≥ 50 recorded real launches (pump.fun + Raydium) with
price paths and capture metadata to feed the backtest engine. **This
directory is intentionally empty**: real launches must be CAPTURED from live
feeds — fabricating "recorded" data would violate project rule 1 (never
fabricate data) and would silently poison backtest results with fake history.

Status anchor: `evidence/live/launch_dataset_capture.json` — `NOT_RUN` until
a capture session fills this directory and records the attestation.

## Required record schema (one JSON file per launch)

```json
{
  "schema_version": 1,
  "source": "pumpfun | raydium",
  "mint": "<base58 mint>",
  "captured_at": "<RFC3339 UTC>",
  "capture": {
    "feed": "geyser | pumpportal_ws | logs_poll",
    "rpc_provider": "<provider label, no endpoints with keys>",
    "host_region": "<region of the capture host>",
    "tool": "<script/tool + version used>"
  },
  "launch_event": {
    "slot": 0,
    "block_time": 0,
    "signature": "<launch tx signature>",
    "creator": "<base58 creator wallet>",
    "initial_virtual_reserves": {"base": "0", "quote": "0"}
  },
  "price_path": [
    {"slot": 0, "ts": 0, "price_quote_per_base": "0", "reserve_base": "0", "reserve_quote": "0", "event": "launch | trade | migrate"}
  ],
  "terminal": {"outcome": "rugged | migrated | censored | truncated", "at_slot": 0}
}
```

Rules:
- Amounts/prices are STRINGS (no float round-trips); consumers parse to
  integers/decimals.
- `price_path` must be contiguous per source from the launch slot; gaps must
  be marked with `"event": "gap"` rows rather than silently dropped.
- No wallet in the dataset may be one of the operator's own funded wallets
  (avoid self-referential benchmarks); capture metadata records the feed,
  not any private key.
- Filenames: `<source>_<mint-first-8>_<yyyymmdd>.json`.

## Capture procedure (turns NOT_RUN into PASSED)

1. Point the existing feed harness (`module-sniper` feeds + PumpPortal/Geyser
   clients) at a capture profile with recording enabled.
2. Record for a bounded window (e.g. 24 h) on mainnet; keep raw events.
3. Post-process into the schema above; keep ≥ 50 launches with full paths.
4. Wire the recorded set into the backtest engine: today
   `backtest/dataset.rs::DatasetLabel` has only a `Synthetic` variant and
   `tests/backtest_golden.rs` reads `tests/fixtures/backtest/` only, so the
   capture work includes adding a `Recorded` variant, a loader for this
   directory, and golden expectations over the recorded set. Record the
   dataset content hash in `evidence/live/launch_dataset_capture.json` as the
   attestation, flipping it to PASSED.

Until step 4 completes, backtests run against the labelled SYNTHETIC fixtures
in `tests/fixtures/backtest/` and every backtest output stays marked
synthetic — never presented as historical performance.
