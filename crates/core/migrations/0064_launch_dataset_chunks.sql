-- 0064_launch_dataset_chunks.sql — hash-chain chunk index for one recorded
-- launch dataset (BATCH 02). GLOBAL market data: NO tenant column.
--
-- Each chunk file lives on the filesystem (FsDatasetStore). This table is the
-- durable metadata index: one row per chunk, carrying the SHA-256 of the
-- chunk's bytes and the SHA-256 of the PREVIOUS chunk (hash chain). The
-- verifier recomputes every chunk hash, walks prev_sha256 and checks the
-- manifest's sha256_root; a single flipped byte, missing chunk or reordered
-- chunk breaks the chain.
--
-- ON DELETE CASCADE: deleting the dataset row removes its chunk + verification
-- metadata. The file blobs are owned by the FsDatasetStore and are removed by
-- the store, not by this FK.
--
-- Additive + idempotent. ROLLBACK (manual):
--   DROP TABLE IF EXISTS recorded_launch_dataset_chunks;

CREATE TABLE IF NOT EXISTS recorded_launch_dataset_chunks (
    dataset_id    uuid NOT NULL
                  REFERENCES recorded_launch_datasets (dataset_id) ON DELETE CASCADE,
    chunk_index   bigint NOT NULL,
    first_slot    bigint NOT NULL DEFAULT 0,
    last_slot     bigint NOT NULL DEFAULT 0,
    record_count  bigint NOT NULL DEFAULT 0,
    byte_len      bigint NOT NULL DEFAULT 0,
    sha256        text NOT NULL,
    prev_sha256   text NOT NULL,
    PRIMARY KEY (dataset_id, chunk_index),
    CONSTRAINT recorded_launch_dataset_chunks_index_positive CHECK (chunk_index >= 1)
);

CREATE INDEX IF NOT EXISTS recorded_launch_dataset_chunks_dataset_idx
    ON recorded_launch_dataset_chunks (dataset_id, chunk_index);

COMMENT ON TABLE recorded_launch_dataset_chunks IS
    'Per-chunk hash-chain metadata (global market data, no tenant scope). '
    'sha256 = hash of chunk bytes; prev_sha256 = sha256 of the prior chunk.';
