-- Token lifetime checks must describe immutable row data, not the clock at
-- the instant of an INSERT or UPDATE. The old expires_at > now() checks
-- rejected legitimate revocation/consumption/maintenance of aged rows and
-- made restoring expired audit rows impossible.
--
-- Keep issued lifetimes strictly positive. Whether a credential is usable
-- RIGHT NOW remains the responsibility of the locked confirm queries, which
-- require expires_at > now() AND an unused, unrevoked token. This migration
-- does not make an expired credential acceptable.
--
-- Do not restore the volatile checks on rollback: existing expired rows are
-- valid historical data and would fail validation. Roll the application
-- forward; no rows or credentials are removed by this schema repair.

ALTER TABLE password_reset_tokens
    DROP CONSTRAINT password_reset_tokens_expires_at_check;
ALTER TABLE password_reset_tokens
    ADD CONSTRAINT password_reset_tokens_expires_at_check
    CHECK (expires_at > created_at);

ALTER TABLE email_verifications
    DROP CONSTRAINT email_verifications_expires_at_check;
ALTER TABLE email_verifications
    ADD CONSTRAINT email_verifications_expires_at_check
    CHECK (expires_at > created_at);
