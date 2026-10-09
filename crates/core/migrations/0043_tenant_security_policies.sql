-- Durable tenant security policy state. MFA enrollment records remain in
-- user_mfa_devices; this table stores only organization policy, never a test
-- secret or a browser-generated default.

CREATE TABLE IF NOT EXISTS tenant_security_policies (
    organization_id       uuid PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    mfa_enforced          boolean NOT NULL DEFAULT false,
    ip_allowlist           text[] NOT NULL DEFAULT '{}',
    session_duration_hours integer NOT NULL DEFAULT 12 CHECK (session_duration_hours BETWEEN 1 AND 168),
    require_signed_commits boolean NOT NULL DEFAULT false,
    updated_by             uuid REFERENCES users(id) ON DELETE SET NULL,
    updated_at             timestamptz NOT NULL DEFAULT now()
);
