# Recorded launch datasets (Batch 02)

Recorder + store + verifier + replay for honestly-labelled launch data. This
is the reference for the `module-sniper::dataset` module, `dataset_tool`,
migrations 0063-0065, and the server's `PgDatasetStore`.

## 1. Honesty rules (read first)

1. **Synthetic fixtures are never "recorded".** The generated
   `crates/module-sniper/fixtures/replay/*.json` scenarios are not real market
   data. `dataset_tool import-fixtures` labels the result
   `source = synthetic_fixture`, forever.
2. **`authoritative` is decided by exactly one module** —
   `module-sniper::dataset::verifier`. A dataset is authoritative **only**
   when `hash_chain_ok && coverage_ok && source == recorded`. A self-declared
   `authoritative`/`status` flag in a manifest is never trusted: the verifier
   recomputes the verdict from the bytes.
3. **A synthetic fixture can never become authoritative**, no matter how clean
   its hash chain or coverage, because the `source` label blocks it.
4. **No fabricated data.** A real "recorded" dataset must come from a bounded
   capture session against a real configured feed
   (`scripts/run-dataset-capture.sh`). Until that capture is wired and run,
   backtests over these datasets stay labelled synthetic/non-authoritative.

`evidence/live/launch_dataset_capture.json` stays `NOT_RUN` until a real
capture populates `crates/module-sniper/tests/fixtures/launches/`.

## 2. Single-owner concerns

| Concern | Owner |
|---|---|
| On-disk byte format (NDJSON line, `RecordedEvent`, manifests) | `dataset/format.rs` |
| Hash chain (chunk SHA-256, `prev_sha256`, `sha256_root`, constant-time compare) | `dataset/hashing.rs` |
| Validated config (env parsing, fail-closed) | `dataset/policy.rs` |
| File persistence (layout, atomic writes, fsync, resume) | `dataset/store.rs` |
| Streaming recorder (bounded mpsc, measured drops, rotation, seal) | `dataset/recorder.rs` |
| Verification + the authoritative decision | `dataset/verifier.rs` |
| Deterministic replay source | `dataset/recorded_source.rs` |
| Replay-fixture importer (synthetic labelling) | `dataset/fixture_import.rs` |
| Postgres metadata store | `crates/server/.../dataset_store_pg.rs` |

No other module re-implements encoding, hashing or path layout.

## 3. On-disk layout

```
<root>/<dataset_id>/manifest.json
<root>/<dataset_id>/chunk-000001.ndjson
<root>/<dataset_id>/chunk-000002.ndjson
...
```

* `manifest.json` — `DatasetManifest`: `dataset_id`, `name`, `source`,
  `cluster`, `schema_version`, `slot_start/end`, `record_count`,
  `sha256_root`, `status`, `authoritative` (cached label only), timestamps,
  and ordered `chunks: Vec<ChunkManifest>`.
* `chunk-NNNNNN.ndjson` — one `DatasetRecord` per line:
  `{ schema_version, seq, slot, event }`. `seq` is the dataset-wide monotonic
  index; `slot` is the capture slot used for coverage; `event` is a
  `RecordedEvent`.

### `RecordedEvent` variants (real existing types)

| kind | payload |
|---|---|
| `launch` | `crate::event::LaunchEvent` |
| `first_slot_buys` | `Vec<risk_intel::bundler::FirstSlotBuy>` + mint/slot |
| `dev_sell` | fields mirroring `risk_intel::DevSellObservation` |
| `pool_price_tick` | `backtest::dataset::Tick` + mint/slot |

Adjacently tagged (`{"kind": ..., "data": ...}`); serde struct fields keep a
fixed declaration order, so `encode_line` is byte-stable. **Strict versioning**:
`schema_version = DATASET_SCHEMA_VERSION = 1`; any other value is rejected on
encode and decode of both lines and the manifest.

### Hash chain

* `sha256` of a chunk = SHA-256 of that chunk's exact bytes
  (`bot_core::auth::sha256_hex`).
* `prev_sha256` of chunk *i* = `sha256` of chunk *i-1*; chunk 1 links to
  `GENESIS_PREV_SHA256` (64 zero hex chars).
* `sha256_root` = SHA-256 over the domain-separated, ordered list of all
  chunk hashes.
* Digest comparisons use a constant-time compare.

The verifier recomputes every chunk hash, walks `prev_sha256`, checks the
root, strict chunk ordering and byte/record counts. A flipped byte, deleted
chunk, reordered chunk or torn trailing line -> `hash_chain_ok = false`.

## 4. Recorder

