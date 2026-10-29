# GAP — Batch 07 row updates (integration wiring)

`GAP-MISSING-FILES.md` is owned by Batch 01 and is **not** edited here. This
file records what Batch 07 changed against the seams handed over by Batches
02, 03 and 04 (`batch-02-rows.md`, `batch-03-wiring.md`, `batch-04-wiring.md`)
on top of the integration-cleanup tree. It states only what the repository
now shows and what was actually run in-session; it upgrades no LIVE status.

## Rows closed by this batch

| Area | Status | Evidence |
|---|---|---|
| Telegram executing side (`TradeCommandPort`) | DONE (Batch 07) | `crates/server/src/telegram_trade_port.rs`: deployment-organization-bound port (other orgs refused), Jupiter ExactIn through `Executor::send_prebuilt`, deterministic `telegram-<command_id>` intent ids, completed-receipt memo + deterministic trade-id replay guard, write-ahead intent journal when a sink is wired |
| Telegram execution safety semantics | DONE (Batch 07) | Every order re-checks kill switch, mode + `allow_live_trading`, Telegram caps intersected with `risk.max_position_quote`, slippage ceiling, minimum SOL reserve and wallet balance. Live books **only** `ExecStatus::Confirmed`; `Sent`/`SendUnknown` return a signature receipt for reconciliation and are never booked as fills. Paper books the quote; simulate builds and really simulates |
| Telegram console attachment | DONE (Batch 07) | `crates/server/src/main.rs::build_telegram_console`: attaches only when `telegram.enabled && telegram.trading_enabled` + database + deployment organization + both caps positive + `PgTradeStore::installed_tables() == 3`. Any missing rung logs and runs the notify/control-only bot — no in-memory fallback for money commands |
| Telegram RLS scope | DONE (Batch 07) | `bot_core::tenant::rls_context::scope_tenant_context`; the bot task (`module_telegram::spawn_with_console`) and the binding-sync task run inside the deployment-org scope so PG trade stores see `app.org_id` |
| Durable chat link sync | DONE (Batch 07) | `TelegramBinding` gained `telegram_user_id` + `trading_enabled` (serde-defaulted, legacy documents still decode; bind refuses trading without a user and a rebind always rewrites both fields). Server syncs the deployment-org binding into `TradeConsole` links at boot and every 5 s; any change unlinks the previous chat first (cancelling its pending commands) |
| Consent administration (HTTP/OpenAPI/UI) | DONE (Batch 07) | `GET /api/tenant/telegram/consent` + `POST /api/tenant/telegram/consent/revoke` (view + revoke only; grant stays in-chat; the phrase digest is never serialised). Revoke also cancels pending commands. OpenAPI paths + regenerated `openapi/openapi.{json,yaml}`; control-plane page shows consent state/caps and a revoke action |
| Raydium CPMM routing | DONE (Batch 07) | `trade_raydium_cpmm` (default OFF): log/Geyser detection of CP-Swap `Initialize`, market loader + direct entry (`EntryRoute::RaydiumCpmmDirect`), exit marking/builders, Jupiter fallback. `Venue::RaydiumCpmm`, `LaunchProtocol::RaydiumCpmm` |
| Raydium LaunchLab routing | DONE (Batch 07) | `trade_raydium_launchlab` (default OFF): detection of the four initialize instructions (incl. Token2022 account-only fallback), curve loader, direct entry (`EntryRoute::RaydiumLaunchLabDirect`), exit builders, closed-curve → Jupiter fallback. `Venue::RaydiumLaunchLab`, `LaunchProtocol::RaydiumLaunchLab`, `SwapVenue::RaydiumLaunchLab` |
| Launch-dataset recorder runtime | DONE (Batch 07) | `[sniper] record_launch_dataset` + `DATASET_*`/`LIVE_DATASET_CAPTURE` env; server spawns the `Recorder`, attaches via `Sniper::with_recorder`, seals inside the 15 s module-drain. Recording observes launches before trading gates (`docs/BACKTEST-DATASETS.md` §8/§10) |

## Rows still open — honest remainder

