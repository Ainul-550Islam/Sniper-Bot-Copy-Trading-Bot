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

## Files 3–5

Not started. File 3 requires a green `CI` on `main` first.
