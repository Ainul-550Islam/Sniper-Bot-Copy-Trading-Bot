-- Durable control-plane records for tenant alerts, support tickets, and
-- notification preferences. Every table is tenant-owned and indexed by the
-- organization key used by the handlers.

CREATE TABLE IF NOT EXISTS tenant_alerts (
    id                uuid PRIMARY KEY,
    organization_id   uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    severity          text NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    category          text NOT NULL,
    title             text NOT NULL,
    message           text NOT NULL,
    acknowledged_at   timestamptz,
    acknowledged_by   uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS tenant_alerts_org_created_idx
    ON tenant_alerts (organization_id, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS tenant_alerts_org_ack_idx
    ON tenant_alerts (organization_id, acknowledged_at, created_at DESC);

CREATE TABLE IF NOT EXISTS support_tickets (
    id                uuid PRIMARY KEY,
    organization_id   uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_by        uuid REFERENCES users(id) ON DELETE SET NULL,
    subject           text NOT NULL,
    priority          text NOT NULL CHECK (priority IN ('low', 'normal', 'high', 'urgent')),
    description       text NOT NULL,
    status            text NOT NULL DEFAULT 'open'
                      CHECK (status IN ('open', 'in_progress', 'resolved', 'closed')),
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS support_tickets_org_created_idx
    ON support_tickets (organization_id, created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS notification_preferences (
    organization_id          uuid PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    email_trade_executed     boolean NOT NULL DEFAULT true,
    email_risk_breach        boolean NOT NULL DEFAULT true,
    email_daily_summary      boolean NOT NULL DEFAULT false,
    telegram_instant_fills   boolean NOT NULL DEFAULT true,
    telegram_circuit_breaker boolean NOT NULL DEFAULT true,
    webhook_forwarding       boolean NOT NULL DEFAULT true,
    updated_by               uuid REFERENCES users(id) ON DELETE SET NULL,
    updated_at               timestamptz NOT NULL DEFAULT now()
);
