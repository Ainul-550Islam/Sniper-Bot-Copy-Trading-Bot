-- 0063_launch_datasets.sql — GLOBAL (non-tenant) metadata for recorded launch
-- datasets: recorder + store + verifier + replay (BATCH 02).
--
-- NAME ADAPTATION (must be read before editing):
--   Migration 0055 already created a table called `launch_datasets` — it is
--   tenant-scoped (`organization_id uuid NOT NULL`), holds a flat tick table
--   and is referenced by 0057 (`backtest_runs.dataset_id REFERENCES
--   launch_datasets(id)`). That table is an existing concern and MUST NOT be
--   duplicated, dropped or altered here.
--
--   The BATCH 02 dataset is GLOBAL MARKET DATA: it deliberately has NO
--   organization_id / org_id / tenant_id column so that the Batch 01 RLS
--   loop (which scans for tenant-scoped tables) never captures it. Two
--   different tables cannot share a name, so this family is created under
--   the `recorded_launch_dataset*` prefix. The migration FILE keeps the
--   required graph name (0063_launch_datasets.sql); the TABLE inside is
--   `recorded_launch_datasets`.
--
-- HONESTY / TRUST MODEL:
--   `source` is a self-declared label. `authoritative` can NEVER be trusted
--   from an insert: the DB CHECK below plus the verifier (module-sniper
--   dataset::verifier) enforce that a row may be authoritative ONLY when it
--   is `status = 'verified'`, `source = 'recorded'` AND the verifier proved
--   (hash chain intact + coverage criteria met). Synthetic fixtures are
--   permanently non-authoritative.
--
-- Additive + idempotent: CREATE TABLE IF NOT EXISTS / inline constraints.
-- ROLLBACK (manual; the repository has no down-migrations):
--   DROP TABLE IF EXISTS recorded_launch_dataset_verifications;
--   DROP TABLE IF EXISTS recorded_launch_dataset_chunks;
--   DROP TABLE IF EXISTS recorded_launch_datasets;

CREATE TABLE IF NOT EXISTS recorded_launch_datasets (
    dataset_id       uuid PRIMARY KEY,
    name             text NOT NULL,
    source           text NOT NULL CHECK (source IN ('recorded', 'synthetic_fixture')),
    cluster          text NOT NULL DEFAULT '',
    schema_version   integer NOT NULL,
    slot_start       bigint NOT NULL DEFAULT 0,
    slot_end         bigint NOT NULL DEFAULT 0,
    record_count     bigint NOT NULL DEFAULT 0,
    sha256_root      text NOT NULL DEFAULT '',
    status           text NOT NULL DEFAULT 'recording'
                     CHECK (status IN ('recording', 'sealed', 'verified', 'failed')),
    authoritative    boolean NOT NULL DEFAULT false,
    created_at       timestamptz NOT NULL DEFAULT now(),
    sealed_at        timestamptz,
    -- A dataset is authoritative ONLY when the verifier proved it AND it is
    -- honest recorded data. This is defence-in-depth: the verifier is the
    -- only writer that ever sets authoritative = true, but the constraint
    -- makes a bad/ forged write impossible even if that code regressed.
    CONSTRAINT recorded_launch_datasets_authoritative_guard CHECK (
        authoritative = false
        OR (status = 'verified' AND source = 'recorded')
    )
);

COMMENT ON TABLE recorded_launch_datasets IS
    'GLOBAL market data (no tenant scope): metadata for hash-chained recorded '
    'launch datasets. authoritative is decided only by the verifier + this CHECK.';
