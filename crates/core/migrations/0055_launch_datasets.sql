-- 0055_launch_datasets.sql — recorded market data for honest backtests.
--
-- Additive only. A dataset is captured from live (paper-mode) launch events and
-- price ticks; a backtest may only be labelled "recorded" and "authoritative"
-- when it runs over one of these rows. Nothing here is synthetic.
--
-- ROLLBACK (manual; the repository has no down-migrations):
--   DROP TABLE IF EXISTS launch_dataset_ticks;
--   DROP TABLE IF EXISTS launch_datasets;

CREATE TABLE IF NOT EXISTS launch_datasets (
    id                  uuid PRIMARY KEY,
    organization_id     uuid NOT NULL,
    source              text NOT NULL,
    started_at          timestamptz,
    ended_at            timestamptz,
    event_count         integer NOT NULL DEFAULT 0,
    attestation_sha256  text,
    created_at          timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_launch_datasets_org_started
    ON launch_datasets (organization_id, started_at DESC);

CREATE TABLE IF NOT EXISTS launch_dataset_ticks (
    dataset_id          uuid NOT NULL REFERENCES launch_datasets (id) ON DELETE CASCADE,
    mint                text NOT NULL,
    slot                bigint NOT NULL,
    ts                  timestamptz NOT NULL,
    price_lamports      numeric(30,0),
    liquidity_lamports  numeric(30,0),
    PRIMARY KEY (dataset_id, mint, slot)
);

CREATE INDEX IF NOT EXISTS idx_launch_dataset_ticks_mint_ts
    ON launch_dataset_ticks (dataset_id, mint, ts);
