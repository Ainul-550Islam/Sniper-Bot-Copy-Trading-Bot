-- 0039_webhook_hardening.sql — durable webhook metadata required by the SaaS API.
--
-- Migration 0037 created the tenant-scoped endpoint and delivery tables but
-- omitted the user-visible description field. Keep the original secret column
-- for wire compatibility while the application enforces one-time disclosure,
-- tenant predicates, HTTPS-only targets, redirect rejection, and delivery
-- journaling.

ALTER TABLE webhook_endpoints
    ADD COLUMN IF NOT EXISTS description VARCHAR(256) NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_org_created
    ON webhook_endpoints (organization_id, created_at DESC, id DESC);

ALTER TABLE webhook_deliveries
    ADD COLUMN IF NOT EXISTS response_body_digest VARCHAR(64);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_endpoint_status
    ON webhook_deliveries (endpoint_id, status, created_at DESC);
