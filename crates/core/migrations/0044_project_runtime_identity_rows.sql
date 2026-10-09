-- Keep the relational identity tables required by legacy foreign keys
-- aligned with the canonical serialized SaaS records. Runtime-record writes
-- after this migration are mirrored atomically by PostgresSaasRepo.
--
-- Existing deployments may have been created while only the runtime
-- projection was written. Backfill in FK order so TOTP devices, security
-- policies, API keys, billing rows, and membership/session projections can
-- continue to use their relational references.

INSERT INTO users (
    id, email, email_verified, display_name, password_hash, status,
    platform_admin, created_at, updated_at, last_login_at
)
SELECT
    (record->>'id')::uuid,
    record->>'email',
    (record->>'email_verified')::boolean,
    COALESCE(record->>'display_name', ''),
    record->>'password_hash',
    record->>'status',
    (record->>'platform_admin')::boolean,
    (record->>'created_at')::timestamptz,
    (record->>'updated_at')::timestamptz,
    NULLIF(record->>'last_login_at', '')::timestamptz
FROM saas_runtime_records
WHERE kind = 'user'
ON CONFLICT (id) DO UPDATE SET
    email = EXCLUDED.email,
    email_verified = EXCLUDED.email_verified,
    display_name = EXCLUDED.display_name,
    password_hash = EXCLUDED.password_hash,
    status = EXCLUDED.status,
    platform_admin = EXCLUDED.platform_admin,
    updated_at = EXCLUDED.updated_at,
    last_login_at = EXCLUDED.last_login_at;

INSERT INTO organizations (
    id, slug, name, status, created_by, created_at, updated_at,
    suspended_at, suspend_reason
)
SELECT
    (record->>'id')::uuid,
    record->>'slug',
    record->>'name',
    record->>'status',
    NULLIF(record->>'created_by', '')::uuid,
    (record->>'created_at')::timestamptz,
    (record->>'updated_at')::timestamptz,
    NULLIF(record->>'suspended_at', '')::timestamptz,
    COALESCE(record->>'suspend_reason', '')
FROM saas_runtime_records
WHERE kind = 'organization'
ON CONFLICT (id) DO UPDATE SET
    slug = EXCLUDED.slug,
    name = EXCLUDED.name,
    status = EXCLUDED.status,
    created_by = EXCLUDED.created_by,
    updated_at = EXCLUDED.updated_at,
    suspended_at = EXCLUDED.suspended_at,
    suspend_reason = EXCLUDED.suspend_reason;

INSERT INTO organization_members (
    id, organization_id, user_id, role, status, invited_by, created_at, updated_at
)
SELECT
    (record->>'id')::uuid,
    (record->>'organization_id')::uuid,
    (record->>'user_id')::uuid,
    record->>'role',
    record->>'status',
    NULLIF(record->>'invited_by', '')::uuid,
    (record->>'created_at')::timestamptz,
    (record->>'updated_at')::timestamptz
FROM saas_runtime_records
WHERE kind = 'membership'
ON CONFLICT (organization_id, user_id) DO UPDATE SET
    id = EXCLUDED.id,
    role = EXCLUDED.role,
    status = EXCLUDED.status,
    invited_by = EXCLUDED.invited_by,
    updated_at = EXCLUDED.updated_at;

INSERT INTO sessions (
    id, user_id, organization_id, token_hash, token_prefix, user_agent, ip,
    created_at, last_seen_at, expires_at, revoked_at, revoke_reason
)
SELECT
    (record->>'id')::uuid,
    (record->>'user_id')::uuid,
    NULLIF(record->>'organization_id', '')::uuid,
    record->>'token_hash',
    COALESCE(record->>'token_prefix', ''),
    COALESCE(record->>'user_agent', ''),
    COALESCE(record->>'ip', ''),
    (record->>'created_at')::timestamptz,
    (record->>'last_seen_at')::timestamptz,
    (record->>'expires_at')::timestamptz,
    NULLIF(record->>'revoked_at', '')::timestamptz,
    COALESCE(record->>'revoke_reason', '')
FROM saas_runtime_records
WHERE kind = 'session'
ON CONFLICT (id) DO UPDATE SET
    organization_id = EXCLUDED.organization_id,
    last_seen_at = EXCLUDED.last_seen_at,
    expires_at = EXCLUDED.expires_at,
    revoked_at = EXCLUDED.revoked_at,
    revoke_reason = EXCLUDED.revoke_reason;
