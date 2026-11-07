-- Record when the most recent exit fill changed cumulative realized PnL.
-- Legacy rows remain NULL because their partial-exit day cannot be inferred
-- from updated_at, which also changes for ordinary mark/risk updates.
ALTER TABLE positions
    ADD COLUMN IF NOT EXISTS realized_at TIMESTAMPTZ;