| Area | Status | Why |
|---|---|---|
| Hosted multi-tenant Telegram trading | NOT IMPLEMENTED (by design) | The port is bound to the deployment organization and rejects others. Per-tenant execution needs a tenant executor factory that `main.rs` does not assemble today; approximating it inside the port would be unsafe |
| Live Telegram trade evidence | NOT_RUN | No live order was placed. Paper is the default; live needs `execution.mode = live` **and** `allow_live_trading` plus consent, caps and confirmation |
| Live CPMM/LaunchLab round trips | NOT_RUN | Builders are exercised by unit tests with synthetic account encoders and by quote math tests; no funded on-chain swap was executed |
| Recorded launch dataset | NOT_RUN | The runtime is wired and unit-tested, but no real capture session ran; `evidence/live/launch_dataset_capture.json` remains whatever it was — untouched by this batch |
| Real Telegram Bot API session | NOT_RUN | Tests drive local fakes; no request left the process to `api.telegram.org` |
| Hosted GitHub CI for the new wiring | NOT_RUN | All gates were run locally in-session (see the batch report); no hosted run exists |

## Known pre-existing flakes observed while gating (not introduced here)

* `custody::health` / `custody::kms::health` unit tests mutate process
  environment variables (`VAULT_ADDR`, `LIVE_CUSTODY`, …) from parallel
  tests; under a parallel `cargo test` they can race and fail, and pass
  when rerun alone. The custody rotation PG tests likewise assume a
  fresh database (profile-name collision on a reused DB). CI's canonical
  `cargo test --workspace -- --test-threads=1` invocation is green;
  Batch 07 did not touch those files and records the flake instead of
  widening its ownership.

## Notes for the merge

* Two exhaustive-match fallouts outside the original seam list were forced
  by the new `Venue`/`LaunchProtocol` variants and are recorded in the
  batch target tree: `crates/module-copy/src/policy.rs`,
  `crates/module-sniper/tests/property.rs`,
  `crates/module-sniper/tests/geyser_detect.rs` (subscription filter now
  includes the CPMM/LaunchLab programs via the default Jupiter fallback).
* `release-manifest.json`, `openapi/openapi.{json,yaml}` and the stat
  documents changed only through their own generators
  (`scripts/update-release-manifest.sh`, `scripts/export-openapi.sh`,
  `scripts/generate-stats.sh` + `scripts/sync-stat-markers.sh`).
* `FILES-MANIFEST.txt` now describes the combined cleanup + Batch 07
  delivered tree; `scripts/verify-batch07-complete.sh` is its verifier.
  The cleanup package (`integration-cleanup-2026-10-28.zip`) is unchanged
  and remains the artifact that reproduces the cleanup tree on its own.

## Hotfix — 2026-10-29 (pre-push, on top of the Batch 07 tree)

A live paper-mode smoke run against mainnet (public RPC + PumpPortal,
no keypair, no database) exposed a pre-existing parser defect that no
test fixture covered: extended pump.fun bonding-curve accounts leave
the v2 `quote_mint` field zeroed when SOL-quoted, and
`BondingCurveState::parse` returned `Some(Pubkey::default())` for those
accounts. `PumpContext::load` then asked for the token program of the
system program and every pump-curve entry aborted with "mint
11111111111111111111111111111111 is owned by unsupported program
NativeLoader1111111111111111111111111111111" (fail-closed; no trade was
ever built). The defect is byte-identical at the base commit; neither
the cleanup batch nor Batch 07 introduced it.

* Fix (`crates/solana-kit/src/pump.rs`, added to Batch 07 ownership for
  this hotfix): the parser normalizes a zeroed `quote_mint` to `None`,
  and `quote_mint_or_wsol()` refuses the default pubkey for states
  built any other way (e.g. deserialized). Three regression tests cover
  the zeroed-field account at v2 and full length, an explicit quote
  mint, and the accessor guard. solana-kit: 659 passed / 0 failed
  (was 656); module-sniper: 338 / 0; clippy + fmt clean.
* Re-smoke evidence (same setup, rebuilt binary): 147 launch events,
  every entry candidate passed market loading and stopped only at the
  risk balance gate ("balance 0.0000 below reserve 0.0500" — the
  ephemeral paper wallet is unfunded by design); zero occurrences of
  the old error in logs or journal. An actual paper fill still needs a
  funded wallet and remains NOT_RUN, as do all live/funded paths.
* One operational note from that run: under a debug build on a small
  sandbox host, the PumpSwap log consumer overflowed its queue and the
  supervisor auto-disabled the sniper module after the
  consecutive-failure limit (the process and API stayed up, and the
  accounting lease self-recovered). Recorded here as a runtime tuning
  item for deployment (release build, production RPC), not as a code
  change in this hotfix.
