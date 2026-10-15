-- 0057_backtest_runs_truth_columns.sql — make the data source of every backtest
-- explicit, and record why a run is blocked instead of inventing a result.
--
-- Additive only. Existing rows were all produced by the synthetic simulator
-- (the only generator this deployment had), so they are backfilled as
-- synthetic and NOT authoritative.
--
-- ROLLBACK (manual):
--   ALTER TABLE backtest_runs
--       DROP COLUMN IF EXISTS dataset_id,
--       DROP COLUMN IF EXISTS blocked_reason,
--       DROP COLUMN IF EXISTS authoritative,
--       DROP COLUMN IF EXISTS data_source;

ALTER TABLE backtest_runs
    ADD COLUMN IF NOT EXISTS data_source     text,
    ADD COLUMN IF NOT EXISTS authoritative   boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS blocked_reason  text,
    ADD COLUMN IF NOT EXISTS dataset_id      uuid REFERENCES launch_datasets (id);

UPDATE backtest_runs
   SET data_source = 'synthetic',
       authoritative = false
 WHERE data_source IS NULL;
