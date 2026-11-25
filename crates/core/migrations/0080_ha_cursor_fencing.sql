-- HA cursor fencing + processing acknowledgement (GAP: "HA cursor fencing",
-- "HA cursor advance before processing and unfenced writes").
--
-- Until now `ha_cursors` was a blind upsert: any process, any generation,
-- any time could overwrite another worker's progress, and there was no way
-- to tell whether the advance recorded in the position column had ever
-- produced its side effect. A crash between "cursor saved" and "event
-- processed" therefore silently skipped the event forever.
--
-- Two columns make that state explicit:
--
--   * `pending` — the cursor advanced to `position`/`token` but the caller
--     has NOT confirmed the work yet. A crash with `pending = true` means
--     "position may be beyond what was actually done"; recovery replays from
--     the last position whose work was confirmed.
--   * `fence`   — the worker `(holder@generation)` that last wrote this
--     cursor. Writers present their lease generation; a write from an older
--     generation than the row already carries is refused (`fenced`), and a
--     write that would move the position/token BACKWARDS while pending is
--     refused (`stale`). Both are deterministic, not "last write wins".
--
-- Writers that present no fencing (no HA lease configured, pure local
-- single-process mode) keep the old behaviour by design; they are not
-- silently granted cluster authority -- the row records `fence = ''` and an
-- empty fence never displaces a fenced row.
ALTER TABLE ha_cursors ADD COLUMN IF NOT EXISTS pending boolean NOT NULL DEFAULT false;
ALTER TABLE ha_cursors ADD COLUMN IF NOT EXISTS fence text NOT NULL DEFAULT '';

-- Recovery asks "which cursors were advanced but never acknowledged?" —
-- keep that lookup cheap even with every feed scope in the table.
CREATE INDEX IF NOT EXISTS ha_cursors_pending_idx
    ON ha_cursors (feed, updated_at DESC) WHERE pending;

-- Bounded metric/audit vocabulary for refused writes is in code
-- (`CursorWrite`), the durable trace is the count of rows whose fence was
-- replaced by a newer generation:
CREATE INDEX IF NOT EXISTS ha_cursors_fence_idx ON ha_cursors (fence);
