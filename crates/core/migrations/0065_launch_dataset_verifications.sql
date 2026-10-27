-- 0065_launch_dataset_verifications.sql — immutable audit of every verifier
-- run over a recorded launch dataset (BATCH 02). GLOBAL market data: NO
-- tenant column.
--
-- A verification row is appended each time dataset::verifier::verify runs and
-- is persisted by the server's PgDatasetStore. `outcome` is the verifier's
-- own verdict; `authoritative` is stored on the dataset row (0063) and is
-- flipped ONLY from this verifier result, guarded by the 0063 CHECK.
--
-- GRANTs are guarded: Batch 01 creates roles bot_app / bot_system, but these
-- migrations must also apply on a fresh DB (or on top of only 0001-0059)
-- where those roles do not exist yet. Each GRANT is issued only when the role
-- is present, so the migration works with OR without Batch 01.
--
-- Additive + idempotent. ROLLBACK (manual):
--   DROP TABLE IF EXISTS recorded_launch_dataset_verifications;

CREATE TABLE IF NOT EXISTS recorded_launch_dataset_verifications (
    id                bigserial PRIMARY KEY,
    dataset_id        uuid NOT NULL
                      REFERENCES recorded_launch_datasets (dataset_id) ON DELETE CASCADE,
    verified_at       timestamptz NOT NULL DEFAULT now(),
    verifier_version  text NOT NULL,
    hash_chain_ok     boolean NOT NULL,
    gaps_found        integer NOT NULL DEFAULT 0,
    max_slot_gap      bigint NOT NULL DEFAULT 0,
    coverage_ok       boolean NOT NULL,
    outcome           text NOT NULL CHECK (outcome IN ('pass', 'fail')),
    detail            jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS recorded_launch_dataset_verifications_dataset_idx
    ON recorded_launch_dataset_verifications (dataset_id, verified_at DESC);

COMMENT ON TABLE recorded_launch_dataset_verifications IS
    'One row per verifier run (global market data). Immutable audit of the '
    'hash-chain + coverage verdict that decides authoritative.';

-- Guarded role grants: no-op when the Batch 01 roles do not exist yet.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_app') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON recorded_launch_datasets TO bot_app';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON recorded_launch_dataset_chunks TO bot_app';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON recorded_launch_dataset_verifications TO bot_app';
        EXECUTE 'GRANT USAGE, SELECT ON SEQUENCE recorded_launch_dataset_verifications_id_seq TO bot_app';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_system') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON recorded_launch_datasets TO bot_system';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON recorded_launch_dataset_chunks TO bot_system';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON recorded_launch_dataset_verifications TO bot_system';
        EXECUTE 'GRANT USAGE, SELECT ON SEQUENCE recorded_launch_dataset_verifications_id_seq TO bot_system';
    END IF;
END
$$;
