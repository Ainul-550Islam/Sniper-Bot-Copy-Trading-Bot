-- 0069_custody_signer_profile_tenant_fk.sql
-- Bind the parent profile and signer to the SAME organization using an RI
-- constraint. Migration 0020's ordinary trigger SELECT runs under the caller's
-- RLS context: an invisible foreign profile makes EXISTS false, so the trigger
-- alone does not enforce this invariant. PostgreSQL FK checks enforce the
-- complete referenced tuple independently of parent-row visibility.
--
-- Existing migrations are immutable. This additive migration intentionally
-- VALIDATES existing rows and fails closed if a legacy mismatch exists. A
-- privileged operator must investigate those rows before retrying; do not
-- silently rehome signers, delete fencing data or weaken the constraint.
-- Privileged read-only preflight (do not expose this across tenant APIs):
-- SELECT s.id, s.organization_id, p.organization_id AS profile_organization_id
-- FROM custody_signers s JOIN custody_profiles p ON p.id = s.custody_profile_id
-- WHERE s.organization_id IS DISTINCT FROM p.organization_id;

DO $migration$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'public.custody_profiles'::regclass
          AND conname = 'custody_profiles_id_org_key'
    ) THEN
        ALTER TABLE public.custody_profiles
            ADD CONSTRAINT custody_profiles_id_org_key UNIQUE (id, organization_id);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'public.custody_signers'::regclass
          AND conname = 'custody_signers_profile_org_fk'
    ) THEN
        ALTER TABLE public.custody_signers
            ADD CONSTRAINT custody_signers_profile_org_fk
            FOREIGN KEY (custody_profile_id, organization_id)
            REFERENCES public.custody_profiles (id, organization_id)
            ON UPDATE RESTRICT ON DELETE RESTRICT;
    END IF;
END
$migration$;
