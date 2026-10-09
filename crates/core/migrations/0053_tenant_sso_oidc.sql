-- 0053_tenant_sso_oidc.sql — OIDC additions to the SSO config table
-- (migration 0038 created `tenant_sso_configs` for the SAML-first design).
--
-- GAP-MAP v2 (P2): `crates/server/src/saas/sso.rs` implements the OIDC
-- authorization-code + PKCE flow. Three things the 0038 table lacks:
--
-- 1. `redirect_uri` — the callback URL registered with the IdP. The code
--    flow is redirect-binding: the value sent in /authorize MUST equal the
--    one sent in the token request, so it is a configuration fact, not a
--    per-request parameter.
-- 2. `role_mapping` — JSON object mapping an IdP role claim value to one of
--    the eight application roles. Absent/unknown values map to the weakest
--    role (viewer) in the application layer; the DB keeps the shape honest.
-- 3. `enforce_sso` already exists (0038); no change.
--
-- Security notes carried by sso.rs:
-- * `client_secret_encrypted` stores the `enc:v1:` AES-256-GCM ciphertext
--   (SSO_CLIENT_SECRET_ENCRYPTION_KEY), never plaintext;
-- * platform-admin can NEVER be granted through SSO role mapping — the
--   application layer refuses that mapping value;
-- * JIT-provisioned users get an unusable password marker; their only
--   authentication path is the IdP (or a password reset the org owner
--   triggers explicitly).

ALTER TABLE tenant_sso_configs
    ADD COLUMN IF NOT EXISTS redirect_uri text NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS role_mapping jsonb NOT NULL DEFAULT '{}'::jsonb;

-- role_mapping values are constrained to the real application roles so a
-- typo cannot silently grant a role that later deserializes to a default.
-- (platform_admin is deliberately absent: it is never assignable via SSO.)
CREATE OR REPLACE FUNCTION check_sso_role_mapping_values() RETURNS trigger AS $$
DECLARE
    v text;
BEGIN
    FOR v IN SELECT jsonb_object_keys(NEW.role_mapping) LOOP
        IF NEW.role_mapping->>v IS DISTINCT FROM NULL THEN
            IF NEW.role_mapping->>v NOT IN (
                'org_owner', 'org_admin', 'trader',
                'security_admin', 'billing_admin', 'auditor', 'viewer'
            ) THEN
                RAISE EXCEPTION 'sso role_mapping value "%" is not a valid application role',
                    NEW.role_mapping->>v;
            END IF;
        END IF;
    END LOOP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS sso_role_mapping_values ON tenant_sso_configs;
CREATE TRIGGER sso_role_mapping_values
    BEFORE INSERT OR UPDATE ON tenant_sso_configs
    FOR EACH ROW EXECUTE FUNCTION check_sso_role_mapping_values();

COMMENT ON COLUMN tenant_sso_configs.redirect_uri IS
    'OIDC callback URL registered with the IdP; must match exactly between '
    'authorize and token requests (redirect binding).';
COMMENT ON COLUMN tenant_sso_configs.role_mapping IS
    'IdP role claim value -> application role (org_owner/org_admin/trader/'
    'security_admin/billing_admin/auditor/viewer). Unknown values fall back '
    'to viewer. platform_admin is never assignable here.';
