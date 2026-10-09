-- Add explicit durable audit vocabulary for custody rotation workflow events.
-- Rotation actions are compliance events distinct from signer status changes.

ALTER TABLE custody_audit
    DROP CONSTRAINT IF EXISTS custody_audit_action_check;

ALTER TABLE custody_audit
    ADD CONSTRAINT custody_audit_action_check
    CHECK (action IN (
        'profile_created',
        'profile_activated',
        'profile_suspended',
        'profile_revoked',
        'profile_closed',
        'signer_created',
        'signer_activated',
        'signer_suspended',
        'signer_revoked',
        'signer_closed',
        'capability_attached',
        'capability_detached',
        'provider_changed',
        'rotation_created',
        'rotation_activated',
        'rotation_revoked'
    ));
