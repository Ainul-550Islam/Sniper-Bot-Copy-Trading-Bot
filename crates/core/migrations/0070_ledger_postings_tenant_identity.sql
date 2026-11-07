-- 0070_ledger_postings_tenant_identity.sql — make ledger posting idempotency
-- tenant-local to match the composite ledger_events identity introduced by
-- 0033.
--
-- 0033 changed ledger_events.event_id from globally unique to
-- (organization_id, event_id), but retained ledger_postings' old UNIQUE
-- (event_id, seq) arbiter. Event ids are digests of event fields and do not
-- include organization_id, so two organizations can legitimately record the
-- same event id. The global child uniqueness then rejected (or DO NOTHING-
-- skipped) the second tenant's postings despite its separate parent event,
-- leaving an accepted ledger event without its balanced posting rows.
--
-- Replace the old global constraint with the tenant-scoped identity in the
-- same migration that updates the writer's ON CONFLICT target. The composite
-- FK to ledger_events from 0033 remains unchanged.

ALTER TABLE public.ledger_postings
    DROP CONSTRAINT IF EXISTS ledger_postings_event_id_seq_key;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM pg_constraint
         WHERE conname = 'ledger_postings_tenant_event_seq_key'
           AND conrelid = 'public.ledger_postings'::regclass
    ) THEN
        ALTER TABLE public.ledger_postings
            ADD CONSTRAINT ledger_postings_tenant_event_seq_key
            UNIQUE (organization_id, event_id, seq);
    END IF;
END
$$;