`Recorder::new(store, manifest, cfg)` returns a cloneable `RecorderSink` and a
`Recorder` worker. `RecorderSink::record(event)` is a non-blocking `try_send`
on a bounded `tokio::sync::mpsc`; a full/closed channel drops the event but
increments a **measured** `dropped` counter (never silent). `Recorder::run`
appends one fsynced NDJSON line per event, rotates a chunk at
`chunk_max_records`/`chunk_max_bytes`, and on graceful shutdown (all sinks
dropped, or `RecorderSink::seal()`) finalizes the chain and atomically writes
the sealed manifest.

`ChunkWriter` is the synchronous core (used by the importer and recovery
tests). `ChunkWriter::resume` reopens a `recording` dataset after a crash,
truncates a torn trailing line, and rebuilds state with no loss/duplication.

## 5. Verifier and coverage

`verifier::verify(store, id, policy)` produces a `VerificationReport`:

* `hash_chain_ok` — integrity (see §3).
* `gaps_found`, `max_slot_gap`, `out_of_order`, `overlap` — slot coverage.
* `coverage_ok` — `record_count >= min_records`, `slot_span >= min_slot_span`,
  `gaps_found == 0`, no out-of-order/overlap, and `hash_chain_ok`.
* `outcome` — `pass` iff `hash_chain_ok && coverage_ok`.
* `authoritative` — `pass && source == recorded` (the ONLY trusted flag).

`VerifierPolicy` (`DATASET_MIN_RECORDS`, `DATASET_MIN_SLOT_SPAN`,
`DATASET_MAX_SLOT_GAP`) is validated and fail-closed on invalid values.

## 6. Postgres metadata (migrations 0063-0065)

Three **GLOBAL market-data** tables (deliberately **no** `organization_id` /
`org_id` / `tenant_id`, so Batch 01's RLS loop never captures them):

* `recorded_launch_datasets` — metadata. CHECK:
  `authoritative = false OR (status = 'verified' AND source = 'recorded')`.
* `recorded_launch_dataset_chunks` — per-chunk hash-chain metadata
  (`ON DELETE CASCADE`).
* `recorded_launch_dataset_verifications` — append-only verifier audit
  (`outcome CHECK IN ('pass','fail')`).

> **Name adaptation.** Migration 0055 already created a tenant-scoped table
> `launch_datasets` (referenced by 0057). Two tables cannot share a name and
> the Batch 02 tables must be global, so this family uses the
> `recorded_launch_dataset*` prefix. The migration *files* keep the required
> graph names. 0055/0057 are untouched.

`GRANT`s to `bot_app` / `bot_system` are guarded by role-existence checks, so
0065 applies on a fresh DB (0001-0059 only) **and** on top of Batch 01's
0060-0062. `PgDatasetStore::record_verification` is the only writer that
flips `authoritative`, exclusively from a verifier report; the CHECK is
defence-in-depth. Chunk + verification rows cascade on delete.

## 7. CLI (`dataset_tool`)

```
dataset_tool verify <dir>                              # recompute chain + coverage; non-zero on failure
dataset_tool inspect <dir>                             # manifest + verifier summary
dataset_tool import-fixtures <fixtures_dir> <out_dir>  # honest synthetic_fixture dataset
dataset_tool replay-hash <dir>                         # deterministic output hash
```

`import-fixtures` writes `<out_dir>/LATEST` (the dataset id). `verify` prints
`source`, `authoritative`, `hash_chain_ok`, `coverage_ok`, `outcome`, etc.
Replaying the same dataset twice yields the identical `replay-hash`.

## 8. Environment variables

| Variable | Meaning | Default |
|---|---|---|
| `DATASET_DIR` | capture output root | (required for capture) |
| `DATASET_CHUNK_MAX_RECORDS` | records per chunk | 1000 |
| `DATASET_CHUNK_MAX_BYTES` | bytes per chunk | 4 MiB |
| `DATASET_CHANNEL_CAPACITY` | recorder mpsc capacity | 1024 |
| `DATASET_MIN_RECORDS` | verifier min records | 1 |
| `DATASET_MIN_SLOT_SPAN` | verifier min slot span | 0 |
| `DATASET_MAX_SLOT_GAP` | verifier max allowed slot gap | u64::MAX |

## 9. Integration note (later batch)

`Sniper::with_recorder(RecorderSink)` is an optional builder hook (default
`None`, zero behaviour change). The integration batch attaches a `Recorder`,
spawns `Recorder::run`, and hands the sink to the sniper; observed
`LaunchEvent`s are then captured. `scripts/run-dataset-capture.sh` is the
bounded-session runbook that seals and verifies. No synthetic data is
produced by that path.
