-- A backtest run and its strategy must belong to the same organization.
-- The original single-column strategy FK proves only that the strategy
-- exists; it does not prove tenant ownership. Refuse migration if existing
-- rows violate the new invariant so operators can repair them explicitly.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
          FROM backtest_runs br
          JOIN tenant_strategies ts ON ts.id = br.strategy_id
         WHERE ts.organization_id <> br.organization_id
    ) THEN
        RAISE EXCEPTION
            'migration 0045 refused: backtest_runs contains strategies owned by another organization';
    END IF;
END
$$;

ALTER TABLE tenant_strategies
    ADD CONSTRAINT tenant_strategies_organization_id_id_key
    UNIQUE (organization_id, id);

ALTER TABLE backtest_runs
    DROP CONSTRAINT backtest_runs_strategy_id_fkey;

ALTER TABLE backtest_runs
    ADD CONSTRAINT backtest_runs_strategy_organization_fkey
    FOREIGN KEY (organization_id, strategy_id)
    REFERENCES tenant_strategies (organization_id, id)
    ON DELETE CASCADE;
