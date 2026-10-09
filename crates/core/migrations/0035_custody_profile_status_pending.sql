-- 0035_custody_profile_status_pending.sql — PROMPT 6/10 §P0 Phase 1:
-- make custody-profile creation durable.
--
-- AUDIT FIRST (per the migration rule — no manufactured changes):
--   0020 created custody_signers.status with
--     CHECK (status IN ('pending','active','suspended','revoked','closed'))
--   but custody_profiles.status with
--     CHECK (status IN ('active','suspended','revoked','closed')).
--
--   The domain model (bot_core::custody::CustodyStatus) has FIVE states,
--   and every custody profile is CREATED Pending and activated
--   separately (`saas/custody.rs` create_profile inserts
--   status='pending'). Because of the missing 'pending' value, every
--   durable INSERT of a new custody profile violated the constraint and
--   failed — silently, because the write's Result was discarded. The
--   in-process map kept working, so the gap only became visible when
--   PROMPT 6's PostgreSQL-backed rotation test asserted the row exists.
--
-- FIX: bring custody_profiles.status to the same five-value domain the
-- signers table already has. Idempotent swap of the CHECK constraint;
-- no data changes (no row can currently hold an out-of-domain value —
-- writes with 'pending' always failed).

ALTER TABLE custody_profiles
    DROP CONSTRAINT IF EXISTS custody_profiles_status_check;

ALTER TABLE custody_profiles
    ADD CONSTRAINT custody_profiles_status_check
    CHECK (status IN ('pending','active','suspended','revoked','closed'));
