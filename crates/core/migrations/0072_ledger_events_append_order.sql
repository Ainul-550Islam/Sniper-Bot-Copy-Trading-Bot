-- 0072_ledger_events_append_order.sql — index the canonical journal replay order.
--
-- Live accounting applies events in serialized durable-append order. Recovery
-- must replay that same order; event timestamps are venue facts and may arrive
-- late or out of order. This index keeps tenant-scoped recovery ordered by the
-- journal timestamp and deterministic event id.
CREATE INDEX IF NOT EXISTS ledger_events_append_order_idx
    ON public.ledger_events (organization_id, recorded_at, event_id);
