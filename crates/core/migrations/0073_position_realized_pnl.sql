-- Track cumulative realized PnL separately from exit proceeds and remaining
-- cost basis. Existing rows are deliberately left NULL: their historical
-- partial-exit cost allocations cannot be reconstructed reliably from the
-- aggregate position row alone. Risk/reporting readers must treat NULL as
-- unknown rather than assuming zero or using the lossy legacy formula.
ALTER TABLE positions
    ADD COLUMN IF NOT EXISTS realized_pnl_quote DOUBLE PRECISION;
