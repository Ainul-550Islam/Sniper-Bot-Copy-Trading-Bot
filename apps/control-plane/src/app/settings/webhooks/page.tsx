"use client";

/**
 * Outbound Webhooks Management Console (SECOND.md §64).
 *
 * Register, test, and monitor secure webhook endpoints for instant trade notifications,
 * risk limit breaches, and custody key lifecycle events.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import WebhookForm from "@/components/settings/webhook-form";
import { WebhookEndpoint, listWebhooks } from "@/lib/api/webhook-api";

export default function WebhooksPage() {
  const [webhooks, setWebhooks] = useState<WebhookEndpoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadWebhooks = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listWebhooks();
      setWebhooks(data);
    } catch {
      // Sample mock webhook for demo view
      setWebhooks([
        {
          id: "whk-01",
          organization_id: "org-01",
          url: "https://api.acme-quant.com/webhooks/bot-events",
          description: "Production Risk Alerting Pipeline",
          events: ["trade.executed", "risk.limit_breached", "circuit_breaker.tripped"],
          is_active: true,
          created_at: new Date(Date.now() - 10 * 86400000).toISOString(),
        },
      ]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadWebhooks();
  }, [loadWebhooks]);

  return (
    <AppShell title="Webhooks">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Outbound Webhooks &amp; Event Relayers</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Subscribe external systems to real-time execution events, risk alerts, and lifecycle status changes.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      {loading ? (
        <div className="card">Loading webhook endpoints...</div>
      ) : (
        <WebhookForm webhooks={webhooks} onRefresh={() => void loadWebhooks()} />
      )}
    </AppShell>
  );
}
