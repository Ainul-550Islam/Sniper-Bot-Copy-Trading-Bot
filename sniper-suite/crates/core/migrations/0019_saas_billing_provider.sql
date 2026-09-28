-- 0019_saas_billing_provider.sql — durable provider-neutral commercial billing records.
--
-- Extends the TASK 7A billing foundation (plans/subscriptions/entitlements/usage)
-- with commercial truth: who paid, how, when, and what the provider said.
-- All rows are tenant-scoped (organization_id) and idempotent.
--
-- Tables introduced:
--   payment_customers      — provider-neutral customer identity per organization
--   checkout_sessions      — idempotent checkout/session creation
--   payment_transactions   — money movement attempts and their final state
--   invoices               — provider invoices linked to subscriptions/transactions
--   provider_events        — deduplication ledger for inbound provider events
--   payment_state_history  — append-only audit of every payment state change
--
-- Invariants:
--   * Never store full card/payment credentials or provider secrets. Only
--     opaque references (provider_customer_id, provider_session_id, etc.)
--     and hashes where persistence is required.
--   * Every commercial record links to organization_id with a FK.
--   * Unique constraints enforce idempotency (provider_event identity,
--     idempotency keys, provider references).
--   * Lifecycle indexes support operational queries (by org, status, period).
--   * Safe state transitions are validated in Rust; DB CHECKs enforce the vocabulary.
--   * Preserve existing plans/subscriptions/entitlements/usage (no DROP/ALTER on them).

-- Payment customers: one row per organization per provider, maps our tenant to provider's customer object
CREATE TABLE IF NOT EXISTS payment_customers (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    provider            text        NOT NULL CHECK (provider IN ('manual','stripe','paddle')),
    provider_customer_id text       NOT NULL,
    email               text        NOT NULL DEFAULT '',
    display_name        text        NOT NULL DEFAULT '',
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, provider),
    UNIQUE (provider, provider_customer_id)
);
CREATE INDEX IF NOT EXISTS payment_customers_org_idx ON payment_customers (organization_id, provider);
CREATE INDEX IF NOT EXISTS payment_customers_provider_idx ON payment_customers (provider, provider_customer_id);

-- Checkout sessions: idempotent provider session creation. Idempotency key is per-organization.
CREATE TABLE IF NOT EXISTS checkout_sessions (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    subscription_id     uuid        REFERENCES subscriptions(id) ON DELETE SET NULL,
    plan_code           text        NOT NULL CHECK (plan_code IN ('starter','pro','business','enterprise')),
    provider            text        NOT NULL CHECK (provider IN ('manual','stripe','paddle')),
    provider_session_id text,
    idempotency_key     text        NOT NULL,
    status              text        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','open','completed','expired','canceled')),
    amount_cents        bigint,
    currency            text        NOT NULL DEFAULT 'usd' CHECK (currency ~ '^[a-zA-Z]{3}$'),
    success_url         text,
    cancel_url          text,
    expires_at          timestamptz,
    completed_at        timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, idempotency_key),
    UNIQUE (provider, provider_session_id)
);
CREATE INDEX IF NOT EXISTS checkout_sessions_org_status_idx ON checkout_sessions (organization_id, status, created_at);
CREATE INDEX IF NOT EXISTS checkout_sessions_provider_idx ON checkout_sessions (provider, provider_session_id) WHERE provider_session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS checkout_sessions_expires_idx ON checkout_sessions (expires_at) WHERE status IN ('pending','open');

-- Payment transactions: canonical money state, idempotent via provider identity + idempotency_key
CREATE TABLE IF NOT EXISTS payment_transactions (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    subscription_id     uuid        REFERENCES subscriptions(id) ON DELETE SET NULL,
    checkout_session_id uuid        REFERENCES checkout_sessions(id) ON DELETE SET NULL,
    invoice_id          uuid,
    provider            text        NOT NULL CHECK (provider IN ('manual','stripe','paddle')),
    provider_payment_id text,
    provider_customer_id text,
    idempotency_key     text        NOT NULL,
    amount_cents        bigint      NOT NULL CHECK (amount_cents >= 0),
    currency            text        NOT NULL CHECK (currency ~ '^[a-zA-Z]{3}$'),
    status              text        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','requires_action','authorized','succeeded','failed','canceled','refunded','partially_refunded')),
    failure_code        text,
    failure_message     text,
    retryable           boolean     NOT NULL DEFAULT false,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    succeeded_at        timestamptz,
    failed_at           timestamptz,
    UNIQUE (organization_id, idempotency_key),
    UNIQUE (provider, provider_payment_id)
);
CREATE INDEX IF NOT EXISTS payment_transactions_org_status_idx ON payment_transactions (organization_id, status, created_at);
CREATE INDEX IF NOT EXISTS payment_transactions_org_period_idx ON payment_transactions (organization_id, created_at);
CREATE INDEX IF NOT EXISTS payment_transactions_provider_idx ON payment_transactions (provider, provider_payment_id) WHERE provider_payment_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS payment_transactions_subscription_idx ON payment_transactions (subscription_id, created_at) WHERE subscription_id IS NOT NULL;

