-- 0050_wallet_pools.sql — named pools of tenant-bound wallets for spreading
-- execution across multiple signers.
--
-- GAP-MAP v2 (P2). Numbering: the GAP MAP listed this as 0046; 0044–0048
-- were taken by P0/P1 work and migration 0048's header reserved 0050 for
-- wallet pools (referrals/platform fees took 0049, demo flag moved to 0051).
--
-- Model: a pool belongs to an organization and aggregates that org's OWN
-- custody signers (`custody_signers`, migration 0020) — never another
-- tenant's, and never entries from the operator-level `wallets` table. The
-- executor (`module-sniper/src/tenant_executor.rs`) picks a member per buy:
--   * `round_robin`     — next active member in deterministic rotation order;
--   * `weighted_split`  — members chosen proportionally to `weight`.
-- Membership changes are immediate (no versioning): a removed member simply
-- stops receiving new orders; open positions stay with their original wallet.

CREATE TABLE IF NOT EXISTS wallet_pools (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id  uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name             text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 64),
    allocation       text NOT NULL DEFAULT 'round_robin' CHECK (allocation IN (
                         'round_robin', 'weighted_split'
                     )),
    -- Cursor for round-robin rotation: index of the last used member. Kept
    -- here (not in Redis) so rotation survives restarts and is auditable.
    rotation_cursor  integer NOT NULL DEFAULT 0 CHECK (rotation_cursor >= 0),
    status           text NOT NULL DEFAULT 'active' CHECK (status IN (
                         'active', 'disabled'
                     )),
    created_by       uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT wallet_pools_org_name_unique UNIQUE (organization_id, name)
);

CREATE INDEX IF NOT EXISTS wallet_pools_org_idx
    ON wallet_pools (organization_id) WHERE status = 'active';

CREATE TABLE IF NOT EXISTS wallet_pool_members (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    pool_id            uuid NOT NULL REFERENCES wallet_pools(id) ON DELETE CASCADE,
    -- The tenant-bound signer. RESTRICT: removing a signer that is still a
    -- pool member is an explicit operation (delete the membership first), so
    -- an in-flight strategy can never lose its wallet by accident.
    custody_signer_id  uuid NOT NULL REFERENCES custody_signers(id) ON DELETE RESTRICT,
    -- Selection weight for `weighted_split` pools; ignored by round_robin.
    weight             integer NOT NULL DEFAULT 1 CHECK (weight BETWEEN 1 AND 1000),
    status             text NOT NULL DEFAULT 'active' CHECK (status IN (
                           'active', 'paused', 'removed'
                       )),
    added_by           uuid REFERENCES users(id) ON DELETE SET NULL,
    added_at           timestamptz NOT NULL DEFAULT now(),
    removed_at         timestamptz,
    CONSTRAINT wallet_pool_members_pool_signer_unique UNIQUE (pool_id, custody_signer_id)
);

CREATE INDEX IF NOT EXISTS wallet_pool_members_pool_idx
    ON wallet_pool_members (pool_id) WHERE status = 'active';

-- ---------------------------------------------------------------------------
-- Cross-tenant guard: a pool may only contain signers of the SAME
-- organization. Mirrors the check_custody_signer_org() defence-in-depth
-- trigger from migration 0020 (enforced in Rust as well).
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION check_wallet_pool_member_org() RETURNS trigger AS $$
DECLARE
    pool_org uuid;
    signer_org uuid;
BEGIN
    SELECT organization_id INTO pool_org
        FROM wallet_pools WHERE id = NEW.pool_id;
    SELECT organization_id INTO signer_org
        FROM custody_signers WHERE id = NEW.custody_signer_id;
    IF pool_org IS NULL OR signer_org IS NULL THEN
        RAISE EXCEPTION 'wallet pool member references a missing pool or signer';
    END IF;
    IF pool_org <> signer_org THEN
        RAISE EXCEPTION 'cross-tenant wallet pool membership is not allowed';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS wallet_pool_member_org_guard ON wallet_pool_members;
CREATE TRIGGER wallet_pool_member_org_guard
    BEFORE INSERT OR UPDATE ON wallet_pool_members
    FOR EACH ROW EXECUTE FUNCTION check_wallet_pool_member_org();

COMMENT ON TABLE wallet_pools IS
    'Named pools of an organization''s custody signers used to spread order '
    'execution across wallets (module-sniper tenant_executor, P2).';
