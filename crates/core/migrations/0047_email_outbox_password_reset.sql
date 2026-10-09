-- Email delivery outbox + password reset + email verification
-- (GAP-MAP v2 P1). Numbered 0047 because 0044–0046 already exist in this
-- tree (project_runtime_identity_rows, backtest_strategy_tenant_integrity,
-- single_pending_totp_per_tenant); the gap map was drafted against an older
-- snapshot that stopped at 0043.
--
-- Design rules:
--   * Only TOKEN HASHES are stored — never plaintext tokens. A database
--     leak must not yield usable reset/verification links.
--   * The outbox is the ONLY way the application sends email: handlers
--     enqueue rows, the dispatcher (crates/server/src/email/outbox.rs)
--     delivers with retry/backoff, and `dedup_key` prevents duplicate
--     deliveries for the same logical event.
--   * Nothing here ever stores passwords or MFA secrets.

CREATE TABLE IF NOT EXISTS email_outbox (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id  uuid REFERENCES organizations(id) ON DELETE SET NULL,
    user_id          uuid REFERENCES users(id) ON DELETE SET NULL,
    recipient        text NOT NULL CHECK (length(recipient) BETWEEN 3 AND 320),
    template_key     text NOT NULL CHECK (template_key IN (
                         'email_verification', 'password_reset',
                         'member_invite', 'security_alert', 'invoice_receipt'
                     )),
    subject          text NOT NULL CHECK (length(subject) <= 300),
    body_text        text NOT NULL,
    body_html        text,
    template_data    jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- pending -> sending -> sent | failed -> dead (after max attempts)
    status           text NOT NULL DEFAULT 'pending' CHECK (status IN (
                         'pending', 'sending', 'sent', 'failed', 'dead'
                     )),
    attempts         integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts     integer NOT NULL DEFAULT 5 CHECK (max_attempts BETWEEN 1 AND 20),
    next_attempt_at  timestamptz NOT NULL DEFAULT now(),
    last_error       text,
    -- One row per logical email event; concurrent producers cannot enqueue
    -- the same reset/invite/verification twice.
    dedup_key        text NOT NULL,
    provider_id      text,               -- external message id once sent
    created_at       timestamptz NOT NULL DEFAULT now(),
    sent_at          timestamptz,
    CONSTRAINT email_outbox_dedup_key_unique UNIQUE (dedup_key)
);

-- Dispatcher poll order: due rows first.
CREATE INDEX IF NOT EXISTS email_outbox_due_idx
    ON email_outbox (status, next_attempt_at)
    WHERE status IN ('pending', 'failed');

-- Per-user recent history (support + abuse review).
CREATE INDEX IF NOT EXISTS email_outbox_user_idx
    ON email_outbox (user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS email_outbox_org_idx
    ON email_outbox (organization_id, created_at DESC);

-- Password reset tokens: hash-only, single use, short-lived. `used_at`
-- marks consumption; `revoked_at` marks invalidation by a later reset
-- request or a password change (all open tokens are revoked then).
CREATE TABLE IF NOT EXISTS password_reset_tokens (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  text NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    expires_at  timestamptz NOT NULL CHECK (expires_at > now()),
    used_at     timestamptz,
    revoked_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS password_reset_tokens_user_idx
    ON password_reset_tokens (user_id, created_at DESC);

-- Email verification tokens: hash-only, single use. One open token per
-- user+email; a new request revokes the previous one (handled in code by
-- setting revoked_at before insert, under the row lock).
CREATE TABLE IF NOT EXISTS email_verifications (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    email        text NOT NULL CHECK (length(email) BETWEEN 3 AND 320),
    token_hash  text NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    expires_at  timestamptz NOT NULL CHECK (expires_at > now()),
    verified_at timestamptz,
    revoked_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS email_verifications_user_idx
    ON email_verifications (user_id, email, created_at DESC);
