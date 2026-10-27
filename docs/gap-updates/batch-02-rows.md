# GAP section 5 — Batch 02 row updates

`GAP-MISSING-FILES.md` is owned by Batch 01 and is **not** edited here. This
file records the replacement rows for **File 2 §5** that the integration step
should merge once Batch 02 lands. It states only what the repository now
shows; it is not a release claim.

## Staleness correction

The shipped `GAP-MISSING-FILES.md` says migration 0057 is `NOT STARTED`. That
is stale: **`crates/core/migrations/0057_backtest_runs_truth_columns.sql`
exists at HEAD** and adds `data_source`, `authoritative`, `blocked_reason`,
`dataset_id` to `backtest_runs`, backfilling existing synthetic runs as
non-authoritative. Likewise `0055_launch_datasets.sql` already exists (a
tenant-scoped launch-dataset table). Batch 02 does not touch either.

## Replacement rows (File 2 §5)

| Area | Status | Evidence |
|---|---|---|
| §5 backtest truthfulness (`data_source`, `authoritative`, migration 0057) | DONE (pre-existing at HEAD) | `crates/core/migrations/0057_backtest_runs_truth_columns.sql`; `0055_launch_datasets.sql` |
| §5 recorded datasets — migrations | DONE (Batch 02) | `0063_launch_datasets.sql`, `0064_launch_dataset_chunks.sql`, `0065_launch_dataset_verifications.sql` (global, no tenant column; guarded GRANTs; idempotent re-apply) |
| §5 recorded datasets — format / hash chain | DONE (Batch 02) | `crates/module-sniper/src/dataset/{format,hashing}.rs`; `tests/dataset_format_roundtrip.rs`, `tests/dataset_verifier.rs` |
| §5 recorded datasets — recorder | DONE (Batch 02) | `dataset/recorder.rs`; `tests/dataset_recorder.rs` (rotation, measured drops, crash resume, graceful seal) |
| §5 recorded datasets — file store | DONE (Batch 02) | `dataset/store.rs` (`DatasetStore` + `FsDatasetStore`, atomic writes, fsync) |
| §5 recorded datasets — verifier (authoritative decider) | DONE (Batch 02) | `dataset/verifier.rs`; `tests/dataset_verifier.rs`; honesty: synthetic never authoritative |
| §5 recorded datasets — deterministic replay | DONE (Batch 02) | `dataset/recorded_source.rs`; `tests/dataset_replay_determinism.rs` (identical hash on replay x2) |
| §5 recorded datasets — fixture import (honest labelling) | DONE (Batch 02) | `dataset/fixture_import.rs`; `tests/dataset_fixture_import.rs` (all 19 replay fixtures -> `synthetic_fixture`, `authoritative=false`) |
| §5 recorded datasets — Postgres metadata store | DONE (Batch 02) | `crates/server/src/trading_data_plane/dataset_store_pg.rs`; `tests/dataset_pg_store.rs`, `tests/dataset_migrations.rs` (real-PG gated) |
| §5 recorded datasets — CLI + capture runbook + CI | DONE (Batch 02) | `src/bin/dataset_tool.rs`; `scripts/run-dataset-capture.sh`; `.github/workflows/datasets.yml` |
| §5 recorded datasets — LIVE capture evidence | NOT_RUN (correct) | `evidence/live/launch_dataset_capture.json` — needs a real capture session; no recorded dataset is claimed yet |
| §5 frontend disclosure components | OPEN (Batch 01 / integration) | Batch 02 adds no frontend changes |
| §5 wiring live feed -> recorder | OPEN (integration batch) | `Sniper::with_recorder` hook exists; the server must attach a `Recorder` and spawn `Recorder::run` |

## Notes for the merge

* Batch 02 tables are `recorded_launch_datasets`,
  `recorded_launch_dataset_chunks`, `recorded_launch_dataset_verifications`
  (the `recorded_` prefix avoids the 0055 `launch_datasets` name collision and
  keeps them GLOBAL / non-tenant).
* Migration numbering: Batch 01 owns 0060-0062, Batch 02 owns 0063-0065;
  merge order 01 then 02 keeps the graph contiguous.
* `authoritative` is decided only by `dataset::verifier`; the DB CHECK on
  `recorded_launch_datasets` is defence-in-depth.
* No HTTP routes, OpenAPI entries or frontend files were changed by Batch 02;
  `check-openapi-coverage.sh` and `check-routes-vs-ui.sh` stay green.
