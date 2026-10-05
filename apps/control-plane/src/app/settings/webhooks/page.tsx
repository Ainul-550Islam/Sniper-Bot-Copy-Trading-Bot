"use client";

/**
 * Outbound webhook management.
 *
 * The page renders only the server's tenant-scoped records. It never replaces
 * an unavailable API with a sample endpoint, because doing so can make an
 * operator believe that notifications are configured when they are not.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import WebhookForm from "@/components/settings/webhook-form";
import { WebhookEndpoint, listWebhooks } from "@/lib/api/webhook-api";
import { toDisplayError } from "@/lib/api";

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
    } catch (err: unknown) {
      setWebhooks([]);
      setError(toDisplayError(err));
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
          Subscribe external systems to execution events, risk alerts, and lifecycle status changes.
        </p>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          <div>{error}</div>
          <button type="button" onClick={() => void loadWebhooks()} className="btn btn-secondary" style={{ marginTop: "0.75rem" }}>
            Retry
          </button>
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
