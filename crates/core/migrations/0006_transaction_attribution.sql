-- 0006_transaction_attribution.sql — Prompt 2 §E/§U: attribution columns on
-- the transactions table so every money-moving execution attempt persists
-- signer identity, venue and the executor's broadcast-attempt count.
--
-- Additive and restart-safe: IF NOT EXISTS, nullable/defaulted columns, no
-- rewrites of existing rows. Apply order: after 0002 (creates the table) and
-- 0005 (reconciliation queue); the migrator runs files in lexicographic
-- order, which is the documented order.

ALTER TABLE transactions ADD COLUMN IF NOT EXISTS signer   text;
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS venue    text;
ALTER TABLE transactions ADD COLUMN IF NOT EXISTS attempts integer NOT NULL DEFAULT 0;

-- Operators query unresolved attempts by signer/venue during incidents.
CREATE INDEX IF NOT EXISTS transactions_signer_idx ON transactions (signer);