-- Invoices: provider invoices linked to subscriptions and optionally to a payment
CREATE TABLE IF NOT EXISTS invoices (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    subscription_id     uuid        REFERENCES subscriptions(id) ON DELETE SET NULL,
    payment_transaction_id uuid     REFERENCES payment_transactions(id) ON DELETE SET NULL,
    provider            text        NOT NULL CHECK (provider IN ('manual','stripe','paddle')),
    provider_invoice_id text,
    invoice_number      text,
    status              text        NOT NULL DEFAULT 'draft'
                        CHECK (status IN ('draft','open','paid','void','uncollectible','refunded')),
    amount_cents        bigint      NOT NULL CHECK (amount_cents >= 0),
    amount_paid_cents   bigint      NOT NULL DEFAULT 0 CHECK (amount_paid_cents >= 0),
    amount_due_cents    bigint      NOT NULL DEFAULT 0 CHECK (amount_due_cents >= 0),
    currency            text        NOT NULL CHECK (currency ~ '^[a-zA-Z]{3}$'),
    period_start        timestamptz,
    period_end          timestamptz,
    due_date            timestamptz,
    paid_at             timestamptz,
    hosted_invoice_url  text,
    invoice_pdf_url     text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_invoice_id)
);
CREATE INDEX IF NOT EXISTS invoices_org_status_idx ON invoices (organization_id, status, created_at);
CREATE INDEX IF NOT EXISTS invoices_org_period_idx ON invoices (organization_id, period_start, period_end);
CREATE INDEX IF NOT EXISTS invoices_subscription_idx ON invoices (subscription_id, created_at) WHERE subscription_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS invoices_provider_idx ON invoices (provider, provider_invoice_id) WHERE provider_invoice_id IS NOT NULL;

-- Ensure payment_transactions.invoice_id FK now that invoices exists (add if not already constrained)
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'payment_transactions_invoice_id_fkey'
    ) THEN
        ALTER TABLE payment_transactions
            ADD CONSTRAINT payment_transactions_invoice_id_fkey
            FOREIGN KEY (invoice_id) REFERENCES invoices(id) ON DELETE SET NULL;
    END IF;
END$$;

-- Provider event deduplication: every inbound provider event is recorded exactly once
CREATE TABLE IF NOT EXISTS provider_events (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        REFERENCES organizations(id) ON DELETE SET NULL,
    provider            text        NOT NULL CHECK (provider IN ('manual','stripe','paddle')),
    provider_event_id   text        NOT NULL,
    event_type          text        NOT NULL,
    idempotency_key     text        NOT NULL,
    payload_hash        text        NOT NULL,
    processed           boolean     NOT NULL DEFAULT false,
    processing_error    text,
    received_at         timestamptz NOT NULL DEFAULT now(),
    processed_at        timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_event_id),
    UNIQUE (provider, idempotency_key)
);
CREATE INDEX IF NOT EXISTS provider_events_org_idx ON provider_events (organization_id, received_at) WHERE organization_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS provider_events_provider_type_idx ON provider_events (provider, event_type, received_at);
CREATE INDEX IF NOT EXISTS provider_events_unprocessed_idx ON provider_events (processed, received_at) WHERE processed = false;

-- Payment state history: append-only audit of every transition (compliance, reconciliation)
CREATE TABLE IF NOT EXISTS payment_state_history (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    payment_transaction_id uuid    NOT NULL REFERENCES payment_transactions(id) ON DELETE CASCADE,
    from_status         text,
    to_status           text        NOT NULL CHECK (to_status IN ('pending','requires_action','authorized','succeeded','failed','canceled','refunded','partially_refunded')),
    reason              text        NOT NULL DEFAULT '',
    provider_event_id   text,
    actor               text        NOT NULL DEFAULT 'system',
    created_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS payment_state_history_payment_idx ON payment_state_history (payment_transaction_id, created_at);
CREATE INDEX IF NOT EXISTS payment_state_history_org_idx ON payment_state_history (organization_id, created_at);
