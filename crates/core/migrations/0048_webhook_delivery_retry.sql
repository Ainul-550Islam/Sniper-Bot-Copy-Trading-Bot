-- 0048_webhook_delivery_retry.sql — durable retry support for tenant webhooks.
--
-- GAP-MAP v2 (P1, server): `webhooks.rs` performed a single delivery attempt
-- and recorded it; nothing retried a failed delivery and no dispatcher
-- existed anywhere in the codebase (verified by audit — the VERIFY item
-- resolved to "absent"). This migration adds the columns a retry dispatcher
-- needs:
--
--   * `payload`         the exact JSON body delivered, kept so retries send
--                       byte-identical payloads (re-signing with a fresh
--                       timestamp but never a mutated body);
--   * `next_retry_at`   when the row becomes eligible for its next attempt;
--                       NULL means "no retry scheduled" (terminal rows and
--                       legacy rows).
--
-- The dispatcher (`saas/webhook_delivery.rs`) claims due rows with
-- `FOR UPDATE SKIP LOCKED`, re-delivers, increments `attempt_count` and
-- schedules the next attempt with exponential backoff. Rows that exhaust
-- MAX_ATTEMPTS become `dead` — the dead letter. `status` keeps its free-text
-- VARCHAR(32) shape: existing values ('pending', 'succeeded', 'failed') stay
-- valid and the dispatcher adds 'dead'.
--
-- Numbering note: the GAP MAP reserved 0048 for referral/platform-fee
-- configuration; that work is P2 and moves to 0049 (wallet pools to 0050)
-- because this P1 fix needed the number first.

ALTER TABLE webhook_deliveries
    ADD COLUMN IF NOT EXISTS payload jsonb NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE webhook_deliveries
    ADD COLUMN IF NOT EXISTS next_retry_at timestamptz;

-- The dispatcher scans exactly this shape: failed rows whose next attempt is
-- due. Partial index keeps it tiny.
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_retry_due
    ON webhook_deliveries (next_retry_at)
    WHERE status = 'failed' AND next_retry_at IS NOT NULL;
