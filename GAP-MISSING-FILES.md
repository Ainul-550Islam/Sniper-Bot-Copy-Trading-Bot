# GAP-MISSING-FILES — master status map

This file tracks the hand-off gap list. It states only what the repository
shows today. Anything not listed as DONE is open, and nothing here is a
release claim (see `docs/KNOWN-LIMITATIONS.md`).

The previous version is archived at `docs/archive/GAP-MISSING-FILES-v2.md`.

## File 2 — P0 build, CI, hygiene, truthfulness

| Area | Status | Evidence |
|---|---|---|
| §2 build blocker (`execution.rs` removed, `.cargo` clang/lld optional) | DONE | `cargo clippy --workspace --all-targets -- -D warnings` exit 0 |
| §2.1 warnings / `FeedHealth` (`reconnects`, `lag_ms` measured) | DONE | module-copy tests; `FeedHealth` in `crates/module-copy/src/feeds.rs` |
| §2 `cargo fmt --all --check` | DONE | exit 0 (app and staking) |
| §3 CI: staking job (pinned action + fallback, timeout) | DONE (local), CI green on `main` BLOCKED | `.github/workflows/ci.yml`; no remote to run on |
| §3 CI: integrity gates (stats, markers) | DONE (local) | `tests/release/stats_current.sh` PASS |
| §3 CI: docker smoke (in-container health probe) | DONE (local smoke), docker job BLOCKED | release binary `/api/health` 200; no docker here |
| §3 CI: frontend contract gate order | DONE | `.github/workflows/frontend-ci.yml` |
| §3 `deny.toml` licenses (first-party private, `solana-config-program-client` clarified) | DONE | `cargo deny check licenses` exit 0 |
| §3 `ci_run.json` | NOT_RUN (correct) | `evidence/live/ci_run.json` |
| §3 Helm/base image digests | OPEN — decision needed | `verify-image-digests.sh` PASS; base image digests in `Dockerfile` differ from current registry tags |
| §4 hygiene: junk removed, `verify-delivery` junk assertion | DONE | `verify-delivery.sh` 8 PASS / 0 FAIL |
| §4 `AUDIT.md` moved to `docs/archive/` | DONE | references updated |
| §4 `git filter-repo` history rewrite (`.cargo/bin/rustup`) | NOT DONE — needs explicit approval (force-push) | `.cargo/bin` is not tracked in HEAD |
| §4 `P0-D-FULL-CODE.md` removed | DONE | 112 unique non-code lines, code present in tree |
| §5 backtest truthfulness (`data_source`, `authoritative`, migration 0057) | NOT STARTED | — |
| §5 recorded datasets (migrations 0055–0056, recorder, dataset store) | NOT STARTED | — |
| §5 frontend disclosure components | NOT STARTED | — |

## File 3 — PART 03: signal producers, deployment contract, hygiene (2026-10-26)

| Area | Status | Evidence |
|---|---|---|
| K1 first-slot producer for `LaunchEvent.first_slot_buys` | DONE | `crates/module-sniper/src/risk_intel/first_slot.rs`; `cargo test -p module-sniper --test first_slot_producer` → 7 PASS / 0 FAIL / 0 ignored (fixture blocks with real bincode transactions; `write_fixtures` runs by default and regenerates them byte-identically); 8 unit tests in the producer |
| K2 creator dev-sell watcher → `DevSellObservation` | DONE | `crates/module-sniper/src/risk_intel/dev_sell.rs`; `cargo test -p module-sniper --test dev_sell_producer` → 7 PASS / 0 FAIL / 0 ignored; 6 unit tests |
| K1/K2 wiring into the feeds and `Sniper::run` | DONE | `crates/module-sniper/src/detect.rs`, `crates/module-sniper/src/lib.rs`; `cargo clippy -p module-sniper --all-targets -- -D warnings` exit 0 |
| K3 tracked junk untracked, `AUDIT.md` archive, `config.toml` untracked | DONE | `scripts/cleanup-tracked-junk.sh` (dry-run → apply → re-run reports clean); `verify-delivery.sh` 8 PASS / 0 FAIL; `verify-script-modes.sh` OK |
| K4 Helm digest-only contract | DONE | `values.yaml` has no `image.tag`; `values.schema.json` added; `tests/release/helm_image_contract.sh` PASS (CI runs exactly this static gate). Verified against helm v3.15.4: `helm lint deploy/helm/sniper-suite --set image.digest=sha256:<64 hex>` → 0 chart(s) failed, and `helm template --set migrationJob.enabled=true --set image.digest=…` renders one `sniper-suite@sha256:…` line in the Deployment and one in the Job (2 total). The chart is deliberately unrenderable with no digest: bare `helm lint` (empty default) reports `image.digest: Does not match pattern '^sha256:[0-9a-f]{64}$'`, as do uppercase, 63-hex, and `sha256:deadbeef` digests, and `--set image.tag=…` fails with `image: Additional property tag is not allowed` even alongside a valid digest — so an operator must pass `--set image.digest=` or `-f` with a digest, which is the documented install line |
| K5 `scripts/bootstrap-config.sh` + `Makefile` | DONE | `make help` renders with `PATH=/usr/bin:/bin`; `bootstrap-config.sh --check` exits 1 when a file is missing, 0 after creating (mode 0600, never overwrites) |
| K6 `integrity-gates` job completeness | DONE | `.github/workflows/ci.yml` now runs `verify-delivery.sh`, `helm_image_contract.sh`, `marketing_claims.sh`, `manifest_current.sh` and `check-routes-vs-ui.sh` (13 steps; YAML parses; the `app` job keeps its own pre-existing route step) |
| K7 README entry points | DONE | Quick start / Testing / Docker use `make init|up|migrate|run|test|gates`; `verify-marketing-claims.sh` OK |
| `stats_current.sh`, `manifest_current.sh` | FAIL BY DESIGN (not this part) | `docs/STATS.md` and `release-manifest.json` are refreshed by the release step (`scripts/generate-stats.sh`, `scripts/update-release-manifest.sh`), not by a feature change |
| §4 `git filter-repo` history rewrite (`.cargo/bin/rustup`) | NOT DONE — needs explicit approval (force-push) | files are untracked at HEAD now; the blobs remain in history |

## Test honesty in `crates/module-sniper/tests` (PART 03 follow-up)

| Area | Status | Evidence |
|---|---|---|
| No fixture generator hidden behind `#[ignore]` | DONE | `write_fixtures` and `regenerate_replay_fixtures` are ordinary tests; `cargo test -p module-sniper` → 302 PASS / 0 FAIL / **0 ignored** and `cargo test -p solana-kit` → 354 / 0 / **0** |
| Stale replay fixtures refreshed | DONE | the 19 committed `crates/module-sniper/fixtures/replay/*.json` predated config fields that already exist at HEAD (`simulate_sell`, `require_sell_probe`, `max_top_holder_pct`, `max_bundler_ratio`, `advanced_exit`, …), so `checked_in_fixtures_match_the_generator` was passing against bytes its own generator no longer produced; re-running the generator makes the tree clean |
| `server/**` gated/live tests | OPEN — not this part | 6 `#[ignore]` remain under `crates/server/` (billing adapters, `live_*`, `trusted_proxy_client_ip`); those paths belong to Part 04 and several require real Postgres or a configured provider |

## Files 4–5 (unchanged by PART 03)

Not started. File 3 required a green `CI` on `main` first; this repository has
no remote configured here, so every CI statement above is a locally executed
command, not a workflow run.
