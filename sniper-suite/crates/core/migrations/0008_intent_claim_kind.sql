-- 0008_intent_claim_kind.sql — extends the reconciliation queue's kind CHECK
-- with 'intent': claims for write-ahead pre-broadcast intents that were never
-- linked to a signature (crash point C, §I). Ambiguous by definition — the
-- worker retries (giving a late `link` a chance) and then parks for
-- operators; the intent's symbol stays entry-gated meanwhile.
--
-- Restart-safe: the new constraint is a strict superset of the old one, so
-- every existing row satisfies it; ADD CONSTRAINT revalidation cannot fail
-- on data written under 0005.

ALTER TABLE reconciliation_state
    DROP CONSTRAINT IF EXISTS reconciliation_state_kind_check;

ALTER TABLE reconciliation_state
    ADD CONSTRAINT reconciliation_state_kind_check
    CHECK (kind IN (
        'order', 'position', 'transaction',
        'polymarket_order', 'balance', 'intent'));
