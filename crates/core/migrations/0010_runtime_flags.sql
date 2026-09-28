-- 0010_runtime_flags.sql — Prompt 3 §Q: cross-replica propagation of the
-- GLOBAL runtime controls (kill switch, per-module enable gates). Written by
-- whichever replica mutates them (AppState -> RuntimeFlagsWriter), read by
-- every replica's sync task. Staleness rule lives in the sync task: kill ON
-- applies immediately; kill OFF / flag changes apply only when the row is
-- newer than the replica's last local decision (flag_touched_at).
--
-- Additive and restart-safe: IF NOT EXISTS only.

CREATE TABLE IF NOT EXISTS runtime_flags (
    flag        text        PRIMARY KEY,
    -- 'kill_switch' | 'module:<name>'
    enabled     boolean     NOT NULL DEFAULT false,
    reason      text        NOT NULL DEFAULT '',
    updated_by  text        NOT NULL DEFAULT '',
    -- replica id that last wrote the flag
    updated_at  timestamptz NOT NULL DEFAULT now()
);
